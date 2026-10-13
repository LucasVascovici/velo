//! `grep` runs in parallel but its output stays sorted and complete (15.7-b).

use crate::commands::{self, grep};
use crate::{InitOptions, ObjectLocation, Repo};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const FILES: usize = 200;

fn path_of(i: usize) -> String {
    // Spread over directories so path order differs from creation order.
    format!("d{}/f{:03}.txt", (i * 7) % 11, (FILES - i) % 200)
}

fn content_of(i: usize) -> String {
    let mut s = format!("header {i}\n");
    if i.is_multiple_of(3) {
        s.push_str("needle here\n");
    }
    if i.is_multiple_of(5) {
        s.push_str("Needle Capitalised\n");
    }
    if i.is_multiple_of(7) {
        s.push_str(&format!("num {}\n", i * 11));
    }
    s.push_str("footer\n");
    s
}

fn fixture(location: ObjectLocation) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    commands::init::run_with(&root, InitOptions::new().objects(location)).unwrap();
    for i in 0..FILES {
        let p = root.join(path_of(i));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content_of(i)).unwrap();
    }
    // Invalid UTF-8 that contains the literal's prefix: must be skipped.
    fs::write(root.join("bin.dat"), [0u8, 159, 146, 150, b'n', b'e', b'e']).unwrap();
    let repo = Repo::open_and_migrate(&root).unwrap();
    let guard = repo.write().unwrap();
    commands::save::run(&guard, Some("fixture"), commands::save::Options::default()).unwrap();
    drop(guard);
    (tmp, root)
}

/// The serial answer: (path, matching line numbers) for lines satisfying `hit`.
fn expected(hit: impl Fn(&str) -> bool) -> Vec<(String, Vec<usize>)> {
    let mut out: Vec<(String, Vec<usize>)> = (0..FILES)
        .filter_map(|i| {
            let nos: Vec<usize> = content_of(i)
                .lines()
                .enumerate()
                .filter(|(_, l)| hit(l))
                .map(|(n, _)| n + 1)
                .collect();
            (!nos.is_empty()).then(|| (path_of(i), nos))
        })
        .collect();
    out.sort();
    out
}

fn summarise(r: &grep::GrepResults) -> Vec<(String, Vec<usize>)> {
    r.files
        .iter()
        .map(|f| {
            (
                f.path.clone(),
                f.lines
                    .iter()
                    .filter(|l| l.is_match)
                    .map(|l| l.line_no)
                    .collect(),
            )
        })
        .collect()
}

fn search(root: &Path, pat: &str, ci: bool, names_only: bool, snap: bool) -> grep::GrepResults {
    let repo = Repo::open_and_migrate(root).unwrap();
    let head = commands::resolve_snapshot_id(&repo, "HEAD").ok();
    let opts = grep::Options {
        snapshot: if snap { head.as_ref() } else { None },
        case_insensitive: ci,
        names_only,
        context: 0,
    };
    grep::run(&repo, pat, opts).unwrap()
}

fn check(location: ObjectLocation) {
    let (_tmp, root) = fixture(location);
    for snap in [false, true] {
        // Literal, case-sensitive: the prefilter path.
        let r = search(&root, "needle", false, false, snap);
        assert_eq!(
            summarise(&r),
            expected(|l| l.contains("needle")),
            "literal snap={snap}"
        );
        // Case-insensitive.
        let r = search(&root, "needle", true, false, snap);
        assert_eq!(
            summarise(&r),
            expected(|l| l.to_lowercase().contains("needle")),
            "-i snap={snap}"
        );
        // A real regex.
        let r = search(&root, r"^num \d+1\b", false, false, snap);
        assert_eq!(
            summarise(&r),
            expected(|l| l.starts_with("num ") && l.ends_with('1')),
            "regex snap={snap}"
        );
        // Names only: paths, sorted, no lines.
        let r = search(&root, "needle", false, true, snap);
        assert!(r.files.iter().all(|f| f.lines.is_empty()));
        let names: Vec<String> = r.files.iter().map(|f| f.path.clone()).collect();
        let want: Vec<String> = expected(|l| l.contains("needle"))
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(names, want, "names-only snap={snap}");
        assert!(names.windows(2).all(|w| w[0] < w[1]), "sorted");
    }
}

#[test]
fn parallel_grep_matches_serial_with_files_backend() {
    check(ObjectLocation::Files);
}

#[test]
fn parallel_grep_matches_serial_with_database_backend() {
    check(ObjectLocation::Database);
}

/// The metadata-free walk must still honour `.veloignore` and skip binary files.
#[test]
fn working_tree_grep_skips_ignored_directories_and_binary_files() {
    let (_tmp, root) = fixture(ObjectLocation::Files);
    fs::write(root.join(".veloignore"), "skipped/\n").unwrap();
    fs::create_dir_all(root.join("skipped")).unwrap();
    fs::write(root.join("skipped/hidden.txt"), "needle in ignored dir\n").unwrap();
    fs::create_dir_all(root.join("target")).unwrap();
    fs::write(root.join("target/built.txt"), "needle in target\n").unwrap();
    fs::write(root.join("bin2.dat"), b"needle\0\xff\xfe").unwrap();
    let r = search(&root, "needle", false, false, false);
    let paths: Vec<&str> = r.files.iter().map(|f| f.path.as_str()).collect();
    assert!(
        !paths.contains(&"skipped/hidden.txt"),
        "ignored dir searched"
    );
    assert!(!paths.contains(&"target/built.txt"), "target searched");
    assert!(!paths.contains(&"bin2.dat") && !paths.contains(&"bin.dat"));
    assert_eq!(paths.len(), expected(|l| l.contains("needle")).len());
}
