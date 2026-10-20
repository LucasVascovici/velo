//! The pack builders read only the rows they send, and a fetch with nothing new
//! imports nothing. Each result is compared with the straightforward
//! whole-table computation the builders used before.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::bundle::{self, Bundle};
use crate::commands::{self, merge, sync};
use crate::transport::Spawn;
use crate::Repo;

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).expect("open repository");
    let guard = repo.write().expect("take write lock");
    f(&guard)
}

fn save(root: &Path, file: &str, content: &str, msg: &str) {
    fs::write(root.join(file), content).unwrap();
    with_write(root, |g| {
        commands::save::run(g, Some(msg), commands::save::Options::default())
    })
    .unwrap()
    .into_result()
    .expect("expected a snapshot");
}

fn setup() -> (TempDir, PathBuf) {
    crate::repo::keyframes_only();
    let holder = TempDir::new().unwrap();
    let origin = holder.path().join("origin");
    fs::create_dir_all(&origin).unwrap();
    commands::init::run(&origin).unwrap();
    save(&origin, "a.txt", "one\n", "base");
    save(&origin, "b.txt", "two\n", "second");
    let copy = holder.path().join("copy");
    sync::clone(
        origin.to_str().unwrap(),
        &Spawn::new("velo"),
        sync::CloneOptions {
            dir: Some(copy.as_path()),
            observer: None,
            ..Default::default()
        },
    )
    .unwrap();
    (holder, copy)
}

/// The previous `reachable_ancestry`: one lookup per snapshot.
fn old_reachable(conn: &rusqlite::Connection, tip: &str) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut stack = vec![tip.to_string()];
    while let Some(h) = stack.pop() {
        if h.is_empty() || seen.contains(&h) {
            continue;
        }
        seen.insert(h.clone());
        if let Ok((p, m)) = conn.query_row(
            "SELECT parent_hash, merge_parent FROM snapshots WHERE hash = ?",
            [&h],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        ) {
            stack.push(p);
            stack.push(m);
        }
    }
    seen
}

fn hashes(conn: &rusqlite::Connection, sql: &str) -> Vec<String> {
    conn.prepare(sql)
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .flatten()
        .collect()
}

/// Compare the pack with the whole-table computation of the same sets.
fn assert_same(
    conn: &rusqlite::Connection,
    repo: &Repo,
    snap_set: &HashSet<String>,
    peer_has: &HashSet<String>,
) {
    let mut want_fm: HashSet<Vec<String>> = HashSet::new();
    let mut want_objects: HashSet<String> = HashSet::new();
    let mut peer_objects: HashSet<String> = HashSet::new();
    let mut stmt = conn
        .prepare("SELECT snapshot_hash, path, hash, mode FROM file_map")
        .unwrap();
    let all_rows: Vec<Vec<String>> = stmt
        .query_map([], |r| {
            Ok(vec![
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?.to_string(),
            ])
        })
        .unwrap()
        .flatten()
        .collect();
    for row in all_rows {
        if snap_set.contains(&row[0]) {
            want_objects.insert(row[2].clone());
            want_fm.insert(row);
        } else if peer_has.contains(&row[0]) {
            peer_objects.insert(row[2].clone());
        }
    }
    for h in &peer_objects {
        want_objects.remove(h);
    }
    let want_snaps: HashSet<String> = hashes(conn, "SELECT hash FROM snapshots")
        .into_iter()
        .filter(|h| snap_set.contains(h))
        .collect();

    let got: Bundle =
        bundle::build_pack_excluding(conn, &repo.objects(), snap_set, peer_has).unwrap();
    assert_eq!(
        got.snapshots
            .iter()
            .map(|s| s.hash.clone())
            .collect::<HashSet<_>>(),
        want_snaps
    );
    assert_eq!(got.file_map.len(), want_fm.len(), "no duplicate rows");
    let got_fm: HashSet<Vec<String>> = got
        .file_map
        .iter()
        .map(|r| {
            vec![
                r.snapshot_hash.clone(),
                r.path.clone(),
                r.hash.clone(),
                r.mode.to_string(),
            ]
        })
        .collect();
    assert_eq!(got_fm, want_fm);
    assert_eq!(
        got.objects
            .iter()
            .map(|o| o.0.clone())
            .collect::<HashSet<_>>(),
        want_objects
    );
}

#[test]
fn packs_match_the_whole_table_computation() {
    let (_h, copy) = setup();
    // A merge, so history has a second parent.
    with_write(&copy, |g| commands::switch::run(g, "side", false)).unwrap();
    save(&copy, "s.txt", "side\n", "side work");
    with_write(&copy, |g| commands::switch::run(g, "main", true)).unwrap();
    save(&copy, "m.txt", "main\n", "main work");
    with_write(&copy, |g| {
        merge::run(g, merge::Mode::Bring { source: "side" })
    })
    .unwrap();

    let repo = Repo::open_and_migrate(&copy).unwrap();
    let conn = repo.conn();
    let mut all = HashSet::new();
    for t in hashes(conn, "SELECT hash FROM snapshots") {
        let new = bundle::reachable_ancestry(conn, &t);
        assert_eq!(new, old_reachable(conn, &t));
        all.extend(new);
    }
    let second = hashes(conn, "SELECT hash FROM snapshots WHERE message = 'second'");
    let peer = bundle::reachable_ancestry(conn, &second[0]);
    assert!(peer.len() >= 2);

    // Push-like: everything, the peer holds the base history.
    assert_same(conn, &repo, &all, &peer);
    // Fetch-like: only what the peer lacks.
    let rest: HashSet<String> = all.difference(&peer).cloned().collect();
    assert!(!rest.is_empty());
    assert_same(conn, &repo, &rest, &peer);
    // Self-contained (bundle create).
    assert_same(conn, &repo, &all, &HashSet::new());
}

#[test]
fn reachable_ancestry_keeps_an_absent_tip_and_follows_merge_parents() {
    let (_h, copy) = setup();
    let repo = Repo::open_and_migrate(&copy).unwrap();
    let conn = repo.conn();
    assert_eq!(
        bundle::reachable_ancestry(conn, "deadbeef"),
        HashSet::from(["deadbeef".to_string()])
    );
    assert!(bundle::reachable_ancestry(conn, "").is_empty());

    conn.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash, merge_parent, created_at_ms)
         VALUES ('m1', 'm', 'main', '', 'x1', 0), ('x1', 'x', 'main', '', '', 0)",
        [],
    )
    .unwrap();
    let set = bundle::reachable_ancestry(conn, "m1");
    assert_eq!(set, HashSet::from(["m1".to_string(), "x1".to_string()]));
    assert_eq!(set, old_reachable(conn, "m1"));
}

#[test]
fn a_fetch_with_nothing_new_imports_nothing() {
    let (_h, copy) = setup();
    let f = with_write(&copy, |g| {
        sync::fetch(g, "origin", &Spawn::new("velo"), Default::default())
    })
    .unwrap();
    assert_eq!((f.snapshots, f.objects), (0, 0));
    assert!(!f.refs.is_empty());

    let repo = Repo::open_and_migrate(&copy).unwrap();
    let b = bundle::build_pack_excluding(
        repo.conn(),
        &repo.objects(),
        &HashSet::new(),
        &HashSet::new(),
    )
    .unwrap();
    assert!(b.snapshots.is_empty() && b.file_map.is_empty() && b.objects.is_empty());
    assert!(b.meta.is_empty() && b.tags.is_empty() && b.renames.is_empty());
}
