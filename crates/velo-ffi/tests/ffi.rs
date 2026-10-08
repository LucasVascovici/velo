//! The C ABI, exercised the way a C caller would: through the exported
//! functions, with raw pointers, and freeing what it is handed.

use std::ffi::{c_char, CStr, CString};
use std::path::Path;
use std::ptr;

use velo_ffi::*;

fn c(text: &str) -> CString {
    CString::new(text).unwrap()
}

unsafe fn take_string(p: *mut c_char) -> String {
    assert!(!p.is_null());
    let s = CStr::from_ptr(p).to_str().unwrap().to_string();
    velo_string_free(p);
    s
}

fn last_message() -> String {
    let p = velo_last_error_message();
    assert!(!p.is_null(), "a failure must leave a message");
    unsafe { CStr::from_ptr(p).to_str().unwrap().to_string() }
}

fn init(dir: &Path) -> *mut VeloRepo {
    let mut repo = ptr::null_mut();
    let path = c(dir.to_str().unwrap());
    assert_eq!(unsafe { velo_repo_init(path.as_ptr(), &mut repo) }, 0);
    assert!(!repo.is_null());
    repo
}

unsafe fn two_file_tree() -> *mut VeloTree {
    let mut tree = ptr::null_mut();
    assert_eq!(velo_tree_new(&mut tree), 0);
    let (a, b) = (c("a.txt"), c("dir/b.sh"));
    assert_eq!(
        velo_tree_add_file(tree, a.as_ptr(), b"alpha\n".as_ptr(), 6, VELO_KIND_REGULAR),
        0
    );
    assert_eq!(
        velo_tree_add_file(
            tree,
            b.as_ptr(),
            b"#!/bin/sh\n".as_ptr(),
            10,
            VELO_KIND_EXECUTABLE
        ),
        0
    );
    tree
}

const OPTS: &str = r#"{"branch":"main","message":"first","timestamp_ms":1700000000000,
    "author":{"name":"Ada","email":"ada@example.com"},
    "meta":{"app":{"run":"42"}}}"#;

unsafe fn save(repo: *const VeloRepo, tree: *const VeloTree, opts: &str) -> String {
    let mut id = ptr::null_mut();
    let opts = c(opts);
    assert_eq!(
        velo_save_tree(repo, tree, opts.as_ptr(), &mut id),
        0,
        "{}",
        last_message()
    );
    take_string(id)
}

#[test]
fn init_open_and_free() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        velo_repo_free(init(dir.path()));
        let mut repo = ptr::null_mut();
        let path = c(dir.path().to_str().unwrap());
        assert_eq!(velo_repo_open(path.as_ptr(), &mut repo), 0);
        assert!(!repo.is_null());
        velo_repo_free(repo);
        // Freeing NULL is a documented no-op.
        velo_repo_free(ptr::null_mut());
        velo_string_free(ptr::null_mut());
        velo_bytes_free(ptr::null_mut(), 0);
        velo_tree_free(ptr::null_mut());
    }
}

#[test]
fn opening_a_missing_path_fails_with_a_message() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nothing-here");
    let mut repo = ptr::null_mut();
    let path = c(missing.to_str().unwrap());
    let code = unsafe { velo_repo_open(path.as_ptr(), &mut repo) };
    assert_eq!(code, VELO_ERR_NOT_A_REPO);
    assert!(repo.is_null());
    assert!(!last_message().is_empty());
    assert_eq!(velo_last_error_code(), code);
}

#[test]
fn save_tree_matches_the_rust_id() {
    use velo_core::tree::{SaveTree, TreeEntry};
    use velo_core::{Author, SnapshotMeta};

    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    unsafe {
        let repo = init(dir.path());
        let tree = two_file_tree();
        let ffi_id = save(repo, tree, OPTS);

        let rust = velo_core::Repo::init(other.path()).unwrap();
        let branch = "main".parse().unwrap();
        let author = Author::with_email("Ada", "ada@example.com").unwrap();
        let mut meta = SnapshotMeta::new();
        meta.set("app", "run", "42").unwrap();
        let rust_id = rust
            .write()
            .unwrap()
            .save_tree(SaveTree {
                branch: &branch,
                parent: None,
                merge_parent: None,
                message: "first",
                entries: vec![
                    TreeEntry::file("a.txt", b"alpha\n".to_vec()),
                    TreeEntry::executable("dir/b.sh", b"#!/bin/sh\n".to_vec()),
                ],
                meta,
                author: Some(&author),
                renames: &[],
                timestamp_ms: Some(1_700_000_000_000),
            })
            .unwrap();
        assert_eq!(ffi_id, rust_id.as_str());

        // Saving never creates files in the repository directory.
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names.iter().any(|n| n == "a.txt" || n == "dir"),
            "{names:?}"
        );

        velo_tree_free(tree);
        velo_repo_free(repo);
    }
}

