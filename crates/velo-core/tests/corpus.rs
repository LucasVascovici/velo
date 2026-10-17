//! Golden corpus: open every repository and bundle written by a released build
//! with the current build, and compare against the checked-in expectations.
//!
//! The goldens live in `tests/corpus/` (see its README.md). Each is copied into
//! a temporary directory before anything touches it: `open_and_migrate` writes,
//! and the checked-in copy must never change.
//!
//! A failure here means this build can no longer read, migrate or refuse a
//! format that shipped. Do not edit the expectations to make it pass: a change
//! to a golden's expected output is a format change, with the rule and the
//! consequences in `tests/corpus/README.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tempfile::TempDir;
use velo_core::commands::{blame, bundle, fsck, history, tag};
use velo_core::sign::{self, SigningKey, TrustedKeys};
use velo_core::tree::FileKind;
use velo_core::tree::{SaveTree, TreeEntry};
use velo_core::{BranchName, Error, Repo, SnapshotId, SnapshotMeta, FORMAT_VERSION};

const RULE: &str = "A golden's expected output changing is a format change: read \
    tests/corpus/README.md (needs a docs/FORMAT.md entry and a changelog line), \
    and never regenerate a golden with a newer build.";

fn corpus() -> PathBuf {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/corpus")).to_path_buf()
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

/// A private copy of the golden repository for `tag`. The returned directory
/// is the repository root.
fn golden_copy(tag: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("repo");
    copy_dir(&corpus().join("repos").join(tag), &root);
    (dir, root)
}

fn expected(tag: &str) -> Value {
    let path = corpus().join("expected").join(format!("{tag}.json"));
    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap()
}

fn tags() -> Vec<String> {
    let mut tags: Vec<String> = fs::read_dir(corpus().join("expected"))
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().into_string().unwrap();
            let stem = name.strip_suffix(".json")?;
            // `<tag>.blame.json` sits beside `<tag>.json`.
            // `bundles.v4.json` describes the signed bundles, not a repository.
            (!stem.ends_with(".blame") && stem != "bundles.v4").then(|| stem.to_string())
        })
        .collect();
    tags.sort();
    assert!(!tags.is_empty(), "no expectations found. {RULE}");
    tags
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

fn id(text: &str) -> SnapshotId {
    text.parse().unwrap()
}

/// Every snapshot reachable from `tip` through both parents, from the
/// expectation alone.
fn expected_ancestry(snapshots: &[Value], tip: &str) -> BTreeSet<String> {
    let by_hash: BTreeMap<&str, &Value> = snapshots.iter().map(|x| (s(&x["hash"]), x)).collect();
    let mut seen = BTreeSet::new();
    let mut stack = vec![tip.to_string()];
    while let Some(h) = stack.pop() {
        if h.is_empty() || !seen.insert(h.clone()) {
            continue;
        }
        if let Some(row) = by_hash.get(h.as_str()) {
            stack.push(s(&row["parent_hash"]).to_string());
            stack.push(s(&row["merge_parent"]).to_string());
        }
    }
    seen
}

