//! Compaction re-mints in place (19.4-a): same ids and rows as the copy-based
//! apply, a database that does not grow, and a dry run that writes nothing.

use std::path::PathBuf;
use tempfile::TempDir;

use crate::commands::compact::{self, Range};
use crate::commands::{fsck, retention, tag};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotId, SnapshotMeta, TagName};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn main_branch() -> BranchName {
    "main".parse().unwrap()
}

fn put(
    repo: &Repo,
    parent: Option<&SnapshotId>,
    files: &[(&str, &str)],
    renames: &[(PathBuf, PathBuf)],
    ts: i64,
) -> SnapshotId {
    let branch = main_branch();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent: None,
            message: &format!("step {ts}"),
            entries: files
                .iter()
                .map(|(p, b)| TreeEntry::file(*p, b.as_bytes().to_vec()))
                .collect(),
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(ts),
            renames,
        })
        .unwrap()
}

fn healthy(repo: &Repo) {
    let report = fsck::check(repo).unwrap();
    assert!(report.is_healthy(), "{:?}", report.problems);
}

fn lines(repo: &Repo, sql: &str) -> Vec<String> {
    let mut stmt = repo.conn().prepare(sql).unwrap();
    stmt.query_map([], |r| {
        let n = r.as_ref().column_count();
        let mut parts = Vec::new();
        for i in 0..n {
            parts.push(match r.get_ref(i)? {
                rusqlite::types::ValueRef::Integer(v) => v.to_string(),
                rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                _ => String::new(),
            });
        }
        Ok(parts.join("|"))
    })
    .unwrap()
    .map(|r| r.unwrap())
    .collect()
}

/// Ten snapshots with fixed timestamps: a squash range 1..=3 holding a rename,
/// a tagged snapshot 4, a second range 5..=6, and a tail 7..=9 whose last is
/// tagged and checked out.
fn scenario() -> (TempDir, Repo, Vec<SnapshotId>) {
    let (t, repo) = setup();
    let mut ids: Vec<SnapshotId> = Vec::new();
    for i in 0..10i64 {
        let body = format!("body {i}\n");
        let mut files: Vec<(&str, &str)> = vec![("keep.txt", "k\n"), ("f.txt", &body)];
        let renames: Vec<(PathBuf, PathBuf)> = if i == 2 {
            files.push(("moved.txt", "m\n"));
            vec![("old.txt".into(), "moved.txt".into())]
        } else if i == 1 {
            files.push(("old.txt", "m\n"));
            vec![]
        } else if i >= 3 {
            files.push(("moved.txt", "m\n"));
            vec![]
        } else {
            vec![]
        };
        let id = put(&repo, ids.last(), &files, &renames, 1_000 * (i + 1));
        ids.push(id);
    }
    let name: TagName = "mid".parse().unwrap();
    tag::create(&repo.write().unwrap(), &name, Some(&ids[4]), false).unwrap();
    let name: TagName = "tip".parse().unwrap();
    tag::create(&repo.write().unwrap(), &name, Some(&ids[9]), false).unwrap();
    std::fs::write(repo.root().join(".velo/PARENT"), ids[9].as_str()).unwrap();
    (t, repo, ids)
}

fn run_scenario(repo: &Repo, ids: &[SnapshotId], dry_run: bool) -> compact::Compaction {
    let guard = repo.write().unwrap();
    compact::run(
        &guard,
        &main_branch(),
        &[
            Range {
                oldest: &ids[1],
                newest: &ids[3],
            },
            Range {
                oldest: &ids[5],
                newest: &ids[6],
            },
        ],
        compact::Options {
            dry_run,
            ..Default::default()
        },
    )
    .unwrap()
}

fn dump(repo: &Repo) -> String {
    let mut out = Vec::new();
    out.extend(lines(
        repo,
        "SELECT hash, parent_hash, message, created_at_ms FROM snapshots ORDER BY created_at_ms",
    ));
    out.extend(lines(
        repo,
        "SELECT old_hash, new_hash FROM compactions ORDER BY old_hash",
    ));
    out.extend(lines(
        repo,
        "SELECT name, snapshot_hash FROM tags ORDER BY name",
    ));
    out.extend(lines(
        repo,
        "SELECT snapshot_hash, from_path, to_path FROM renames ORDER BY snapshot_hash, to_path",
    ));
    out.extend(lines(
        repo,
        "SELECT snapshot_hash, COUNT(*), MIN(hash) FROM file_map GROUP BY snapshot_hash ORDER BY snapshot_hash",
    ));
    out.push(lines(repo, "SELECT tip FROM branches WHERE name = 'main'").join(""));
    out.push(std::fs::read_to_string(repo.root().join(".velo/PARENT")).unwrap());
    out.join("\n")
}

#[test]
fn golden_ids_and_rows_match_the_copying_apply() {
    let (_t, repo, ids) = scenario();
    run_scenario(&repo, &ids, false);
    let got = dump(&repo);
    let golden = include_str!("compaction_in_place.golden");
    assert_eq!(got.trim_end(), golden.trim_end(), "\n{got}\n");
    healthy(&repo);
}

#[test]
fn a_dry_run_writes_nothing() {
    let (_t, repo, ids) = scenario();
    let before = dump(&repo);
    let head = repo.head_token().unwrap();
    let done = run_scenario(&repo, &ids, true);
    assert!(done.dry_run);
    assert_eq!(dump(&repo), before);
    assert_eq!(repo.head_token().unwrap(), head);
}

#[test]
fn retention_does_not_grow_the_database() {
    let (_t, repo) = setup();
    let branch = main_branch();
    let day = 86_400_000i64;
    let base = 1_700_000_000_000i64;
    let mut parent: Option<SnapshotId> = None;
    for n in 0..120i64 {
        let entries: Vec<TreeEntry> = (0..300)
            .map(|i| {
                let v = if i % 120 == (n as usize) % 120 { n } else { 0 };
                TreeEntry::file(
                    format!("d{}/f{i}.txt", i % 10),
                    format!("{i} v{v}\n").into_bytes(),
                )
            })
            .collect();
        let id = repo
            .write()
            .unwrap()
            .save_tree(SaveTree {
                branch: &branch,
                parent: parent.as_ref(),
                merge_parent: None,
                message: &format!("step {n}"),
                entries,
                meta: SnapshotMeta::new(),
                author: None,
                timestamp_ms: Some(base + n * 60 * day / 120),
                renames: &[],
            })
            .unwrap();
        parent = Some(id);
    }
    let pages = |repo: &Repo| -> i64 {
        repo.conn()
            .query_row("PRAGMA page_count", [], |r| r.get(0))
            .unwrap()
    };
    let before = pages(&repo);
    let report = retention::apply(
        &repo.write().unwrap(),
        &branch,
        &retention::Policy::default(),
        retention::Options {
            now_ms: Some(base + 60 * day),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.removed > 0);
    assert!(pages(&repo) <= before, "{} > {before}", pages(&repo));
    healthy(&repo);
}