#[test]
fn reads_return_documented_json_and_bytes() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        let repo = init(dir.path());
        let tree = two_file_tree();
        let id = save(repo, tree, OPTS);
        let main = c("main");

        let mut json = ptr::null_mut();
        assert_eq!(velo_tree_at(repo, main.as_ptr(), &mut json), 0);
        let files: serde_json::Value = serde_json::from_str(&take_string(json)).unwrap();
        let files = files.as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0]["path"], "a.txt");
        assert_eq!(files[0]["kind"], "regular");
        assert_eq!(files[1]["path"], "dir/b.sh");
        assert_eq!(files[1]["kind"], "executable");
        assert!(files[0]["object"].as_str().unwrap().len() > 8);

        let (mut data, mut len) = (ptr::null_mut(), 0usize);
        let path = c("a.txt");
        assert_eq!(
            velo_read_file_at(repo, main.as_ptr(), path.as_ptr(), &mut data, &mut len),
            0
        );
        assert_eq!(std::slice::from_raw_parts(data, len), b"alpha\n");
        velo_bytes_free(data, len);

        let mut json = ptr::null_mut();
        assert_eq!(velo_snapshot(repo, main.as_ptr(), &mut json), 0);
        let snap: serde_json::Value = serde_json::from_str(&take_string(json)).unwrap();
        assert_eq!(snap["id"], id.as_str());
        assert_eq!(snap["message"], "first");
        assert_eq!(snap["branch"], "main");
        assert_eq!(snap["created_at_ms"], 1_700_000_000_000i64);
        assert!(snap["created_at"].is_string());
        assert!(snap["parent"].is_null() && snap["merge_parent"].is_null());
        assert!(snap["tag"].is_null());

        let mut json = ptr::null_mut();
        assert_eq!(velo_snapshot_meta(repo, main.as_ptr(), &mut json), 0);
        let meta: serde_json::Value = serde_json::from_str(&take_string(json)).unwrap();
        assert_eq!(meta["app"]["run"], "42");
        assert_eq!(meta["velo"]["author.name"], "Ada");

        // A second save on top of the first records its parent.
        let child = save(
            repo,
            tree,
            &format!(r#"{{"branch":"main","message":"second","parent":"{id}"}}"#),
        );
        let spec = c(&child);
        let mut json = ptr::null_mut();
        assert_eq!(velo_snapshot(repo, spec.as_ptr(), &mut json), 0);
        let snap: serde_json::Value = serde_json::from_str(&take_string(json)).unwrap();
        assert_eq!(snap["parent"], id.as_str());

        velo_tree_free(tree);
        velo_repo_free(repo);
    }
}

#[test]
fn bad_json_and_null_arguments_have_their_own_codes() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        let repo = init(dir.path());
        let tree = two_file_tree();
        let mut id = ptr::null_mut();

        let bad = c("{not json");
        assert_eq!(
            velo_save_tree(repo, tree, bad.as_ptr(), &mut id),
            VELO_ERR_INVALID_JSON
        );
        let missing = c(r#"{"branch":"main"}"#);
        assert_eq!(
            velo_save_tree(repo, tree, missing.as_ptr(), &mut id),
            VELO_ERR_INVALID_JSON
        );
        assert!(id.is_null());

        assert_eq!(
            velo_save_tree(repo, tree, ptr::null(), &mut id),
            VELO_ERR_NULL_ARGUMENT
        );
        assert_eq!(
            velo_save_tree(ptr::null(), tree, bad.as_ptr(), &mut id),
            VELO_ERR_NULL_ARGUMENT
        );
        assert_eq!(
            velo_repo_open(ptr::null(), ptr::null_mut()),
            VELO_ERR_NULL_ARGUMENT
        );
        assert!(last_message().contains("path"));

        velo_tree_free(tree);
        velo_repo_free(repo);
    }
}

