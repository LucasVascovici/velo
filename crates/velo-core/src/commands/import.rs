//! `velo import-git` — read a `git fast-export` stream into velo.
//!
//! The way back in, and the other half of [`export`](crate::commands::export).
//! A history velo exported carries `Velo-*` trailers on every commit message;
//! when they are present the importer restores the message, branch, exact
//! millisecond timestamp, author, metadata and rename edges from them, so every
//! snapshot comes back with **the id it had**. A plain git history has none, and
//! imports with what git records: author, committer time (whole seconds), the
//! branch the commit was made on, tags and merge parents.
//!
//! The parser reads the commands `git fast-export` writes, plus the ones a
//! hand-written or `fast-import`-style stream may use: `blob`, `commit`,
//! `reset`, `tag`, `feature`, `option`, `progress`, `checkpoint` and `done`;
//! `data` both counted and delimited (`<<DELIM`); `M`, `D`, `R`, `C` and
//! `deleteall` with C-style quoted paths. Signed commits and tags, notes and
//! LFS are out of scope; a `gpgsig` header is read and dropped.
//!
//! # What does not survive
//!
//! - An annotated tag becomes a lightweight velo tag: velo tags carry no
//!   message, so the tagger and tag message are dropped.
//! - A submodule (mode `160000`) has no velo equivalent; its entry is left out
//!   of the tree and counted in [`Imported::skipped_submodules`].
//! - An octopus merge (more than one `merge` line) is refused: a velo snapshot
//!   has at most two parents.
//!
//! # Cancelling
//!
//! Cancellation is checked between commits. Every snapshot saved before it is
//! valid history on its own (each is a whole tree whose parents already exist),
//! so a cancelled import leaves a shorter, consistent history, not a corrupt
//! one. Importing the same stream again is safe: snapshots whose id already
//! exists are left alone.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, Read};
use std::path::PathBuf;

use crate::commands::export::{sanitize_ref_name, unquote};
use crate::error::{Result, VeloError};
use crate::meta::RESERVED_NAMESPACE;
use crate::progress::{Cancel, Observer, Phase, PhaseGuard};
use crate::tree::{Content, FileKind, SaveTree, TreeEntry};
use crate::{Author, BranchName, ObjectHash, SnapshotId, SnapshotMeta, TagName, WriteGuard};

/// How to import.
#[derive(Default)]
pub struct Options<'a> {
    /// Branch for commits whose ref is not `refs/heads/*`; default `imported`.
    pub fallback_branch: Option<&'a BranchName>,
    /// Where to report progress; the repository's own observer when `None`.
    pub observer: Option<&'a dyn Observer>,
    /// Checked between commits.
    pub cancel: Option<&'a Cancel>,
}

/// What was imported.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Imported {
    pub commits: usize,
    pub branches: usize,
    pub tags: usize,
    pub skipped_submodules: usize,
    /// Commits with a `Velo-Snapshot` trailer whose recomputed id differs.
    pub mismatched_ids: usize,
}

/// A blob as the stream gave it, and then as the store holds it.
enum Blob {
    Bytes(Vec<u8>),
    Stored(ObjectHash),
}

/// Where one file's content comes from while a commit's tree is assembled.
#[derive(Clone)]
enum Src {
    Stored(ObjectHash),
    Mark(u64),
    Inline(Vec<u8>),
}

enum Change {
    Modify {
        mode: String,
        src: DataRef,
        path: String,
    },
    Delete(String),
    Rename(String, String),
    Copy(String, String),
    DeleteAll,
}

enum DataRef {
    Mark(u64),
    Inline(Vec<u8>),
}

/// The `Velo-*` trailers of one commit message.
struct Trailer {
    message: String,
    id: String,
    branch: Option<String>,
    time_ms: Option<i64>,
    author: Option<(String, String)>,
    meta: Vec<(String, String, String)>,
    renames: Vec<(String, String)>,
}

/// Line-oriented input with one line of push-back and a line counter, so every
/// error can say where it happened.
struct Reader<'a> {
    input: &'a mut dyn BufRead,
    line: usize,
    pending: Option<String>,
}

