//! The audit report model (`commands::report`).

use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::report::{self, Options, PathChange, SignatureState};
use crate::sign::{self, SigningKey, TrustedKeys};
use crate::tree::{SaveTree, TreeEntry};
use crate::{Author, BranchName, Repo, SnapshotId, SnapshotMeta};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

#[allow(clippy::too_many_arguments)]
fn put(
    repo: &Repo,
    branch: &str,
    parent: Option<&SnapshotId>,
    message: &str,
    entries: Vec<(&str, &[u8])>,
    meta: SnapshotMeta,
    author: Option<&Author>,
    renames: &[(PathBuf, PathBuf)],
    ts: i64,
) -> SnapshotId {
    let branch: BranchName = branch.parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent: None,
            message,
            entries: entries
                .into_iter()
                .map(|(p, b)| TreeEntry::file(p, b.to_vec()))
                .collect(),
            meta,
            author,
            timestamp_ms: Some(ts),
            renames,
        })
        .unwrap()
}

fn plain(
    repo: &Repo,
    branch: &str,
    parent: Option<&SnapshotId>,
    files: Vec<(&str, &[u8])>,
    ts: i64,
) -> SnapshotId {
    put(
        repo,
        branch,
        parent,
        "m",
        files,
        SnapshotMeta::new(),
        None,
        &[],
        ts,
    )
}

#[test]
fn branches_authors_metadata_and_since() {
    let (_t, repo) = setup();
    let ada = Author::new("ada").unwrap();
    let mut meta = SnapshotMeta::new();
    meta.set("mcp", "tool", "velody_save").unwrap();
    meta.set("run", "id", "r1").unwrap();
    let a = put(
        &repo,
        "main",
        None,
        "first",
        vec![("a.txt", b"x\n")],
        meta,
        Some(&ada),
        &[],
        100,
    );
    let b = plain(&repo, "main", Some(&a), vec![("a.txt", b"x\ny\n")], 200);
    plain(&repo, "side", None, vec![("s.txt", b"s\n")], 150);

    let r = report::build(&repo, Options::new()).unwrap();
    let names: Vec<_> = r.branches.iter().map(|t| t.branch.to_string()).collect();
    assert_eq!(names, ["main", "side"]);
    let main = &r.branches[0];
    assert_eq!(main.tip.as_ref(), Some(&b));
    assert_eq!(main.snapshots[0].id, b, "newest first");
    let first = &main.snapshots[1];
    assert_eq!(first.author.as_ref().unwrap().name(), "ada");
    assert!(first
        .meta
        .contains(&("mcp".into(), "tool".into(), "velody_save".into())));
    assert!(first.meta.iter().any(|m| m.0 == "run" && m.2 == "r1"));
    assert_eq!(first.changes[0].change, PathChange::Added);
    assert_eq!(main.snapshots[0].changes[0].change, PathChange::Modified);
    assert!(first.signatures.is_empty());

    let side: BranchName = "side".parse().unwrap();
    let only = report::build(&repo, Options::new().branch(&side)).unwrap();
    assert_eq!(only.branches.len(), 1);

    let since = report::build(&repo, Options::new().since_ms(150)).unwrap();
    assert_eq!(since.branches[0].snapshots.len(), 1);
    assert_eq!(since.branches[0].tip.as_ref(), Some(&b));

    let integrity = r.integrity.unwrap();
    assert_eq!(integrity.snapshots_checked, 3);
    assert_eq!(integrity.ids_verified, 3);
    assert!(integrity.healthy);
    assert!(report::build(&repo, Options::new().fsck(false))
        .unwrap()
        .integrity
        .is_none());
}