#[test]
fn an_unborn_branch_has_no_tip_and_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        let repo = init(dir.path());
        let branch = c("main");
        let mut id = ptr::NonNull::<c_char>::dangling().as_ptr();
        assert_eq!(velo_branch_tip(repo, branch.as_ptr(), &mut id), 0);
        assert!(id.is_null());

        let tree = two_file_tree();
        let saved = save(repo, tree, OPTS);
        assert_eq!(velo_branch_tip(repo, branch.as_ptr(), &mut id), 0);
        assert_eq!(take_string(id), saved);

        velo_tree_free(tree);
        velo_repo_free(repo);
    }
}

#[test]
fn header_declares_every_exported_symbol_and_code() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let header = std::fs::read_to_string(root.join("include/velo.h")).unwrap();

    let mut functions = 0;
    let mut codes = 0;
    for file in [
        "lib.rs",
        "error.rs",
        "repo.rs",
        "tree.rs",
        "history.rs",
        "merge.rs",
        "branches.rs",
    ] {
        let source = std::fs::read_to_string(root.join("src").join(file)).unwrap();
        for line in source.lines() {
            let line = line.trim();
            if let Some(rest) = line
                .strip_prefix("pub unsafe extern \"C\" fn ")
                .or_else(|| line.strip_prefix("pub extern \"C\" fn "))
            {
                let name = rest.split('(').next().unwrap();
                assert!(name.starts_with("velo_"), "{name}");
                assert!(header.contains(&format!("{name}(")), "velo.h lacks {name}");
                functions += 1;
            } else if let Some(rest) = line.strip_prefix("pub const VELO_") {
                let name = format!("VELO_{}", rest.split(':').next().unwrap());
                assert!(
                    header.contains(&format!("#define {name} ")),
                    "velo.h lacks {name}"
                );
                codes += 1;
            }
        }
    }
    assert!(functions >= 27, "found only {functions} exported functions");
    assert!(codes >= 30, "found only {codes} constants");
    assert!(header.contains("#define VELO_ERR_COMPACTED 25"));
}

// ---- history, blame, merge, branches ----

use serde_json::{json, Value};

unsafe fn file_tree(files: &[(&str, &str)]) -> *mut VeloTree {
    let mut tree = ptr::null_mut();
    assert_eq!(velo_tree_new(&mut tree), 0);
    for (path, text) in files {
        let p = c(path);
        assert_eq!(
            velo_tree_add_file(
                tree,
                p.as_ptr(),
                text.as_ptr(),
                text.len(),
                VELO_KIND_REGULAR
            ),
            0
        );
    }
    tree
}

/// Save a one-file tree and return the new snapshot id.
unsafe fn save_file(repo: *const VeloRepo, text: &str, opts: Value) -> String {
    let tree = file_tree(&[("f.txt", text)]);
    let id = save(repo, tree, &opts.to_string());
    velo_tree_free(tree);
    id
}

/// Call a `(repo, text, out_json)` function and parse its JSON result.
unsafe fn call_json(
    f: unsafe extern "C" fn(*const VeloRepo, *const c_char, *mut *mut c_char) -> i32,
    repo: *const VeloRepo,
    arg: Value,
) -> Result<Value, i32> {
    let arg = c(&arg.to_string());
    let mut out = ptr::null_mut();
    match f(repo, arg.as_ptr(), &mut out) {
        0 => Ok(serde_json::from_str(&take_string(out)).unwrap()),
        code => Err(code),
    }
}

