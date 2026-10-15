//! Large results are cut with `truncated: true` and a hint, never sent whole.

use serde_json::{json, Value};
use velo_core::Repo;
use velo_mcp::Server;

fn call(s: &mut Server, name: &str, args: Value) -> Value {
    let req = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                     "params": {"name": name, "arguments": args}});
    serde_json::from_str(&s.handle(&req.to_string()).expect("a response")).unwrap()
}

fn ok(r: &Value) -> &Value {
    assert_eq!(r["result"]["isError"], false, "{r}");
    &r["result"]["structuredContent"]
}

fn store() -> (tempfile::TempDir, Server) {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repo::create_file(&dir.path().join("a.velo")).unwrap();
    (
        dir,
        Server::new(repo, "run-l".into()).with_author("bot".into(), None),
    )
}

fn save(s: &mut Server, message: &str, files: Value) -> String {
    let r = call(
        s,
        "velo_save_tree",
        json!({"branch": "main", "message": message, "files": files}),
    );
    ok(&r)["snapshot"].as_str().unwrap().to_string()
}

#[test]
fn read_file_is_cut_at_max_bytes_and_pages() {
    let (_d, mut s) = store();
    let text = "héllo wörld\n".repeat(50);
    let snap = save(&mut s, "big", json!({"big.txt": text}));
    let r = call(
        &mut s,
        "velo_read_file",
        json!({"snapshot": snap, "path": "big.txt", "max_bytes": 101}),
    );
    let p = ok(&r);
    assert_eq!(p["truncated"], true);
    assert!(p["hint"].as_str().unwrap().contains("offset"));
    let first = p["content"].as_str().unwrap().to_string();
    assert!(first.len() <= 101 && text.starts_with(&first));
    let next = p["next_offset"].as_u64().unwrap();
    assert_eq!(next as usize, first.len());
    let r = call(
        &mut s,
        "velo_read_file",
        json!({"snapshot": snap, "path": "big.txt", "offset": next}),
    );
    let p = ok(&r);
    assert_eq!(p["truncated"], false);
    assert_eq!(format!("{first}{}", p["content"].as_str().unwrap()), text);
}

#[test]
fn read_file_default_is_one_mib() {
    let (_d, mut s) = store();
    let snap = save(
        &mut s,
        "huge",
        json!({"huge.txt": "x".repeat(1024 * 1024 + 10)}),
    );
    let r = call(
        &mut s,
        "velo_read_file",
        json!({"snapshot": snap, "path": "huge.txt"}),
    );
    let p = ok(&r);
    assert_eq!(p["truncated"], true);
    assert_eq!(p["content"].as_str().unwrap().len(), 1024 * 1024);
    assert_eq!(p["size"], 1024 * 1024 + 10);
}

#[test]
fn tree_at_pages() {
    let (_d, mut s) = store();
    let files: serde_json::Map<String, Value> =
        (0..5).map(|i| (format!("f{i}.txt"), json!("x"))).collect();
    let snap = save(&mut s, "five", Value::Object(files));
    let r = call(
        &mut s,
        "velo_tree_at",
        json!({"snapshot": snap, "limit": 2}),
    );
    let p = ok(&r);
    assert_eq!(p["files"].as_array().unwrap().len(), 2);
    assert_eq!(
        (p["truncated"].clone(), p["total"].clone()),
        (json!(true), json!(5))
    );
    let r = call(
        &mut s,
        "velo_tree_at",
        json!({"snapshot": snap, "limit": 2, "offset": p["next_offset"]}),
    );
    assert_eq!(ok(&r)["files"][0]["path"], "f2.txt");
    let r = call(&mut s, "velo_tree_at", json!({"snapshot": snap}));
    assert_eq!(ok(&r)["truncated"], false);
}

#[test]
fn history_and_blame_are_cut() {
    let (_d, mut s) = store();
    let mut last = String::new();
    for i in 0..4 {
        last = save(
            &mut s,
            &format!("m{i}"),
            json!({"f.txt": format!("{}\n", "l\n".repeat(i + 3))}),
        );
    }
    let r = call(&mut s, "velo_history", json!({"limit": 3}));
    let p = ok(&r);
    assert_eq!(p["entries"].as_array().unwrap().len(), 3);
    assert_eq!(p["truncated"], true);
    assert!(p["hint"].as_str().unwrap().contains("velo_history"));
    let r = call(&mut s, "velo_history", json!({"from": p["next_from"]}));
    assert_eq!(ok(&r)["entries"].as_array().unwrap().len(), 1);
    assert_eq!(ok(&r)["truncated"], false);
    let r = call(&mut s, "velo_history", json!({}));
    assert_eq!(ok(&r)["truncated"], false);

    let r = call(
        &mut s,
        "velo_blame",
        json!({"path": "f.txt", "at": last, "limit": 2}),
    );
    let p = ok(&r);
    assert_eq!(p["lines"].as_array().unwrap().len(), 2);
    assert_eq!(p["truncated"], true);
    assert_eq!(p["next_start_line"], 3);
}

#[test]
fn bad_limits_are_error_results() {
    let (_d, mut s) = store();
    let snap = save(&mut s, "one", json!({"a": "x"}));
    for (tool, args) in [
        (
            "velo_read_file",
            json!({"snapshot": snap, "path": "a", "max_bytes": 0}),
        ),
        ("velo_tree_at", json!({"snapshot": snap, "limit": "many"})),
        ("velo_history", json!({"limit": 0})),
        ("velo_tree_at", json!({"snapshot": "0000nothere"})),
    ] {
        let r = call(&mut s, tool, args.clone());
        assert!(r.get("error").is_none(), "{args}: {r}");
        assert_eq!(r["result"]["isError"], true, "{args}: {r}");
    }
}