#[test]
fn signature_states() {
    let (tmp, repo) = setup();
    let id = plain(&repo, "main", None, vec![("a", b"a\n")], 1);
    let k = SigningKey::from_seed([3; 32]);
    sign::sign(&repo.write().unwrap(), &id, &k).unwrap();

    let trusted: TrustedKeys = [k.public_key()].into_iter().collect();
    let r = report::build(&repo, Options::new().trust(&trusted).fsck(false)).unwrap();
    let s = &r.branches[0].snapshots[0].signatures[0];
    assert_eq!(s.state, SignatureState::Trusted);
    assert_eq!(s.fingerprint, k.public_key().to_string()[..16]);

    let r = report::build(&repo, Options::new().fsck(false)).unwrap();
    assert_eq!(
        r.branches[0].snapshots[0].signatures[0].state,
        SignatureState::Untrusted
    );

    rusqlite::Connection::open(tmp.path().join(".velo").join("velo.db"))
        .unwrap()
        .execute_batch(&format!(
            "UPDATE signatures SET signature = '{}'",
            "ab".repeat(64)
        ))
        .unwrap();
    let r = report::build(&repo, Options::new().trust(&trusted).fsck(false)).unwrap();
    assert_eq!(
        r.branches[0].snapshots[0].signatures[0].state,
        SignatureState::Invalid
    );
}

#[test]
fn renames_and_blame_and_skips() {
    let (_t, repo) = setup();
    let ada = Author::new("ada").unwrap();
    let bob = Author::new("bob").unwrap();
    let a = put(
        &repo,
        "main",
        None,
        "one",
        vec![("old.txt", b"l1\n"), ("bin", &[0xff, 0xfe, 0x00])],
        SnapshotMeta::new(),
        Some(&ada),
        &[],
        1,
    );
    let b = put(
        &repo,
        "main",
        Some(&a),
        "two",
        vec![("new.txt", b"l1\nl2\n"), ("bin", &[0xff, 0xfe, 0x00])],
        SnapshotMeta::new(),
        Some(&bob),
        &[(PathBuf::from("old.txt"), PathBuf::from("new.txt"))],
        2,
    );
    let r = report::build(&repo, Options::new()).unwrap();
    let recs = &r.branches[0].snapshots;
    assert_eq!(recs[1].changes.len(), 2, "root lists every file");
    let ch = &recs[0].changes;
    assert!(ch.iter().any(|c| c.path == "new.txt"
        && c.change
            == PathChange::Renamed {
                from: "old.txt".into()
            }));
    assert!(!ch.iter().any(|c| c.change == PathChange::Deleted));

    assert_eq!(r.skipped_files, ["bin"]);
    let f = r.files.iter().find(|f| f.path == "new.txt").unwrap();
    assert_eq!(f.at, b);
    assert_eq!(f.lines[0].snapshot.as_ref(), Some(&a));
    assert_eq!(f.lines[0].author.as_ref().unwrap().name(), "ada");
    assert_eq!(f.lines[1].snapshot.as_ref(), Some(&b));
    assert_eq!(f.lines[1].author.as_ref().unwrap().name(), "bob");

    // The cap sends the rest to skipped_files.
    let capped = report::build(&repo, Options::new().max_blame_files(0).fsck(false)).unwrap();
    assert!(capped.files.is_empty());
    assert_eq!(capped.skipped_files, ["bin", "new.txt"]);

    let paths: [&Path; 1] = [Path::new("new.txt")];
    let chosen = report::build(&repo, Options::new().blame_paths(&paths).fsck(false)).unwrap();
    assert_eq!(chosen.files.len(), 1);
}

#[test]
fn hostile_strings_pass_through_verbatim() {
    let (_t, repo) = setup();
    let evil = "<script>alert(1)</script>";
    let mut meta = SnapshotMeta::new();
    meta.set("x", "k", evil).unwrap();
    put(
        &repo,
        "main",
        None,
        evil,
        vec![("a.txt", evil.as_bytes())],
        meta,
        None,
        &[],
        1,
    );
    let r = report::build(&repo, Options::new()).unwrap();
    assert_eq!(r.branches[0].snapshots[0].message, evil);
    assert!(r.branches[0].snapshots[0].meta.iter().any(|m| m.2 == evil));
    assert_eq!(r.files[0].lines[0].text, evil);
}