#[test]
fn history_filters_by_meta_and_find_snapshots_agrees() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        let repo = init(dir.path());
        let a = save_file(
            repo,
            "1\n",
            json!({"branch":"main","message":"one","timestamp_ms":1700000000000i64,
                   "meta":{"app":{"run":"1"}}}),
        );
        let b = save_file(
            repo,
            "2\n",
            json!({"branch":"main","message":"two","parent":a,"timestamp_ms":1700000001000i64,
                   "meta":{"app":{"run":"2"}}}),
        );
        let c3 = save_file(
            repo,
            "3\n",
            json!({"branch":"main","message":"three","parent":b,"timestamp_ms":1700000002000i64,
                   "meta":{"app":{"run":"2"}}}),
        );

        let ids = |v: &Value| -> Vec<String> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|e| e["id"].as_str().unwrap().to_string())
                .collect()
        };

        let all = call_json(velo_history, repo, json!({"from": c3})).unwrap();
        assert_eq!(ids(&all["entries"]), [c3.clone(), b.clone(), a.clone()]);
        assert!(all["empty"].is_null());
        assert_eq!(all["entries"][0]["message"], "three");

        let eq = json!([{"namespace":"app","key":"run","value":"2"}]);
        let hit = call_json(velo_history, repo, json!({"from": c3, "meta": eq})).unwrap();
        assert_eq!(ids(&hit["entries"]), [c3.clone(), b.clone()]);
        let found = call_json(velo_find_snapshots, repo, eq).unwrap();
        assert_eq!(ids(&found), [c3.clone(), b.clone()]);

        let has = json!([{"namespace":"app","key":"run"}]);
        let limited = call_json(
            velo_history,
            repo,
            json!({"branch":"main","meta":has,"limit":1}),
        )
        .unwrap();
        assert_eq!(ids(&limited["entries"]), std::slice::from_ref(&c3));

        let none = json!([{"namespace":"app","key":"run","value":"nope"}]);
        let miss = call_json(velo_history, repo, json!({"from": c3, "meta": none})).unwrap();
        assert!(miss["entries"].as_array().unwrap().is_empty());
        assert_eq!(miss["empty"], "no_snapshots_matching");

        let unborn = call_json(velo_history, repo, json!({"branch":"ghost"})).unwrap();
        assert_eq!(unborn["empty"], "no_snapshots");

        assert_eq!(
            call_json(velo_find_snapshots, repo, json!([])).unwrap_err(),
            VELO_ERR_INVALID_INPUT
        );
        assert_eq!(
            call_json(velo_history, repo, json!({"limit": "x"})).unwrap_err(),
            VELO_ERR_INVALID_JSON
        );
        velo_repo_free(repo);
    }
}

#[test]
fn blame_attributes_lines_and_honours_the_window() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        let repo = init(dir.path());
        let a = save_file(
            repo,
            "one\ntwo\nthree\n",
            json!({"branch":"main","message":"first","timestamp_ms":1700000000000i64,
                   "author":{"name":"Ada"}}),
        );
        let b = save_file(
            repo,
            "one\nTWO\nthree\n",
            json!({"branch":"main","message":"second","parent":a,"timestamp_ms":1700000001000i64,
                   "author":{"name":"Bob","email":"bob@example.com"}}),
        );

        let path = c("f.txt");
        let blame = |opts: Value| -> Value {
            let opts = c(&opts.to_string());
            let mut out = ptr::null_mut();
            let code = velo_blame(repo, path.as_ptr(), opts.as_ptr(), &mut out);
            assert_eq!(code, 0, "{}", last_message());
            serde_json::from_str(&take_string(out)).unwrap()
        };

        let full = blame(json!({"at": b}));
        assert_eq!(full["snapshot"], b.as_str());
        let lines = full["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["line_no"], 1);
        assert_eq!(lines[0]["line_count"], 1);
        assert_eq!(lines[0]["origin"]["id"], a.as_str());
        assert_eq!(lines[0]["origin"]["author"]["name"], "Ada");
        assert_eq!(lines[0]["origin"]["branch"], "main");
        assert_eq!(lines[1]["origin"]["id"], b.as_str());
        assert_eq!(lines[1]["origin"]["author"]["name"], "Bob");
        assert_eq!(lines[1]["origin"]["author"]["email"], "bob@example.com");

        let window = blame(json!({"at": b, "start_line": 2, "end_line": 2}));
        let lines = window["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["line_no"], 2);
        assert_eq!(lines[0]["text"], "TWO");

        let tail = blame(json!({"at": b, "start_line": 3}));
        assert_eq!(tail["lines"].as_array().unwrap().len(), 1);
        velo_repo_free(repo);
    }
}

/// A base with two branches that both rewrote its only line.
unsafe fn conflicting(repo: *const VeloRepo) -> (String, String, String) {
    let base = save_file(
        repo,
        "base\n",
        json!({"branch":"main","message":"base","timestamp_ms":1700000000000i64}),
    );
    let ours = save_file(
        repo,
        "ours\n",
        json!({"branch":"main","message":"ours","parent":base,"timestamp_ms":1700000001000i64}),
    );
    let theirs = save_file(
        repo,
        "theirs\n",
        json!({"branch":"other","message":"theirs","parent":base,"timestamp_ms":1700000002000i64}),
    );
    (base, ours, theirs)
}

