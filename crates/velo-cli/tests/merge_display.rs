//! `show` lists every parent and `history` marks merge snapshots.

use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn velo(dir: &Path, args: &[&str]) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_velo"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run velo binary");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (s, out.status.success())
}

fn write(dir: &Path, rel: &str, content: &str) {
    std::fs::write(dir.join(rel), content).unwrap();
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let (out, good) = velo(dir, args);
    assert!(good, "velo {args:?} failed:\n{out}");
    out
}

/// First hex id on the oneline row whose text contains `needle`.
fn id_of(oneline: &str, needle: &str) -> String {
    let line = oneline
        .lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle}:\n{oneline}"));
    line.split_whitespace()
        .find(|w| w.len() >= 8 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| panic!("no id in {line}"))
        .to_string()
}

#[test]
fn show_lists_both_parents_and_history_marks_merge() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    ok(d, &["init"]);
    write(d, "a.txt", "a\n");
    write(d, "b.txt", "b\n");
    ok(d, &["save", "base"]);
    ok(d, &["switch", "feature"]);
    write(d, "b.txt", "b feature\n");
    ok(d, &["save", "feature edit"]);
    ok(d, &["switch", "main"]);
    write(d, "a.txt", "a main\n");
    ok(d, &["save", "main edit"]);
    ok(d, &["merge", "feature"]);
    // Finalise the merge if it left a pending MERGE_HEAD.
    let _ = velo(d, &["save", "merged"]);

    let one = ok(d, &["history", "--oneline"]);
    let merge_lines: Vec<&str> = one.lines().filter(|l| l.contains("(merge)")).collect();
    assert_eq!(merge_lines.len(), 1, "{one}");

    let merge_id = id_of(&one, "(merge)");
    let main_id = id_of(&one, "main edit");
    let feat_id = id_of(&one, "feature edit");

    let shown = ok(d, &["show", &merge_id]);
    assert!(shown.contains("parents:"), "{shown}");
    assert!(
        shown.contains(&main_id) && shown.contains(&feat_id),
        "{shown}"
    );

    let plain = ok(d, &["show", &main_id]);
    assert!(
        plain.contains("parent:") && !plain.contains("parents:"),
        "{plain}"
    );
}

#[test]
fn merge_into_an_unborn_branch_says_it_adopts_the_source_history() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    ok(d, &["init"]);
    write(d, "a.txt", "a\n");
    ok(d, &["save", "base"]);
    ok(d, &["switch", "fresh"]);
    let out = ok(d, &["merge", "main"]);
    assert!(
        out.contains("only the history of the merged branch"),
        "{out}"
    );
}
