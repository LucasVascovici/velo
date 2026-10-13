//! Every `velo ...` command quoted in help text must parse.
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

/// Finds the quoted `velo ...` invocations in a block of help text.
fn extract(help: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in help.lines() {
        let t = line.trim();
        let t = match t.split_once(". ") {
            Some((n, rest)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => rest,
            _ => t,
        };
        if t.starts_with("velo ") {
            found.push(t.to_string());
        }
        let mut rest = line;
        while let Some(a) = rest.find('`') {
            let after = &rest[a + 1..];
            let Some(b) = after.find('`') else { break };
            let span = &after[..b];
            if span.starts_with("velo ") {
                found.push(span.to_string());
            }
            rest = &after[b + 1..];
        }
    }
    found
}

/// Turns a quoted invocation into argv (without the leading `velo`).
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
    match Cli::try_parse_from(std::iter::once("velo".to_string()).chain(args)) {
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
    walk(&mut cmd, "velo".to_string(), &mut helps);
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
    let found = extract("  velo diff f.txt --conflict\n  Use `velo save \"x\"` too");
    assert_eq!(found.len(), 2);
    assert!(check(&found[0]).is_err());
    assert!(check(&found[1]).is_ok());
}
