//! `velo export-git` — write a repository's history as a `git fast-import` stream.
//!
//! This answers "can I leave if this doesn't work out?", the question an
//! evaluator asks before trusting a history format. It needs no git: the stream
//! is plain text that `git fast-import` reads, so velo does not depend on git
//! being installed to be *leavable*.
//!
//! Git has no home for velo's metadata, rename edges, branch names or
//! millisecond timestamps, so they ride in `Velo-*` trailers on the commit
//! message. Together with the whole-tree snapshots they carry everything an
//! importer needs to rebuild identical snapshot ids (the importer is a separate
//! task). Store only: nothing here touches the working tree.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::io::Write;

use crate::error::Result;
use crate::meta::RESERVED_NAMESPACE;
use crate::progress::{Cancel, Observer, Phase, PhaseGuard};
use crate::{BranchName, Repo};

/// What to export.
#[derive(Default)]
pub struct Options<'a> {
    /// Export only these branches' history; empty means every branch, except
    /// `_stash`, `_deleted_*` and `remotes/*`.
    pub branches: &'a [&'a BranchName],
    /// Where to report progress; the repository's own observer when `None`.
    pub observer: Option<&'a dyn Observer>,
    /// Checked between snapshots.
    pub cancel: Option<&'a Cancel>,
}

/// What was written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Exported {
    pub commits: usize,
    pub blobs: usize,
    pub branches: usize,
    pub tags: usize,
}

struct Row {
    id: String,
    message: String,
    branch: String,
    parent: String,
    merge_parent: String,
    created_at_ms: i64,
}

