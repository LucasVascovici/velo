//! Entry points for the `cargo fuzz` targets in the workspace's `fuzz/` crate.
//!
//! Every function here feeds arbitrary bytes to one reader of untrusted input
//! and discards the `Err`: a refusal is the correct answer to hostile input.
//! Only a panic, an abort, an out-of-memory or a timeout is a bug, and those
//! are what the fuzzer reports. Not part of the supported API; compiled only
//! with the `fuzzing` feature.

use std::path::Path;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

use tempfile::TempDir;

use crate::commands::{self, bundle, import};
use crate::serve::http::{Request, Service, Tokens};
use crate::{
    safe_path, sign, transport, BranchName, Limits, ObjectHash, Repo, SnapshotId, TagName,
};

/// Limits tight enough that a fuzz input cannot spend the run on one
/// allocation, and low enough to exercise the refusal paths.
fn tight() -> Limits {
    Limits::default()
        .with_max_object_bytes(16 << 20)
        .with_max_pack_bytes(64 << 20)
        .with_max_string_bytes(1 << 20)
}

fn init_repo(root: &Path) -> Repo {
    commands::init::run(root).expect("init a fuzz repository");
    Repo::open_and_migrate(root).expect("open a fuzz repository")
}

type Shared = Mutex<Option<(TempDir, Repo)>>;

/// Decode a bundle, and when it decodes, import it into a shared scratch
/// repository so the path rule and the limits are exercised too.
pub fn bundle_decode(data: &[u8]) {
    static REPO: OnceLock<Shared> = OnceLock::new();
    let Ok(parsed) = bundle::decode(data, &tight()) else {
        return;
    };
    let mut slot = REPO.get_or_init(|| Mutex::new(None)).lock().unwrap();
    let (_, repo) = slot.get_or_insert_with(|| {
        let tmp = TempDir::new().expect("temp dir");
        let repo = init_repo(tmp.path()).limited(tight());
        (tmp, repo)
    });
    if let Ok(guard) = repo.write() {
        let _ = bundle::import(&guard, &parsed);
    };
}

/// Read a ref advertisement, then length-prefixed strings until the end.
pub fn wire(data: &[u8]) {
    let mut r = data;
    let _ = transport::read_refs(&mut r);
    let mut r = data;
    while let Ok(Some(_)) = transport::read_string_opt(&mut r, tight().max_string_bytes) {}
}

/// Drive the HTTP service. The input is `method\npath\nflag\nbody`, where the
/// flag byte picks no credential, a read token, a write token or a wrong one.
pub fn http_service(data: &[u8]) {
    static SERVICE: OnceLock<(TempDir, Service)> = OnceLock::new();
    let (_, svc) = SERVICE.get_or_init(|| {
        let tmp = TempDir::new().expect("temp dir");
        let repo_dir = tmp.path().join("one");
        std::fs::create_dir_all(&repo_dir).expect("repo dir");
        init_repo(&repo_dir);
        let svc = Service::directory(tmp.path())
            .authorize_with(Tokens::new().read("r").write("w"))
            .limits(tight());
        (tmp, svc)
    });
    let mut parts = data.splitn(4, |b| *b == b'\n');
    let method = String::from_utf8_lossy(parts.next().unwrap_or(b""));
    let path = String::from_utf8_lossy(parts.next().unwrap_or(b""));
    let flag = parts.next().and_then(|f| f.first().copied()).unwrap_or(0);
    let body = parts.next().unwrap_or(b"");
    let auth = match flag % 4 {
        0 => None,
        1 => Some("Bearer r"),
        2 => Some("Bearer w"),
        _ => Some("Bearer nope"),
    };
    let _ = svc.handle(&Request::new(&method, &path, auth, body));
}

/// Import a `git fast-export` stream into a fresh single-file repository.
pub fn git_import(data: &[u8]) {
    let Ok(tmp) = TempDir::new() else { return };
    let Ok(repo) = Repo::create_file(&tmp.path().join("fuzz.velo")) else {
        return;
    };
    let repo = repo.limited(tight());
    let Ok(guard) = repo.write() else { return };
    let mut input = data;
    let _ = import::git_fast_export(&guard, &mut input, Default::default());
}

/// The path rule, on every line of the input and on the lines as a tree.
pub fn paths(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let lines: Vec<&str> = text.split('\n').collect();
    for line in &lines {
        let _ = safe_path::check(line);
        let _ = safe_path::check_for_this_platform(line);
    }
    let _ = safe_path::check_tree(lines.iter().copied());
}