fn branches(exp: &Value) -> Vec<String> {
    exp["branch_tips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| s(&t[0]).to_string())
        .collect()
}

/// The tip of `branch`. The goldens' `branch_tips` are empty strings: the
/// released builds did not record tips, so the tip is the newest snapshot
/// recorded on the branch, which `Repo::branch_tip` resolves the same way.
fn tip(repo: &Repo, branch: &str) -> SnapshotId {
    repo.branch_tip(&branch.parse::<BranchName>().unwrap())
        .unwrap()
        .unwrap_or_else(|| panic!("branch {branch} has no tip"))
}

/// Blame every UTF-8 text file at the tip of every branch, in a stable shape.
fn blame_all(repo: &Repo, exp: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for branch in branches(exp) {
        let tip = tip(repo, &branch);
        let mut files = serde_json::Map::new();
        let mut tree = repo.tree_at(&tip).unwrap();
        tree.sort_by(|a, b| a.path.cmp(&b.path));
        for f in tree {
            if f.kind != FileKind::Regular {
                continue;
            }
            let content = repo.read_file_at(&tip, &f.path).unwrap();
            if content.contains(&0) || std::str::from_utf8(&content).is_err() {
                continue;
            }
            let b = blame::run(
                repo,
                Path::new(&f.path),
                blame::Options {
                    at: Some(&tip),
                    ..Default::default()
                },
            )
            .unwrap();
            let lines: Vec<Value> = b
                .lines
                .iter()
                .map(|l| {
                    let o = l.origin.as_ref();
                    json!([
                        l.line_no,
                        l.text,
                        o.map(|o| o.hash.to_string()),
                        o.map(|o| o.created_at.timestamp_millis()),
                        o.map(|o| o.path.to_string_lossy().replace('\\', "/")),
                    ])
                })
                .collect();
            files.insert(f.path.clone(), Value::Array(lines));
        }
        out.insert(branch, Value::Object(files));
    }
    Value::Object(out)
}

#[test]
fn every_golden_repository_opens_migrates_and_reads_back() {
    for tag_name in tags() {
        let exp = expected(&tag_name);
        let (_dir, root) = golden_copy(&tag_name);
        let ctx = format!("[{tag_name}] {RULE}");
        let stored = exp["user_version"].as_u64().unwrap() as u32;

        // 1. Plain open: a repository older than this build is refused with a
        // pointer at migration, and never changed by the attempt; one already
        // at the current format opens as is.
        match Repo::open(&root) {
            Err(Error::MigrationRequired { found, supported }) => {
                assert!(stored < FORMAT_VERSION, "{ctx}");
                assert_eq!((found, supported), (stored, FORMAT_VERSION), "{ctx}");
            }
            Ok(repo) => {
                assert_eq!(stored, FORMAT_VERSION, "{ctx}");
                assert_eq!(repo.format_version().unwrap(), stored, "{ctx}");
            }
            Err(e) => panic!("{ctx}: open returned {e:?}"),
        }
        if tag_name == "v4.0.0" {
            assert!(
                matches!(
                    Repo::open(&root),
                    Err(Error::MigrationRequired {
                        found: 2,
                        supported: 3
                    })
                ),
                "{ctx}: the format-2 golden must need migration"
            );
        }

        // 2. Migrate.
        let repo = Repo::open_and_migrate(&root).unwrap_or_else(|e| panic!("{ctx}: {e:?}"));
        assert_eq!(repo.format_version().unwrap(), FORMAT_VERSION, "{ctx}");

        // 3. fsck.
        let report = fsck::check(&repo).unwrap();
        assert!(
            report.problems.is_empty(),
            "{ctx}: fsck found {:?}",
            report.problems
        );

        // 4. Every snapshot: record, tree, file content, metadata.
        let snapshots = exp["snapshots"].as_array().unwrap();
        assert!(!snapshots.is_empty(), "{ctx}");
        let mut meta_rows: BTreeMap<&str, BTreeSet<(String, String, String)>> = BTreeMap::new();
        for r in exp["snapshot_meta"].as_array().unwrap() {
            meta_rows.entry(s(&r[0])).or_default().insert((
                s(&r[1]).to_string(),
                s(&r[2]).to_string(),
                s(&r[3]).to_string(),
            ));
        }
        for snap in snapshots {
            let hash = s(&snap["hash"]);
            let sid = id(hash);
            let c = format!("[{tag_name} {}] {RULE}", &hash[..12]);

            let e = repo.snapshot(&sid).unwrap_or_else(|e| panic!("{c}: {e:?}"));
            assert_eq!(e.message, s(&snap["message"]), "{c}");
            assert_eq!(
                e.parent.as_ref().map(|p| p.to_string()).unwrap_or_default(),
                s(&snap["parent_hash"]),
                "{c}"
            );
            assert_eq!(
                e.merge_parent
                    .as_ref()
                    .map(|p| p.to_string())
                    .unwrap_or_default(),
                s(&snap["merge_parent"]),
                "{c}"
            );
            assert_eq!(
                e.created_at.timestamp_millis(),
                snap["created_at_ms"].as_i64().unwrap(),
                "{c}"
            );

            let mut want: Vec<(String, String, i64)> = exp["file_map"][hash]
                .as_array()
                .unwrap_or_else(|| panic!("{c}: no file_map"))
                .iter()
                .map(|r| {
                    (
                        s(&r["path"]).to_string(),
                        s(&r["hash"]).to_string(),
                        r["mode"].as_i64().unwrap(),
                    )
                })
                .collect();
            want.sort();
            let mut got = repo.tree_at(&sid).unwrap();
            got.sort_by(|a, b| a.path.cmp(&b.path));
            assert_eq!(
                got.iter()
                    .map(|f| (f.path.clone(), f.object.to_string()))
                    .collect::<Vec<_>>(),
                want.iter()
                    .map(|(p, h, _)| (p.clone(), h.clone()))
                    .collect::<Vec<_>>(),
                "{c}: tree"
            );
            for (f, (_, _, mode)) in got.iter().zip(&want) {
                if *mode == 0 {
                    assert_eq!(f.kind, FileKind::Regular, "{c}: {}", f.path);
                }
                let bytes = repo.read_file_at(&sid, &f.path).unwrap();
                assert_eq!(
                    blake3::hash(&bytes).to_hex().to_string(),
                    f.object.to_string(),
                    "{c}: content of {}",
                    f.path
                );
            }

            let got_meta: BTreeSet<(String, String, String)> = repo
                .snapshot_meta(&sid)
                .unwrap()
                .iter()
                .map(|(n, k, v)| (n.to_string(), k.to_string(), v.to_string()))
                .collect();
            assert_eq!(
                got_meta,
                meta_rows.get(hash).cloned().unwrap_or_default(),
                "{c}: metadata"
            );
        }

        // 5. History from every branch tip, and tags.
        for branch in branches(&exp) {
            let t = tip(&repo, &branch);
            let h = history::run(
                &repo,
                history::Options {
                    from: Some(&t),
                    ..Default::default()
                },
            )
            .unwrap();
            let got: BTreeSet<String> = h.entries.iter().map(|e| e.hash.to_string()).collect();
            assert_eq!(h.entries.len(), got.len(), "{ctx}: duplicate in history");
            assert_eq!(
                got,
                expected_ancestry(snapshots, t.as_str()),
                "{ctx}: history of {branch}"
            );
        }
        let got_tags: BTreeSet<(String, String)> = tag::list(&repo)
            .unwrap()
            .into_iter()
            .map(|t| (t.name.to_string(), t.snapshot.to_string()))
            .collect();
        let want_tags: BTreeSet<(String, String)> = exp["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| (s(&t[0]).to_string(), s(&t[1]).to_string()))
            .collect();
        assert_eq!(got_tags, want_tags, "{ctx}: tags");

        // 6. Blame, against the expectation generated by the 18.3-b build.
        let blame_path = corpus()
            .join("expected")
            .join(format!("{tag_name}.blame.json"));
        let got = blame_all(&repo, &exp);
        if std::env::var_os("VELO_CORPUS_WRITE_BLAME").is_some() {
            let doc = json!({
                "generated_by": "the velo-core build of task 18.3-b, NOT the \
                    release that wrote the golden; checked in once. A change \
                    here is a format change (tests/corpus/README.md).",
                "tag": tag_name,
                "blame": got,
            });
            let mut text = serde_json::to_string_pretty(&doc).unwrap();
            text.push('\n');
            fs::write(&blame_path, text).unwrap();
            continue;
        }
        let want: Value = serde_json::from_slice(
            &fs::read(&blame_path).unwrap_or_else(|e| panic!("{ctx}: {blame_path:?}: {e}")),
        )
        .unwrap();
        assert_eq!(want["blame"], got, "{ctx}: blame");
    }
}

/// The snapshot ids in `bundles/synth.v2.bundle`. It was synthesized from a
/// two-snapshot history without renames made by the v4.0.0 build, so it matches
/// none of the goldens' expectations; `fsck` recomputes both ids from content.
const SYNTH_V2_IDS: [&str; 2] = [
    "3e90a20676c6eead37f172f75a51c4691aebbb7ee8bc87fc96a94f296ab9e98e",
    "caaaa76d61baaec067803e11ec4a4e08d5520d081d20d764ec6082ccb1039e2a",
];

/// Each bundle with the tag whose expectation it carries, or `None` for the
/// synthesized v2 bundle.
fn bundle_cases() -> Vec<(PathBuf, Option<String>)> {
    let mut cases = Vec::new();
    for entry in fs::read_dir(corpus().join("bundles")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.ends_with(".bundle"), "{name}");
        // The v4 signature bundles have their own tests and expectation file.
        if name.ends_with(".v4.bundle") {
            continue;
        }
        let tag = if name.starts_with("synth.") {
            None
        } else {
            Some(name.split(".v").next().unwrap().to_string())
        };
        cases.push((path, tag));
    }
    cases.sort();
    assert!(cases.len() > tags().len(), "missing bundles. {RULE}");
    cases
}

fn fresh_repo() -> (TempDir, Repo) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("fresh");
    fs::create_dir_all(&root).unwrap();
    let repo = Repo::init(&root).unwrap();
    (dir, repo)
}

