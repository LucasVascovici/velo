//! Every `velody ...` command quoted in help text must parse.
//!
//! Help text that cites a command or flag that does not exist sends users down
//! a dead end, and it used to be found only by the team simulation. This walks
//! every (nested) subcommand, pulls the quoted invocations out of its long help
//! and feeds each to the real parser, so such a mistake fails the suite.

use clap::error::ErrorKind;
use clap::{Command, CommandFactory, Parser};

use super::Cli;

/// Replaces the placeholders help text uses with concrete values, longest
/// first so `<remote>/<branch>` wins over `<branch>`.
const PLACEHOLDERS: &[(&str, &str)] = &[
    ("<remote>/<branch>", "origin/main"),
    ("\"<message>\"", "\"msg\""),
    ("<message>", "\"msg\""),
    ("ours|theirs|both", "ours"),
    ("ours|theirs", "ours"),
    ("<side>", "ours"),
    ("<file>", "f.txt"),
    ("<FILE>", "f.txt"),
    ("<branch>", "feature"),
    ("<snapshot>", "abc123"),
    ("<target>", "abc123"),
    ("<hash>", "abc123"),
    ("<url>", "/tmp/r"),
    ("<name>", "n"),
    ("<n>", "3"),
    ("<COMMAND>", "log"),
];

/// Cuts an invocation at a trailing comment or shell operator.
fn trim_invocation(s: &str) -> &str {
    let mut end = s.len();
    for pat in [" #", " — ", " - ", "|", "&&", ";", ")"] {
        if let Some(i) = s.find(pat) {
            end = end.min(i);
        }
    }
    s[..end].trim()
}

/// Drops `[...]` optional groups and `…`.
fn drop_optional(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            '…' => {}
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Splits on whitespace, honouring double and single quotes.
fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut have = false;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                have = true;
            }
            (None, c) if c.is_whitespace() => {
                if have || !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if have || !cur.is_empty() {
        args.push(cur);
    }
    args
}

/// Finds the quoted `velody ...` invocations in a block of help text.
fn extract(help: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in help.lines() {
        let t = line.trim();
        let t = match t.split_once(". ") {
            Some((n, rest)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => rest,
            _ => t,
        };
        if t.starts_with("velody ") {
            found.push(t.to_string());
        }
        let mut rest = line;
        while let Some(a) = rest.find('`') {
            let after = &rest[a + 1..];
            let Some(b) = after.find('`') else { break };
            let span = &after[..b];
            if span.starts_with("velody ") {
                found.push(span.to_string());
            }
            rest = &after[b + 1..];
        }
    }
    found
}

/// Turns a quoted invocation into argv (without the leading `velody`).
fn normalise(inv: &str) -> Vec<String> {
    let mut s = trim_invocation(inv).to_string();
    for (from, to) in PLACEHOLDERS {
        s = s.replace(from, to);
    }
    let s = drop_optional(&s);
    let mut args = split_args(&s);
    // A bare `N` stands for a count.
    for a in &mut args {
        if a == "N" {
            *a = "3".into();
        }
    }
    args.remove(0);
    args
}

/// Parses one invocation; `Err` carries the clap error kind.
fn check(inv: &str) -> Result<(), ErrorKind> {
    let args = normalise(inv);
    match Cli::try_parse_from(std::iter::once("velody".to_string()).chain(args)) {
        Ok(_) => Ok(()),
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) => Ok(()),
        Err(e) => Err(e.kind()),
    }
}

fn walk(cmd: &mut Command, path: String, out: &mut Vec<(String, String)>) {
    out.push((path.clone(), cmd.render_long_help().to_string()));
    for sub in cmd.get_subcommands_mut() {
        let p = format!("{path} {}", sub.get_name());
        walk(sub, p, out);
    }
}

