//! `velo serve-http`: tokens, read-only, limits, connections, directory mode.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use tempfile::TempDir;

/// Run `velo <args>` in `dir` with an optional `VELO_TOKEN`; (output, ok).
fn velo(dir: &Path, args: &[&str], token: Option<&str>) -> (String, bool) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_velo"));
    cmd.args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env_remove("VELO_TOKEN");
    if let Some(t) = token {
        cmd.env("VELO_TOKEN", t);
    }
    let out = cmd.output().expect("failed to run velo");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (text, out.status.success())
}

struct Server {
    child: Child,
    addr: String,
    url: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn(path: &Path, extra: &[&str], serve_token: Option<&str>) -> Server {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_velo"));
    cmd.args([
        "serve-http",
        path.to_str().unwrap(),
        "--listen",
        "127.0.0.1:0",
    ])
    .args(extra)
    .env_remove("VELO_SERVE_TOKEN")
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    if let Some(t) = serve_token {
        cmd.env("VELO_SERVE_TOKEN", t);
    }
    let mut child = cmd.spawn().expect("failed to spawn velo serve-http");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let url = line
        .trim()
        .strip_prefix("listening on ")
        .unwrap_or_else(|| panic!("unexpected first line: {line:?}"))
        .to_string();
    let addr = url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    Server { child, addr, url }
}

fn write(dir: &Path, name: &str, body: &str) {
    std::fs::write(dir.join(name), body).unwrap();
}

/// A repository with one snapshot.
fn repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    assert!(velo(dir, &["init"], None).1);
    write(dir, "a.txt", "one\n");
    assert!(velo(dir, &["save", "C0"], None).1);
}

/// Clone `url` into `root/clone` and add an unpushed snapshot.
fn clone_with_change(root: &Path, url: &str, token: Option<&str>) -> std::path::PathBuf {
    let (out, ok) = velo(root, &["clone", url, "clone"], token);
    assert!(ok, "clone failed:\n{out}");
    let clone = root.join("clone");
    write(&clone, "new.txt", "x\n");
    assert!(velo(&clone, &["save", "C1"], None).1);
    clone
}

/// Send raw bytes, return the whole response text.
fn raw(addr: &str, request: &[u8]) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    s.write_all(request).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn default_policy_serves_anonymous_fetch_and_refuses_push() {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    repo(&origin);
    let server = spawn(&origin, &[], None);
    let before = velo(&origin, &["history", "--oneline"], None).0;
    let clone = clone_with_change(root.path(), &server.url, None);
    let (out, ok) = velo(&clone, &["push"], None);
    assert!(!ok, "{out}");
    assert!(out.contains("refused access"), "{out}");
    assert_eq!(velo(&origin, &["history", "--oneline"], None).0, before);
}

#[test]
fn server_token_gates_every_request() {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    repo(&origin);
    let server = spawn(&origin, &[], Some("s3cret"));

    let (out, ok) = velo(root.path(), &["clone", &server.url, "anon"], None);
    assert!(!ok, "anonymous fetch should be refused:\n{out}");
    assert!(out.contains("refused access"), "{out}");

    let clone = clone_with_change(root.path(), &server.url, Some("s3cret"));
    let (out, ok) = velo(&clone, &["push"], None);
    assert!(!ok && out.contains("refused access"), "{out}");
    let (out, ok) = velo(&clone, &["push"], Some("wrong"));
    assert!(!ok && out.contains("refused access"), "{out}");
    let (out, ok) = velo(&clone, &["push"], Some("s3cret"));
    assert!(ok && out.contains("Pushed"), "{out}");
}

#[test]
fn token_file_separates_read_and_write_tokens() {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    repo(&origin);
    let tokens = root.path().join("tokens");
    std::fs::write(&tokens, "# team\nread rtok\n\nwrite wtok\n").unwrap();
    let server = spawn(&origin, &["--token-file", tokens.to_str().unwrap()], None);
    let clone = clone_with_change(root.path(), &server.url, Some("rtok"));
    let (out, ok) = velo(&clone, &["push"], Some("rtok"));
    assert!(!ok && out.contains("refused access"), "{out}");
    let (out, ok) = velo(&clone, &["push"], Some("wtok"));
    assert!(ok, "{out}");
}

#[test]
fn read_only_refuses_push_even_with_a_valid_token() {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    repo(&origin);
    let server = spawn(&origin, &["--read-only"], Some("s3cret"));
    let clone = clone_with_change(root.path(), &server.url, Some("s3cret"));
    let (out, ok) = velo(&clone, &["push"], Some("s3cret"));
    assert!(!ok, "{out}");
    assert!(out.contains("does not accept pushes"), "{out}");
}