#[test]
fn merge_plan_commit_and_base() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        let repo = init(dir.path());
        let (base, ours, theirs) = conflicting(repo);

        let (o, t) = (c(&ours), c(&theirs));
        let mut out = ptr::null_mut();
        assert_eq!(
            velo_merge_plan(repo, o.as_ptr(), t.as_ptr(), &mut out),
            0,
            "{}",
            last_message()
        );
        let plan: Value = serde_json::from_str(&take_string(out)).unwrap();
        assert_eq!(plan["base"], base.as_str());
        let file = &plan["files"][0];
        assert_eq!(file["path"], "f.txt");
        assert_eq!(file["action"], "conflicted");
        for side in ["base", "ours", "theirs"] {
            assert_eq!(file[side].as_str().unwrap().len(), 64, "{side}");
        }

        // No resolution: the conflict is refused, naming the path.
        let unresolved = json!({"branch":"main","ours":ours,"theirs":theirs,"message":"merge"});
        let spec = c(&unresolved.to_string());
        let mut id = ptr::null_mut();
        assert_eq!(
            velo_merge_commit(repo, spec.as_ptr(), &mut id),
            VELO_ERR_CONFLICTS
        );
        assert!(id.is_null());
        assert!(last_message().contains("f.txt"), "{}", last_message());

        let resolved = json!({"branch":"main","ours":ours,"theirs":theirs,"message":"merge",
            "timestamp_ms":1700000003000i64,
            "resolutions":{"f.txt":{"content_base64":"cmVzb2x2ZWQK"}}});
        let spec = c(&resolved.to_string());
        assert_eq!(
            velo_merge_commit(repo, spec.as_ptr(), &mut id),
            0,
            "{}",
            last_message()
        );
        let merged = take_string(id);

        let m = c(&merged);
        let (mut data, mut len) = (ptr::null_mut(), 0usize);
        let f = c("f.txt");
        assert_eq!(
            velo_read_file_at(repo, m.as_ptr(), f.as_ptr(), &mut data, &mut len),
            0
        );
        assert_eq!(std::slice::from_raw_parts(data, len), b"resolved\n");
        velo_bytes_free(data, len);

        let mut snapshot = ptr::null_mut();
        assert_eq!(velo_snapshot(repo, m.as_ptr(), &mut snapshot), 0);
        let snapshot: Value = serde_json::from_str(&take_string(snapshot)).unwrap();
        assert_eq!(snapshot["merge_parent"], theirs.as_str());

        // Theirs is an ancestor of the merge, so it is the merge base.
        let mut base_id = ptr::null_mut();
        assert_eq!(
            velo_merge_base(repo, m.as_ptr(), t.as_ptr(), &mut base_id),
            0
        );
        assert_eq!(take_string(base_id), theirs);
        velo_repo_free(repo);
    }
}

#[test]
fn merge_base_of_unrelated_histories_is_null() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        let repo = init(dir.path());
        let a = save_file(repo, "a\n", json!({"branch":"main","message":"a"}));
        let b = save_file(repo, "b\n", json!({"branch":"other","message":"b"}));
        let (a, b) = (c(&a), c(&b));
        let mut out = ptr::dangling_mut::<c_char>();
        assert_eq!(velo_merge_base(repo, a.as_ptr(), b.as_ptr(), &mut out), 0);
        assert!(out.is_null());
        velo_repo_free(repo);
    }
}

#[test]
fn branches_create_list_and_set_tip() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        let repo = init(dir.path());
        let a = save_file(repo, "a\n", json!({"branch":"main","message":"a"}));
        let b = save_file(
            repo,
            "b\n",
            json!({"branch":"main","message":"b","parent":a}),
        );

        let (name, at) = (c("feature"), c(&a));
        assert_eq!(velo_branch_create(repo, name.as_ptr(), at.as_ptr()), 0);
        let mut tip = ptr::null_mut();
        assert_eq!(velo_branch_tip(repo, name.as_ptr(), &mut tip), 0);
        assert_eq!(take_string(tip), a);

        let mut out = ptr::null_mut();
        assert_eq!(velo_branches(repo, &mut out), 0);
        let list: Value = serde_json::from_str(&take_string(out)).unwrap();
        let feature = list
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == "feature")
            .expect("feature is listed");
        assert_eq!(feature["tip"]["id"], a.as_str());
        assert!(feature["is_current"].is_boolean());

        let to = c(&b);
        assert_eq!(velo_branch_set_tip(repo, name.as_ptr(), to.as_ptr()), 0);
        assert_eq!(velo_branch_tip(repo, name.as_ptr(), &mut tip), 0);
        assert_eq!(take_string(tip), b);

        let unknown = c(&"0".repeat(64));
        assert_eq!(
            velo_branch_set_tip(repo, name.as_ptr(), unknown.as_ptr()),
            VELO_ERR_NOT_FOUND
        );
        velo_repo_free(repo);
    }
}