impl Reader<'_> {
    fn bad(&self, what: impl std::fmt::Display) -> VeloError {
        VeloError::invalid(format!(
            "git fast-export stream, line {}: {}",
            self.line, what
        ))
    }

    fn read_raw(&mut self) -> Result<Option<Vec<u8>>> {
        let mut buf = Vec::new();
        if self.input.read_until(b'\n', &mut buf)? == 0 {
            return Ok(None);
        }
        self.line += 1;
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        Ok(Some(buf))
    }

    fn next_line(&mut self) -> Result<Option<String>> {
        if let Some(line) = self.pending.take() {
            return Ok(Some(line));
        }
        Ok(self
            .read_raw()?
            .map(|b| String::from_utf8_lossy(&b).into_owned()))
    }

    fn unread(&mut self, line: String) {
        self.pending = Some(line);
    }

    /// The payload of a `data` command; `spec` is what follows `data `.
    fn data(&mut self, spec: &str) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        if let Some(delim) = spec.strip_prefix("<<") {
            loop {
                match self.read_raw()? {
                    None => return Err(self.bad(format!("data delimiter '{delim}' not found"))),
                    Some(l) if l == delim.as_bytes() => break,
                    Some(l) => {
                        out.extend_from_slice(&l);
                        out.push(b'\n');
                    }
                }
            }
        } else {
            let n: u64 = spec
                .trim()
                .parse()
                .map_err(|_| self.bad(format!("'{spec}' is not a data length")))?;
            // `take` rather than a pre-sized buffer: a lying length must not
            // allocate before the bytes prove it.
            (&mut *self.input).take(n).read_to_end(&mut out)?;
            if out.len() as u64 != n {
                return Err(self.bad("the stream ended inside a data block"));
            }
            self.line += out.iter().filter(|b| **b == b'\n').count();
        }
        // The LF after a data block is optional.
        if let Some(next) = self.next_line()? {
            if !next.is_empty() {
                self.unread(next);
            }
        }
        Ok(out)
    }
}

struct Importer<'g, 'r, 'a> {
    guard: &'g WriteGuard<'g>,
    r: Reader<'r>,
    fallback: BranchName,
    marks: HashMap<u64, SnapshotId>,
    oids: HashMap<String, SnapshotId>,
    blobs: HashMap<u64, Blob>,
    refs: HashMap<String, SnapshotId>,
    trailer_branch: HashMap<SnapshotId, String>,
    branches: HashSet<String>,
    tags: HashSet<String>,
    out: Imported,
    progress: PhaseGuard<'a>,
}

/// Read a `git fast-export` stream from `input` and import it.
///
/// See the [module docs](self) for what is restored, what is dropped and what
/// a cancelled import leaves behind.
///
/// # Errors
/// [`InvalidInput`](VeloError::InvalidInput) naming the line for a malformed
/// stream, [`Unsupported`](VeloError::Unsupported) for an octopus merge or a
/// command velo cannot honour, and [`Cancelled`](VeloError::Cancelled) when the
/// caller asked to stop.
pub fn git_fast_export(
    guard: &WriteGuard,
    input: &mut dyn BufRead,
    options: Options<'_>,
) -> Result<Imported> {
    let fallback = match options.fallback_branch {
        Some(b) => b.clone(),
        None => "imported".parse()?,
    };
    let progress = PhaseGuard::cancellable(
        options.observer.unwrap_or_else(|| guard.repo().observer()),
        Phase::Importing,
        None,
        options.cancel,
    );
    let mut imp = Importer {
        guard,
        r: Reader {
            input,
            line: 0,
            pending: None,
        },
        fallback,
        marks: HashMap::new(),
        oids: HashMap::new(),
        blobs: HashMap::new(),
        refs: HashMap::new(),
        trailer_branch: HashMap::new(),
        branches: HashSet::new(),
        tags: HashSet::new(),
        out: Imported::default(),
        progress,
    };
    imp.run()?;
    imp.out.branches = imp.branches.len();
    imp.out.tags = imp.tags.len();
    Ok(imp.out)
}

fn parse_mark(text: &str) -> Option<u64> {
    text.trim().strip_prefix(':')?.parse().ok()
}

