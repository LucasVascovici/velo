//! Runs under Node (`wasm-pack test --node bindings/wasm`) against in-memory
//! repositories. Each test uses its own repository name: the in-memory VFS is
//! shared by every repository in the module instance.
#![cfg(target_arch = "wasm32")]

use js_sys::{Array, Reflect, Uint8Array};
use velo_wasm::Repo;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

/// Build a JS value from an object-literal expression.
fn js(expr: &str) -> JsValue {
    js_sys::eval(&format!("({expr})")).expect("test literal evaluates")
}

fn get(obj: &JsValue, key: &str) -> JsValue {
    Reflect::get(obj, &JsValue::from_str(key)).unwrap()
}

fn text(obj: &JsValue, key: &str) -> String {
    get(obj, key).as_string().unwrap()
}

fn code_of(err: &JsValue) -> String {
    get(err, "code").as_string().unwrap_or_default()
}

fn save(
    repo: &Repo,
    branch: &str,
    message: &str,
    parent: Option<&str>,
    files: &[(&str, &str)],
) -> String {
    let entries: Vec<String> = files
        .iter()
        .map(|(p, d)| format!("{{path: {p:?}, data: {d:?}}}"))
        .collect();
    let parent = parent.map_or(String::new(), |p| format!("parent: {p:?},"));
    let input = js(&format!(
        "{{branch: {branch:?}, message: {message:?}, {parent} entries: [{}]}}",
        entries.join(",")
    ));
    repo.save_tree(input).unwrap()
}

#[wasm_bindgen_test]
fn save_list_and_read() {
    let repo = Repo::create_in_memory("save_list_and_read.db").unwrap();
    let input = js("{branch: 'main', message: 'first', entries: [
            {path: 'a.txt', data: 'hello\\n'},
            {path: 'src/b.bin', data: new Uint8Array([1, 2, 3])}]}");
    let id = repo.save_tree(input).unwrap();

    let files = Array::from(&repo.tree_at(&id).unwrap());
    assert_eq!(files.length(), 2);
    let paths: Vec<String> = files.iter().map(|f| text(&f, "path")).collect();
    assert!(paths.contains(&"a.txt".to_string()));
    assert!(paths.contains(&"src/b.bin".to_string()));
    assert_eq!(text(&files.get(0), "kind"), "regular");

    assert_eq!(
        repo.read_file_at(&id, "a.txt").unwrap().to_vec(),
        b"hello\n"
    );
    let bin: Uint8Array = repo.read_file_at(&id, "src/b.bin").unwrap();
    assert_eq!(bin.to_vec(), vec![1, 2, 3]);
    assert_eq!(repo.branch_tip("main").unwrap(), Some(id));
}

#[wasm_bindgen_test]
fn fixed_timestamp_gives_the_native_id() {
    let repo = Repo::create_in_memory("fixed_timestamp.db").unwrap();
    let input = js(
        "{branch: 'main', message: 'first', timestampMs: 1700000000000, entries: [
            {path: 'a.txt', data: 'hello\\n'},
            {path: 'src/b.txt', data: 'world\\n'}]}",
    );
    // Computed natively with velo-core's `Repo::create_file` and `save_tree`
    // for the same inputs, so the wasm build hashes exactly as the host does.
    assert_eq!(
        repo.save_tree(input).unwrap(),
        "76313fbc39aecb241fd5f5045554705db62daf9dcd822f486fcab2a096a2ddc0"
    );
}

#[wasm_bindgen_test]
fn history_then_blame() {
    let repo = Repo::create_in_memory("history_then_blame.db").unwrap();
    let first = save(&repo, "main", "one", None, &[("a.txt", "alpha\nbeta\n")]);
    let second = save(
        &repo,
        "main",
        "two",
        Some(&first),
        &[("a.txt", "alpha\nBETA\n")],
    );

    let log = Array::from(&repo.history(js(&format!("{{from: {second:?}}}"))).unwrap());
    assert_eq!(log.length(), 2);
    assert_eq!(text(&log.get(0), "id"), second);
    assert_eq!(text(&log.get(1), "message"), "one");
    assert!(get(&log.get(1), "parent").is_null());
    assert!(get(&log.get(0), "createdAt").is_instance_of::<js_sys::Date>());

    let blame = repo
        .blame("a.txt", js(&format!("{{at: {second:?}}}")))
        .unwrap();
    let lines = Array::from(&get(&blame, "lines"));
    assert_eq!(lines.length(), 2);
    assert_eq!(text(&get(&lines.get(0), "origin"), "id"), first);
    assert_eq!(text(&get(&lines.get(1), "origin"), "id"), second);
    assert_eq!(text(&lines.get(1), "text"), "BETA");
}

#[wasm_bindgen_test]
fn merge_conflict_then_resolution() {
    let repo = Repo::create_in_memory("merge_conflict.db").unwrap();
    let base = save(&repo, "main", "base", None, &[("a.txt", "x\n")]);
    let ours = save(&repo, "main", "ours", Some(&base), &[("a.txt", "ours\n")]);
    let theirs = save(
        &repo,
        "topic",
        "theirs",
        Some(&base),
        &[("a.txt", "theirs\n")],
    );

    assert_eq!(repo.merge_base(&ours, &theirs).unwrap(), Some(base));
    let plan = repo.merge_plan(&ours, &theirs).unwrap();
    let files = Array::from(&get(&plan, "files"));
    assert_eq!(files.length(), 1);
    assert_eq!(text(&files.get(0), "action"), "conflicted");

    let unresolved = js(&format!(
        "{{branch: 'main', ours: {ours:?}, theirs: {theirs:?}, message: 'merge'}}"
    ));
    let err = repo.merge_commit(unresolved).unwrap_err();
    assert_eq!(code_of(&err), "Conflicts");
    let paths = Array::from(&get(&err, "paths"));
    assert_eq!(paths.get(0).as_string().unwrap(), "a.txt");

    let resolved = js(&format!(
        "{{branch: 'main', ours: {ours:?}, theirs: {theirs:?}, message: 'merge',
           resolutions: {{'a.txt': 'merged\\n'}}}}"
    ));
    let merged = repo.merge_commit(resolved).unwrap();
    assert_eq!(
        repo.read_file_at(&merged, "a.txt").unwrap().to_vec(),
        b"merged\n"
    );
    let info = repo.snapshot(&merged).unwrap();
    assert_eq!(text(&info, "mergeParent"), theirs);
}

#[wasm_bindgen_test]
fn unknown_parent_is_not_found() {
    let repo = Repo::create_in_memory("unknown_parent.db").unwrap();
    let missing = "0".repeat(64);
    let input = js(&format!(
        "{{branch: 'main', message: 'm', parent: {missing:?}, entries: [{{path: 'a', data: 'x'}}]}}"
    ));
    let err = repo.save_tree(input).unwrap_err();
    assert_eq!(code_of(&err), "NotFound");
    assert!(err.is_instance_of::<js_sys::Error>());
}

#[wasm_bindgen_test]
async fn open_persistent_is_unsupported_outside_a_worker() {
    let err = match Repo::open_persistent("persist.db".to_string()).await {
        Ok(_) => panic!("openPersistent must reject under Node"),
        Err(e) => e,
    };
    assert_eq!(code_of(&err), "Unsupported");
}
