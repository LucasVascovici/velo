//! Helpers and the cookbook scenario shared by the browser test files.
#![allow(dead_code)]

use js_sys::{Array, Reflect};
use velo_wasm::Repo;
use wasm_bindgen::prelude::*;

/// Build a JS value from an object-literal expression.
pub fn js(expr: &str) -> JsValue {
    js_sys::eval(&format!("({expr})")).expect("test literal evaluates")
}

pub fn get(obj: &JsValue, key: &str) -> JsValue {
    Reflect::get(obj, &JsValue::from_str(key)).unwrap()
}

pub fn text(obj: &JsValue, key: &str) -> String {
    get(obj, key).as_string().unwrap()
}

pub fn code_of(err: &JsValue) -> String {
    get(err, "code").as_string().unwrap_or_default()
}

pub fn save(
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

/// What the scenario leaves behind, for the caller to check after a reopen.
pub struct Outcome {
    pub merged: String,
    pub ours: String,
}

/// The cookbook scenario: three snapshots on `main`, a branch off the first,
/// history, a merge plan that conflicts, a resolved merge commit, and `fsck`.
pub fn scenario(repo: &Repo) -> Outcome {
    let base = save(
        repo,
        "main",
        "base",
        None,
        &[("a.txt", "x\n"), ("b.txt", "b\n")],
    );
    let second = save(
        repo,
        "main",
        "second",
        Some(&base),
        &[("a.txt", "x\n"), ("b.txt", "b2\n")],
    );
    let ours = save(
        repo,
        "main",
        "ours",
        Some(&second),
        &[("a.txt", "ours\n"), ("b.txt", "b2\n")],
    );
    let theirs = save(
        repo,
        "topic",
        "theirs",
        Some(&base),
        &[("a.txt", "theirs\n"), ("b.txt", "b\n")],
    );

    let log = Array::from(&repo.history(js(&format!("{{from: {ours:?}}}"))).unwrap());
    assert_eq!(log.length(), 3);
    assert_eq!(text(&log.get(0), "id"), ours);
    assert_eq!(text(&log.get(2), "message"), "base");

    let plan = repo.merge_plan(&ours, &theirs).unwrap();
    let files = Array::from(&get(&plan, "files"));
    assert!(files
        .iter()
        .any(|f| text(&f, "path") == "a.txt" && text(&f, "action") == "conflicted"));

    let unresolved = js(&format!(
        "{{branch: 'main', ours: {ours:?}, theirs: {theirs:?}, message: 'merge'}}"
    ));
    assert_eq!(
        code_of(&repo.merge_commit(unresolved).unwrap_err()),
        "Conflicts"
    );
    let resolved = js(&format!(
        "{{branch: 'main', ours: {ours:?}, theirs: {theirs:?}, message: 'merge',
           resolutions: {{'a.txt': 'merged\\n'}}}}"
    ));
    let merged = repo.merge_commit(resolved).unwrap();
    assert_eq!(
        repo.read_file_at(&merged, "a.txt").unwrap().to_vec(),
        b"merged\n"
    );
    assert_eq!(repo.branch_tip("main").unwrap(), Some(merged.clone()));

    check_fsck(repo);
    Outcome { merged, ours }
}

pub fn check_fsck(repo: &Repo) {
    let report = repo.fsck().unwrap();
    assert_eq!(get(&report, "ok").as_bool(), Some(true), "fsck: {report:?}");
    assert_eq!(Array::from(&get(&report, "problems")).length(), 0);
    assert!(Array::is_array(&get(&report, "cruft")));
}

/// History from the merge reaches all four snapshots with the content intact.
pub fn check_history(repo: &Repo, outcome: &Outcome) {
    let log = Array::from(
        &repo
            .history(js(&format!("{{from: {:?}, all: true}}", outcome.merged)))
            .unwrap(),
    );
    assert!(log.length() >= 4, "history has {} entries", log.length());
    assert_eq!(text(&log.get(0), "id"), outcome.merged);
    assert!(log.iter().any(|e| text(&e, "id") == outcome.ours));
    assert_eq!(
        repo.read_file_at(&outcome.merged, "a.txt")
            .unwrap()
            .to_vec(),
        b"merged\n"
    );
}
