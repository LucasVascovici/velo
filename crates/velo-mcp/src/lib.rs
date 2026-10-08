//! A synchronous stdio MCP server over a velo repository.
//!
//! [`Server::handle`] takes one JSON-RPC line and returns the response line, if
//! any. The binary only wires it to stdin and stdout, which keeps the whole
//! protocol testable in-process.

pub mod protocol;
pub mod tools;

use serde_json::{json, Value};
use velo_core::Repo;

/// The run id: `--run`, else `$VELO_MCP_RUN`, else `run-<epoch_ms>`.
///
/// The environment is read by the binary and passed in, so this stays pure.
pub fn resolve_run_id(flag: Option<String>, env: Option<String>) -> String {
    flag.filter(|s| !s.is_empty())
        .or(env.filter(|s| !s.is_empty()))
        .unwrap_or_else(|| format!("run-{}", chrono::Utc::now().timestamp_millis()))
}

/// One server over one repository, handling one request at a time.
pub struct Server {
    repo: Repo,
    run: String,
    client: Option<String>,
    author: Option<(String, Option<String>)>,
}

impl Server {
    /// A server for `repo` that stamps writes with `run`.
    pub fn new(repo: Repo, run: String) -> Self {
        Server {
            repo,
            run,
            client: None,
            author: None,
        }
    }

    /// Use a fixed author instead of the client name.
    pub fn with_author(mut self, name: String, email: Option<String>) -> Self {
        self.author = Some((name, email));
        self
    }

