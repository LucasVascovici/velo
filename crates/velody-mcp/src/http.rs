//! MCP's Streamable HTTP transport, JSON-response flavour.
//!
//! One endpoint, `/mcp`. `POST` carries one JSON-RPC message and is answered
//! with `application/json` (the spec allows that instead of an SSE stream);
//! notifications and responses get `202 Accepted`. `GET` is `405` because the
//! server never opens a stream of its own. `DELETE` ends a session.
//!
//! Security follows the spec: the `Origin` header, when present, must be a
//! loopback origin or one named with `--allow-origin` (DNS-rebinding
//! protection); every request needs a bearer token when tokens are configured,
//! compared in constant time by [`velody_core::serve::http::Tokens`]; and the
//! server refuses to listen on a non-loopback address without a token.
//!
//! Concurrency: a [`Server`] owns a `Repo`, which is `Send` but not `Sync`, so
//! one `Server` sits behind a `Mutex` and requests run one at a time. A few
//! worker threads read and check requests (including the body, before taking
//! the lock) so a slow client cannot stall the others. Sessions live in a map
//! next to the `Server`; each keeps its own client name, which
//! [`Server::handle_as`] swaps in for the call.

use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use velody_core::serve::http::{Access, Authorize, Operation, Tokens};

use crate::Server;

/// The most body bytes one request may carry.
pub const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;
/// The most sessions alive at once.
pub const MAX_SESSIONS: usize = 1024;
const WORKERS: usize = 4;

/// Transport policy.
#[derive(Clone, Debug, Default)]
pub struct HttpConfig {
    /// Bearer tokens; `None` means no authentication (loopback only).
    pub tokens: Option<Tokens>,
    /// Origins allowed in addition to loopback ones, compared exactly.
    pub allow_origins: Vec<String>,
}

/// Refuse a bind that would expose an unauthenticated server off the machine.
pub fn check_bind(listen: &str, have_token: bool) -> Result<(), String> {
    let addrs: Vec<SocketAddr> = listen
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve '{listen}': {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("'{listen}' resolves to no address"));
    }
    if !have_token && addrs.iter().any(|a| !a.ip().is_loopback()) {
        return Err(format!(
            "refusing to listen on non-loopback '{listen}' without a token; set VELODY_MCP_TOKEN or pass --token-file"
        ));
    }
    Ok(())
}

/// Whether an `Origin` header value names this machine.
pub fn origin_is_loopback(origin: &str) -> bool {
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    if scheme != "http" && scheme != "https" {
        return false;
    }
    let authority = rest.trim_end_matches('/');
    if authority.contains(['/', '@', '?', '#']) {
        return false;
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        match v6.split_once(']') {
            Some((h, _)) => h,
            None => return false,
        }
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

struct Session {
    client: Option<String>,
    version: String,
}

struct State {
    server: Server,
    sessions: HashMap<String, Session>,
}

/// A response to send: status, extra headers, optional JSON body.
struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Option<String>,
}

impl Reply {
    fn status(status: u16) -> Self {
        Reply {
            status,
            headers: Vec::new(),
            body: None,
        }
    }
    fn text(status: u16, msg: &str) -> Self {
        Reply {
            status,
            headers: vec![("Content-Type", "text/plain; charset=utf-8".into())],
            body: Some(msg.to_string()),
        }
    }
    fn json(body: String) -> Self {
        Reply {
            status: 200,
            headers: vec![("Content-Type", "application/json".into())],
            body: Some(body),
        }
    }
}

/// An unguessable session id: 128 bits from the OS-seeded std hasher, which
/// is enough because every request is also bearer-authenticated.
fn new_session_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let part = |salt: u64| {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(salt);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        h.finish()
    };
    format!("{:016x}{:016x}", part(1), part(2))
}