fn all_ids(repo: &Repo) -> BTreeSet<String> {
    history::run(
        repo,
        history::Options {
            all: true,
            ..Default::default()
        },
    )
    .unwrap()
    .entries
    .iter()
    .map(|e| e.hash.to_string())
    .collect()
}

#[test]
fn every_corpus_bundle_applies_with_the_recorded_ids() {
    for (path, tag_name) in bundle_cases() {
        let ctx = format!("[{}] {RULE}", path.file_name().unwrap().to_string_lossy());
        let (_dir, repo) = fresh_repo();
        // Applying only reads the bundle, but copy it anyway: nothing in the
        // corpus is ever handed to the code under test by its real path.
        let copy = _dir.path().join("in.bundle");
        fs::copy(&path, &copy).unwrap();
        let g = repo.write().unwrap();
        bundle::apply(&g, &copy).unwrap_or_else(|e| panic!("{ctx}: {e:?}"));
        drop(g);

        let want: BTreeSet<String> = match &tag_name {
            // `history --all` leaves out the stash branch, whose snapshot is
            // not branch history. (The undone snapshots in `trash_ids` are not
            // in `snapshots` at all.)
            Some(t) => expected(t)["snapshots"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|x| s(&x["branch"]) != "_stash")
                .map(|x| s(&x["hash"]).to_string())
                .collect(),
            None => SYNTH_V2_IDS.iter().map(|h| h.to_string()).collect(),
        };
        let got = all_ids(&repo);
        assert_eq!(got, want, "{ctx}: snapshot ids");
        let report = fsck::check(&repo).unwrap();
        assert!(report.problems.is_empty(), "{ctx}: {:?}", report.problems);
    }
}

