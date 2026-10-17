//! No SQL outside `treestore` may name a tree table.
//!
//! A reader or writer that names `file_map`, `file_delta` or `delta_base`
//! itself would quietly see the wrong files once a snapshot's tree is stored as
//! a delta, rather than fail. This test reads every Rust source file under
//! `crates/*/src` and fails, naming the file and line, on any SQL clause that
//! does. Identifiers such as `bundle.file_map` or `orphan_file_map` are not SQL
//! and are not flagged.

use std::path::{Path, PathBuf};

use regex::Regex;

/// Files allowed to name a tree table in SQL, each with the reason. The test
/// fails if an entry no longer matches, so this list cannot go stale.
const EXEMPT: &[(&str, &str)] = &[(
    "crates/velo-core/src/db.rs",
    "holds the schema definition, which has to create the tree tables",
)];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn pattern() -> Regex {
    Regex::new(
        r"(?is)\b(FROM|INTO|UPDATE|JOIN|TABLE(\s+IF\s+NOT\s+EXISTS)?)\s+(file_map|file_delta|delta_base)\b",
    )
    .unwrap()
}

/// `(relative path, line)` of every match in `crates/*/src`, outside the
/// accessor itself and the tests.
fn hits(root: &Path) -> Vec<(String, usize)> {
    let re = pattern();
    let mut files = Vec::new();
    for krate in std::fs::read_dir(root.join("crates")).unwrap() {
        let src = krate.unwrap().path().join("src");
        if src.is_dir() {
            collect(&src, &mut files);
        }
    }
    files.sort();
    let mut found = Vec::new();
    for file in files {
        let rel = file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        if rel == "crates/velo-core/src/treestore.rs"
            || rel == "crates/velo-core/src/tests.rs"
            || rel.starts_with("crates/velo-core/src/tests/")
        {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for m in re.find_iter(&text) {
            let line = text[..m.start()].bytes().filter(|b| *b == b'\n').count() + 1;
            found.push((rel.clone(), line));
        }
    }
    found
}

#[test]
fn no_sql_outside_treestore_names_a_tree_table() {
    let found = hits(&workspace_root());
    let offenders: Vec<String> = found
        .iter()
        .filter(|(f, _)| !EXEMPT.iter().any(|(e, _)| e == f))
        .map(|(f, l)| format!("{f}:{l}"))
        .collect();
    assert!(
        offenders.is_empty(),
        "SQL naming file_map, file_delta or delta_base outside crate::treestore \
         (go through the accessor instead): {offenders:?}"
    );
}

#[test]
fn every_exemption_still_matches() {
    let found = hits(&workspace_root());
    for (file, reason) in EXEMPT {
        assert!(
            found.iter().any(|(f, _)| f == file),
            "{file} is exempt ({reason}) but no longer names a tree table in SQL: \
             remove the stale exemption"
        );
    }
}

#[test]
fn the_pattern_flags_sql_and_not_identifiers() {
    let re = pattern();
    assert!(re.is_match("SELECT * FROM file_map"));
    assert!(re.is_match("select 1 from\n   file_delta where x"));
    assert!(re.is_match("CREATE TABLE IF NOT EXISTS delta_base ("));
    assert!(!re.is_match("let n = bundle.file_map.len();"));
    assert!(!re.is_match("collected.orphan_file_map"));
    assert!(!re.is_match("fn load_file_map(conn)"));
}
