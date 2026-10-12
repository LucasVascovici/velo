//! Discoverability: aliases, git-habit hints, remote branches in history, and
//! the wording around undo and redo.

use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// Run `velo <args>` in `dir`; return (stdout+stderr, exit code).
fn run(dir: &Path, args: &[&str]) -> (String, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_velo"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (s, out.status.code())
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let (out, code) = run(dir, args);
    assert_eq!(code, Some(0), "velo {args:?} failed:\n{out}");
    out
}

fn repo_with_two_saves() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    ok(d, &["init"]);
    std::fs::write(d.join("f.txt"), "one\n").unwrap();
    ok(d, &["save", "first"]);
    std::fs::write(d.join("f.txt"), "two\n").unwrap();
    ok(d, &["save", "second"]);
    tmp
}

#[test]
fn log_and_dash_n_work() {
    let tmp = repo_with_two_saves();
    let d = tmp.path();
    let log = ok(d, &["log", "--oneline", "-n", "1"]);
    assert_eq!(log.lines().filter(|l| l.contains("second")).count(), 1);
    assert!(!log.contains("first"), "{log}");
    let hist = ok(d, &["history", "--oneline", "-n", "1"]);
    assert!(hist.contains("second") && !hist.contains("first"), "{hist}");
}

#[test]
fn help_shows_each_default_once() {
    let tmp = TempDir::new().unwrap();
    let h = ok(tmp.path(), &["help", "history"]);
    assert_eq!(h.matches("[default: 20]").count(), 1, "{h}");
    let g = ok(tmp.path(), &["help", "grep"]);
    assert_eq!(g.matches("[default: 2]").count(), 1, "{g}");
}

#[test]
fn mv_examples_stay_on_separate_lines() {
    let tmp = TempDir::new().unwrap();
    let h = ok(tmp.path(), &["help", "mv"]);
    assert!(
        h.lines().any(|l| l
            .trim_start()
            .starts_with("velo mv notes.txt docs/notes.md")),
        "{h}"
    );
    assert!(
        h.lines()
            .any(|l| l.trim_start().starts_with("velo save \"move the notes\"")),
        "{h}"
    );
}

#[test]
fn git_habit_commands_get_a_hint() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    let (out, code) = run(d, &["history-all"]);
    assert_eq!(code, Some(2));
    assert!(out.contains("velo history --all"), "{out}");
    let (out, _) = run(d, &["diff-worktree"]);
    assert!(out.contains("velo diff"), "{out}");
    let (out, _) = run(d, &["add", "."]);
    assert!(out.contains("no staging area"), "{out}");
    let (out, code) = run(d, &["frobnicate"]);
    assert_eq!(code, Some(2));
    assert!(!out.contains("hint:"), "{out}");
}

#[test]
fn history_branch_accepts_a_remote_branch() {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    ok(&origin, &["init"]);
    std::fs::write(origin.join("a.txt"), "a\n").unwrap();
    ok(&origin, &["save", "C0"]);
    ok(root.path(), &["clone", origin.to_str().unwrap(), "clone"]);
    let clone = root.path().join("clone");
    std::fs::write(clone.join("b.txt"), "b\n").unwrap();
    ok(&clone, &["save", "pushed work"]);
    ok(&clone, &["push"]);

    for spelling in ["origin/main", "remotes/origin/main"] {
        let out = ok(&clone, &["history", "--oneline", "--branch", spelling]);
        assert!(out.contains("pushed work"), "{spelling}: {out}");
    }
}

#[test]
fn undo_says_files_were_rewound_and_save_hints_redo() {
    let tmp = repo_with_two_saves();
    let d = tmp.path();
    let out = ok(d, &["undo"]);
    assert!(out.contains("Working files rewound to"), "{out}");
    let out = ok(d, &["save", "x"]);
    assert!(out.contains("Nothing to save"), "{out}");
    assert!(out.contains("velo redo"), "{out}");
    assert!(out.contains("second"), "{out}");
}