fn header(req: &tiny_http::Request, name: &str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

/// The checks that need no body: path, origin, token, method.
fn precheck(req: &tiny_http::Request, cfg: &HttpConfig) -> Option<Reply> {
    let path = req.url().split('?').next().unwrap_or("");
    if path != "/mcp" {
        return Some(Reply::text(404, "not found: the endpoint is /mcp"));
    }
    if let Some(origin) = header(req, "origin") {
        if !origin_is_loopback(&origin) && !cfg.allow_origins.contains(&origin) {
            return Some(Reply::text(
                403,
                "origin not allowed; pass --allow-origin to permit it",
            ));
        }
    }
    if let Some(tokens) = &cfg.tokens {
        let bearer = header(req, "authorization").and_then(|v| {
            let (scheme, rest) = v.split_once(' ')?;
            let t = rest.trim();
            (scheme.eq_ignore_ascii_case("bearer") && !t.is_empty()).then(|| t.to_string())
        });
        // Every token is a write token here: velody-mcp has no read-only tier.
        match tokens.authorize("", Operation::Receive, bearer.as_deref()) {
            Access::Allow => {}
            _ => {
                let mut r = Reply::text(401, "a valid bearer token is required");
                r.headers.push(("WWW-Authenticate", "Bearer".into()));
                return Some(r);
            }
        }
    }
    match req.method() {
        tiny_http::Method::Post | tiny_http::Method::Delete => None,
        tiny_http::Method::Get => {
            let mut r = Reply::text(405, "this server opens no event stream; use POST");
            r.headers.push(("Allow", "POST, DELETE".into()));
            Some(r)
        }
        _ => {
            let mut r = Reply::status(405);
            r.headers.push(("Allow", "POST, DELETE".into()));
            Some(r)
        }
    }
}

fn route(
    state: &Mutex<State>,
    session: Option<String>,
    version: Option<String>,
    post: Option<&str>,
) -> Reply {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    // `post == None` is DELETE.
    let Some(body) = post else {
        return match session {
            None => Reply::text(400, "Mcp-Session-Id header required"),
            Some(id) => {
                if st.sessions.remove(&id).is_some() {
                    Reply::status(204)
                } else {
                    Reply::text(404, "unknown session")
                }
            }
        };
    };
    let msg: Result<Value, _> = serde_json::from_str(body);
    let Ok(msg) = msg else {
        return Reply::text(400, "body is not valid JSON");
    };
    if !msg.is_object() {
        return Reply::text(
            400,
            "body must be one JSON-RPC message (batches are not supported)",
        );
    }
    let is_initialize =
        msg.get("method").and_then(Value::as_str) == Some("initialize") && msg.get("id").is_some();
    if is_initialize {
        if st.sessions.len() >= MAX_SESSIONS {
            return Reply::text(503, "too many sessions; DELETE one first");
        }
        let mut client = None;
        let Some(reply) = st.server.handle_as(body, &mut client) else {
            return Reply::status(202);
        };
        let negotiated = serde_json::from_str::<Value>(&reply)
            .ok()
            .and_then(|v| v["result"]["protocolVersion"].as_str().map(String::from));
        let mut r = Reply::json(reply);
        if let Some(version) = negotiated {
            let id = new_session_id();
            r.headers.push(("Mcp-Session-Id", id.clone()));
            st.sessions.insert(id, Session { client, version });
        }
        return r;
    }
    let Some(id) = session else {
        return Reply::text(400, "Mcp-Session-Id header required");
    };
    let st = &mut *st;
    let Some(s) = st.sessions.get_mut(&id) else {
        return Reply::text(404, "unknown or ended session; initialize again");
    };
    if let Some(v) = version {
        if v != s.version {
            return Reply::text(
                400,
                &format!(
                    "MCP-Protocol-Version {v} does not match the negotiated {}",
                    s.version
                ),
            );
        }
    }
    // A client response to a request we never sent has no method.
    if msg.get("method").is_none() && msg.get("id").is_some() {
        return Reply::status(202);
    }
    match st.server.handle_as(body, &mut s.client) {
        Some(reply) => Reply::json(reply),
        None => Reply::status(202),
    }
}

fn handle_request(mut req: tiny_http::Request, state: &Mutex<State>, cfg: &HttpConfig) {
    let reply = match precheck(&req, cfg) {
        Some(r) => r,
        None => {
            let session = header(&req, "mcp-session-id");
            let version = header(&req, "mcp-protocol-version");
            if *req.method() == tiny_http::Method::Delete {
                route(state, session, version, None)
            } else {
                let mut buf = Vec::new();
                let read = req
                    .as_reader()
                    .take(MAX_BODY_BYTES + 1)
                    .read_to_end(&mut buf);
                match read {
                    Err(_) => Reply::text(400, "could not read the body"),
                    Ok(_) if buf.len() as u64 > MAX_BODY_BYTES => {
                        Reply::text(413, "request body too large")
                    }
                    Ok(_) => match String::from_utf8(buf) {
                        Ok(body) => route(state, session, version, Some(&body)),
                        Err(_) => Reply::text(400, "body is not UTF-8"),
                    },
                }
            }
        }
    };
    let mut resp = tiny_http::Response::from_data(reply.body.unwrap_or_default().into_bytes())
        .with_status_code(reply.status);
    for (k, v) in reply.headers {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp.add_header(h);
        }
    }
    let _ = req.respond(resp);
}

/// Listen on `listen` and serve `server` until the process is killed.
///
/// The bound address goes to stderr as `listening on http://ADDR/mcp`, so a
/// caller that passed port 0 can learn the port. Stdout stays unused.
pub fn serve(server: Server, listen: &str, cfg: HttpConfig) -> std::io::Result<()> {
    check_bind(listen, cfg.tokens.is_some()).map_err(std::io::Error::other)?;
    let http = Arc::new(
        tiny_http::Server::http(listen).map_err(|e| std::io::Error::other(e.to_string()))?,
    );
    let addr = http
        .server_addr()
        .to_ip()
        .ok_or_else(|| std::io::Error::other("server has no IP address"))?;
    eprintln!("velody-mcp: listening on http://{addr}/mcp");
    let state = Arc::new(Mutex::new(State {
        server,
        sessions: HashMap::new(),
    }));
    let cfg = Arc::new(cfg);
    let mut threads = Vec::new();
    for _ in 0..WORKERS {
        let (http, state, cfg) = (http.clone(), state.clone(), cfg.clone());
        threads.push(std::thread::spawn(move || {
            while let Ok(req) = http.recv() {
                handle_request(req, &state, &cfg);
            }
        }));
    }
    for t in threads {
        let _ = t.join();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_origins() {
        for ok in [
            "http://localhost:3000",
            "http://127.0.0.1",
            "https://[::1]:8080",
            "http://LOCALHOST",
        ] {
            assert!(origin_is_loopback(ok), "{ok}");
        }
        for bad in [
            "null",
            "http://evil.example",
            "http://localhost.evil.example",
            "http://127.0.0.1@evil.example",
            "file://localhost",
        ] {
            assert!(!origin_is_loopback(bad), "{bad}");
        }
    }

    #[test]
    fn bind_policy() {
        assert!(check_bind("127.0.0.1:0", false).is_ok());
        assert!(check_bind("0.0.0.0:0", false).is_err());
        assert!(check_bind("0.0.0.0:0", true).is_ok());
    }
}