#[test]
fn a_repository_written_by_a_newer_version_is_refused_and_left_alone() {
    let (_dir, root) = golden_copy("v5.1.2");
    let db = root.join(".velo/velo.db");
    let newer = FORMAT_VERSION + 1;
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.pragma_update(None, "user_version", newer).unwrap();
    }
    let read_version = || -> u32 {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    };

    match Repo::open(&root) {
        Err(Error::SchemaTooNew { found, supported }) => {
            assert_eq!((found, supported), (newer, FORMAT_VERSION), "{RULE}")
        }
        other => panic!("{RULE}: open gave {:?}", other.map(|_| ())),
    }
    assert_eq!(read_version(), newer, "open changed user_version");
    match Repo::open_and_migrate(&root) {
        Err(Error::SchemaTooNew { found, supported }) => {
            assert_eq!((found, supported), (newer, FORMAT_VERSION), "{RULE}")
        }
        other => panic!("{RULE}: open_and_migrate gave {:?}", other.map(|_| ())),
    }
    assert_eq!(
        read_version(),
        newer,
        "open_and_migrate changed user_version"
    );
}

/// The v4 spec fixture (`tests/corpus/v4`, written by `generate_v4.py` from
/// FORMAT.md 11.5) is a repository this build cannot read: it must be refused
/// as too new, and refusing must not touch a byte of it. 20.3 replaces this
/// with a test that reads the fixture back.
#[test]
fn the_v4_spec_fixture_is_refused_by_a_v3_build() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("repo");
    copy_dir(&corpus().join("v4/repo"), &root);
    let db = root.join(".velo/velo.db");
    let before = fs::read(&db).unwrap();

    // The hand-built raw-block Zstd frames decode, and hash to their names.
    let mut objects = 0;
    for entry in fs::read_dir(root.join(".velo/objects")).unwrap() {
        let entry = entry.unwrap();
        let content = zstd::decode_all(&fs::read(entry.path()).unwrap()[..]).unwrap();
        assert_eq!(
            blake3::hash(&content).to_hex().to_string(),
            entry.file_name().to_string_lossy(),
            "object {:?} does not hash to its name",
            entry.file_name()
        );
        objects += 1;
    }
    assert!(objects > 0);

    match Repo::open_and_migrate(&root) {
        Err(Error::SchemaTooNew { found, supported }) => {
            assert_eq!((found, supported), (4, 3), "{RULE}")
        }
        other => panic!("{RULE}: open_and_migrate gave {:?}", other.map(|_| ())),
    }
    assert_eq!(
        fs::read(&db).unwrap(),
        before,
        "the refused open changed velo.db"
    );
}

