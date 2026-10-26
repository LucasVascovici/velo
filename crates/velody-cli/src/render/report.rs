//! Render the audit report as one self-contained HTML file, or as JSON.
//!
//! Everything in a report is attacker-controlled: messages, metadata, author
//! names, branch names, file names and line text all come from whoever wrote
//! the history. An audit page that ran script from the history it audits
//! would be worse than no page, so the rules here are narrow and total:
//!
//! - every string from the model goes through [`esc_into`], the only function
//!   that writes model text into markup;
//! - model text is only ever element content, never part of an attribute, so
//!   there is no `href`, `src`, `style` or event attribute to inject into;
//! - element ids come from indices, never from paths or names;
//! - the page has no `<script>` element (expand and collapse use
//!   `<details>`), no remote resource, and a Content-Security-Policy that
//!   forbids everything but its own inline style.
//!
//! The renderer works on a small [`View`] converted from the core model, so the
//! fuzz tests can feed it arbitrary strings without constructing the
//! `#[non_exhaustive]` model types.

use std::fmt::Write as _;

use serde_json::{json, Value};
use velody_core::commands::report::{
    PathChange, Report, SignatureState, SignatureStatus, SnapshotRecord,
};

/// The policy written into the file. `default-src 'none'` forbids scripts,
/// frames, fonts, connections and everything else; styles are inline only.
const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'";