#[test]
fn oversized_request_is_refused_as_limit_exceeded() {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    repo(&origin);
    let server = spawn(
        &origin,
        &["--max-request-bytes", "1000", "--allow-anonymous-push"],
        None,
    );
    let clone = clone_with_change(root.path(), &server.url, None);
    // Incompressible-looking content, well past 1000 bytes.
    let big: String = (0..4000u32)
        .map(|i| format!("{:x}", i.wrapping_mul(2654435761)))
        .collect();
    write(&clone, "big.txt", &big);
    assert!(velo(&clone, &["save", "big"], None).1);
    let (out, ok) = velo(&clone, &["push"], None);
    assert!(!ok, "{out}");
    assert!(out.contains("exceeds the limit"), "{out}");
}

#[test]
fn directory_mode_serves_two_repositories_and_refuses_traversal() {
    let root = TempDir::new().unwrap();
    let repos = root.path().join("repos");
    repo(&repos.join("alpha"));
    repo(&repos.join("team").join("beta"));
    write(&repos.join("alpha"), "alpha-only.txt", "a\n");
    assert!(velo(&repos.join("alpha"), &["save", "A1"], None).1);
    let server = spawn(&repos, &["--directory"], None);

    let (out, ok) = velo(
        root.path(),
        &["clone", &format!("{}alpha/", server.url), "ca"],
        None,
    );
    assert!(ok, "{out}");
    assert!(root.path().join("ca/alpha-only.txt").exists());
    let (out, ok) = velo(
        root.path(),
        &["clone", &format!("{}team/beta/", server.url), "cb"],
        None,
    );
    assert!(ok, "{out}");
    assert!(root.path().join("cb/a.txt").exists());

    let resp = raw(
        &server.addr,
        b"GET /../repos/alpha/velo/v1/refs HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert!(!resp.starts_with("HTTP/1.1 200"), "{resp}");
    let resp = raw(
        &server.addr,
        b"GET /alpha/../alpha/velo/v1/refs HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert!(resp.contains("400"), "{resp}");
    assert!(resp.contains("velo: bad-path"), "{resp}");
}

#[test]
fn oversized_content_length_is_refused_without_reading_the_body() {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    repo(&origin);
    let server = spawn(&origin, &["--max-request-bytes", "100"], None);
    let resp = raw(
        &server.addr,
        b"POST /velo/v1/upload HTTP/1.1\r\nHost: x\r\nContent-Length: 5000\r\nConnection: close\r\n\r\n",
    );
    assert!(resp.contains("413"), "{resp}");
    assert!(resp.contains("velo: too-large"), "{resp}");
}

#[test]
fn excess_concurrent_requests_get_503_busy() {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    repo(&origin);
    let server = spawn(&origin, &["--max-connections", "1"], None);
    // One request with a promised body that never arrives pins the worker.
    let mut hog = TcpStream::connect(&server.addr).unwrap();
    hog.write_all(
        b"POST /velo/v1/upload HTTP/1.1\r\nHost: x\r\nContent-Length: 5000\r\n\r\npartial",
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let resp = raw(
        &server.addr,
        b"GET /velo/v1/refs HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert!(resp.contains("503"), "{resp}");
    assert!(resp.contains("velo: busy"), "{resp}");
    drop(hog);
}

#[test]
fn absurd_content_length_cannot_kill_workers_or_leak_slots() {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    repo(&origin);
    let server = spawn(
        &origin,
        &["--max-connections", "1", "--max-request-bytes", "100"],
        None,
    );
    // More than max_connections + 1 requests: each one panics inside
    // tiny_http's drain-on-drop, which must not cost a worker or a slot.
    // The client sees the 413 before the worker has finished draining the
    // body and released its slot, so a 503 on the next request is a race, not
    // a leak. A leaked slot would never clear: retry each request on 503 for
    // a few seconds.
    let retry_when_busy = |request: &[u8]| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let resp = raw(&server.addr, request);
            if !resp.contains("503") || std::time::Instant::now() > deadline {
                return resp;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    for _ in 0..5 {
        let resp = retry_when_busy(
            b"POST /velo/v1/upload HTTP/1.1\r\nHost: x\r\nContent-Length: 18446744073709551615\r\nConnection: close\r\n\r\n",
        );
        assert!(resp.contains("413"), "{resp}");
    }
    let resp =
        retry_when_busy(b"GET /velo/v1/refs HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
}

#[test]
fn stalled_request_does_not_pin_the_busy_responder() {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    repo(&origin);
    let server = spawn(&origin, &["--max-connections", "1"], None);
    let stall = b"POST /velo/v1/upload HTTP/1.1\r\nHost: x\r\nContent-Length: 5000\r\n\r\npartial";
    // The first hog pins the only worker; the second is refused 503 on the
    // spare thread, whose drop would block on the unread body.
    let mut hog1 = TcpStream::connect(&server.addr).unwrap();
    hog1.write_all(stall).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let mut hog2 = TcpStream::connect(&server.addr).unwrap();
    hog2.write_all(stall).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let resp = raw(
        &server.addr,
        b"GET /velo/v1/refs HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert!(resp.contains("503"), "{resp}");
    assert!(resp.contains("velo: busy"), "{resp}");
    drop((hog1, hog2));
}
