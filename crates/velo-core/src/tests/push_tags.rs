//! `push` publishes tags whose snapshot the remote already has, and says so;
//! it never moves a tag the remote already holds.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::sync::{self, Pushed};
use crate::commands::{self, tag};
use crate::transport::Spawn;
use crate::{Repo, SnapshotId, TagName};

fn name(s: &str) -> TagName {
    s.parse().unwrap()
}

fn save(root: &Path, file: &str, msg: &str) -> SnapshotId {
    fs::write(root.join(file), format!("{msg}\n")).unwrap();
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    commands::save::run(&guard, Some(msg), commands::save::Options::default())
        .unwrap()
        .into_result()
        .expect("snapshot")
        .hash
}

fn tag_at(root: &Path, tag_name: &str, id: &SnapshotId) {
    let repo = Repo::open_and_migrate(root).unwrap();
    tag::create(&repo.write().unwrap(), &name(tag_name), Some(id), false).unwrap();
}

fn tags(root: &Path) -> Vec<(String, String)> {
    let repo = Repo::open_and_migrate(root).unwrap();
    tag::list(&repo)
        .unwrap()
        .into_iter()
        .map(|t| (t.name.to_string(), t.snapshot.to_string()))
        .collect()
}

fn clone_to(url: &str, dir: &Path, token: Option<&str>) {
    sync::clone_with_token(
        url,
        &Spawn::new("velo"),
        sync::CloneOptions {
            dir: Some(dir),
            observer: None,
            ..Default::default()
        },
        token,
    )
    .unwrap();
}

fn push(root: &Path, token: Option<String>) -> sync::PushReport {
    let repo = Repo::open_and_migrate(root)
        .unwrap()
        .credentials(move |_| token.clone());
    let guard = repo.write().unwrap();
    sync::push_report(
        &guard,
        "origin",
        None,
        &Spawn::new("velo"),
        Default::default(),
    )
    .unwrap()
}

fn origin(holder: &Path) -> (PathBuf, SnapshotId) {
    let origin = holder.join("origin");
    fs::create_dir_all(&origin).unwrap();
    commands::init::run(&origin).unwrap();
    let base = save(&origin, "a.txt", "base");
    (origin, base)
}

/// The shared scenario: push, tag the pushed tip, push again; a fresh clone has
/// the tag and the report names it. Also, a same-named tag on the remote stays.
fn scenario(url: &str, origin: &Path, base: &SnapshotId, holder: &Path, token: Option<&str>) {
    let copy = holder.join("copy");
    clone_to(url, &copy, token);
    // The remote takes the name `rel` after the clone.
    tag_at(origin, "rel", base);
    let mine = save(&copy, "b.txt", "mine");
    let report = push(&copy, token.map(String::from));
    assert!(matches!(report.pushed, Pushed::Sent { .. }));
    assert!(report.tags.is_empty());

    // Tag after the snapshot is already on the remote.
    tag_at(&copy, "v1", &mine);
    tag_at(&copy, "rel", &mine);
    let report = push(&copy, token.map(String::from));
    assert!(matches!(report.pushed, Pushed::AlreadyUpToDate { .. }));
    assert!(report.tags.contains(&"v1".to_string()), "{:?}", report.tags);

    let fresh = holder.join("fresh");
    clone_to(url, &fresh, token);
    let got = tags(&fresh);
    assert!(got.contains(&("v1".to_string(), mine.to_string())));
    // The remote's own tag was not moved.
    assert!(got.contains(&("rel".to_string(), base.to_string())));

    // Nothing left to publish: the next push reports no tags.
    assert!(push(&copy, token.map(String::from)).tags.is_empty());
}

#[test]
fn a_tag_made_after_its_snapshot_was_pushed_reaches_a_path_remote() {
    let holder = TempDir::new().unwrap();
    let (o, base) = origin(holder.path());
    scenario(o.to_str().unwrap(), &o, &base, holder.path(), None);
}

#[cfg(feature = "http")]
mod http {
    use super::*;
    use crate::serve::http::{Request, Service, Tokens};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serve `connections` requests through the real service, then stop.
    fn serve(root: PathBuf, connections: usize) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let svc = Service::single(root).authorize_with(Tokens::new().write("w"));
        let handle = std::thread::spawn(move || {
            for _ in 0..connections {
                let (mut conn, _) = listener.accept().unwrap();
                let mut buf = Vec::new();
                let mut byte = [0u8; 1];
                while !buf.ends_with(b"\r\n\r\n") {
                    if conn.read(&mut byte).unwrap() == 0 {
                        break;
                    }
                    buf.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let mut first = head.lines().next().unwrap().split_whitespace();
                let (method, path) = (first.next().unwrap(), first.next().unwrap());
                let header = |n: &str| {
                    head.lines().find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case(n).then(|| v.trim().to_string())
                    })
                };
                let len: usize = header("content-length")
                    .map(|v| v.parse().unwrap())
                    .unwrap_or(0);
                let mut body = vec![0u8; len];
                conn.read_exact(&mut body).unwrap();
                let auth = header("authorization");
                let r = svc.handle(&Request::new(method, path, auth.as_deref(), &body));
                let head = format!(
                    "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    r.status,
                    r.content_type,
                    r.body.len()
                );
                conn.write_all(head.as_bytes()).unwrap();
                conn.write_all(&r.body).unwrap();
            }
        });
        (url, handle)
    }

    #[test]
    fn a_tag_made_after_its_snapshot_was_pushed_reaches_an_http_remote() {
        let holder = TempDir::new().unwrap();
        let (o, base) = origin(holder.path());
        // Connections: clone (refs + upload), push (refs + receive), push,
        // fresh clone (refs + upload), final push. Surplus is harmless.
        let (url, server) = serve(o.clone(), 10);
        scenario(&url, &o, &base, holder.path(), Some("w"));
        drop(server); // the thread ends on its own; do not block on spare accepts
    }
}