/// Every parser that takes an id or a name from outside.
pub fn ids(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let _ = SnapshotId::from_str(&text);
    let _ = ObjectHash::from_str(&text);
    let _ = BranchName::from_str(&text);
    let _ = TagName::from_str(&text);
    let _ = sign::PublicKey::from_str(&text);
}

/// How many snapshots the delta-table fuzz repository holds.
const DELTA_SNAPSHOTS: usize = 4;

fn delta_repo() -> (TempDir, Repo, Vec<SnapshotId>) {
    use crate::tree::{SaveTree, TreeEntry};
    use crate::SnapshotMeta;

    let tmp = TempDir::new().expect("temp dir");
    let mut repo = init_repo(tmp.path()).limited(tight());
    // K = 2 makes the saves alternate keyframe and delta.
    repo.set_keyframe_interval(2);
    let branch: BranchName = "main".parse().expect("branch name");
    let mut ids: Vec<SnapshotId> = Vec::new();
    for i in 0..DELTA_SNAPSHOTS {
        let entries = vec![
            TreeEntry::file("a.txt", format!("a{i}").into_bytes()),
            TreeEntry::file("dir/b.txt", b"b".to_vec()),
            TreeEntry::file(format!("n{i}.txt"), b"n".to_vec()),
        ];
        let id = repo
            .write()
            .expect("writer")
            .save_tree(SaveTree {
                branch: &branch,
                parent: ids.last(),
                merge_parent: None,
                message: "fuzz",
                entries,
                meta: SnapshotMeta::default(),
                author: None,
                timestamp_ms: Some(1 + i as i64),
                renames: &[],
            })
            .expect("save a fuzz snapshot");
        ids.push(id);
    }
    (tmp, repo, ids)
}

/// Replace the stored delta tables with records decoded from the input, then
/// read every snapshot every way a reader can.
///
/// A record is `op, a, b, len` followed by `len` path bytes. `op % 2` picks a
/// `tree_deltas` row (snapshot `a`, base `b`; an index past the last snapshot
/// names a base that does not exist) or a `tree_delta_rows` row (snapshot `a`,
/// the path, and in `b` whether the hash and the mode are set). Cycles, missing
/// bases, hash and mode set one without the other, hostile paths and
/// million-link chains are all reachable.
pub fn delta_table(data: &[u8]) {
    static REPO: OnceLock<Mutex<(TempDir, Repo, Vec<SnapshotId>)>> = OnceLock::new();
    let mut slot = REPO
        .get_or_init(|| Mutex::new(delta_repo()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (_, repo, ids) = &mut *slot;
    let conn = repo.conn();
    if crate::tree_store::fuzz_clear(conn).is_err() {
        return;
    }
    let name = |i: u8| -> String {
        ids.get(usize::from(i))
            .map_or_else(|| "f".repeat(64), |id| id.to_string())
    };
    let mut rest = data;
    while let [op, a, b, len, tail @ ..] = rest {
        let take = usize::from(*len).min(tail.len());
        let (path, after) = tail.split_at(take);
        rest = after;
        let snapshot = name(*a % DELTA_SNAPSHOTS as u8);
        if op % 2 == 0 {
            let _ = crate::tree_store::fuzz_set_base(
                conn,
                &snapshot,
                &name(*b % (DELTA_SNAPSHOTS as u8 + 1)),
            );
        } else {
            let path = String::from_utf8_lossy(path).into_owned();
            let hash = (b & 1 != 0).then(|| format!("{:0>64}", format!("{b:x}")));
            let mode = (b & 2 != 0).then_some(0o644_i64);
            let _ = crate::tree_store::fuzz_set_row(conn, &snapshot, &path, hash.as_deref(), mode);
        }
    }
    for id in ids.iter() {
        let _ = repo.tree_at(id);
        let _ = repo.read_file_at(id, "a.txt");
        let _ = repo.read_file_at(id, "dir/b.txt");
    }
    for path in [&[][..], &[Path::new("a.txt")][..], &[Path::new("dir")][..]] {
        let _ = commands::history::run(
            repo,
            commands::history::Options {
                paths: path,
                ..Default::default()
            },
        );
    }
    let _ = commands::fsck::check(repo);
}
