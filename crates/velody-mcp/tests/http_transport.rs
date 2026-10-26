//! Spawns the real binary with `--http 127.0.0.1:0` and speaks HTTP/1.1 to it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use serde_json::{json, Value};

struct Mcp {
    child: Child,
    addr: String,
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn(extra: &[&str], envs: &[(&str, &str)], store: &Path) -> Mcp {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_velody-mcp"));
    cmd.args(["--store", store.to_str().unwrap(), "--create", "--http"])
        .arg("127.0.0.1:0")
        .args(extra)
        .env_remove("VELODY_MCP_TOKEN")
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut err = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    err.read_line(&mut line).unwrap();
    let addr = line
        .split("listening on http://")
        .nth(1)
        .and_then(|r| r.trim().strip_suffix("/mcp"))
        .unwrap_or_else(|| panic!("no listening line: {line:?}"))
        .to_string();
    // Keep draining stderr so the child never blocks on a full pipe.
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = err.read_to_end(&mut sink);
    });
    Mcp { child, addr }
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {:?}", self.body))
    }
}

fn request(addr: &str, method: &str, headers: &[(&str, &str)], body: &str) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    let mut req = format!(
        "{method} /mcp HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    Resp {
        status,
        headers,
        body: body.to_string(),
    }
}

fn rpc(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

fn init_params() -> Value {
    json!({"protocolVersion": "2025-06-18", "clientInfo": {"name": "http-agent"}})
}

#[test]
fn unauthenticated_and_bad_origin_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let m = spawn(
        &[],
        &[("VELODY_MCP_TOKEN", "s3cret")],
        &dir.path().join("r.velo"),
    );
    let body = rpc(1, "initialize", init_params());
    let r = request(&m.addr, "POST", &[], &body);
    assert_eq!(r.status, 401);
    assert_eq!(r.header("www-authenticate"), Some("Bearer"));
    let r = request(&m.addr, "POST", &[("Authorization", "Bearer wrong")], &body);
    assert_eq!(r.status, 401);
    let r = request(
        &m.addr,
        "POST",
        &[
            ("Authorization", "Bearer s3cret"),
            ("Origin", "http://evil.example"),
        ],
        &body,
    );
    assert_eq!(r.status, 403);
    let r = request(
        &m.addr,
        "POST",
        &[
            ("Authorization", "Bearer s3cret"),
            ("Origin", "http://localhost:3000"),
        ],
        &body,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn allow_origin_permits_a_named_origin() {
    let dir = tempfile::tempdir().unwrap();
    let m = spawn(
        &["--allow-origin", "https://agents.example"],
        &[],
        &dir.path().join("r.velo"),
    );
    let body = rpc(1, "ping", json!({}));
    // No session yet, so a permitted origin gets as far as the session check.
    let r = request(
        &m.addr,
        "POST",
        &[("Origin", "https://agents.example")],
        &body,
    );
    assert_eq!(r.status, 400);
    let r = request(
        &m.addr,
        "POST",
        &[("Origin", "https://other.example")],
        &body,
    );
    assert_eq!(r.status, 403);
}

#[test]
fn non_loopback_bind_without_token_refuses_to_start() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_velody-mcp"))
        .args([
            "--store",
            dir.path().join("r.velo").to_str().unwrap(),
            "--create",
        ])
        .args(["--http", "0.0.0.0:0"])
        .env_remove("VELODY_MCP_TOKEN")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("without a token"));
}

#[test]
fn session_flow_and_save_tree() {
    let dir = tempfile::tempdir().unwrap();
    let tokens = dir.path().join("tokens");
    std::fs::write(&tokens, "# comment\ntok-a\ntok-b\n").unwrap();
    let m = spawn(
        &["--token-file", tokens.to_str().unwrap()],
        &[],
        &dir.path().join("r.velo"),
    );
    let auth = ("Authorization", "Bearer tok-b");

    let init = request(
        &m.addr,
        "POST",
        &[auth],
        &rpc(1, "initialize", init_params()),
    );
    assert_eq!(init.status, 200);
    assert_eq!(init.header("content-type"), Some("application/json"));
    assert_eq!(init.json()["result"]["protocolVersion"], "2025-06-18");
    let sid = init.header("mcp-session-id").unwrap().to_string();
    let s = ("Mcp-Session-Id", sid.as_str());

    // The session id is required afterwards, and must be known.
    let list = rpc(2, "tools/list", json!({}));
    assert_eq!(request(&m.addr, "POST", &[auth], &list).status, 400);
    let bogus = ("Mcp-Session-Id", "nope");
    assert_eq!(request(&m.addr, "POST", &[auth, bogus], &list).status, 404);

    // A notification is accepted with no body.
    let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let r = request(&m.addr, "POST", &[auth, s], note);
    assert_eq!((r.status, r.body.as_str()), (202, ""));

    // A mismatched protocol version is refused; the negotiated one passes.
    let v = ("MCP-Protocol-Version", "2024-11-05");
    assert_eq!(request(&m.addr, "POST", &[auth, s, v], &list).status, 400);
    let v = ("MCP-Protocol-Version", "2025-06-18");
    let r = request(&m.addr, "POST", &[auth, s, v], &list);
    assert_eq!(r.status, 200);
    let names: Vec<String> = r.json()["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"velody_save_tree".to_string()));

    let call = rpc(
        3,
        "tools/call",
        json!({"name": "velody_save_tree", "arguments": {
            "branch": "main", "message": "over http", "files": {"a.txt": "one\n"}}}),
    );
    let r = request(&m.addr, "POST", &[auth, s], &call);
    assert_eq!(r.status, 200);
    let res = &r.json()["result"];
    assert_eq!(res["isError"], false, "{res}");
    let snap = res["structuredContent"]["snapshot"].as_str().unwrap();
    assert_eq!(snap.len(), 64);

    // The session's own client name stamped the metadata.
    let meta = rpc(
        4,
        "tools/call",
        json!({"name": "velody_metadata", "arguments": {"snapshot": snap}}),
    );
    let r = request(&m.addr, "POST", &[auth, s], &meta);
    assert_eq!(
        r.json()["result"]["structuredContent"]["meta"]["mcp"]["client"],
        "http-agent"
    );

    // DELETE ends the session; it is then unknown.
    assert_eq!(request(&m.addr, "DELETE", &[auth, s], "").status, 204);
    assert_eq!(request(&m.addr, "POST", &[auth, s], &list).status, 404);
    assert_eq!(request(&m.addr, "DELETE", &[auth, s], "").status, 404);
}

#[test]
fn get_is_405() {
    let dir = tempfile::tempdir().unwrap();
    let m = spawn(&[], &[], &dir.path().join("r.velo"));
    let r = request(&m.addr, "GET", &[], "");
    assert_eq!(r.status, 405);
    assert!(r.header("allow").unwrap().contains("POST"));
}