const STYLE: &str = "\
body { font: 14px/1.45 system-ui, sans-serif; margin: 2rem auto; max-width: 70rem; padding: 0 1rem; color: #1b1f24; }
code, pre, .code { font-family: ui-monospace, Consolas, monospace; }
pre { margin: 0.3rem 0; white-space: pre-wrap; overflow-wrap: anywhere; }
h1 { margin-bottom: 0.2rem; }
.muted { color: #6a737d; }
.verdict { padding: 0.5rem 0.8rem; border-radius: 4px; font-weight: 600; }
.verdict.ok { background: #dafbe1; color: #116329; }
.verdict.bad { background: #ffebe9; color: #a40e26; }
.verdict.unchecked { background: #eaeef2; color: #424a53; }
details { border: 1px solid #d0d7de; border-radius: 4px; margin: 0.4rem 0; padding: 0.3rem 0.6rem; }
summary { cursor: pointer; }
table { border-collapse: collapse; margin: 0.4rem 0; }
th, td { border: 1px solid #d0d7de; padding: 0.15rem 0.5rem; text-align: left; vertical-align: top; }
td.code { white-space: pre-wrap; overflow-wrap: anywhere; }
.badge { border-radius: 3px; padding: 0 0.4rem; font-size: 12px; font-weight: 600; }
.badge.trusted { background: #dafbe1; color: #116329; }
.badge.untrusted { background: #fff8c5; color: #7d4e00; }
.badge.invalid { background: #ffebe9; color: #a40e26; }
.badge.unverifiable { background: #ffebe9; color: #a40e26; }
.badge.unsigned { background: #eaeef2; color: #424a53; }
.chg { font-weight: 600; }
";

/// What the page says about integrity.
#[derive(Clone, Debug)]
pub enum Verdict {
    NotChecked,
    Ok {
        checked: usize,
        verified: usize,
    },
    Problems {
        problems: usize,
        checked: usize,
        verified: usize,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum SigState {
    Trusted,
    Untrusted,
    Invalid,
    Unverifiable,
}

impl SigState {
    fn class(self) -> &'static str {
        match self {
            SigState::Trusted => "trusted",
            SigState::Untrusted => "untrusted",
            SigState::Invalid => "invalid",
            SigState::Unverifiable => "unverifiable",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SigView {
    pub fingerprint: String,
    pub state: SigState,
}

#[derive(Clone, Copy, Debug)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Clone, Debug)]
pub struct ChangeView {
    pub path: String,
    pub kind: ChangeKind,
    /// Only meaningful for [`ChangeKind::Renamed`].
    pub from: String,
}

#[derive(Clone, Debug)]
pub struct SnapView {
    pub id: String,
    pub short: String,
    pub time: String,
    pub author: Option<String>,
    pub message: String,
    pub has_parent: bool,
    pub has_merge_parent: bool,
    pub parent: String,
    pub merge_parent: String,
    pub meta: Vec<(String, String, String)>,
    pub signatures: Vec<SigView>,
    pub changes: Vec<ChangeView>,
}

#[derive(Clone, Debug)]
pub struct BranchView {
    pub name: String,
    pub tip_short: Option<String>,
    pub snapshots: Vec<SnapView>,
}

#[derive(Clone, Debug)]
pub struct LineView {
    pub line_no: usize,
    pub snapshot_short: Option<String>,
    pub author: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct FileView {
    pub path: String,
    pub at_short: String,
    pub lines: Vec<LineView>,
}

/// Everything the page shows, as plain strings and numbers.
#[derive(Clone, Debug)]
pub struct View {
    pub repo: String,
    pub generated: String,
    pub verdict: Verdict,
    pub branches: Vec<BranchView>,
    pub files: Vec<FileView>,
    pub skipped: Vec<String>,
}

/// Milliseconds since the epoch as `YYYY-MM-DD HH:MM:SS UTC`.
pub fn format_ms(ms: i64) -> String {
    match chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms) {
        Some(t) => t.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        None => format!("{ms} ms"),
    }
}

fn author_text(a: &velody_core::Author) -> String {
    a.to_string()
}

fn short(id: &velody_core::SnapshotId) -> String {
    id.short().to_string()
}

fn state_of(s: &SignatureStatus) -> SigState {
    match s.state {
        SignatureState::Trusted => SigState::Trusted,
        SignatureState::Untrusted => SigState::Untrusted,
        SignatureState::Invalid => SigState::Invalid,
        // The enum is non_exhaustive: a state this build does not know cannot
        // be vouched for, so it is shown as unverifiable.
        _ => SigState::Unverifiable,
    }
}

fn snap_view(s: &SnapshotRecord) -> SnapView {
    SnapView {
        id: s.id.as_str().to_string(),
        short: short(&s.id),
        time: format_ms(s.created_at_ms),
        author: s.author.as_ref().map(author_text),
        message: s.message.clone(),
        has_parent: s.parent.is_some(),
        has_merge_parent: s.merge_parent.is_some(),
        parent: s
            .parent
            .as_ref()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default(),
        merge_parent: s
            .merge_parent
            .as_ref()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default(),
        meta: s.meta.clone(),
        signatures: s
            .signatures
            .iter()
            .map(|g| SigView {
                fingerprint: g.fingerprint.clone(),
                state: state_of(g),
            })
            .collect(),
        changes: s
            .changes
            .iter()
            .map(|c| {
                let (kind, from) = match &c.change {
                    PathChange::Added => (ChangeKind::Added, String::new()),
                    PathChange::Modified => (ChangeKind::Modified, String::new()),
                    PathChange::Deleted => (ChangeKind::Deleted, String::new()),
                    PathChange::Renamed { from } => (ChangeKind::Renamed, from.clone()),
                    _ => (ChangeKind::Modified, String::new()),
                };
                ChangeView {
                    path: c.path.clone(),
                    kind,
                    from,
                }
            })
            .collect(),
    }
}

/// Convert the core model into what the page shows.
pub fn view(report: &Report, repo: &str, generated: &str) -> View {
    View {
        repo: repo.to_string(),
        generated: generated.to_string(),
        verdict: match &report.integrity {
            None => Verdict::NotChecked,
            Some(i) if i.healthy => Verdict::Ok {
                checked: i.snapshots_checked,
                verified: i.ids_verified,
            },
            Some(i) => Verdict::Problems {
                problems: i.problems,
                checked: i.snapshots_checked,
                verified: i.ids_verified,
            },
        },
        branches: report
            .branches
            .iter()
            .map(|b| BranchView {
                name: b.branch.as_str().to_string(),
                tip_short: b.tip.as_ref().map(short),
                snapshots: b.snapshots.iter().map(snap_view).collect(),
            })
            .collect(),
        files: report
            .files
            .iter()
            .map(|f| FileView {
                path: f.path.clone(),
                at_short: short(&f.at),
                lines: f
                    .lines
                    .iter()
                    .map(|l| LineView {
                        line_no: l.line_no,
                        snapshot_short: l.snapshot.as_ref().map(short),
                        author: l.author.as_ref().map(author_text),
                        text: l.text.clone(),
                    })
                    .collect(),
            })
            .collect(),
        skipped: report.skipped_files.clone(),
    }
}

/// Append `text` to `out` as safe element content.
///
/// The one place model text enters markup. `& < > " '` become entities, so
/// nothing can open a tag or close an attribute. Control characters (except
/// newline and tab), DEL, C1 controls and the bidirectional controls are
/// written as visible `\u{XXXX}` text instead of raw: a raw U+202E would let a
/// path or message display backwards and pass for something else.
pub fn esc_into(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\n' | '\t' => out.push(c),
            c if c.is_control()
                || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
                || matches!(c, '\u{200E}' | '\u{200F}' | '\u{061C}') =>
            {
                let _ = write!(out, "\\u{{{:04X}}}", c as u32);
            }
            c => out.push(c),
        }
    }
}

fn badges(out: &mut String, sigs: &[SigView], with_fingerprint: bool) {
    if sigs.is_empty() {
        out.push_str("<span class=\"badge unsigned\">unsigned</span>");
    }
    for s in sigs {
        let _ = write!(
            out,
            "<span class=\"badge {0}\">{0}</span> ",
            s.state.class()
        );
        if with_fingerprint {
            out.push_str("<code>");
            esc_into(out, &s.fingerprint);
            out.push_str("</code> ");
        }
    }
}

fn snapshot(out: &mut String, bi: usize, si: usize, s: &SnapView) {
    let _ = write!(
        out,
        "<details class=\"snap\" id=\"b{bi}-s{si}\"><summary><code>"
    );
    esc_into(out, &s.short);
    out.push_str("</code> <time>");
    esc_into(out, &s.time);
    out.push_str("</time> <span class=\"author\">");
    match &s.author {
        Some(a) => esc_into(out, a),
        None => out.push_str("no author"),
    }
    out.push_str("</span> <span class=\"msg\">");
    esc_into(out, s.message.split('\n').next().unwrap_or(""));
    out.push_str("</span> ");
    badges(out, &s.signatures, false);
    out.push_str("</summary>\n");

    out.push_str("<p>Snapshot <code>");
    esc_into(out, &s.id);
    out.push_str("</code></p>\n");
    if s.has_parent {
        out.push_str("<p>Parent <code>");
        esc_into(out, &s.parent);
        out.push_str("</code></p>\n");
    }
    if s.has_merge_parent {
        out.push_str("<p>Merge parent <code>");
        esc_into(out, &s.merge_parent);
        out.push_str("</code></p>\n");
    }
    out.push_str("<pre class=\"message\">");
    esc_into(out, &s.message);
    out.push_str("</pre>\n");

    if s.meta.is_empty() {
        out.push_str("<p class=\"muted\">No metadata.</p>\n");
    } else {
        out.push_str(
            "<table class=\"meta\"><tr><th>namespace</th><th>key</th><th>value</th></tr>\n",
        );
        for (ns, k, v) in &s.meta {
            out.push_str("<tr><td>");
            esc_into(out, ns);
            out.push_str("</td><td>");
            esc_into(out, k);
            out.push_str("</td><td class=\"code\">");
            esc_into(out, v);
            out.push_str("</td></tr>\n");
        }
        out.push_str("</table>\n");
    }

    out.push_str("<p>Signatures: ");
    badges(out, &s.signatures, true);
    out.push_str("</p>\n");

    if s.changes.is_empty() {
        out.push_str("<p class=\"muted\">No changed files.</p>\n");
    } else {
        out.push_str("<ul class=\"changes\">\n");
        for c in &s.changes {
            let word = match c.kind {
                ChangeKind::Added => "added",
                ChangeKind::Modified => "modified",
                ChangeKind::Deleted => "deleted",
                ChangeKind::Renamed => "renamed",
            };
            let _ = write!(out, "<li><span class=\"chg {word}\">{word}</span> ");
            if matches!(c.kind, ChangeKind::Renamed) {
                out.push_str("<code>");
                esc_into(out, &c.from);
                out.push_str("</code> to ");
            }
            out.push_str("<code>");
            esc_into(out, &c.path);
            out.push_str("</code></li>\n");
        }
        out.push_str("</ul>\n");
    }
    out.push_str("</details>\n");
}

/// Render the whole page.
pub fn html(v: &View) -> String {
    let mut out = String::new();
    out.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    let _ = writeln!(
        out,
        "<meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">"
    );
    out.push_str("<title>velody report</title>\n<style>\n");
    out.push_str(STYLE);
    out.push_str("</style>\n</head>\n<body>\n<header>\n<h1>Velody audit report</h1>\n");
    out.push_str("<p>Repository <code>");
    esc_into(&mut out, &v.repo);
    out.push_str("</code></p>\n<p class=\"muted\">Generated ");
    esc_into(&mut out, &v.generated);
    out.push_str("</p>\n</header>\n");

    match &v.verdict {
        Verdict::NotChecked => {
            out.push_str("<p class=\"verdict unchecked\">Integrity: not checked</p>\n")
        }
        Verdict::Ok { checked, verified } => {
            let _ = writeln!(
                out,
                "<p class=\"verdict ok\">fsck recomputed {verified} of {checked} snapshot ids: OK</p>"
            );
        }
        Verdict::Problems {
            problems,
            checked,
            verified,
        } => {
            let _ = writeln!(
                out,
                "<p class=\"verdict bad\">fsck found {problems} problem(s); recomputed {verified} of {checked} snapshot ids</p>"
            );
        }
    }

    out.push_str("<h2>Timeline</h2>\n");
    for (bi, b) in v.branches.iter().enumerate() {
        let _ = write!(
            out,
            "<section class=\"branch\" id=\"branch-{bi}\">\n<h3>Branch <code>"
        );
        esc_into(&mut out, &b.name);
        out.push_str("</code>");
        if let Some(tip) = &b.tip_short {
            out.push_str(" <span class=\"muted\">tip <code>");
            esc_into(&mut out, tip);
            out.push_str("</code></span>");
        }
        out.push_str("</h3>\n");
        if b.snapshots.is_empty() {
            out.push_str("<p class=\"muted\">No snapshots in range.</p>\n");
        }
        for (si, s) in b.snapshots.iter().enumerate() {
            snapshot(&mut out, bi, si, s);
        }
        out.push_str("</section>\n");
    }

    out.push_str("<h2>Line provenance</h2>\n");
    if v.files.is_empty() {
        out.push_str("<p class=\"muted\">No files blamed.</p>\n");
    }
    for (fi, f) in v.files.iter().enumerate() {
        let _ = write!(
            out,
            "<details class=\"file\" id=\"file-{fi}\"><summary><code>"
        );
        esc_into(&mut out, &f.path);
        out.push_str("</code> at <code>");
        esc_into(&mut out, &f.at_short);
        let _ = writeln!(out, "</code> ({} lines)</summary>", f.lines.len());
        out.push_str(
            "<table><tr><th>line</th><th>snapshot</th><th>author</th><th>text</th></tr>\n",
        );
        for l in &f.lines {
            let _ = write!(out, "<tr><td>{}</td><td><code>", l.line_no);
            match &l.snapshot_short {
                Some(s) => esc_into(&mut out, s),
                None => out.push_str("unknown"),
            }
            out.push_str("</code></td><td>");
            match &l.author {
                Some(a) => esc_into(&mut out, a),
                None => out.push_str("no author"),
            }
            out.push_str("</td><td class=\"code\">");
            esc_into(&mut out, &l.text);
            out.push_str("</td></tr>\n");
        }
        out.push_str("</table>\n</details>\n");
    }
    if !v.skipped.is_empty() {
        out.push_str("<h3>Skipped files</h3>\n<ul>\n");
        for p in &v.skipped {
            out.push_str("<li><code>");
            esc_into(&mut out, p);
            out.push_str("</code></li>\n");
        }
        out.push_str("</ul>\n");
    }
    out.push_str("</body>\n</html>\n");
    out
}

fn author_json(a: &Option<velody_core::Author>) -> Value {
    match a {
        Some(a) => json!({ "name": a.name(), "email": a.email() }),
        None => Value::Null,
    }
}

fn id_json(id: &Option<velody_core::SnapshotId>) -> Value {
    match id {
        Some(i) => json!(i.as_str()),
        None => Value::Null,
    }
}

/// The report as JSON, mirroring every field of the model.
pub fn json(report: &Report, repo: &str, generated: &str) -> Value {
    let branches: Vec<Value> = report
        .branches
        .iter()
        .map(|b| {
            let snaps: Vec<Value> = b
                .snapshots
                .iter()
                .map(|s| {
                    let sigs: Vec<Value> = s
                        .signatures
                        .iter()
                        .map(|g| {
                            json!({
                                "public_key": g.public_key,
                                "fingerprint": g.fingerprint,
                                "state": state_of(g).class(),
                            })
                        })
                        .collect();
                    let changes: Vec<Value> = s
                        .changes
                        .iter()
                        .map(|c| match &c.change {
                            PathChange::Added => json!({"path": c.path, "change": "added"}),
                            PathChange::Modified => json!({"path": c.path, "change": "modified"}),
                            PathChange::Deleted => json!({"path": c.path, "change": "deleted"}),
                            PathChange::Renamed { from } => {
                                json!({"path": c.path, "change": "renamed", "from": from})
                            }
                            _ => json!({"path": c.path, "change": "unknown"}),
                        })
                        .collect();
                    let meta: Vec<Value> = s
                        .meta
                        .iter()
                        .map(|(n, k, v)| json!({"namespace": n, "key": k, "value": v}))
                        .collect();
                    json!({
                        "id": s.id.as_str(),
                        "parent": id_json(&s.parent),
                        "merge_parent": id_json(&s.merge_parent),
                        "created_at_ms": s.created_at_ms,
                        "message": s.message,
                        "author": author_json(&s.author),
                        "meta": meta,
                        "signatures": sigs,
                        "changes": changes,
                    })
                })
                .collect();
            json!({
                "branch": b.branch.as_str(),
                "tip": id_json(&b.tip),
                "snapshots": snaps,
            })
        })
        .collect();
    let files: Vec<Value> = report
        .files
        .iter()
        .map(|f| {
            let lines: Vec<Value> = f
                .lines
                .iter()
                .map(|l| {
                    json!({
                        "line_no": l.line_no,
                        "text": l.text,
                        "snapshot": id_json(&l.snapshot),
                        "author": author_json(&l.author),
                    })
                })
                .collect();
            json!({"path": f.path, "at": f.at.as_str(), "lines": lines})
        })
        .collect();
    let integrity = match &report.integrity {
        Some(i) => json!({
            "snapshots_checked": i.snapshots_checked,
            "ids_verified": i.ids_verified,
            "problems": i.problems,
            "healthy": i.healthy,
        }),
        None => Value::Null,
    };
    json!({
        "repository": repo,
        "generated": generated,
        "branches": branches,
        "files": files,
        "skipped_files": report.skipped_files,
        "integrity": integrity,
    })
}

/// Say where the report went.
pub fn print_written(path: &std::path::Path) {
    println!("Wrote report to {}", path.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn esc(text: &str) -> String {
        let mut s = String::new();
        esc_into(&mut s, text);
        s
    }

    /// A page with snapshots, a rename, metadata, signatures, a blamed file
    /// and skipped files, every string drawn from `next`. Its structure does
    /// not depend on the strings, which is what the `<` count relies on.
    fn fixture(next: &mut dyn FnMut() -> String) -> View {
        fn snap(s: &mut dyn FnMut() -> String, signed: bool, root: bool) -> SnapView {
            SnapView {
                id: s(),
                short: s(),
                time: s(),
                author: if root { None } else { Some(s()) },
                message: s(),
                has_parent: !root,
                has_merge_parent: false,
                parent: s(),
                merge_parent: s(),
                meta: vec![(s(), s(), s()), (s(), s(), s())],
                signatures: if signed {
                    vec![
                        SigView {
                            fingerprint: s(),
                            state: SigState::Trusted,
                        },
                        SigView {
                            fingerprint: s(),
                            state: SigState::Invalid,
                        },
                    ]
                } else {
                    Vec::new()
                },
                changes: vec![
                    ChangeView {
                        path: s(),
                        kind: ChangeKind::Added,
                        from: s(),
                    },
                    ChangeView {
                        path: s(),
                        kind: ChangeKind::Renamed,
                        from: s(),
                    },
                ],
            }
        }
        let b0 = BranchView {
            name: next(),
            tip_short: Some(next()),
            snapshots: vec![snap(next, true, false), snap(next, false, true)],
        };
        let b1 = BranchView {
            name: next(),
            tip_short: None,
            snapshots: vec![snap(next, true, true)],
        };
        let file = FileView {
            path: next(),
            at_short: next(),
            lines: vec![
                LineView {
                    line_no: 1,
                    snapshot_short: Some(next()),
                    author: Some(next()),
                    text: next(),
                },
                LineView {
                    line_no: 2,
                    snapshot_short: None,
                    author: None,
                    text: next(),
                },
            ],
        };
        View {
            repo: next(),
            generated: next(),
            verdict: Verdict::Ok {
                checked: 3,
                verified: 3,
            },
            branches: vec![b0, b1],
            files: vec![file],
            skipped: vec![next(), next()],
        }
    }

    fn render_with(pool: &[String]) -> String {
        let mut i = 0;
        html(&fixture(&mut || {
            let t = pool[i % pool.len()].clone();
            i += 1;
            t
        }))
    }

    fn template() -> String {
        html(&fixture(&mut String::new))
    }

    /// Text has no raw `<`, so the tags are exactly what sits between a `<`
    /// and the next `>`; none may carry an `on...=` event-handler attribute.
    fn assert_no_event_attrs(out: &str) {
        for part in out.split('<').skip(1) {
            let tag = part.split('>').next().unwrap_or("").to_ascii_lowercase();
            let bytes = tag.as_bytes();
            for i in 1..bytes.len() {
                if bytes[i] == b'o'
                    && bytes[i - 1].is_ascii_whitespace()
                    && tag[i..].starts_with("on")
                {
                    let rest = &tag[i + 2..];
                    let name_len = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
                    let after = rest[name_len..].trim_start();
                    assert!(
                        !(name_len > 0 && after.starts_with('=')),
                        "event attribute in tag: {tag}"
                    );
                }
            }
        }
    }

    fn check(out: &str, template_lt: usize) {
        assert!(!out.to_ascii_lowercase().contains("<script"));
        assert_eq!(out.matches('<').count(), template_lt, "stray markup");
        assert!(out.contains(&format!(
            "<meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">"
        )));
        assert_no_event_attrs(out);
    }

    fn seeds() -> Vec<String> {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/tree_paths");
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("fuzz seeds are in the repository") {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            out.push(String::from_utf8_lossy(&bytes).into_owned());
        }
        assert!(!out.is_empty());
        out
    }

    fn hostile() -> Vec<String> {
        let mut v: Vec<String> = [
            "<script>alert(1)</script>",
            "\"><img src=x onerror=alert(1)>",
            "</style><script>",
            "javascript:alert(1)",
            "\u{202E}",
            "\0",
            "a\u{0}b\u{7f}c\u{1b}[31m\r\n\t",
            "' onmouseover='alert(1)",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        v.push("<script>".repeat(100 * 1024 / 8));
        v.extend(seeds());
        v
    }

    #[test]
    fn escapes_markup_and_quotes() {
        assert_eq!(
            esc("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }

    #[test]
    fn controls_and_bidi_are_visible_text() {
        assert_eq!(esc("a\u{202E}b"), "a\\u{202E}b");
        assert_eq!(esc("\0\x1b\x7f"), "\\u{0000}\\u{001B}\\u{007F}");
        assert_eq!(esc("\u{2066}\u{2069}"), "\\u{2066}\\u{2069}");
        assert_eq!(esc("line\n\tnext"), "line\n\tnext");
        assert!(!esc("x\u{202E}y").contains('\u{202E}'));
    }

    #[test]
    fn hostile_corpus_in_every_field_stays_inert() {
        let lt = template().matches('<').count();
        let all = hostile();
        for h in &all {
            check(&render_with(std::slice::from_ref(h)), lt);
        }
        check(&render_with(&all), lt);
    }

    #[test]
    fn template_has_csp_no_script_and_no_remote_urls() {
        let t = template();
        check(&t, t.matches('<').count());
        for bad in ["http:", "https:", "//", "<script"] {
            assert!(!t.contains(bad), "{bad}");
        }
    }

    #[test]
    fn verdict_wording() {
        let mut v = fixture(&mut String::new);
        assert!(html(&v).contains("fsck recomputed 3 of 3 snapshot ids: OK"));
        v.verdict = Verdict::NotChecked;
        assert!(html(&v).contains("not checked"));
        v.verdict = Verdict::Problems {
            problems: 2,
            checked: 3,
            verified: 1,
        };
        assert!(html(&v).contains("verdict bad"));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn arbitrary_text_never_adds_markup(
            pool in proptest::collection::vec(
                prop_oneof![
                    any::<String>(),
                    proptest::sample::select(hostile()),
                    ".{0,20}",
                ],
                1..80,
            )
        ) {
            let lt = template().matches('<').count();
            check(&render_with(&pool), lt);
        }
    }
}