    /// Handle one request line. Notifications return `None`.
    pub fn handle(&mut self, line: &str) -> Option<String> {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(protocol::error(
                    &Value::Null,
                    protocol::PARSE_ERROR,
                    &format!("parse error: {e}"),
                ))
            }
        };
        // No `id` means a notification (or a client response we never asked for).
        let id = msg.get("id")?.clone();
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            return Some(protocol::error(&id, -32600, "invalid request: no method"));
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        Some(match method {
            "initialize" => {
                self.client = params
                    .pointer("/clientInfo/name")
                    .and_then(Value::as_str)
                    .map(String::from);
                let requested = params.get("protocolVersion").and_then(Value::as_str);
                protocol::result(&id, protocol::initialize_result(requested))
            }
            "ping" => protocol::result(&id, json!({})),
            "tools/list" => protocol::result(&id, json!({"tools": tools::list()})),
            "tools/call" => self.tools_call(&id, &params),
            other => protocol::error(
                &id,
                protocol::METHOD_NOT_FOUND,
                &format!("method not found: {other}"),
            ),
        })
    }

    fn tools_call(&self, id: &Value, params: &Value) -> String {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return protocol::error(id, protocol::INVALID_PARAMS, "missing tool name");
        };
        let empty = json!({});
        let args = match params.get("arguments") {
            None | Some(Value::Null) => &empty,
            Some(a) => a,
        };
        let client = self.client.as_deref().unwrap_or("unknown");
        let ctx = tools::Context {
            repo: &self.repo,
            run: &self.run,
            client,
            author: self.author.as_ref(),
        };
        match tools::call(&ctx, name, args) {
            Ok(result) => {
                let text = serde_json::to_string_pretty(&result).unwrap_or_default();
                protocol::result(
                    id,
                    json!({"content": [{"type": "text", "text": text}],
                           "structuredContent": result, "isError": false}),
                )
            }
            Err(tools::ToolError::Params(msg)) => {
                protocol::error(id, protocol::INVALID_PARAMS, &msg)
            }
            Err(tools::ToolError::Velo(e)) => {
                let variant = tools::variant_name(&e);
                let mut text = format!("{variant}: {e}");
                let mut structured = json!({"error": variant, "message": e.to_string()});
                match &e {
                    velo_core::Error::DirtyWorkingTree { paths }
                    | velo_core::Error::Conflicts { paths } => {
                        let paths: Vec<String> = paths
                            .iter()
                            .map(|p| p.to_string_lossy().replace('\\', "/"))
                            .collect();
                        if matches!(e, velo_core::Error::Conflicts { .. }) {
                            // Many clients show the model only the text, so the
                            // paths and the next step must be in it.
                            let source = args
                                .get("source")
                                .and_then(Value::as_str)
                                .unwrap_or("<source>");
                            let hint = format!(
                                "Conflicted paths: {}. Call velo_merge_plan {{\"source\": \"{source}\"}} and pass `resolutions` for each conflicted path to velo_merge_apply.",
                                paths.join(", ")
                            );
                            text = format!("{text}\n{hint}");
                            structured["hint"] = json!(hint);
                        }
                        structured["paths"] = json!(paths);
                    }
                    _ => {}
                }
                protocol::result(
                    id,
                    json!({"content": [{"type": "text", "text": text}],
                           "structuredContent": structured, "isError": true}),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> (tempfile::TempDir, Server) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        (dir, Server::new(repo, "run-a".into()))
    }

    fn rpc(s: &mut Server, id: u64, method: &str, params: Value) -> Value {
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let reply = s.handle(&req.to_string()).expect("a response");
        serde_json::from_str(&reply).unwrap()
    }

    fn call(s: &mut Server, name: &str, args: Value) -> Value {
        rpc(s, 9, "tools/call", json!({"name": name, "arguments": args}))
    }

    fn init(s: &mut Server) {
        rpc(
            s,
            1,
            "initialize",
            json!({"protocolVersion": "2025-03-26",
            "clientInfo": {"name": "test-agent"}}),
        );
    }

    #[test]
    fn initialize_handshake() {
        let (_d, mut s) = server();
        let r = rpc(
            &mut s,
            1,
            "initialize",
            json!({"protocolVersion": "2024-11-05",
            "clientInfo": {"name": "c"}}),
        );
        assert_eq!(r["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(r["result"]["capabilities"]["tools"]["listChanged"], false);
        assert_eq!(r["result"]["serverInfo"]["name"], "velo-mcp");
        let r = rpc(&mut s, 2, "initialize", json!({"protocolVersion": "1999"}));
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert!(s
            .handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());
        assert_eq!(rpc(&mut s, 3, "ping", json!({}))["result"], json!({}));
    }

    #[test]
    fn tools_list_names() {
        let (_d, mut s) = server();
        let r = rpc(&mut s, 1, "tools/list", json!({}));
        let names: Vec<&str> = r["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                assert_eq!(t["inputSchema"]["type"], "object");
                t["name"].as_str().unwrap()
            })
            .collect();
        assert_eq!(
            names,
            [
                "velo_save",
                "velo_restore",
                "velo_status",
                "velo_diff",
                "velo_history",
                "velo_metadata",
                "velo_branch",
                "velo_merge_plan",
                "velo_merge_apply",
                "velo_blame"
            ]
        );
    }

    #[test]
    fn save_records_run_identity() {
        let (_d, mut s) = server();
        init(&mut s);
        let r = call(
            &mut s,
            "velo_save",
            json!({"message": "first", "tool_call_id": "tc1",
            "meta": {"eval": {"score": "3"}}}),
        );
        assert_eq!(r["result"]["isError"], false);
        let snap = r["result"]["structuredContent"]["snapshot"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(snap.len(), 64);
        let m = call(&mut s, "velo_metadata", json!({"snapshot": snap}));
        let m = &m["result"]["structuredContent"];
        assert_eq!(m["meta"]["mcp"]["run"], "run-a");
        assert_eq!(m["meta"]["mcp"]["tool"], "velo_save");
        assert_eq!(m["meta"]["mcp"]["client"], "test-agent");
        assert_eq!(m["meta"]["mcp"]["tool_call_id"], "tc1");
        assert_eq!(m["meta"]["eval"]["score"], "3");
        assert_eq!(m["author"]["name"], "test-agent");
        let again = call(&mut s, "velo_save", json!({"message": "noop"}));
        assert_eq!(again["result"]["structuredContent"]["saved"], false);
    }

    #[test]
    fn restore_refuses_dirty_tree() {
        let (d, mut s) = server();
        init(&mut s);
        let r = call(&mut s, "velo_save", json!({"message": "first"}));
        let snap = r["result"]["structuredContent"]["snapshot"]
            .as_str()
            .unwrap()
            .to_string();
        std::fs::write(d.path().join("a.txt"), "dirty\n").unwrap();
        let r = call(
            &mut s,
            "velo_restore",
            json!({"snapshot": snap, "force": true}),
        );
        assert_eq!(r["result"]["isError"], true);
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("DirtyWorkingTree: "), "{text}");
        assert_eq!(r["result"]["structuredContent"]["paths"][0], "a.txt");
        assert_eq!(
            std::fs::read_to_string(d.path().join("a.txt")).unwrap(),
            "dirty\n"
        );
    }

    #[test]
    fn history_filters_by_run() {
        let (d, mut s) = server();
        init(&mut s);
        call(&mut s, "velo_save", json!({"message": "one"}));
        s.run = "run-b".into();
        std::fs::write(d.path().join("a.txt"), "two\n").unwrap();
        call(&mut s, "velo_save", json!({"message": "two"}));
        let r = call(
            &mut s,
            "velo_history",
            json!({"meta": [
            {"namespace": "mcp", "key": "run", "value": "run-b"}]}),
        );
        let e = r["result"]["structuredContent"]["entries"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0]["message"], "two");
        assert_eq!(e[0]["run"], "run-b");
        let r = call(
            &mut s,
            "velo_history",
            json!({"meta": [{"namespace": "mcp", "key": "run"}]}),
        );
        assert_eq!(
            r["result"]["structuredContent"]["entries"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn diff_shows_edit() {
        let (d, mut s) = server();
        init(&mut s);
        call(&mut s, "velo_save", json!({"message": "one"}));
        std::fs::write(d.path().join("a.txt"), "two\n").unwrap();
        let r = call(&mut s, "velo_diff", json!({}));
        let f = &r["result"]["structuredContent"]["files"][0];
        assert_eq!(f["path"], "a.txt");
        assert_eq!(f["change"], "modified");
        let lines = f["hunks"][0]["lines"].to_string();
        assert!(lines.contains("-one") && lines.contains("+two"), "{lines}");
        let st = call(&mut s, "velo_status", json!({}));
        assert_eq!(st["result"]["structuredContent"]["modified"][0], "a.txt");
    }

    #[test]
    fn error_codes() {
        let (_d, mut s) = server();
        init(&mut s);
        assert_eq!(rpc(&mut s, 1, "nope", json!({}))["error"]["code"], -32601);
        let bad = serde_json::from_str::<Value>(&s.handle("{not json").unwrap()).unwrap();
        assert_eq!(bad["error"]["code"], -32700);
        assert!(bad["id"].is_null());
        assert_eq!(
            call(&mut s, "velo_nope", json!({}))["error"]["code"],
            -32602
        );
        for ns in ["velo", "mcp"] {
            let r = call(
                &mut s,
                "velo_save",
                json!({"message": "x", "meta": {ns: {"k": "v"}}}),
            );
            assert_eq!(r["error"]["code"], -32602, "{ns}");
        }
        let r = call(&mut s, "velo_save", json!({}));
        assert_eq!(r["error"]["code"], -32602);
    }

    fn sc(r: &Value) -> &Value {
        &r["result"]["structuredContent"]
    }

    fn agent_repo() -> (tempfile::TempDir, Server, Server) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        std::fs::write(dir.path().join("f.txt"), "top\nmid\nend\n").unwrap();
        let mut s1 = Server::new(repo, "r1".into());
        init(&mut s1);
        call(&mut s1, "velo_save", json!({"message": "base"}));
        let s2 = Server::new(Repo::open(dir.path()).unwrap(), "r2".into());
        (dir, s1, s2)
    }

    /// Branch a1 saved by r1, main edited by r2 on the same line; returns with
    /// main checked out.
    fn conflicted() -> (tempfile::TempDir, Server, Server) {
        let (d, mut s1, mut s2) = agent_repo();
        let f = d.path().join("f.txt");
        let r = call(
            &mut s1,
            "velo_branch",
            json!({"action": "create", "name": "a1"}),
        );
        assert_eq!(r["result"]["isError"], false, "{r}");
        let r = call(
            &mut s1,
            "velo_branch",
            json!({"action": "switch", "name": "a1"}),
        );
        assert_eq!(sc(&r)["result"], "started_unborn", "{r}");
        std::fs::write(&f, "top\nmid-a1\nend\nadded by a1\n").unwrap();
        call(&mut s1, "velo_save", json!({"message": "a1 work"}));
        call(
            &mut s1,
            "velo_branch",
            json!({"action": "switch", "name": "main"}),
        );
        std::fs::write(&f, "top\nmid-main\nend\n").unwrap();
        init(&mut s2);
        let r = call(&mut s2, "velo_save", json!({"message": "main work"}));
        assert_eq!(r["result"]["isError"], false, "{r}");
        (d, s1, s2)
    }

    #[test]
    fn agent_branch_plan_apply() {
        let (d, _s1, mut s2) = conflicted();
        let r = call(&mut s2, "velo_merge_plan", json!({"source": "a1"}));
        let p = sc(&r);
        assert_eq!(p["clean"], false, "{r}");
        assert_eq!(
            p["files"][0]["conflict"]["ours_text"],
            "top\nmid-main\nend\n"
        );
        assert_eq!(
            p["files"][0]["conflict"]["theirs_text"],
            "top\nmid-a1\nend\nadded by a1\n"
        );
        assert!(p["apply_hint"]
            .as_str()
            .unwrap()
            .contains("velo_merge_apply"));

        let r = call(
            &mut s2,
            "velo_merge_apply",
            json!({"source": "a1", "message": "m"}),
        );
        assert_eq!(r["result"]["isError"], true);
        let t = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(t.starts_with("Conflicts: "), "{t}");
        assert!(t.contains("f.txt") && t.contains("velo_merge_plan"), "{t}");
        assert_eq!(r["result"]["structuredContent"]["paths"][0], "f.txt");

        let r = call(
            &mut s2,
            "velo_merge_apply",
            json!({"source": "a1", "message": "merge a1", "tool_call_id": "t9",
                   "resolutions": {"f.txt": {"content": "resolved\n"}}}),
        );
        assert_eq!(r["result"]["isError"], false, "{r}");
        let snap = sc(&r)["snapshot"].as_str().unwrap().to_string();
        assert_eq!(
            std::fs::read_to_string(d.path().join("f.txt")).unwrap(),
            "resolved\n"
        );
        let m = call(&mut s2, "velo_metadata", json!({"snapshot": snap}));
        assert_eq!(sc(&m)["meta"]["mcp"]["run"], "r2");
        assert_eq!(sc(&m)["meta"]["mcp"]["tool"], "velo_merge_apply");
        let h = call(&mut s2, "velo_history", json!({"limit": 1}));
        let e = &sc(&h)["entries"][0];
        assert_eq!(e["snapshot"], snap.as_str());
        assert!(e["merge_parent"].is_string());
        let st = call(&mut s2, "velo_status", json!({}));
        assert_eq!(sc(&st)["position"], snap.as_str());
    }

    #[test]
    fn blame_names_the_run() {
        let (_d, _s1, mut s2) = conflicted();
        let r = call(
            &mut s2,
            "velo_merge_apply",
            json!({"source": "a1", "message": "merge",
                   "resolutions": {"f.txt": {"content": "top\nmid-main\nend\nadded by a1\n"}}}),
        );
        assert_eq!(r["result"]["isError"], false, "{r}");
        let r = call(&mut s2, "velo_blame", json!({"path": "f.txt"}));
        let lines = sc(&r)["lines"].as_array().unwrap().clone();
        assert_eq!(lines.len(), 4, "{r}");
        assert_eq!(lines[3]["text"], "added by a1");
        assert_eq!(lines[3]["origin"]["run"], "r1");
        let r = call(
            &mut s2,
            "velo_blame",
            json!({"path": "f.txt", "start_line": 4, "end_line": 4}),
        );
        assert_eq!(sc(&r)["lines"].as_array().unwrap().len(), 1);
        assert_eq!(sc(&r)["lines"][0]["line_no"], 4);
    }

    #[test]
    fn dirty_tree_blocks_apply_and_switch() {
        let (d, _s1, mut s2) = conflicted();
        std::fs::write(d.path().join("f.txt"), "dirty\n").unwrap();
        let r = call(
            &mut s2,
            "velo_merge_apply",
            json!({"source": "a1", "message": "m",
                   "resolutions": {"f.txt": "theirs"}}),
        );
        assert_eq!(r["result"]["isError"], true);
        let t = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(t.starts_with("DirtyWorkingTree: "), "{t}");
        let r = call(
            &mut s2,
            "velo_branch",
            json!({"action": "switch", "name": "a1", "force": true}),
        );
        assert_eq!(r["result"]["isError"], true);
        let t = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(t.starts_with("DirtyWorkingTree: "), "{t}");
        assert_eq!(
            std::fs::read_to_string(d.path().join("f.txt")).unwrap(),
            "dirty\n"
        );
    }

    #[test]
    fn merge_plan_writes_nothing() {
        let (d, _s1, mut s2) = conflicted();
        let before = s2.repo.head_token().unwrap();
        let file = std::fs::read_to_string(d.path().join("f.txt")).unwrap();
        call(&mut s2, "velo_merge_plan", json!({"source": "a1"}));
        assert_eq!(s2.repo.head_token().unwrap(), before);
        assert_eq!(
            std::fs::read_to_string(d.path().join("f.txt")).unwrap(),
            file
        );
        let r = call(&mut s2, "velo_branch", json!({"action": "list"}));
        assert_eq!(sc(&r)["branches"].as_array().unwrap().len(), 2);
    }
}