fn is_hex_oid(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Importer<'_, '_, '_> {
    fn run(&mut self) -> Result<()> {
        while let Some(line) = self.r.next_line()? {
            if line.is_empty() {
                continue;
            }
            let (cmd, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
            match cmd {
                "blob" => self.blob()?,
                "commit" => {
                    self.progress.check()?;
                    self.commit(rest.trim())?;
                    self.progress.tick();
                }
                "reset" => self.reset(rest.trim())?,
                "tag" => self.tag(rest.trim())?,
                "feature" | "option" | "progress" | "checkpoint" => {}
                "done" => break,
                other => {
                    return Err(match other {
                        "get-mark" | "cat-blob" | "ls" | "alias" => VeloError::unsupported(
                            format!("line {}: '{other}' is not supported.", self.r.line),
                        ),
                        _ => self.r.bad(format!("unknown command '{other}'")),
                    })
                }
            }
        }
        Ok(())
    }

    fn blob(&mut self) -> Result<()> {
        let mut mark = None;
        loop {
            let Some(line) = self.r.next_line()? else {
                return Err(self.r.bad("a blob has no data"));
            };
            if let Some(m) = line.strip_prefix("mark ") {
                mark = Some(parse_mark(m).ok_or_else(|| self.r.bad("bad mark"))?);
            } else if line.starts_with("original-oid ") {
            } else if let Some(spec) = line.strip_prefix("data ") {
                let bytes = self.r.data(spec)?;
                if let Some(m) = mark {
                    self.blobs.insert(m, Blob::Bytes(bytes));
                }
                return Ok(());
            } else {
                return Err(self.r.bad(format!("unexpected '{line}' in a blob")));
            }
        }
    }

    /// A `from`/`merge` operand: a mark, an oid of an imported commit, or a ref.
    /// `None` is git's all-zero "no commit".
    fn resolve(&self, spec: &str) -> Result<Option<SnapshotId>> {
        let spec = spec.trim();
        if let Some(m) = spec.strip_prefix(':') {
            let mark: u64 = m
                .parse()
                .map_err(|_| self.r.bad(format!("bad mark '{spec}'")))?;
            return self
                .marks
                .get(&mark)
                .cloned()
                .map(Some)
                .ok_or_else(|| self.r.bad(format!("mark {spec} is not a commit")));
        }
        if is_hex_oid(spec) {
            if spec.bytes().all(|b| b == b'0') {
                return Ok(None);
            }
            return self
                .oids
                .get(&spec.to_ascii_lowercase())
                .cloned()
                .map(Some)
                .ok_or_else(|| self.r.bad(format!("commit {spec} was not imported")));
        }
        self.refs
            .get(spec)
            .or_else(|| self.refs.get(&format!("refs/heads/{spec}")))
            .cloned()
            .map(Some)
            .ok_or_else(|| self.r.bad(format!("unknown commit '{spec}'")))
    }

    fn parse_path(&self, text: &str) -> Result<(String, String)> {
        if text.starts_with('"') {
            let (path, rest) = unquote_c(text).ok_or_else(|| self.r.bad("bad quoted path"))?;
            Ok((path, rest.strip_prefix(' ').unwrap_or(rest).to_string()))
        } else {
            let (path, rest) = text.split_once(' ').unwrap_or((text, ""));
            Ok((path.to_string(), rest.to_string()))
        }
    }

    fn single_path(&self, text: &str) -> Result<String> {
        if text.starts_with('"') {
            let (path, rest) = unquote_c(text).ok_or_else(|| self.r.bad("bad quoted path"))?;
            if !rest.is_empty() {
                return Err(self.r.bad("text after a quoted path"));
            }
            Ok(path)
        } else if text.is_empty() {
            Err(self.r.bad("a path is missing"))
        } else {
            Ok(text.to_string())
        }
    }

    fn commit(&mut self, refname: &str) -> Result<()> {
        let start_line = self.r.line;
        let mut mark = None;
        let mut oid = None;
        let mut author_line = None;
        let mut committer_line = None;
        let message;
        loop {
            let Some(line) = self.r.next_line()? else {
                return Err(self.r.bad("a commit has no message"));
            };
            if let Some(m) = line.strip_prefix("mark ") {
                mark = Some(parse_mark(m).ok_or_else(|| self.r.bad("bad mark"))?);
            } else if let Some(o) = line.strip_prefix("original-oid ") {
                oid = Some(o.trim().to_ascii_lowercase());
            } else if let Some(a) = line.strip_prefix("author ") {
                author_line = Some(a.to_string());
            } else if let Some(c) = line.strip_prefix("committer ") {
                committer_line = Some(c.to_string());
            } else if line.starts_with("encoding ") {
            } else if line.starts_with("gpgsig ") {
                // Signatures are out of scope; the block still has to be read.
                let spec = self.r.next_line()?.unwrap_or_default();
                self.r.data(spec.strip_prefix("data ").unwrap_or(""))?;
            } else if let Some(spec) = line.strip_prefix("data ") {
                message = String::from_utf8_lossy(&self.r.data(spec)?).into_owned();
                break;
            } else {
                return Err(self.r.bad(format!("unexpected '{line}' in a commit")));
            }
        }
        let committer = committer_line.ok_or_else(|| self.r.bad("a commit has no committer"))?;
        let (_, _, secs) =
            parse_ident(&committer).ok_or_else(|| self.r.bad("bad committer line"))?;

        let mut from: Option<Option<SnapshotId>> = None;
        let mut merge = None;
        let mut changes = Vec::new();
        while let Some(line) = self.r.next_line()? {
            if line.is_empty() {
                break;
            }
            let (cmd, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
            match cmd {
                "from" => from = Some(self.resolve(rest)?),
                "merge" => {
                    if merge.is_some() {
                        return Err(VeloError::unsupported(format!(
                            "line {}: octopus merges (more than two parents) cannot be imported; a snapshot has at most two.",
                            self.r.line
                        )));
                    }
                    merge = Some(self.resolve(rest)?);
                }
                "M" => {
                    let mut parts = rest.splitn(3, ' ');
                    let (Some(mode), Some(dataref), Some(path)) =
                        (parts.next(), parts.next(), parts.next())
                    else {
                        return Err(self.r.bad("a filemodify needs a mode, data and path"));
                    };
                    let path = self.single_path(path)?;
                    let src = if mode == "160000" {
                        DataRef::Inline(Vec::new())
                    } else if dataref == "inline" {
                        let spec = self.r.next_line()?.unwrap_or_default();
                        let Some(spec) = spec.strip_prefix("data ") else {
                            return Err(self.r.bad("inline content needs a data command"));
                        };
                        DataRef::Inline(self.r.data(spec)?)
                    } else if let Some(m) = parse_mark(dataref) {
                        DataRef::Mark(m)
                    } else {
                        return Err(self
                            .r
                            .bad(format!("unsupported data reference '{dataref}'")));
                    };
                    changes.push(Change::Modify {
                        mode: mode.to_string(),
                        src,
                        path,
                    });
                }
                "D" => changes.push(Change::Delete(self.single_path(rest)?)),
                "R" | "C" => {
                    let (src, dst) = self.parse_path(rest)?;
                    let dst = self.single_path(&dst)?;
                    changes.push(if cmd == "R" {
                        Change::Rename(src, dst)
                    } else {
                        Change::Copy(src, dst)
                    });
                }
                "deleteall" => changes.push(Change::DeleteAll),
                _ => {
                    self.r.unread(line);
                    break;
                }
            }
        }

        let parent = match from {
            Some(p) => p,
            None => self.refs.get(refname).cloned(),
        };
        let merge_parent = merge.flatten();

        // The tree: the parent's, plus this commit's changes.
        let repo = self.guard.repo();
        let mut tree: BTreeMap<String, (Src, FileKind)> = BTreeMap::new();
        if let Some(p) = &parent {
            for f in repo.tree_at(p)? {
                tree.insert(f.path, (Src::Stored(f.object), f.kind));
            }
        }
        let mut moved: Vec<(String, String)> = Vec::new();
        for change in changes {
            match change {
                Change::DeleteAll => tree.clear(),
                Change::Delete(p) => {
                    take_matching(&mut tree, &p);
                }
                Change::Rename(src, dst) => {
                    for (suffix, v) in take_matching(&mut tree, &src) {
                        moved.push((format!("{src}{suffix}"), format!("{dst}{suffix}")));
                        tree.insert(format!("{dst}{suffix}"), v);
                    }
                }
                Change::Copy(src, dst) => {
                    for (suffix, v) in take_matching(&mut tree, &src) {
                        tree.insert(format!("{src}{suffix}"), v.clone());
                        tree.insert(format!("{dst}{suffix}"), v);
                    }
                }
                Change::Modify { mode, src, path } => {
                    let kind = match mode.as_str() {
                        "100644" | "644" => FileKind::Regular,
                        "100755" | "755" => FileKind::Executable,
                        "120000" => FileKind::Symlink,
                        "160000" => {
                            self.out.skipped_submodules += 1;
                            take_matching(&mut tree, &path);
                            continue;
                        }
                        other => {
                            return Err(self.r.bad(format!("unsupported file mode '{other}'")))
                        }
                    };
                    let src = match src {
                        DataRef::Inline(b) => Src::Inline(b),
                        DataRef::Mark(m) => match self.blobs.get(&m) {
                            Some(Blob::Bytes(_)) => Src::Mark(m),
                            Some(Blob::Stored(h)) => Src::Stored(h.clone()),
                            None => return Err(self.r.bad(format!("blob :{m} was never defined"))),
                        },
                    };
                    take_matching(&mut tree, &path);
                    tree.insert(path, (src, kind));
                }
            }
        }

        // Velo's own trailers, when the commit has them, say exactly what the
        // snapshot was; otherwise git's fields are all there is.
        let trailer = parse_trailer(&message).map_err(|e| {
            VeloError::invalid(format!(
                "git fast-export stream, line {start_line}: bad Velo trailer: {e}"
            ))
        })?;
        let mut meta = SnapshotMeta::new();
        let text;
        let timestamp_ms;
        let author;
        let renames: Vec<(PathBuf, PathBuf)>;
        let mut branch: Option<BranchName> = None;
        let expected;
        match trailer {
            Some(t) => {
                for (ns, key, value) in &t.meta {
                    // `velo` is reserved; authors arrive through Velo-Author.
                    if ns != RESERVED_NAMESPACE {
                        meta.set(ns.as_str(), key.as_str(), value.as_str())?;
                    }
                }
                author = t.author.and_then(|(name, email)| {
                    if email.is_empty() {
                        Author::new(name)
                    } else {
                        Author::with_email(name, email)
                    }
                    .ok()
                });
                renames = t
                    .renames
                    .iter()
                    .map(|(a, b)| (PathBuf::from(a), PathBuf::from(b)))
                    .collect();
                timestamp_ms = t.time_ms.unwrap_or(secs.saturating_mul(1000));
                branch = t.branch.as_deref().and_then(|b| b.parse().ok());
                expected = Some(t.id);
                text = t.message;
            }
            None => {
                author = parse_ident(author_line.as_deref().unwrap_or(&committer))
                    .and_then(|(name, email, _)| Author::with_email(name, email).ok());
                let mut seen = HashSet::new();
                renames = moved
                    .iter()
                    .filter(|(from, to)| from != to && tree.contains_key(to))
                    .filter(|pair| seen.insert((*pair).clone()))
                    .map(|(a, b)| (PathBuf::from(a), PathBuf::from(b)))
                    .collect();
                timestamp_ms = secs.saturating_mul(1000);
                expected = None;
                text = message;
            }
        }
        let branch = branch.unwrap_or_else(|| {
            refname
                .strip_prefix("refs/heads/")
                .and_then(|b| b.parse().ok())
                .unwrap_or_else(|| self.fallback.clone())
        });

        let mut entries = Vec::with_capacity(tree.len());
        let mut pending: Vec<(u64, String)> = Vec::new();
        for (path, (src, kind)) in tree {
            let content = match src {
                Src::Stored(h) => Content::Stored(h),
                Src::Inline(b) => Content::Bytes(b),
                Src::Mark(m) => {
                    let Some(Blob::Bytes(b)) = self.blobs.get(&m) else {
                        return Err(self.r.bad(format!("blob :{m} was never defined")));
                    };
                    pending.push((m, path.clone()));
                    Content::Bytes(b.clone())
                }
            };
            entries.push(match (content, kind) {
                (Content::Bytes(b), FileKind::Regular) => TreeEntry::file(path, b),
                (Content::Bytes(b), FileKind::Executable) => TreeEntry::executable(path, b),
                (Content::Stored(h), kind) => TreeEntry::stored(path, h, kind),
                (content, kind) => TreeEntry {
                    path,
                    content,
                    kind,
                },
            });
        }

        let id = self.guard.save_tree(SaveTree {
            branch: &branch,
            parent: parent.as_ref(),
            merge_parent: merge_parent.as_ref(),
            message: &text,
            entries,
            meta,
            author: author.as_ref(),
            renames: &renames,
            timestamp_ms: Some(timestamp_ms),
        })?;

        // A blob now in the store is referred to by hash from here on, so a
        // mark used by many commits is hashed and compressed once.
        if !pending.is_empty() {
            let stored: HashMap<String, ObjectHash> = repo
                .tree_at(&id)?
                .into_iter()
                .map(|f| (f.path, f.object))
                .collect();
            for (m, path) in pending {
                if let Some(h) = stored.get(&crate::db::normalise(&path)) {
                    self.blobs.insert(m, Blob::Stored(h.clone()));
                }
            }
        }
        if let Some(expected) = expected {
            if expected != id.as_str() {
                self.out.mismatched_ids += 1;
            }
            self.trailer_branch
                .insert(id.clone(), branch.as_str().to_string());
        }
        if let Some(m) = mark {
            self.marks.insert(m, id.clone());
        }
        if let Some(o) = oid {
            self.oids.insert(o, id.clone());
        }
        self.branches.insert(branch.as_str().to_string());
        self.refs.insert(refname.to_string(), id);
        self.out.commits += 1;
        Ok(())
    }

    fn reset(&mut self, refname: &str) -> Result<()> {
        let mut target = None;
        if let Some(line) = self.r.next_line()? {
            match line.strip_prefix("from ") {
                Some(spec) => target = self.resolve(spec)?,
                None => self.r.unread(line),
            }
        }
        match target {
            Some(id) => {
                self.refs.insert(refname.to_string(), id.clone());
                if let Some(name) = refname.strip_prefix("refs/heads/") {
                    self.point_branch(name, &id)?;
                } else if let Some(name) = refname.strip_prefix("refs/tags/") {
                    self.make_tag(name, &id)?;
                }
            }
            None => {
                self.refs.remove(refname);
            }
        }
        Ok(())
    }

    /// Move (or create) the velo branch a git branch ref stands for.
    ///
    /// An exported name was sanitised for git, so when the snapshot says which
    /// branch it was saved on and that name sanitises to this ref, the original
    /// name is the one that is meant.
    fn point_branch(&mut self, name: &str, id: &SnapshotId) -> Result<()> {
        let mut name = name.to_string();
        if let Some(original) = self.trailer_branch.get(id) {
            if *original == name || sanitize_ref_name(original) == name {
                name = original.clone();
            }
        }
        let branch: BranchName = name
            .parse()
            .map_err(|_| self.r.bad(format!("'{name}' is not a valid branch name")))?;
        let current = self.guard.repo().branch_tip(&branch)?;
        if current.as_ref() != Some(id) {
            if crate::commands::branch_exists(self.guard.conn(), branch.as_str()) {
                crate::commands::branches::set_tip(self.guard, &branch, id)?;
            } else {
                crate::commands::branches::create(self.guard, &branch, Some(id))?;
            }
        }
        self.branches.insert(branch.as_str().to_string());
        Ok(())
    }

    fn make_tag(&mut self, name: &str, id: &SnapshotId) -> Result<()> {
        let tag: TagName = name
            .parse()
            .map_err(|_| self.r.bad(format!("'{name}' is not a valid tag name")))?;
        crate::commands::tag::create(self.guard, &tag, Some(id), true)?;
        self.tags.insert(tag.to_string());
        Ok(())
    }

    /// An annotated tag. The tagger and message have no velo home and are
    /// dropped: the result is a lightweight tag.
    fn tag(&mut self, name: &str) -> Result<()> {
        let name = name.strip_prefix("refs/tags/").unwrap_or(name).to_string();
        let mut target = None;
        loop {
            let Some(line) = self.r.next_line()? else {
                return Err(self.r.bad("a tag has no data"));
            };
            if let Some(spec) = line.strip_prefix("from ") {
                target = self.resolve(spec)?;
            } else if line.starts_with("mark ")
                || line.starts_with("original-oid ")
                || line.starts_with("tagger ")
            {
            } else if let Some(spec) = line.strip_prefix("data ") {
                self.r.data(spec)?;
                break;
            } else {
                return Err(self.r.bad(format!("unexpected '{line}' in a tag")));
            }
        }
        let id = target.ok_or_else(|| self.r.bad("a tag has no 'from'"))?;
        self.make_tag(&name, &id)
    }
}

/// The trailing run of `Velo-*` lines of a commit message, when it is a Velo
/// trailer block (one containing `Velo-Snapshot`) set off by a blank line.
fn parse_trailer(text: &str) -> std::result::Result<Option<Trailer>, String> {
    let body = text.trim_end_matches('\n');
    let mut end = body.len();
    let mut block_start = body.len();
    loop {
        let line_start = body[..end].rfind('\n').map_or(0, |i| i + 1);
        if !body[line_start..end].starts_with("Velo-") {
            break;
        }
        block_start = line_start;
        if line_start == 0 {
            break;
        }
        end = line_start - 1;
    }
    if block_start == body.len() {
        return Ok(None);
    }
    let message = if block_start == 0 {
        ""
    } else {
        match body[..block_start].strip_suffix("\n\n") {
            Some(m) => m,
            None => return Ok(None),
        }
    };
    let mut t = Trailer {
        message: message.to_string(),
        id: String::new(),
        branch: None,
        time_ms: None,
        author: None,
        meta: Vec::new(),
        renames: Vec::new(),
    };
    let quoted = |rest: &str, n: usize| -> std::result::Result<Vec<String>, String> {
        let mut rest = rest;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let (value, after) =
                unquote(rest.trim_start()).ok_or_else(|| format!("cannot read '{rest}'"))?;
            out.push(value);
            rest = after;
        }
        Ok(out)
    };
    for line in body[block_start..].lines() {
        let Some((key, value)) = line.split_once(": ") else {
            continue;
        };
        match key {
            "Velo-Snapshot" => t.id = value.trim().to_string(),
            "Velo-Branch" => t.branch = quoted(value, 1)?.pop(),
            "Velo-Time-Ms" => {
                t.time_ms = Some(
                    value
                        .trim()
                        .parse()
                        .map_err(|_| format!("'{value}' is not a time"))?,
                )
            }
            "Velo-Author" => {
                let mut v = quoted(value, 2)?;
                let email = v.pop().unwrap_or_default();
                t.author = Some((v.pop().unwrap_or_default(), email));
            }
            "Velo-Meta" => {
                let mut v = quoted(value, 3)?;
                let val = v.pop().unwrap_or_default();
                let key = v.pop().unwrap_or_default();
                t.meta.push((v.pop().unwrap_or_default(), key, val));
            }
            "Velo-Rename" => {
                let mut v = quoted(value, 2)?;
                let to = v.pop().unwrap_or_default();
                t.renames.push((v.pop().unwrap_or_default(), to));
            }
            _ => {}
        }
    }
    if t.id.is_empty() {
        return Ok(None);
    }
    Ok(Some(t))
}

/// Remove `path` and everything under it, returning what was removed keyed by
/// the path relative to `path` (empty for `path` itself).
fn take_matching(
    tree: &mut BTreeMap<String, (Src, FileKind)>,
    path: &str,
) -> Vec<(String, (Src, FileKind))> {
    let mut out = Vec::new();
    if let Some(v) = tree.remove(path) {
        out.push((String::new(), v));
    }
    let prefix = format!("{path}/");
    let keys: Vec<String> = tree
        .range(prefix.clone()..)
        .take_while(|(k, _)| k.starts_with(&prefix))
        .map(|(k, _)| k.clone())
        .collect();
    for k in keys {
        if let Some(v) = tree.remove(&k) {
            out.push((k[path.len()..].to_string(), v));
        }
    }
    out
}

/// `Name <email> secs tz` to `(name, email, secs)`.
fn parse_ident(text: &str) -> Option<(String, String, i64)> {
    let lt = text.rfind('<')?;
    let gt = text.rfind('>')?;
    if gt < lt {
        return None;
    }
    let name = text[..lt].trim_end().to_string();
    let email = text[lt + 1..gt].to_string();
    let secs = text[gt + 1..].split_whitespace().next()?.parse().ok()?;
    Some((name, email, secs))
}

/// One C-style quoted string from the start of `input`, and the text after it.
fn unquote_c(input: &str) -> Option<(String, &str)> {
    let bytes = input.strip_prefix('"')?.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let rest = &input[1 + i + 1..];
                return Some((String::from_utf8(out).ok()?, rest));
            }
            b'\\' => {
                i += 1;
                let c = *bytes.get(i)?;
                match c {
                    b'a' => out.push(7),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'v' => out.push(11),
                    b'\\' | b'"' => out.push(c),
                    b'0'..=b'7' => {
                        let mut v = u32::from(c - b'0');
                        let mut n = 1;
                        while n < 3 {
                            match bytes.get(i + 1) {
                                Some(d @ b'0'..=b'7') => {
                                    v = v * 8 + u32::from(d - b'0');
                                    i += 1;
                                    n += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(u8::try_from(v).ok()?);
                    }
                    _ => return None,
                }
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    None
}
