//! A store-only session: a single-file repository, no working tree on disk.

use serde_json::{json, Value};
use velody_core::Repo;
use velody_mcp::Server;

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
        "velody_save_tree",
        "velody_tree_at",
        "velody_read_file",
        "velody_history",
    ] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    for gone in [
        "velody_save",
        "velody_restore",
        "velody_status",
        "velody_diff",
        "velody_merge_apply",
    ] {
        assert!(!names.contains(&gone), "{gone} should be omitted");
    }

    // Create: three files, one binary, on an unborn branch.
    let binary: Vec<u8> = vec![0, 159, 146, 150, 255, 1];
    let b64 = base64_encode(&binary);
    let first = call(
        &mut s,
        3,
        "velody_save_tree",
        json!({"branch": "main", "message": "create", "tool_call_id": "tc-1",
               "files": {"a.txt": "one\n", "b.txt": "two\n", "dir/c.txt": "three\n"}}),
    );
    let first_id = ok(&first)["snapshot"].as_str().unwrap().to_string();
    assert!(ok(&first)["parent"].is_null());
    let bin = call(
        &mut s,
        4,
        "velody_save_tree",
        json!({"branch": "main", "message": "binary", "encoding": "base64",
               "files": {"blob.bin": b64}}),
    );
    let bin_id = ok(&bin)["snapshot"].as_str().unwrap().to_string();
    assert_eq!(ok(&bin)["parent"], first_id.as_str());

    // Modify one file, delete another, with an explicit base.
    let third = call(
        &mut s,
        5,
        "velody_save_tree",
        json!({"branch": "main", "message": "edit", "base": first_id,
               "parent": bin_id, "files": {"a.txt": "ONE\n", "b.txt": null}}),
    );
    let third = ok(&third);
    assert_eq!(third["changed"], json!(["a.txt"]));
    assert_eq!(third["deleted"], json!(["b.txt"]));
    let third_id = third["snapshot"].as_str().unwrap().to_string();

    let tree = call(&mut s, 6, "velody_tree_at", json!({"snapshot": third_id}));
    let paths: Vec<&str> = ok(&tree)["files"]
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
        "velody_read_file",
        json!({"snapshot": third_id, "path": "a.txt"}),
    );
    assert_eq!(
        ok(&a)["content"],
        "ONE
"
    );
    assert_eq!(ok(&a)["encoding"], "utf8");
    assert_eq!(ok(&a)["truncated"], false);

    // Binary content round-trips, and utf8 falls back to base64 for it.
    for enc in ["base64", "utf8"] {
        let r = call(
            &mut s,
            8,
            "velody_read_file",
            json!({"snapshot": bin_id, "path": "blob.bin", "encoding": enc}),
        );
        assert_eq!(ok(&r)["content"], json!(b64));
        assert_eq!(ok(&r)["encoding"], "base64");
    }

    // History lists all three, each stamped with the run identity.
    let h = call(&mut s, 10, "velody_history", json!({}));
    let entries = ok(&h)["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().all(|e| e["run"] == "run-s"));
    let m = call(&mut s, 11, "velody_metadata", json!({"snapshot": third_id}));
    let m = ok(&m);
    assert_eq!(m["author"]["name"], "bot");
    assert_eq!(m["meta"]["mcp"]["tool"], "velody_save_tree");
    assert_eq!(m["meta"]["mcp"]["client"], "agent-x");
    assert_eq!(m["meta"]["mcp"]["run"], "run-s");

    // Working-tree tools are refused, naming Unsupported and the alternative.
    for tool in ["velody_status", "velody_save"] {
        let r = call(&mut s, 12, tool, json!({"message": "m"}));
        assert_eq!(r["result"]["isError"], true, "{r}");
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Unsupported"), "{text}");
        assert!(text.contains("velody_"), "{text}");
    }
    let r = call(
        &mut s,
        13,
        "velody_branch",
        json!({"action": "switch", "name": "x"}),
    );
    assert_eq!(r["result"]["isError"], true);

    // Branch list and create work without a checkout.
    let r = call(&mut s, 14, "velody_branch", json!({"action": "list"}));
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
        "velody_save_tree",
        json!({"branch": "main", "message": "m1", "files": {"a": "1", "b": "2"}}),
    );
    let r = call(
        &mut s,
        2,
        "velody_save_tree",
        json!({"branch": "main", "message": "m2", "replace": true, "files": {"c": "3"}}),
    );
    let id = ok(&r)["snapshot"].as_str().unwrap().to_string();
    let tree = call(&mut s, 3, "velody_tree_at", json!({"snapshot": id}));
    assert_eq!(ok(&tree)["files"].as_array().unwrap().len(), 1);
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine};
    STANDARD.encode(bytes)
}

fn store(name: &str) -> (tempfile::TempDir, Server) {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repo::create_file(&dir.path().join(name)).unwrap();
    (dir, Server::new(repo, "r".into()))
}

