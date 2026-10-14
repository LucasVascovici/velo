//! A store-only session: a single-file repository, no working tree on disk.

use serde_json::{json, Value};
use velo_core::Repo;
use velo_mcp::Server;

fn rpc(s: &mut Server, id: u64, method: &str, params: Value) -> Value {
    let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    serde_json::from_str(&s.handle(&req.to_string()).expect("a response")).unwrap()
}

fn call(s: &mut Server, id: u64, name: &str, args: Value) -> Value {
    rpc(
        s,
        id,
        "tools/call",
        json!({"name": name, "arguments": args}),
    )
}

fn ok(r: &Value) -> &Value {
    assert_eq!(r["result"]["isError"], false, "{r}");
    &r["result"]["structuredContent"]
}

#[test]
fn store_only_session_needs_no_working_tree() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("agent.velo");
    let repo = Repo::create_file(&db).unwrap();
    let mut s = Server::new(repo, "run-s".into()).with_author("bot".into(), None);
    rpc(
        &mut s,
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "clientInfo": {"name": "agent-x"}}),
    );

    // The tool list drops working-tree tools and gains the store tools.
    let list = rpc(&mut s, 2, "tools/list", json!({}));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for want in [
        "velo_save_tree",
        "velo_tree_at",
        "velo_read_file",
        "velo_history",
    ] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    for gone in [
        "velo_save",
        "velo_restore",
        "velo_status",
        "velo_diff",
        "velo_merge_apply",
    ] {
        assert!(!names.contains(&gone), "{gone} should be omitted");
    }

    // Create: three files, one binary, on an unborn branch.
    let binary: Vec<u8> = vec![0, 159, 146, 150, 255, 1];
    let b64 = base64_encode(&binary);
    let first = call(
        &mut s,
        3,
        "velo_save_tree",
        json!({"branch": "main", "message": "create", "tool_call_id": "tc-1",
               "files": {"a.txt": "one\n", "b.txt": "two\n", "dir/c.txt": "three\n"}}),
    );
    let first_id = ok(&first)["snapshot"].as_str().unwrap().to_string();
    assert!(ok(&first)["parent"].is_null());
    let bin = call(
        &mut s,
        4,
        "velo_save_tree",
        json!({"branch": "main", "message": "binary", "encoding": "base64",
               "files": {"blob.bin": b64}}),
    );
    let bin_id = ok(&bin)["snapshot"].as_str().unwrap().to_string();
    assert_eq!(ok(&bin)["parent"], first_id.as_str());

    // Modify one file, delete another, with an explicit base.
    let third = call(
        &mut s,
        5,
        "velo_save_tree",
        json!({"branch": "main", "message": "edit", "base": first_id,
               "parent": bin_id, "files": {"a.txt": "ONE\n", "b.txt": null}}),
    );
    let third = ok(&third);
    assert_eq!(third["changed"], json!(["a.txt"]));
    assert_eq!(third["deleted"], json!(["b.txt"]));
    let third_id = third["snapshot"].as_str().unwrap().to_string();

    let tree = call(&mut s, 6, "velo_tree_at", json!({"snapshot": third_id}));
    let paths: Vec<&str> = ok(&tree)
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    // Carried forward from `base`: the binary file was not in it.
    assert!(paths.contains(&"a.txt") && paths.contains(&"dir/c.txt"));
    assert!(!paths.contains(&"b.txt") && !paths.contains(&"blob.bin"));

    let a = call(
        &mut s,
        7,
        "velo_read_file",
        json!({"snapshot": third_id, "path": "a.txt"}),
    );
    assert_eq!(ok(&a), &json!({"content": "ONE\n", "encoding": "utf8"}));

    // Binary content round-trips, and utf8 falls back to base64 for it.
    for enc in ["base64", "utf8"] {
        let r = call(
            &mut s,
            8,
            "velo_read_file",
            json!({"snapshot": bin_id, "path": "blob.bin", "encoding": enc}),
        );
        assert_eq!(ok(&r), &json!({"content": b64, "encoding": "base64"}));
    }

    // History lists all three, each stamped with the run identity.
    let h = call(&mut s, 10, "velo_history", json!({}));
    let entries = ok(&h)["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().all(|e| e["run"] == "run-s"));
    let m = call(&mut s, 11, "velo_metadata", json!({"snapshot": third_id}));
    let m = ok(&m);
    assert_eq!(m["author"]["name"], "bot");
    assert_eq!(m["meta"]["mcp"]["tool"], "velo_save_tree");
    assert_eq!(m["meta"]["mcp"]["client"], "agent-x");
    assert_eq!(m["meta"]["mcp"]["run"], "run-s");

    // Working-tree tools are refused, naming Unsupported and the alternative.
    for tool in ["velo_status", "velo_save"] {
        let r = call(&mut s, 12, tool, json!({"message": "m"}));
        assert_eq!(r["result"]["isError"], true, "{r}");
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Unsupported"), "{text}");
        assert!(text.contains("velo_"), "{text}");
    }
    let r = call(
        &mut s,
        13,
        "velo_branch",
        json!({"action": "switch", "name": "x"}),
    );
    assert_eq!(r["result"]["isError"], true);

    // Branch list and create work without a checkout.
    let r = call(&mut s, 14, "velo_branch", json!({"action": "list"}));
    assert!(!ok(&r)["branches"].as_array().unwrap().is_empty());

    // Nothing on disk but the database, its WAL files and the write lock.
    drop(s);
    for e in std::fs::read_dir(dir.path()).unwrap() {
        let name = e.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            [
                "agent.velo",
                "agent.velo-wal",
                "agent.velo-shm",
                "agent.velo.lock"
            ]
            .contains(&name.as_str()),
            "unexpected file {name}"
        );
    }
}

#[test]
fn replace_ignores_the_base() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repo::create_file(&dir.path().join("r.velo")).unwrap();
    let mut s = Server::new(repo, "r".into());
    call(
        &mut s,
        1,
        "velo_save_tree",
        json!({"branch": "main", "message": "m1", "files": {"a": "1", "b": "2"}}),
    );
    let r = call(
        &mut s,
        2,
        "velo_save_tree",
        json!({"branch": "main", "message": "m2", "replace": true, "files": {"c": "3"}}),
    );
    let id = ok(&r)["snapshot"].as_str().unwrap().to_string();
    let tree = call(&mut s, 3, "velo_tree_at", json!({"snapshot": id}));
    assert_eq!(ok(&tree).as_array().unwrap().len(), 1);
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine};
    STANDARD.encode(bytes)
}
