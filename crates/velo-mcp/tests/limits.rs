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
    let r = call(
        &mut s,
        "velo_history",
        json!({"limit": 3, "offset": p["next_offset"]}),
    );
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

/// Pages through every entry of `args`, returning the snapshot ids in order.
fn page_all(s: &mut Server, mut args: Value, limit: u64) -> Vec<String> {
    let mut seen = Vec::new();
    let mut offset = 0u64;
    loop {
        args["limit"] = json!(limit);
        args["offset"] = json!(offset);
        let r = call(s, "velo_history", args.clone());
        let p = ok(&r);
        for e in p["entries"].as_array().unwrap() {
            seen.push(e["snapshot"].as_str().unwrap().to_string());
        }
        if p["truncated"] != true {
            return seen;
        }
        offset = p["next_offset"].as_u64().unwrap();
    }
}

#[test]
fn history_pages_across_a_merge_without_loss() {
    let (_d, mut s) = store();
    let base = save(&mut s, "base", json!({"f": "0"}));
    let tree = |s: &mut Server, branch: &str, msg: &str, parent: &str, merge: Option<&str>| {
        let mut a = json!({"branch": branch, "message": msg, "parent": parent,
                           "files": {"f": msg}});
        if let Some(m) = merge {
            a["merge_parent"] = json!(m);
        }
        let r = call(s, "velo_save_tree", a);
        ok(&r)["snapshot"].as_str().unwrap().to_string()
    };
    let a1 = tree(&mut s, "attempt", "a1", &base, None);
    let a2 = tree(&mut s, "attempt", "a2", &a1, None);
    let x = tree(&mut s, "main", "x", &base, None);
    let m = tree(&mut s, "main", "m", &x, Some(&a2));

    let all = page_all(&mut s, json!({}), 100);
    assert_eq!(all.len(), 5, "{all:?}");
    for limit in [1, 2, 3] {
        let mut got = page_all(&mut s, json!({}), limit);
        assert_eq!(got, all, "limit {limit}");
        got.sort();
        got.dedup();
        assert_eq!(got.len(), 5);
        for id in [&base, &a1, &a2, &x, &m] {
            assert!(got.contains(id), "{id} missing at limit {limit}");
        }
    }
    // Paging with `branch` keeps that scope on every page.
    let on_main = page_all(&mut s, json!({"branch": "main"}), 100);
    assert!(!on_main.is_empty());
    assert_eq!(page_all(&mut s, json!({"branch": "main"}), 1), on_main);
}

#[test]
fn read_files_returns_in_order_with_base64_fallback() {
    let (_d, mut s) = store();
    let snap = save(&mut s, "two", json!({"a.txt": "alpha", "b.txt": "beta"}));
    let bin = call(
        &mut s,
        "velo_save_tree",
        json!({"branch": "main", "message": "bin", "encoding": "base64",
               "files": {"c.bin": "AP+B"}}),
    );
    let snap2 = ok(&bin)["snapshot"].as_str().unwrap().to_string();
    let r = call(
        &mut s,
        "velo_read_files",
        json!({"snapshot": snap2, "paths": ["c.bin", "b.txt", "a.txt"]}),
    );
    let p = ok(&r);
    assert_eq!(p["truncated"], false);
    assert_eq!(p["omitted"], json!([]));
    let f = p["files"].as_array().unwrap();
    let paths: Vec<_> = f.iter().map(|x| x["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["c.bin", "b.txt", "a.txt"]);
    assert_eq!(f[0]["encoding"], "base64");
    assert_eq!(f[0]["content"], "AP+B");
    assert_eq!(f[0]["size"], 3);
    assert_eq!(f[1]["encoding"], "utf8");
    assert_eq!(f[1]["content"], "beta");
    let _ = snap;
}

#[test]
fn read_files_budget_omits_the_rest_and_says_how_to_continue() {
    let (_d, mut s) = store();
    let snap = save(
        &mut s,
        "three",
        json!({"a.txt": "x".repeat(10), "b.txt": "y".repeat(10), "c.txt": "z".repeat(3)}),
    );
    let r = call(
        &mut s,
        "velo_read_files",
        json!({"snapshot": snap, "paths": ["a.txt", "b.txt", "c.txt"], "max_bytes": 15}),
    );
    let p = ok(&r);
    assert_eq!(p["truncated"], true);
    assert_eq!(p["files"].as_array().unwrap().len(), 1);
    assert_eq!(p["omitted"], json!(["b.txt", "c.txt"]));
    assert!(p["hint"].as_str().unwrap().contains("omitted"));
    let r = call(
        &mut s,
        "velo_read_files",
        json!({"snapshot": snap, "paths": ["b.txt", "c.txt"], "max_bytes": 15}),
    );
    assert_eq!(ok(&r)["files"].as_array().unwrap().len(), 2);
}

#[test]
fn read_files_missing_path_is_an_error() {
    let (_d, mut s) = store();
    let snap = save(&mut s, "one", json!({"a.txt": "a"}));
    let r = call(
        &mut s,
        "velo_read_files",
        json!({"snapshot": snap, "paths": ["a.txt", "nope.txt"]}),
    );
    assert_eq!(r["result"]["isError"], true, "{r}");
}
