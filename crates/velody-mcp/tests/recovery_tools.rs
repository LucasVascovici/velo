//! The recovery tools: undo, redo, tags, conflicts, metadata find, blame range
//! and a busy repository.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use velody_core::Repo;
use velody_mcp::Server;

fn rpc(s: &mut Server, id: u64, method: &str, params: Value) -> Value {
    let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    serde_json::from_str(&s.handle(&req.to_string()).expect("a response")).unwrap()
}

fn call(s: &mut Server, name: &str, args: Value) -> Value {
    rpc(s, 9, "tools/call", json!({"name": name, "arguments": args}))
}

fn ok(r: &Value) -> &Value {
    assert_eq!(r["result"]["isError"], false, "{r}");
    &r["result"]["structuredContent"]
}

fn server(dir: &std::path::Path, run: &str) -> Server {
    let mut s = Server::new(Repo::discover(dir).unwrap(), run.into());
    rpc(
        &mut s,
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "clientInfo": {"name": "agent"}}),
    );
    s
}

fn names(s: &mut Server) -> Vec<String> {
    rpc(s, 2, "tools/list", json!({}))["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

fn project() -> (tempfile::TempDir, Server) {
    let dir = tempfile::tempdir().unwrap();
    Repo::init(dir.path()).unwrap();
    std::fs::write(dir.path().join("f.txt"), "one\ntwo\nthree\n").unwrap();
    let s = server(dir.path(), "r1");
    (dir, s)
}

#[test]
fn save_undo_redo_round_trip() {
    let (d, mut s) = project();
    let first = ok(&call(&mut s, "velody_save", json!({"message": "first"})))["snapshot"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::write(d.path().join("f.txt"), "changed\n").unwrap();
    let second = ok(&call(&mut s, "velody_save", json!({"message": "second"})))["snapshot"]
        .as_str()
        .unwrap()
        .to_string();

    let r = call(&mut s, "velody_undo", json!({}));
    let u = ok(&r);
    assert_eq!(u["undone"], second.as_str(), "{r}");
    assert_eq!(u["now_at"], first.as_str());
    assert_eq!(
        std::fs::read_to_string(d.path().join("f.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );

    let r = call(&mut s, "velody_redo", json!({}));
    assert_eq!(ok(&r)["restored"], second.as_str(), "{r}");
    assert_eq!(
        std::fs::read_to_string(d.path().join("f.txt")).unwrap(),
        "changed\n"
    );
}

#[test]
fn tag_create_list_delete() {
    let (_d, mut s) = project();
    let snap = ok(&call(&mut s, "velody_save", json!({"message": "first"})))["snapshot"]
        .as_str()
        .unwrap()
        .to_string();
    let r = call(
        &mut s,
        "velody_tag",
        json!({"action": "create", "name": "v1"}),
    );
    assert_eq!(ok(&r)["snapshot"], snap.as_str(), "{r}");
    // A taken name is refused: there is no overwrite.
    let r = call(
        &mut s,
        "velody_tag",
        json!({"action": "create", "name": "v1", "force": true}),
    );
    assert_eq!(r["result"]["isError"], true, "{r}");
    let r = call(&mut s, "velody_tag", json!({"action": "list"}));
    assert_eq!(ok(&r)["tags"][0]["name"], "v1");
    assert_eq!(ok(&r)["tags"][0]["snapshot"], snap.as_str());
    let r = call(
        &mut s,
        "velody_tag",
        json!({"action": "delete", "name": "v1"}),
    );
    assert_eq!(ok(&r)["deleted"], "v1");
    let r = call(&mut s, "velody_tag", json!({"action": "list"}));
    assert!(ok(&r)["tags"].as_array().unwrap().is_empty());
    let r = call(&mut s, "velody_tag", json!({"action": "create"}));
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(r.get("error").is_none());
}

#[test]
fn conflicts_appear_as_data_after_a_merge() {
    let (d, mut s) = project();
    let f = d.path().join("f.txt");
    ok(&call(&mut s, "velody_save", json!({"message": "base"})));
    ok(&call(
        &mut s,
        "velody_branch",
        json!({"action": "create", "name": "agent-1"}),
    ));
    ok(&call(
        &mut s,
        "velody_branch",
        json!({"action": "switch", "name": "agent-1"}),
    ));
    std::fs::write(&f, "one\ntwo-a1\nthree\n").unwrap();
    ok(&call(&mut s, "velody_save", json!({"message": "a1"})));
    ok(&call(
        &mut s,
        "velody_branch",
        json!({"action": "switch", "name": "main"}),
    ));
    std::fs::write(&f, "one\ntwo-main\nthree\n").unwrap();
    ok(&call(&mut s, "velody_save", json!({"message": "main"})));

    // The tool surface only merges through velody_merge_apply; the stopped state
    // that `velody merge` leaves is produced by core directly.
    let repo = Repo::discover(d.path()).unwrap();
    {
        let guard = repo.write().unwrap();
        let out = velody_core::commands::merge::run(
            &guard,
            velody_core::commands::merge::Mode::Bring { source: "agent-1" },
        );
        assert!(out.is_ok(), "{out:?}");
    }
    drop(repo);

    let r = call(&mut s, "velody_conflicts", json!({}));
    let c = ok(&r);
    let files = c["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{r}");
    assert_eq!(files[0]["path"], "f.txt");
    assert!(files[0]["ancestor"].is_string() && files[0]["ours"].is_string());
    assert_eq!(files[0]["hunks"][0]["ours"], json!(["two-main"]));
    assert_eq!(files[0]["hunks"][0]["theirs"], json!(["two-a1"]));
}

#[test]
fn find_by_metadata() {
    let (d, mut s) = project();
    ok(&call(
        &mut s,
        "velody_save",
        json!({"message": "scored", "meta": {"eval": {"score": "9"}}}),
    ));
    std::fs::write(d.path().join("f.txt"), "x\n").unwrap();
    ok(&call(
        &mut s,
        "velody_save",
        json!({"message": "other", "meta": {"eval": {"score": "1"}}}),
    ));
    let r = call(
        &mut s,
        "velody_find",
        json!({"filters": [["eval", "score", "9"]]}),
    );
    let found = ok(&r)["snapshots"].as_array().unwrap();
    assert_eq!(found.len(), 1, "{r}");
    assert_eq!(found[0]["message"], "scored");
    assert_eq!(found[0]["run"], "r1");
    let r = call(
        &mut s,
        "velody_find",
        json!({"filters": [["eval", "score"]], "branch": "main"}),
    );
    assert_eq!(ok(&r)["snapshots"].as_array().unwrap().len(), 2, "{r}");
    let r = call(&mut s, "velody_find", json!({"filters": []}));
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(r.get("error").is_none());
}

#[test]
fn blame_range_and_invalid_range() {
    let (_d, mut s) = project();
    ok(&call(&mut s, "velody_save", json!({"message": "first"})));
    let r = call(
        &mut s,
        "velody_blame",
        json!({"path": "f.txt", "start_line": 2, "end_line": 3}),
    );
    let lines = ok(&r)["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2, "{r}");
    assert_eq!(lines[0]["line_no"], 2);
    assert_eq!(lines[1]["text"], "three");
    for args in [
        json!({"path": "f.txt", "start_line": 3, "end_line": 2}),
        json!({"path": "f.txt", "start_line": 0}),
        json!({"path": "f.txt", "start_line": 99}),
    ] {
        let r = call(&mut s, "velody_blame", args.clone());
        assert_eq!(r["result"]["isError"], true, "{args}: {r}");
    }
}

#[test]
fn repack_reports_all_fields_and_validates_dry_run() {
    let (_d, mut s) = project();
    ok(&call(&mut s, "velody_save", json!({"message": "first"})));
    let fields = [
        "snapshots_converted",
        "keyframes_kept",
        "chains_shortened",
        "rows_before",
        "rows_after",
        "bytes_before",
        "bytes_after",
        "dry_run",
    ];
    for (args, want) in [
        (json!({"dry_run": true}), true),
        (json!({"dry_run": false}), false),
        (json!({}), false),
    ] {
        let r = call(&mut s, "velody_repack", args.clone());
        let out = ok(&r);
        assert_eq!(out["dry_run"], want, "{args}: {r}");
        for f in fields {
            assert!(out.get(f).is_some(), "{f} missing: {r}");
        }
    }
    let r = call(&mut s, "velody_repack", json!({"dry_run": "yes"}));
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(r.to_string().contains("'dry_run' must be a boolean"), "{r}");
}

#[test]
fn store_mode_lists_tag_and_find_only() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repo::create_file(&dir.path().join("a.velo")).unwrap();
    let mut s = Server::new(repo, "r".into());
    let n = names(&mut s);
    for t in ["velody_tag", "velody_find"] {
        assert!(n.iter().any(|x| x == t), "{t}");
    }
    for t in ["velody_undo", "velody_redo", "velody_conflicts"] {
        assert!(!n.iter().any(|x| x == t), "{t}");
        let r = call(&mut s, t, json!({}));
        assert_eq!(r["result"]["isError"], true, "{t}: {r}");
        assert_eq!(r["result"]["structuredContent"]["error"], "Unsupported");
    }
    let (_d, mut s) = project();
    let n = names(&mut s);
    assert!(["velody_undo", "velody_redo", "velody_conflicts"]
        .iter()
        .all(|t| n.iter().any(|x| x == t)));
    // No tool offers a force argument or rewrites history.
    let list = rpc(&mut s, 3, "tools/list", json!({}));
    for t in list["result"]["tools"].as_array().unwrap() {
        assert!(t["inputSchema"]["properties"].get("force").is_none(), "{t}");
        let name = t["name"].as_str().unwrap();
        for banned in ["squash", "rebase", "compact", "retention", "push", "gc"] {
            assert!(!name.contains(banned), "{name}");
        }
    }
}

#[test]
fn busy_repository_names_the_holder() {
    let (d, s) = project();
    let mut s = s.with_lock_wait(Duration::from_secs(1));
    let other = Repo::open(d.path()).unwrap();
    let held = other.write().unwrap();

    let started = Instant::now();
    let r = call(&mut s, "velody_save", json!({"message": "blocked"}));
    let waited = started.elapsed();
    assert!(waited < Duration::from_secs(4), "{waited:?}");
    assert_eq!(r["result"]["isError"], true, "{r}");
    let pid = std::process::id();
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("Locked: "), "{text}");
    assert!(text.contains(&format!("pid {pid}")), "{text}");
    let sc = &r["result"]["structuredContent"];
    assert_eq!(sc["error"], "Locked");
    assert_eq!(sc["holder"]["pid"], pid);
    assert!(sc["holder"]["started_at_ms"].is_i64(), "{sc}");

    drop(held);
    let r = call(&mut s, "velody_save", json!({"message": "free now"}));
    assert_eq!(r["result"]["isError"], false, "{r}");
}