#[test]
fn every_quoted_command_in_help_parses() {
    let mut cmd = Cli::command();
    cmd.build();
    let mut helps = Vec::new();
    walk(&mut cmd, "velody".to_string(), &mut helps);
    let mut total = 0;
    let mut failures = Vec::new();
    for (path, help) in &helps {
        for inv in extract(help) {
            total += 1;
            if let Err(kind) = check(&inv) {
                failures.push(format!("[{path}] `{inv}` -> {kind:?}"));
            }
        }
    }
    assert!(total > 40, "only found {total} quoted invocations");
    assert!(
        failures.is_empty(),
        "help quotes unparseable commands:\n{}",
        failures.join("\n")
    );
}

#[test]
fn extractor_flags_a_bad_example() {
    let found = extract("  velody diff f.txt --conflict\n  Use `velody save \"x\"` too");
    assert_eq!(found.len(), 2);
    assert!(check(&found[0]).is_err());
    assert!(check(&found[1]).is_ok());
}

/// Placeholders that only the docs use, applied before the shared ones.
const DOC_PLACEHOLDERS: &[(&str, &str)] = &[
    ("<path>", "p"),
    ("<dir>", "d"),
    ("<remote>", "origin"),
    ("<tag>", "v1"),
    ("<version>", "1"),
];

/// Doc lines that are intentionally not valid commands, as
/// `(file suffix, exact line, reason)`.
const DOC_ALLOWLIST: &[(&str, &str, &str)] = &[(
    "COLLABORATION.md",
    "velody sync   (or by hand: velody fetch, velody merge origin/main, velody save \"Merge origin/main\", velody push)",
    "sample `velody status` output, a hint line rather than a command",
)];

/// Fence info strings whose blocks hold shell sessions.
const SHELL_INFO: &[&str] = &["", "sh", "bash", "shell", "console", "text"];

/// Finds `velody ...` invocations in shell-like fenced blocks of a markdown
/// document, returning `(line number, invocation)`.
fn extract_markdown(md: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut in_block = false;
    let mut shell = false;
    for (i, line) in md.lines().enumerate() {
        let t = line.trim();
        if let Some(info) = t.strip_prefix("```") {
            if in_block {
                in_block = false;
            } else {
                in_block = true;
                shell = SHELL_INFO.contains(&info.trim());
            }
            continue;
        }
        if !in_block || !shell {
            continue;
        }
        let t = t
            .strip_prefix("$ ")
            .or_else(|| t.strip_prefix("> "))
            .unwrap_or(t);
        if t.starts_with("velody ") {
            found.push((i + 1, t.to_string()));
        }
    }
    found
}

fn collect_md(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_md(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

#[test]
fn every_velody_command_in_docs_parses() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    let readme = root.join("README.md");
    if readme.is_file() {
        files.push(readme);
    }
    collect_md(&root.join("docs"), &mut files);
    if files.is_empty() {
        return; // a packaged crate has no docs beside it
    }
    files.sort();
    let mut total = 0;
    let mut failures = Vec::new();
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let name = f.to_string_lossy().replace('\\', "/");
        for (n, inv) in extract_markdown(&text) {
            if DOC_ALLOWLIST
                .iter()
                .any(|(suffix, line, _)| name.ends_with(suffix) && inv == *line)
            {
                continue;
            }
            total += 1;
            let mut s = inv.clone();
            for (from, to) in DOC_PLACEHOLDERS {
                s = s.replace(from, to);
            }
            if let Err(kind) = check(&s) {
                failures.push(format!("{name}:{n}: `{inv}` -> {kind:?}"));
            }
        }
    }
    assert!(total > 50, "only found {total} documented invocations");
    assert!(
        failures.is_empty(),
        "docs quote unparseable commands:\n{}",
        failures.join("\n")
    );
}

#[test]
fn markdown_extractor_handles_prompts_comments_and_other_blocks() {
    let md = "text\n```sh\n$ velody save \"x\"  # note\n> velody log\nvelody: output\nls\n```\n```rust\nvelody nope\n```\n```\nvelody status\n```\n";
    let got = extract_markdown(md);
    let invs: Vec<&str> = got.iter().map(|(_, s)| s.as_str()).collect();
    assert_eq!(
        invs,
        ["velody save \"x\"  # note", "velody log", "velody status"]
    );
    assert_eq!(got[0].0, 3);
    assert!(check(&got[0].1).is_ok());
}
