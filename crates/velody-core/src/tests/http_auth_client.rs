//! The HTTP client's bearer token and the typed errors for the server's
//! refusals, against a fake server that answers one canned response.

#![cfg(feature = "http")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tempfile::TempDir;

use crate::commands::{self, sync};
use crate::error::VelodyError;
use crate::transport::{self, Spawn};
use crate::Repo;

/// Serve one connection with `status` and `body`, recording the request head.
fn serve_once(status: &str, body: &'static str) -> (String, Arc<Mutex<String>>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let head = Arc::new(Mutex::new(String::new()));
    let seen = head.clone();
    let status = status.to_string();
    let handle = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        while !buf.ends_with(b"\r\n\r\n") {
            if conn.read(&mut byte).unwrap() == 0 {
                break;
            }
            buf.push(byte[0]);
        }
        let text = String::from_utf8_lossy(&buf).to_string();
        let len = text
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let mut payload = vec![0u8; len];
        conn.read_exact(&mut payload).unwrap();
        *seen.lock().unwrap() = text;
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        conn.write_all(resp.as_bytes()).unwrap();
    });
    (url, head, handle)
}

fn repo_with_remote(dir: &Path, url: &str) {
    commands::init::run(dir).unwrap();
    let repo = Repo::open_and_migrate(dir).unwrap();
    let guard = repo.write().unwrap();
    commands::remote::add(&guard, "origin", url).unwrap();
}

/// Fetch from a server answering `status`/`body`, with the token `s3cret`
/// supplied through the handle's credential provider.
fn fetch_against(status: &str, body: &'static str) -> (crate::Result<()>, String) {
    let (url, head, server) = serve_once(status, body);
    let dir = TempDir::new().unwrap();
    repo_with_remote(dir.path(), &url);
    let wanted = url.clone();
    let repo = Repo::open_and_migrate(dir.path())
        .unwrap()
        .credentials(move |u| (u == wanted).then(|| "s3cret".to_string()));
    let guard = repo.write().unwrap();
    let out = sync::fetch(&guard, "origin", &Spawn::new("velody"), Default::default()).map(|_| ());
    server.join().unwrap();
    let head = head.lock().unwrap().clone();
    (out, head)
}

#[test]
fn the_credential_provider_supplies_the_bearer_header() {
    let (out, head) = fetch_against("404 Not Found", "velody: not-found: x");
    assert!(out.is_err());
    assert!(
        head.lines()
            .any(|l| l.eq_ignore_ascii_case("authorization: Bearer s3cret")),
        "{head}"
    );
}

#[test]
fn no_provider_sends_no_authorization() {
    let (url, head, server) = serve_once("401 Unauthorized", "velody: unauthenticated: no token");
    let dir = TempDir::new().unwrap();
    repo_with_remote(dir.path(), &url);
    let repo = Repo::open_and_migrate(dir.path()).unwrap();
    let guard = repo.write().unwrap();
    let out = sync::fetch(&guard, "origin", &Spawn::new("velody"), Default::default());
    server.join().unwrap();
    assert!(matches!(out, Err(VelodyError::AccessDenied { .. })));
    assert!(!head
        .lock()
        .unwrap()
        .to_ascii_lowercase()
        .contains("authorization"));
}

#[test]
fn unauthenticated_and_forbidden_are_access_denied() {
    for (status, body, detail) in [
        (
            "401 Unauthorized",
            "velody: unauthenticated: bad token",
            "bad token",
        ),
        (
            "403 Forbidden",
            "velody: read-only: server is read-only",
            "server is read-only",
        ),
    ] {
        let (out, _) = fetch_against(status, body);
        match out {
            Err(VelodyError::AccessDenied { detail: d, remote }) => {
                assert_eq!(d, detail);
                assert!(remote.starts_with("http://"));
                assert!(!d.contains("s3cret") && !remote.contains("s3cret"));
            }
            other => panic!("expected AccessDenied, got {other:?}"),
        }
    }
}

#[test]
fn too_large_not_found_and_busy_are_typed() {
    let (out, _) = fetch_against("413 Payload Too Large", "velody: too-large: big");
    assert!(matches!(
        out,
        Err(VelodyError::LimitExceeded {
            max: 0,
            found: None,
            ..
        })
    ));
    let (out, _) = fetch_against("404 Not Found", "velody: not-found: nope");
    assert!(matches!(out, Err(VelodyError::NotFound { .. })));
    let (out, _) = fetch_against("503 Service Unavailable", "velody: busy: try later");
    assert!(
        matches!(&out, Err(VelodyError::Io(e)) if e.to_string() == "try later"),
        "{out:?}"
    );
}

#[test]
fn a_url_with_userinfo_is_refused_without_echoing_it() {
    let spawn = Spawn::new("velody");
    for url in [
        "https://user:hunter2@host/repo",
        "http://hunter2@host:8080/",
    ] {
        for result in [
            transport::open(url, &spawn),
            transport::open_with_token(url, &spawn, Some("t")),
        ] {
            match result {
                Err(VelodyError::InvalidInput { detail }) => {
                    assert!(!detail.contains("hunter2"), "{detail}");
                    assert!(detail.contains("token"), "{detail}");
                }
                _ => panic!("expected InvalidInput for {url}"),
            }
        }
    }
    // An @ in the path is not userinfo.
    assert!(transport::open("http://host/a@b", &spawn).is_ok());
}