fn err_text(r: &Value) -> String {
    assert_eq!(r["result"]["isError"], true, "{r}");
    r["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

fn tree_objects(s: &mut Server, snap: &str) -> Value {
    let t = call(s, 90, "velody_tree_at", json!({"snapshot": snap}));
    ok(&t)["files"].clone()
}

#[test]
fn changes_form_puts_deletes_and_renames() {
    let (_d, mut s) = store("c.velo");
    call(
        &mut s,
        1,
        "velody_save_tree",
        json!({"branch": "main", "message": "m1",
               "files": {"a.txt": "one\n", "old.txt": "gone\n", "n.md": "notes\n"}}),
    );
    let r = call(
        &mut s,
        2,
        "velody_save_tree",
        json!({"branch": "main", "message": "m2", "changes": [
            {"op": "put", "path": "a.txt", "content": "ONE\n"},
            {"op": "delete", "path": "old.txt"},
            {"op": "rename", "from": "n.md", "to": "docs/n.md"}]}),
    );
    let out = ok(&r).clone();
    assert_eq!(out["renamed"], json!([["n.md", "docs/n.md"]]));
    assert_eq!(out["deleted"], json!(["old.txt"]));
    let snap = out["snapshot"].as_str().unwrap();
    let files = tree_objects(&mut s, snap);
    let paths: Vec<&str> = files
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["a.txt", "docs/n.md"]);
    // Every written id is the object velody_tree_at lists for that path.
    let written = out["written"].as_object().unwrap();
    assert_eq!(written.len(), 1);
    for f in files.as_array().unwrap() {
        if let Some(o) = written.get(f["path"].as_str().unwrap()) {
            assert_eq!(o, &f["object"]);
        }
    }
    let a = call(
        &mut s,
        3,
        "velody_read_file",
        json!({"snapshot": snap, "path": "a.txt"}),
    );
    assert_eq!(ok(&a)["content"], "ONE\n");
}

#[test]
fn files_form_matches_the_replace_path() {
    // The default path goes through save_changes; replace keeps save_tree.
    // Both must store the same tree with the same object ids.
    let (_d1, mut fast) = store("f.velo");
    let (_d2, mut slow) = store("s.velo");
    let first = json!({"branch": "main", "message": "m1", "files": {"a": "1\r\n", "b": "2\n"}});
    let second =
        json!({"branch": "main", "message": "m2", "files": {"a": "x\n", "b": null, "c": "3\n"}});
    call(&mut fast, 1, "velody_save_tree", first.clone());
    call(&mut slow, 1, "velody_save_tree", first);
    let f = call(&mut fast, 2, "velody_save_tree", second);
    let sl = call(
        &mut slow,
        2,
        "velody_save_tree",
        json!({"branch": "main", "message": "m2", "replace": true,
               "files": {"a": "x\n", "c": "3\n"}}),
    );
    let fid = ok(&f)["snapshot"].as_str().unwrap().to_string();
    let sid = ok(&sl)["snapshot"].as_str().unwrap().to_string();
    let ft = tree_objects(&mut fast, &fid);
    assert_eq!(ft, tree_objects(&mut slow, &sid));
    assert_eq!(ok(&f)["written"], ok(&sl)["written"]);
    let w = ok(&f)["written"].as_object().unwrap();
    assert_eq!(w.len(), 2);
    for e in ft.as_array().unwrap() {
        assert_eq!(&e["object"], &w[e["path"].as_str().unwrap()]);
    }
}

#[test]
fn files_and_changes_conflict_and_one_is_required() {
    let (_d, mut s) = store("e.velo");
    let both = call(
        &mut s,
        1,
        "velody_save_tree",
        json!({"branch": "main", "message": "m", "files": {"a": "1"},
               "changes": [{"op": "put", "path": "b", "content": "2"}]}),
    );
    assert!(err_text(&both).contains("not both"));
    let neither = call(
        &mut s,
        2,
        "velody_save_tree",
        json!({"branch": "main", "message": "m"}),
    );
    assert!(err_text(&neither).contains("'files' or 'changes'"));
    let replace = call(
        &mut s,
        3,
        "velody_save_tree",
        json!({"branch": "main", "message": "m", "replace": true,
               "changes": [{"op": "put", "path": "b", "content": "2"}]}),
    );
    assert!(err_text(&replace).contains("replace"));
}

#[test]
fn files_form_ignores_a_null_for_a_path_the_parent_lacks() {
    let (_d, mut s) = store("n.velo");
    let r = call(
        &mut s,
        1,
        "velody_save_tree",
        json!({"branch": "main", "message": "m1", "files": {"a": "1", "never-existed": null}}),
    );
    assert_eq!(ok(&r)["deleted"], json!([]));
    let id = ok(&r)["snapshot"].as_str().unwrap().to_string();
    let r2 = call(
        &mut s,
        2,
        "velody_save_tree",
        json!({"branch": "main", "message": "m2", "files": {"b": "2", "a": null, "ghost": null}}),
    );
    assert_eq!(ok(&r2)["deleted"], json!(["a"]));
    let snap = ok(&r2)["snapshot"].as_str().unwrap().to_string();
    let tree = tree_objects(&mut s, &snap);
    let paths: Vec<&str> = tree
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["b"]);
    assert!(!id.is_empty());
}
