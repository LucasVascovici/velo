//! `show` lists every parent and `history` marks merge snapshots.

use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn velody(dir: &Path, args: &[&str]) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_velody"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run velody binary");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (s, out.status.success())
}

fn write(dir: &Path, rel: &str, content: &str) {
    std::fs::write(dir.join(rel), content).unwrap();
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let (out, good) = velody(dir, args);
    assert!(good, "velody {args:?} failed:\n{out}");
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
    let _ = velody(d, &["save", "merged"]);

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

fn mk(dir: &Path) {
    ok(dir, &["init"]);
}

#[test]
fn merge_summary_after_mv_and_edit_agrees_with_status() {
    let t = TempDir::new().unwrap();
    let d = t.path();
    mk(d);
    write(
        d,
        "a.txt",
        "1
2
3
4
5
6
7
8
",
    );
    ok(d, &["save", "base"]);
    ok(d, &["switch", "feat"]);
    ok(d, &["mv", "a.txt", "b.txt"]);
    write(
        d,
        "b.txt",
        "1
two
3
4
5
6
7
8
",
    );
    ok(d, &["save", "move and edit"]);
    ok(d, &["switch", "main"]);
    write(
        d,
        "other.txt",
        "x
",
    );
    ok(d, &["save", "other"]);
    let merge = ok(d, &["merge", "feat"]);
    assert!(merge.contains("New:      1"), "{merge}");
    assert!(merge.contains("Updated:  0"), "{merge}");
    assert!(merge.contains("Deleted:  1"), "{merge}");
    let status = ok(d, &["status"]);
    assert!(status.contains("New 1 file(s)"), "{status}");
    assert!(status.contains("Deleted 1 file(s)"), "{status}");
}

#[test]
fn cherry_pick_of_a_present_change_prints_only_nothing_to_apply() {
    let t = TempDir::new().unwrap();
    let d = t.path();
    mk(d);
    write(
        d,
        "f.txt",
        "a
b
c
d
e
f
g
",
    );
    ok(d, &["save", "base"]);
    ok(d, &["switch", "feat"]);
    write(
        d,
        "f.txt",
        "A
b
c
d
e
f
g
",
    );
    ok(d, &["save", "feat"]);
    ok(d, &["switch", "main"]);
    write(
        d,
        "f.txt",
        "A
b
c
d
e
f
G
",
    );
    ok(d, &["save", "main"]);
    let out = ok(d, &["cherry-pick", "feat"]);
    assert!(out.contains("Nothing to apply"), "{out}");
    assert!(!out.contains("Updated"), "{out}");
    assert!(!out.contains("Auto-merged"), "{out}");
}

#[test]
fn blame_lines_past_the_end_says_how_long_the_file_is() {
    let t = TempDir::new().unwrap();
    let d = t.path();
    mk(d);
    write(
        d,
        "f.txt",
        "one
two
three
",
    );
    ok(d, &["save", "base"]);
    let out = Command::new(env!("CARGO_BIN_EXE_velody"))
        .args(["blame", "f.txt", "--lines", "100-110"])
        .current_dir(d)
        .output()
        .unwrap();
    assert!(out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("file has 3 lines"), "{err}");
}

#[test]
fn conflicts_text_shows_context_under_both_sides() {
    let t = TempDir::new().unwrap();
    let d = t.path();
    mk(d);
    write(
        d,
        "f.txt",
        "c1
c2
mid
c3
c4
",
    );
    ok(d, &["save", "base"]);
    ok(d, &["switch", "feat"]);
    write(
        d,
        "f.txt",
        "c1
c2
theirs
c3
c4
",
    );
    ok(d, &["save", "feat"]);
    ok(d, &["switch", "main"]);
    write(
        d,
        "f.txt",
        "c1
c2
ours
c3
c4
",
    );
    ok(d, &["save", "main"]);
    let _ = velody(d, &["merge", "feat"]);
    let (out, good) = velody(d, &["conflicts", "f.txt"]);
    assert!(good, "{out}");
    let ours_at = out.find("ours (your last saved").expect(&out);
    let theirs_at = out.find("theirs (feat").expect(&out);
    let (ours_block, theirs_block) = (&out[ours_at..theirs_at], &out[theirs_at..]);
    for block in [ours_block, theirs_block] {
        assert!(block.contains("c2"), "{out}");
        assert!(block.contains("c3"), "{out}");
    }
}
