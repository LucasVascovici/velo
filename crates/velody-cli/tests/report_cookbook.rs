//! `velody report` on the agent-checkpointing cookbook scenario: who wrote each
//! line of the agent's run.
//!
//! The scenario mirrors `bindings/python/examples/agent_checkpointing.py` and
//! `docs/cookbook/agent-checkpointing.md`: a human `base`, two attempts on
//! branches `attempt-1` and `attempt-2`, and the winner merged into main.
//!
//! `docs/report-sample.html` is generated from it with fixed timestamps. To
//! regenerate it after changing the report or this scenario:
//!
//! ```text
//! cargo test -p velody-cli --test report_cookbook -- --ignored write_sample
//! ```
//!
//! The repository path and the "generated" time are the only things that vary
//! between runs, so the sample writer replaces them with fixed text.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;
use velody_core::tree::{SaveTree, TreeEntry};
use velody_core::{Author, BranchName, Repo, SnapshotMeta};

const BASE: &str = "def total(xs):\n    return sum(xs)\n\nprint(total([1, 2]))\n";
const GENERATOR: &str = "def total(xs):\n    return sum(x for x in xs)\n\nprint(total([1, 2]))\n";

/// A fixed epoch for the scenario: 2025-01-06T09:00:00Z, one minute apart.
const T0: i64 = 1_736_154_000_000;
const MIN: i64 = 60_000;

fn velody(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_velody"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run velody binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn agent_meta(run: &str, call: &str, model: &str, tests: Option<&str>) -> SnapshotMeta {
    let mut meta = SnapshotMeta::new();
    meta.set("agent", "run", run).unwrap();
    meta.set("agent", "tool_call_id", call).unwrap();
    meta.set("agent", "model", model).unwrap();
    if let Some(t) = tests {
        meta.set("agent", "tests", t).unwrap();
    }
    meta
}

#[allow(clippy::too_many_arguments)]
fn tool_call(
    repo: &Repo,
    branch: &str,
    parent: &velody_core::SnapshotId,
    run: &str,
    call: &str,
    model: &str,
    content: &str,
    message: &str,
    tests: Option<&str>,
    at: i64,
) -> velody_core::SnapshotId {
    let branch: BranchName = branch.parse().unwrap();
    let author = Author::new(format!("agent-{run}")).unwrap();
    repo.write_timeout(std::time::Duration::from_secs(10))
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent: Some(parent),
            merge_parent: None,
            message,
            entries: vec![TreeEntry::file("app.py", content.as_bytes().to_vec())],
            meta: agent_meta(run, call, model, tests),
            author: Some(&author),
            timestamp_ms: Some(at),
            renames: &[],
        })
        .unwrap()
}

fn scenario() -> TempDir {
    use velody_core::commands::merge::{self, MergeCommit};

    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    assert_eq!(velody(d, &["init"]).2, 0);
    let repo = Repo::open_and_migrate(d).unwrap();
    let main: BranchName = "main".parse().unwrap();
    let human = Author::new("human").unwrap();

    let base = repo
        .write_timeout(std::time::Duration::from_secs(10))
        .unwrap()
        .save_tree(SaveTree {
            branch: &main,
            parent: None,
            merge_parent: None,
            message: "base",
            entries: vec![TreeEntry::file("app.py", BASE.as_bytes().to_vec())],
            meta: SnapshotMeta::new(),
            author: Some(&human),
            timestamp_ms: Some(T0),
            renames: &[],
        })
        .unwrap();

    let a1 = tool_call(
        &repo,
        "attempt-1",
        &base,
        "run-1",
        "call-1",
        "model-a",
        GENERATOR,
        "rewrite total as a generator",
        None,
        T0 + MIN,
    );
    let a2 = tool_call(
        &repo,
        "attempt-1",
        &a1,
        "run-1",
        "call-2",
        "model-a",
        &format!("{GENERATOR}print(total([]))\n"),
        "add an empty-list check",
        Some("pass"),
        T0 + 2 * MIN,
    );
    tool_call(
        &repo,
        "attempt-2",
        &base,
        "run-2",
        "call-1",
        "model-b",
        "def total(xs):\n    return 0\n",
        "stub total",
        Some("fail"),
        T0 + 3 * MIN,
    );

    let mut meta = SnapshotMeta::new();
    meta.set("agent", "picked", "run-1").unwrap();
    let none: [(String, merge::Resolution); 0] = [];
    merge::commit(
        &repo
            .write_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        MergeCommit {
            branch: &main,
            ours: &base,
            theirs: &a2,
            resolutions: &none,
            message: "take attempt-1",
            meta,
            author: Some(&human),
            timestamp_ms: Some(T0 + 4 * MIN),
        },
    )
    .unwrap();
    drop(repo);
    tmp
}

fn json_of(d: &Path) -> serde_json::Value {
    let (out, err, code) = velody(d, &["report", "--json"]);
    assert_eq!(code, 0, "{err}");
    serde_json::from_str(&out).expect("valid JSON on stdout")
}

#[test]
fn report_shows_who_wrote_each_line_of_the_run() {
    let tmp = scenario();
    let d = tmp.path();

    let v = json_of(d);
    assert_eq!(v["integrity"]["healthy"], true);
    let files = v["files"].as_array().unwrap();
    // One blamed copy per branch tip; the cookbook's answer is main's.
    let main_tip = v["branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["branch"] == "main")
        .unwrap()["tip"]
        .clone();
    let app = files
        .iter()
        .find(|f| f["path"] == "app.py" && f["at"] == main_tip)
        .expect("app.py blamed at main's tip");
    let who: Vec<(String, String)> = app["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["text"].as_str().unwrap().to_string(),
                l["author"]["name"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let author_of = |text: &str| {
        who.iter()
            .find(|(t, _)| t == text)
            .unwrap_or_else(|| panic!("no line {text:?} in {who:?}"))
            .1
            .clone()
    };
    assert_eq!(author_of("    return sum(x for x in xs)"), "agent-run-1");
    assert_eq!(author_of("print(total([]))"), "agent-run-1");
    assert_eq!(author_of("def total(xs):"), "human");
    assert_eq!(author_of("print(total([1, 2]))"), "human");

    let (out, err, code) = velody(d, &["report", "-o", "run.html"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("run.html"), "{out}");
    let html = std::fs::read_to_string(d.join("run.html")).unwrap();
    assert!(html.contains("Line provenance"));
    assert!(html.contains("agent-run-1") && html.contains("human"));
    assert!(html.contains("agent-run-2"));
    assert!(
        html.contains("fsck recomputed 5 of 5 snapshot ids: OK"),
        "every id should be recomputed"
    );
}

/// Writes `docs/report-sample.html`. Ignored so CI never rewrites tracked files.
#[test]
#[ignore = "regenerates docs/report-sample.html"]
fn write_sample() {
    let tmp = scenario();
    let d = tmp.path();
    let v = json_of(d);
    let repo_path = v["repository"].as_str().unwrap().to_string();
    let generated = v["generated"].as_str().unwrap().to_string();

    let (_, err, code) = velody(d, &["report", "-o", "run.html"]);
    assert_eq!(code, 0, "{err}");
    let html = std::fs::read_to_string(d.join("run.html")).unwrap();
    let html = html
        .replace(&repo_path, "agent-checkpointing-run")
        .replace(&generated, "2025-01-06 09:10 UTC (fixed for the sample)");
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/report-sample.html");
    std::fs::write(out, html).unwrap();
}
