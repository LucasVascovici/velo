//! Diff and blame follow the path's merge driver: paragraphs and top-level keys
//! are attributed and compared as units, lines stay the default.

use tempfile::TempDir;
use velo_merge::ParagraphDriver;

use crate::commands::blame;
use crate::commands::diff::{self, FileChange, LineTag};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Drivers, Repo, SnapshotId, SnapshotMeta};

fn setup(drivers: Drivers) -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap().merging(drivers);
    (tmp, repo)
}

fn save(repo: &Repo, parent: Option<&SnapshotId>, file: &str, body: &str, ts: i64) -> SnapshotId {
    let branch: BranchName = "main".parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent: None,
            message: &format!("step {ts}"),
            entries: vec![TreeEntry::file(file, body.as_bytes().to_vec())],
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(ts),
            renames: &[],
        })
        .unwrap()
}

fn paragraphs() -> Drivers {
    Drivers::new().with("*.md", ParagraphDriver).unwrap()
}

const V1: &str = "one\ntwo\n\nthree\nfour\n\nfive\n";
const V2: &str = "one\ntwo\n\nTHREE\nFOUR\n\nfive\n";

#[test]
fn paragraph_blame_attributes_each_paragraph_to_its_own_snapshot() {
    let (_tmp, repo) = setup(paragraphs());
    let s1 = save(&repo, None, "n.md", V1, 1000);
    let s2 = save(&repo, Some(&s1), "n.md", V2, 2000);
    let b = blame::run(
        &repo,
        std::path::Path::new("n.md"),
        blame::Options {
            at: Some(&s2),
            ..Default::default()
        },
    )
    .unwrap();
    let got: Vec<_> = b
        .lines
        .iter()
        .map(|l| {
            (
                l.line_no,
                l.line_count,
                l.text.clone(),
                l.origin.clone().unwrap().hash,
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (1, 2, "one\ntwo".to_string(), s1.clone()),
            (3, 1, "".to_string(), s1.clone()),
            (4, 2, "THREE\nFOUR".to_string(), s2.clone()),
            (6, 1, "".to_string(), s1.clone()),
            (7, 1, "five".to_string(), s1.clone()),
        ]
    );
}

#[cfg(feature = "json")]
#[test]
fn json_blame_credits_only_the_edited_key() {
    let drivers = Drivers::new()
        .with("*.json", velo_merge::JsonDriver)
        .unwrap();
    let (_tmp, repo) = setup(drivers);
    let s1 = save(
        &repo,
        None,
        "c.json",
        "{\n  \"a\": 1,\n  \"b\": {\n    \"x\": 1\n  },\n  \"c\": 3\n}\n",
        1000,
    );
    let s2 = save(
        &repo,
        Some(&s1),
        "c.json",
        "{\n  \"a\": 1,\n  \"b\": {\n    \"x\": 2\n  },\n  \"c\": 3\n}\n",
        2000,
    );
    let b = blame::run(
        &repo,
        std::path::Path::new("c.json"),
        blame::Options {
            at: Some(&s2),
            ..Default::default()
        },
    )
    .unwrap();
    let who: Vec<_> = b
        .lines
        .iter()
        .map(|l| (l.line_no, l.origin.clone().unwrap().hash == s2))
        .collect();
    assert_eq!(
        who,
        vec![(1, false), (2, false), (3, true), (6, false), (7, false)]
    );
}

#[test]
fn a_blame_window_returns_only_overlapping_units() {
    let (_tmp, repo) = setup(paragraphs());
    let s1 = save(&repo, None, "n.md", V1, 1000);
    let b = blame::run(
        &repo,
        std::path::Path::new("n.md"),
        blame::Options {
            at: Some(&s1),
            lines: Some(3..5),
            ..Default::default()
        },
    )
    .unwrap();
    // Lines 3-4 are the blank separator and the first line of "three\nfour".
    let starts: Vec<_> = b.lines.iter().map(|l| l.line_no).collect();
    assert_eq!(starts, vec![3, 4]);
}

#[test]
fn line_driver_blame_has_unit_line_counts() {
    let (_tmp, repo) = setup(Drivers::new());
    let s1 = save(&repo, None, "n.md", V1, 1000);
    let b = blame::run(
        &repo,
        std::path::Path::new("n.md"),
        blame::Options {
            at: Some(&s1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(b.lines.len(), 7);
    assert!(b.lines.iter().all(|l| l.line_count == 1));
}

#[test]
fn paragraph_diff_yields_whole_paragraphs() {
    let (_tmp, repo) = setup(paragraphs());
    let s1 = save(&repo, None, "n.md", V1, 1000);
    let s2 = save(&repo, Some(&s1), "n.md", V2, 2000);
    let check = |d: diff::Diff| {
        let FileChange::Modified { hunks } = &d.files[0].change else {
            panic!("expected a modification");
        };
        let lines: Vec<_> = hunks.iter().flat_map(|h| &h.lines).collect();
        let removed: Vec<_> = lines.iter().filter(|l| l.tag == LineTag::Removed).collect();
        let added: Vec<_> = lines.iter().filter(|l| l.tag == LineTag::Added).collect();
        assert_eq!(removed.len(), 1);
        assert_eq!(added.len(), 1);
        assert_eq!(removed[0].text, "three\nfour");
        assert_eq!(added[0].text, "THREE\nFOUR");
        assert_eq!(added[0].line_no, Some(4));
    };
    check(diff::between(&repo, &s1, Some(&s2), &[]).unwrap());
    check(diff::snapshot_diff(&repo, repo.conn(), s1.as_str(), s2.as_str(), &None).unwrap());
}

#[test]
fn line_driver_diff_is_unchanged() {
    let (_tmp, repo) = setup(Drivers::new());
    let s1 = save(&repo, None, "n.md", V1, 1000);
    let s2 = save(&repo, Some(&s1), "n.md", V2, 2000);
    let d = diff::between(&repo, &s1, Some(&s2), &[]).unwrap();
    let FileChange::Modified { hunks } = &d.files[0].change else {
        panic!("expected a modification");
    };
    assert_eq!(*hunks, diff::build_hunks(V1, V2));
}