#[test]
fn a_bundle_from_a_future_wire_version_is_refused_and_imports_nothing() {
    let mut bytes = fs::read(corpus().join("bundles/v5.1.2.v3.bundle")).unwrap();
    // The u32 version follows the 8-byte magic (docs/FORMAT.md section 9).
    bytes[8..12].copy_from_slice(&99u32.to_le_bytes());
    let (dir, repo) = fresh_repo();
    let copy = dir.path().join("future.bundle");
    fs::write(&copy, &bytes).unwrap();

    let g = repo.write().unwrap();
    match bundle::apply(&g, &copy) {
        Err(Error::InvalidInput { detail }) => assert!(
            detail.contains("Unsupported bundle format version 99"),
            "{RULE}: {detail}"
        ),
        other => panic!(
            "{RULE}: expected the version refusal, got {:?}",
            other.err()
        ),
    }
    drop(g);
    assert!(
        all_ids(&repo).is_empty(),
        "a refused bundle imported history"
    );
}

// ── Signed bundles (wire v4) ─────────────────────────────────────────────────
//
// No release has written wire v4 yet: these goldens were written by the 5.2.0
// development build and are pinned from now on (see README.md).

const UNKNOWN_ALGORITHM: &str = "test-algo";

fn test_key() -> SigningKey {
    let seed = fs::read_to_string(corpus().join("keys/test.seed")).unwrap();
    SigningKey::from_hex(&seed).unwrap()
}

fn v4_expected() -> Value {
    serde_json::from_slice(&fs::read(corpus().join("expected/bundles.v4.json")).unwrap()).unwrap()
}

