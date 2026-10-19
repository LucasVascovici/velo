//! Lint: no SQL string outside `tree_store` names a table that holds a tree.
//!
//! Reading around the accessor is how a storage change breaks callers one at
//! a time, so it is a failing build rather than a review comment. The
//! technique is the same as the help-parses and parity checks: scan the
//! source text. Only string literals count; comments and identifiers (such
//! as the public `orphan_file_map` field) do not.

use std::path::{Path, PathBuf};

/// Tables whose rows are a snapshot's tree. 20.3-b adds the v4 names.
const TREE_TABLES: &[&str] = &["file_map"];

/// Files allowed to name a tree table, each with the reason.
const EXEMPT: &[(&str, &str)] = &[
    (
        "db.rs",
        "defines the schema, which has to name every table it creates",
    ),
    (
        "tests.rs",
        "tests inspect storage directly on purpose, to prove what is on disk",
    ),
    (
        "tests/",
        "tests inspect storage directly on purpose, to prove what is on disk",
    ),
    (
        "tree_store.rs",
        "is the accessor: the one module that owns the tree tables",
    ),
];

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The string literals of Rust source `src`, comments and char literals
/// skipped. Normal and raw (`r#"..."#`) strings are both handled.
fn string_literals(src: &str) -> Vec<String> {
    let b: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            let mut depth = 1;
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if c == 'r' && (i == 0 || !is_word(b[i - 1])) && raw_opens(&b, i) {
            let mut j = i + 1;
            let mut hashes = 0;
            while b[j] == '#' {
                hashes += 1;
                j += 1;
            }
            j += 1;
            let start = j;
            let mut close = vec!['"'];
            close.extend(std::iter::repeat_n('#', hashes));
            while j + close.len() <= b.len() && b[j..j + close.len()] != close[..] {
                j += 1;
            }
            out.push(b[start..j.min(b.len())].iter().collect());
            i = j + close.len();
        } else if c == '"' {
            let mut j = i + 1;
            let mut s = String::new();
            while j < b.len() && b[j] != '"' {
                if b[j] == '\\' {
                    j += 1;
                }
                if j < b.len() {
                    s.push(b[j]);
                }
                j += 1;
            }
            out.push(s);
            i = j + 1;
        } else if c == '\'' {
            // A char literal (`'"'`, `'\''`) or a lifetime; only the former
            // can hide a quote, and it closes within a few characters.
            if b.get(i + 1) == Some(&'\\') {
                i += 2;
                while i < b.len() && b[i] != '\'' {
                    i += 1;
                }
                i += 1;
            } else if b.get(i + 2) == Some(&'\'') {
                i += 3;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Whether the `r` at `i` opens a raw string (`r"`, `r#"`, ...).
fn raw_opens(b: &[char], i: usize) -> bool {
    let mut j = i + 1;
    while b.get(j) == Some(&'#') {
        j += 1;
    }
    b.get(j) == Some(&'"')
}

/// The first of `tables` that `lit` mentions as a whole word.
fn names_table(lit: &str, tables: &[&str]) -> Option<String> {
    for t in tables {
        let mut from = 0;
        while let Some(p) = lit[from..].find(t) {
            let s = from + p;
            let e = s + t.len();
            let before = lit[..s].chars().next_back().is_none_or(|c| !is_word(c));
            let after = lit[e..].chars().next().is_none_or(|c| !is_word(c));
            if before && after {
                return Some((*t).to_string());
            }
            from = e;
        }
    }
    None
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn exempt(rel: &str) -> bool {
    EXEMPT.iter().any(|(p, _)| {
        if p.ends_with('/') {
            rel.starts_with(p)
        } else {
            rel == *p
        }
    })
}

#[test]
fn no_sql_outside_tree_store_names_a_tree_table() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&src, &mut files);
    assert!(files.len() > 20, "the scan found too few files");
    let mut bad = Vec::new();
    for f in files {
        let rel = f
            .strip_prefix(&src)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if exempt(&rel) {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        for lit in string_literals(&text) {
            if let Some(t) = names_table(&lit, TREE_TABLES) {
                bad.push(format!("{rel}: a string names `{t}`: {}", lit.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "SQL outside tree_store names a tree table; go through the accessor:\n{}",
        bad.join("\n")
    );
}

#[test]
fn every_exemption_has_a_reason() {
    for (file, reason) in EXEMPT {
        assert!(reason.len() > 10, "{file} is exempt without a reason");
    }
}

#[test]
fn scanner_flags_a_planted_literal_and_ignores_comments() {
    let hit = |s: &str| {
        string_literals(s)
            .iter()
            .any(|l| names_table(l, TREE_TABLES).is_some())
    };
    assert!(hit(r#"let q = "SELECT * FROM file_map";"#));
    assert!(hit(
        r##"let q = r#"SELECT path FROM file_map WHERE a = "x""#;"##
    ));
    assert!(!hit("// SELECT * FROM file_map\nlet x = 1;"));
    assert!(!hit("/* \"file_map\" */ let x = 1;"));
    assert!(!hit("let orphan_file_map = 3;"));
    assert!(!hit(r#"let q = "SELECT * FROM file_map_x";"#));
    assert!(!hit(r#"let c = '"'; let d = 1;"#));
}