/// Write the history reachable from the selected branches to `out`.
///
/// Every snapshot goes onto one ref, `refs/velo/export`, parents first, and the
/// branch and tag refs are then pointed at the right marks with `reset`. One
/// ref for all commits is what lets a snapshot be written once however many
/// branches contain it.
pub fn git_fast_import(repo: &Repo, out: &mut dyn Write, options: Options<'_>) -> Result<Exported> {
    let conn = repo.conn();

    // Which branches, and their tips.
    let names: Vec<String> = if options.branches.is_empty() {
        crate::commands::all_branch_names(conn)
    } else {
        options.branches.iter().map(|b| b.to_string()).collect()
    };
    let mut tips: Vec<(String, String)> = Vec::new();
    for name in &names {
        if let Some(tip) = crate::commands::branch_tip(conn, name).filter(|t| !t.is_empty()) {
            tips.push((name.clone(), tip));
        }
    }

    // Everything reachable, through both parents.
    let mut wanted: HashSet<String> = HashSet::new();
    for (_, tip) in &tips {
        wanted.extend(crate::commands::ancestors(conn, tip)?.into_keys());
    }
    // A tag on one of these branches may point at a snapshot the tip no longer
    // reaches (after an undo, say); it still belongs to the branch.
    let selected: HashSet<&str> = names.iter().map(String::as_str).collect();
    let mut tags: Vec<(String, String)> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT t.name, t.snapshot_hash, s.branch FROM tags t
             JOIN snapshots s ON s.hash = t.snapshot_hash ORDER BY t.name",
        )?;
        let found: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .flatten()
            .collect();
        for (name, snap, branch) in found {
            if wanted.contains(&snap) {
                tags.push((name, snap));
            } else if selected.contains(branch.as_str()) {
                wanted.extend(crate::commands::ancestors(conn, &snap)?.into_keys());
                tags.push((name, snap));
            }
        }
    }

    let mut rows: HashMap<String, Row> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT hash, message, branch, parent_hash, merge_parent, created_at_ms
             FROM snapshots WHERE hash = ?",
        )?;
        for id in &wanted {
            if let Ok(row) = stmt.query_row([id], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    message: r.get(1)?,
                    branch: r.get(2)?,
                    parent: r.get(3)?,
                    merge_parent: r.get(4)?,
                    created_at_ms: r.get(5)?,
                })
            }) {
                rows.insert(id.clone(), row);
            }
        }
    }
    let order = topological(&rows);

    let progress = PhaseGuard::cancellable(
        options.observer.unwrap_or_else(|| repo.observer()),
        Phase::Exporting,
        Some(order.len() as u64),
        options.cancel,
    );

    let mut next_mark: u64 = 0;
    let mut blob_marks: HashMap<String, u64> = HashMap::new();
    let mut commit_marks: HashMap<&str, u64> = HashMap::new();
    let mut exported = Exported::default();

    for id in &order {
        progress.check()?;
        let row = &rows[id.as_str()];
        let snapshot = crate::SnapshotId::from_stored(id.clone());
        let files = repo.tree_at(&snapshot)?;

        let mut file_marks = Vec::with_capacity(files.len());
        for file in &files {
            let mark = match blob_marks.get(file.object.as_str()) {
                Some(m) => *m,
                None => {
                    let bytes = repo.read_object(&file.object)?;
                    next_mark += 1;
                    write!(out, "blob\nmark :{}\ndata {}\n", next_mark, bytes.len())?;
                    out.write_all(&bytes)?;
                    out.write_all(b"\n")?;
                    blob_marks.insert(file.object.as_str().to_string(), next_mark);
                    exported.blobs += 1;
                    next_mark
                }
            };
            file_marks.push(mark);
        }

        let meta = repo.snapshot_meta(&snapshot)?;
        let author = meta.author();
        let (name, email) = match &author {
            Some(a) => (
                a.name().replace(['<', '>'], " "),
                a.email().unwrap_or("").replace(['<', '>'], " "),
            ),
            None => ("velo".to_string(), "velo@localhost".to_string()),
        };
        let secs = row.created_at_ms.div_euclid(1000);

        let mut message = format!("{}\n\n", row.message);
        message.push_str(&format!("Velo-Snapshot: {}\n", row.id));
        message.push_str(&format!("Velo-Branch: {}\n", quote(&row.branch)));
        message.push_str(&format!("Velo-Time-Ms: {}\n", row.created_at_ms));
        if let Some(a) = &author {
            message.push_str(&format!(
                "Velo-Author: {} {}\n",
                quote(a.name()),
                quote(a.email().unwrap_or(""))
            ));
        }
        for (ns, key, value) in meta.iter() {
            if ns == RESERVED_NAMESPACE {
                continue;
            }
            message.push_str(&format!(
                "Velo-Meta: {} {} {}\n",
                quote(ns),
                quote(key),
                quote(value)
            ));
        }
        let mut stmt = conn.prepare(
            "SELECT from_path, to_path FROM renames WHERE snapshot_hash = ?
             ORDER BY to_path, from_path",
        )?;
        let renames: Vec<(String, String)> = stmt
            .query_map([&row.id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .flatten()
            .collect();
        for (from, to) in &renames {
            message.push_str(&format!("Velo-Rename: {} {}\n", quote(from), quote(to)));
        }

        next_mark += 1;
        let mark = next_mark;
        write!(
            out,
            "commit refs/velo/export\nmark :{mark}\nauthor {name} <{email}> {secs} +0000\ncommitter {name} <{email}> {secs} +0000\ndata {}\n{message}\n",
            message.len()
        )?;
        if let Some(p) = commit_marks.get(row.parent.as_str()) {
            writeln!(out, "from :{p}")?;
        }
        if let Some(q) = commit_marks.get(row.merge_parent.as_str()) {
            writeln!(out, "merge :{q}")?;
        }
        out.write_all(b"deleteall\n")?;
        for (file, blob) in files.iter().zip(&file_marks) {
            let mode = match file.kind {
                crate::tree::FileKind::Regular => "100644",
                crate::tree::FileKind::Executable => "100755",
                crate::tree::FileKind::Symlink => "120000",
            };
            writeln!(out, "M {mode} :{blob} {}", quote_path(&file.path))?;
        }
        out.write_all(b"\n")?;

        commit_marks.insert(row.id.as_str(), mark);
        exported.commits += 1;
        progress.tick();
    }

    for (name, tip) in &tips {
        if let Some(mark) = commit_marks.get(tip.as_str()) {
            write!(out, "reset refs/heads/{name}\nfrom :{mark}\n\n")?;
            exported.branches += 1;
        }
    }
    for (name, snap) in &tags {
        if let Some(mark) = commit_marks.get(snap.as_str()) {
            write!(out, "reset refs/tags/{name}\nfrom :{mark}\n\n")?;
            exported.tags += 1;
        }
    }
    out.write_all(b"done\n")?;
    out.flush()?;
    Ok(exported)
}

/// Parents first; among the ready, oldest first, then by id, so the stream is
/// deterministic.
fn topological(rows: &HashMap<String, Row>) -> Vec<String> {
    let known = |p: &str| !p.is_empty() && rows.contains_key(p);
    let mut waiting: HashMap<&str, usize> = HashMap::new();
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut ready: BinaryHeap<Reverse<(i64, &str)>> = BinaryHeap::new();
    for row in rows.values() {
        let mut parents: Vec<&str> = Vec::new();
        for p in [row.parent.as_str(), row.merge_parent.as_str()] {
            if known(p) && !parents.contains(&p) {
                parents.push(p);
            }
        }
        if parents.is_empty() {
            ready.push(Reverse((row.created_at_ms, row.id.as_str())));
        } else {
            waiting.insert(row.id.as_str(), parents.len());
            for p in parents {
                children.entry(p).or_default().push(row.id.as_str());
            }
        }
    }
    let mut order = Vec::with_capacity(rows.len());
    while let Some(Reverse((_, id))) = ready.pop() {
        order.push(id.to_string());
        for child in children.get(id).into_iter().flatten() {
            let left = waiting.get_mut(child).expect("a child is waiting");
            *left -= 1;
            if *left == 0 {
                ready.push(Reverse((rows[*child].created_at_ms, child)));
            }
        }
    }
    order
}

/// A JSON-style double-quoted string, so a trailer value can hold anything.
pub(crate) fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Read one [`quote`]d string from the start of `input`, returning it and the
/// text after it. `None` when `input` does not start with a well-formed one.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn unquote(input: &str) -> Option<(String, &str)> {
    let mut chars = input.strip_prefix('"')?.char_indices();
    let mut out = String::new();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &input[1 + i + 1..])),
            '\\' => match chars.next()?.1 {
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let mut code = 0u32;
                    for _ in 0..4 {
                        code = code * 16 + chars.next()?.1.to_digit(16)?;
                    }
                    out.push(char::from_u32(code)?);
                }
                _ => return None,
            },
            c => out.push(c),
        }
    }
    None
}

/// A path as fast-import wants it: bare when it is plain, C-style quoted when
/// it holds a quote, backslash or control character. A path that merely
/// starts with `"` is covered by the same rule.
fn quote_path(path: &str) -> String {
    if !path
        .chars()
        .any(|c| c.is_control() || c == '"' || c == '\\')
    {
        return path.to_string();
    }
    let mut out = String::from("\"");
    for c in path.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("\\{:03o}", b));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