/// Apply `name` to a fresh repository, returning the repository and the result.
fn apply_v4(name: &str) -> (TempDir, Repo, velo_core::Result<bundle::Applied>) {
    let (dir, repo) = fresh_repo();
    let copy = dir.path().join("in.bundle");
    fs::copy(corpus().join("bundles").join(name), &copy).unwrap();
    let g = repo.write().unwrap();
    let result = bundle::apply(&g, &copy);
    drop(g);
    (dir, repo, result)
}

fn version_byte(name: &str) -> u32 {
    let bytes = fs::read(corpus().join("bundles").join(name)).unwrap();
    u32::from_le_bytes(bytes[8..12].try_into().unwrap())
}

fn expected_ids(exp: &Value) -> Vec<SnapshotId> {
    exp["snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| id(s(h)))
        .collect()
}

#[test]
fn the_signed_v4_bundle_applies_and_verifies_as_trusted() {
    let exp = v4_expected();
    assert_eq!(version_byte("signed.v4.bundle"), 4, "{RULE}");
    let (_dir, repo, result) = apply_v4("signed.v4.bundle");
    result.unwrap_or_else(|e| panic!("{e:?}. {RULE}"));
    let ids = expected_ids(&exp);
    let got = all_ids(&repo);
    let want: BTreeSet<String> = ids.iter().map(|i| i.to_string()).collect();
    assert_eq!(got, want, "{RULE}");

    let public = test_key().public_key();
    assert_eq!(public.to_string(), s(&exp["signer_public_key"]), "{RULE}");
    let trusted: TrustedKeys = [public].into_iter().collect();
    for snapshot in &ids {
        let v = sign::verify(&repo, snapshot, &trusted).unwrap();
        assert!(v.is_trusted(), "{snapshot}: {v:?}. {RULE}");
    }
    assert!(fsck::check(&repo).unwrap().problems.is_empty());
}

#[test]
fn the_bad_signature_v4_bundle_is_refused_and_imports_nothing() {
    assert_eq!(version_byte("bad-signature.v4.bundle"), 4, "{RULE}");
    let (_dir, repo, result) = apply_v4("bad-signature.v4.bundle");
    match result {
        Err(Error::UntrustedData { .. }) => {}
        other => panic!("expected UntrustedData, got {other:?}. {RULE}"),
    }
    assert!(all_ids(&repo).is_empty(), "{RULE}");
}

#[test]
fn the_unknown_algorithm_v4_bundle_applies_and_is_unverifiable() {
    let exp = v4_expected();
    assert_eq!(version_byte("unknown-algorithm.v4.bundle"), 4, "{RULE}");
    let (_dir, repo, result) = apply_v4("unknown-algorithm.v4.bundle");
    result.unwrap_or_else(|e| panic!("{e:?}. {RULE}"));
    let ids = expected_ids(&exp);
    assert_eq!(all_ids(&repo).len(), ids.len(), "{RULE}");
    let trusted: TrustedKeys = [test_key().public_key()].into_iter().collect();
    let row_snapshot = id(s(&exp["unknown_algorithm_snapshot"]));
    for snapshot in &ids {
        let v = sign::verify(&repo, snapshot, &trusted).unwrap();
        assert!(!v.is_trusted(), "{snapshot}: {v:?}");
        if *snapshot == row_snapshot {
            assert_eq!(v.unverifiable.len(), 1, "{v:?}. {RULE}");
            assert!(v.invalid.is_empty() && v.trusted.is_empty());
            let rows = sign::signatures(&repo, snapshot).unwrap();
            assert_eq!(rows[0].algorithm, UNKNOWN_ALGORITHM);
        } else {
            assert!(v.unverifiable.is_empty());
        }
    }
}

/// One-off generator for the three v4 goldens and `expected/bundles.v4.json`.
///
/// Run only deliberately: `VELO_CORPUS_WRITE=1 cargo test -p velo-core --test
/// corpus -- --ignored write_the_v4_goldens`. Timestamps are fixed, so the
/// snapshot ids are stable; the golden-change rule in README.md applies to the
/// output.
#[test]
#[ignore = "writes the checked-in v4 goldens; set VELO_CORPUS_WRITE=1"]
fn write_the_v4_goldens() {
    if std::env::var("VELO_CORPUS_WRITE").as_deref() != Ok("1") {
        panic!("set VELO_CORPUS_WRITE=1 to rewrite the pinned v4 goldens");
    }
    let key = test_key();
    let main: BranchName = "main".parse().unwrap();

    let history_repo = || {
        let (dir, repo) = fresh_repo();
        let g = repo.write().unwrap();
        let mut ids: Vec<SnapshotId> = Vec::new();
        for (n, body) in [(1i64, "one\n"), (2, "two\n")] {
            let new = g
                .save_tree(SaveTree {
                    branch: &main,
                    parent: ids.last(),
                    merge_parent: None,
                    message: "corpus",
                    entries: vec![TreeEntry::file("f.txt", body.as_bytes().to_vec())],
                    meta: SnapshotMeta::new(),
                    author: None,
                    timestamp_ms: Some(1_700_000_000_000 + n),
                    renames: &[],
                })
                .unwrap();
            ids.push(new);
        }
        drop(g);
        (dir, repo, ids)
    };

    let out = corpus().join("bundles");
    // Signed.
    let (_d, repo, ids) = history_repo();
    let g = repo.write().unwrap();
    let sigs: Vec<_> = ids
        .iter()
        .map(|i| sign::sign(&g, i, &key).unwrap())
        .collect();
    drop(g);
    let signed = out.join("signed.v4.bundle");
    bundle::create(&repo, &signed, None).unwrap();

    // Bad signature: flip the last hex digit of the second signature.
    let mut bytes = fs::read(&signed).unwrap();
    let needle = sigs[1].signature.as_bytes();
    let at = bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .unwrap();
    let last = at + needle.len() - 1;
    bytes[last] = if bytes[last] == b'0' { b'1' } else { b'0' };
    fs::write(out.join("bad-signature.v4.bundle"), &bytes).unwrap();

    // Unknown algorithm: one row on the first snapshot.
    let (_d2, repo2, ids2) = history_repo();
    assert_eq!(ids, ids2, "timestamps must make ids deterministic");
    let db = rusqlite::Connection::open(repo2.root().join(".velo/velo.db")).unwrap();
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS signatures (
            snapshot_hash TEXT NOT NULL, algorithm TEXT NOT NULL,
            public_key TEXT NOT NULL, signature TEXT NOT NULL,
            PRIMARY KEY (snapshot_hash, public_key));",
    )
    .unwrap();
    db.execute(
        "INSERT INTO signatures VALUES (?, ?, ?, ?)",
        rusqlite::params![
            ids2[0].as_str(),
            UNKNOWN_ALGORITHM,
            "ab".repeat(32),
            "cd".repeat(64)
        ],
    )
    .unwrap();
    drop(db);
    bundle::create(&repo2, &out.join("unknown-algorithm.v4.bundle"), None).unwrap();

    let exp = json!({
        "snapshots": ids.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
        "signer_public_key": key.public_key().to_string(),
        "signed": {"bundle": "signed.v4.bundle", "rows": [ids[0].to_string(), ids[1].to_string()]},
        "bad_signature": {"bundle": "bad-signature.v4.bundle", "flipped_row": ids[1].to_string()},
        "unknown_algorithm": {"bundle": "unknown-algorithm.v4.bundle", "algorithm": UNKNOWN_ALGORITHM},
        "unknown_algorithm_snapshot": ids[0].to_string(),
    });
    let mut text = serde_json::to_string_pretty(&exp).unwrap();
    text.push('\n');
    fs::write(corpus().join("expected/bundles.v4.json"), text).unwrap();
}
