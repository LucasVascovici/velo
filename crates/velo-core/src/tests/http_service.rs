//! The HTTP service: routing, bearer tokens, read-only, limits, directory mode.

use std::fs;
use std::path::Path;
use tempfile::TempDir;

use crate::commands::{self, bundle};
use crate::serve::http::{
    ct_eq, Access, Authorize, Operation, Request, Service, Tokens, BAD_PATH, FORBIDDEN, NOT_FOUND,
    READ_ONLY, TOO_LARGE, UNAUTHENTICATED,
};
use crate::{transport, Limits, Repo};

/// A repository at `root` with one saved snapshot containing `name`.
fn repo_with_save(root: &Path, name: &str) -> String {
    fs::create_dir_all(root).unwrap();
    commands::init::run(root).unwrap();
    fs::write(root.join(name), "content\n").unwrap();
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    commands::save::run(&guard, Some("one"), commands::save::Options::default())
        .unwrap()
        .into_result()
        .expect("snapshot")
        .hash
        .into_string()
}

fn snapshot_count(root: &Path) -> i64 {
    let conn = crate::db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
    conn.query_row("SELECT COUNT(*) FROM snapshots", [], |r| r.get(0))
        .unwrap()
}

fn push_body(root: &Path, tip: &str) -> Vec<u8> {
    let conn = crate::db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
    let set = bundle::reachable_ancestry(&conn, tip);
    let pack = bundle::build_pack(
        &conn,
        &crate::storage::ObjectStore::at(root.join(".velo/objects")),
        &set,
    )
    .unwrap();
    let mut body = Vec::new();
    transport::write_string(&mut body, "main").unwrap();
    transport::write_string(&mut body, tip).unwrap();
    body.extend_from_slice(&bundle::encode(&pack));
    body
}

fn text(r: &crate::serve::http::Response) -> String {
    String::from_utf8_lossy(&r.body).into_owned()
}

fn assert_refusal(r: &crate::serve::http::Response, status: u16, kind: &str) {
    assert_eq!(r.status, status, "{}", text(r));
    assert_eq!(r.content_type, "text/plain");
    assert!(
        text(r).starts_with(&format!("velo: {kind}: ")) && text(r).ends_with('\n'),
        "{}",
        text(r)
    );
}

#[test]
fn ct_eq_behaves() {
    assert!(ct_eq(b"secret", b"secret"));
    assert!(!ct_eq(b"secret", b"secreT"));
    assert!(!ct_eq(b"secret", b"secre"));
    assert!(!ct_eq(b"secre", b"secret"));
    assert!(ct_eq(b"", b""));
    assert!(!ct_eq(b"", b"x"));
}

#[test]
fn route_and_method_errors() {
    let tmp = TempDir::new().unwrap();
    repo_with_save(tmp.path(), "a.txt");
    let svc = Service::single(tmp.path());
    assert_refusal(
        &svc.handle(&Request::new("GET", "/nope", None, b"")),
        404,
        NOT_FOUND,
    );
    assert_eq!(
        svc.handle(&Request::new("POST", "/velo/v1/refs", None, b""))
            .status,
        405
    );
    assert_eq!(
        svc.handle(&Request::new("GET", "/x/velo/v1/refs?a=b", None, b""))
            .status,
        200
    );
}

#[test]
fn receive_without_token_is_401_and_changes_nothing() {
    let origin = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    commands::init::run(origin.path()).unwrap();
    let tip = repo_with_save(work.path(), "b.txt");
    let before = snapshot_count(origin.path());
    let svc = Service::single(origin.path()).authorize_with(Tokens::new().read("r").write("w"));
    let body = push_body(work.path(), &tip);

    let r = svc.handle(&Request::new("POST", "/velo/v1/receive", None, &body));
    assert_refusal(&r, 401, UNAUTHENTICATED);
    assert!(r.www_authenticate);
    let r = svc.handle(&Request::new(
        "POST",
        "/velo/v1/receive",
        Some("Bearer nope"),
        &body,
    ));
    assert_eq!(r.status, 401);
    assert_eq!(snapshot_count(origin.path()), before);

    let r = svc.handle(&Request::new(
        "POST",
        "/velo/v1/receive",
        Some("Bearer r"),
        &body,
    ));
    assert_refusal(&r, 403, FORBIDDEN);
    assert_eq!(snapshot_count(origin.path()), before);

    // Authorized push: scheme is case-insensitive, imports and returns OK.
    let r = svc.handle(&Request::new(
        "POST",
        "/velo/v1/receive",
        Some("bEaReR w"),
        &body,
    ));
    assert_eq!(r.status, 200, "{}", text(&r));
    let status = transport::read_string(&mut std::io::Cursor::new(&r.body[..]), u32::MAX).unwrap();
    assert!(status.starts_with("OK "), "{status}");
    assert_eq!(snapshot_count(origin.path()), before + 1);

    // A read token can fetch.
    let r = svc.handle(&Request::new("GET", "/velo/v1/refs", Some("Bearer r"), b""));
    assert_eq!(r.status, 200);
}

