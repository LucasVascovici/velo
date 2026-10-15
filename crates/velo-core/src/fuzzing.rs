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
    }
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