#[test]
fn read_only_refuses_push_only() {
    let origin = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    repo_with_save(origin.path(), "a.txt");
    let tip = repo_with_save(work.path(), "b.txt");
    let before = snapshot_count(origin.path());
    let svc = Service::single(origin.path()).read_only(true);
    let body = push_body(work.path(), &tip);
    let r = svc.handle(&Request::new("POST", "/velo/v1/receive", None, &body));
    assert_refusal(&r, 403, READ_ONLY);
    assert_eq!(snapshot_count(origin.path()), before);
    assert_eq!(
        svc.handle(&Request::new("GET", "/velo/v1/refs", None, b""))
            .status,
        200
    );
    assert_eq!(
        svc.handle(&Request::new("POST", "/velo/v1/upload", None, b""))
            .status,
        200
    );
}

#[test]
fn oversized_body_is_413() {
    let tmp = TempDir::new().unwrap();
    repo_with_save(tmp.path(), "a.txt");
    let limits = Limits {
        max_pack_bytes: 8,
        ..Limits::default()
    };
    let svc = Service::single(tmp.path()).limits(limits);
    assert_eq!(svc.max_request_bytes(), 8);
    let r = svc.handle(&Request::new("POST", "/velo/v1/upload", None, &[0u8; 9]));
    assert_refusal(&r, 413, TOO_LARGE);
}

#[test]
fn directory_mode_refuses_unsafe_paths() {
    let tmp = TempDir::new().unwrap();
    repo_with_save(&tmp.path().join("alpha"), "a.txt");
    let svc = Service::directory(tmp.path());
    for p in [
        "/../x/velo/v1/refs",
        "/%2e%2e/x/velo/v1/refs",
        "/.velo/x/velo/v1/refs",
        "//etc/velo/v1/refs",
        "/a//b/velo/v1/refs",
    ] {
        let r = svc.handle(&Request::new("GET", p, None, b""));
        assert_refusal(&r, 400, BAD_PATH);
    }
    let r = svc.handle(&Request::new("GET", "/missing/velo/v1/refs", None, b""));
    assert_refusal(&r, 404, NOT_FOUND);
}

#[test]
fn two_repositories_in_one_directory() {
    let tmp = TempDir::new().unwrap();
    repo_with_save(&tmp.path().join("alpha"), "a.txt");
    repo_with_save(&tmp.path().join("team/beta"), "b.txt");
    let svc = Service::directory(tmp.path());
    for p in ["/alpha/velo/v1/refs", "/team/beta/velo/v1/refs"] {
        let r = svc.handle(&Request::new("GET", p, None, b""));
        assert_eq!(r.status, 200, "{p}: {}", text(&r));
    }
}

#[test]
fn custom_authorizer_sees_repository_and_operation() {
    struct OnlyAlphaReads;
    impl Authorize for OnlyAlphaReads {
        fn authorize(&self, repo: &str, op: Operation, bearer: Option<&str>) -> Access {
            match (repo, op.is_write(), bearer) {
                (_, _, None) => Access::Unauthenticated,
                ("alpha", false, _) => Access::Allow,
                _ => Access::Forbidden,
            }
        }
    }
    let tmp = TempDir::new().unwrap();
    repo_with_save(&tmp.path().join("alpha"), "a.txt");
    repo_with_save(&tmp.path().join("beta"), "b.txt");
    let svc = Service::directory(tmp.path()).authorize_with(OnlyAlphaReads);
    let get = |p: &str| {
        svc.handle(&Request::new("GET", p, Some("Bearer t"), b""))
            .status
    };
    assert_eq!(get("/alpha/velo/v1/refs"), 200);
    assert_eq!(get("/beta/velo/v1/refs"), 403);
}
