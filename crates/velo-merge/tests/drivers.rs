//! The driver trait, its default, and whole-file hunks.

use velo_merge::{
    build_resolved_content, diff3, whole_file_conflict, Decision, LineDriver, MergeDriver,
    MergeResult,
};

fn same(a: &MergeResult, b: &MergeResult) -> bool {
    match (a, b) {
        (MergeResult::Clean(x), MergeResult::Clean(y)) => x == y,
        (MergeResult::Conflicted(x), MergeResult::Conflicted(y)) => {
            x.len() == y.len()
                && x.iter().zip(y).all(|(p, q)| {
                    (p.ancestor_start, p.ancestor_end, &p.ours, &p.theirs)
                        == (q.ancestor_start, q.ancestor_end, &q.ours, &q.theirs)
                })
        }
        _ => false,
    }
}

#[test]
fn line_driver_equals_diff3() {
    let cases = [
        ("A\nB\nC\nD\nE\n", "A1\nB\nC\nD\nE\n", "A\nB\nC\nD\nE1\n"),
        ("A\nB\nC\n", "A\nX\nC\n", "A\nY\nC\n"),
        ("A\nB\nC\n", "A\nB\nC\n", "A\nB_NEW\nC\n"),
        ("", "x\n", "y\n"),
    ];
    for (a, o, t) in cases {
        assert!(same(&LineDriver.merge(a, o, t), &diff3(a, o, t)));
    }
    assert_eq!(LineDriver.name(), "line");
}

#[test]
fn whole_file_conflict_spans_the_file() {
    let h = whole_file_conflict("a\nb\nc\n", "x\ny\n", "z\n");
    assert_eq!(h.len(), 1);
    let h = &h[0];
    assert_eq!((h.id, h.ancestor_start, h.ancestor_end), (0, 0, 3));
    assert_eq!(h.ours, vec!["x", "y"]);
    assert_eq!(h.theirs, vec!["z"]);
    assert!(h.context_before.is_empty() && h.context_after.is_empty());
    assert!(h.decision.is_none());
}

#[test]
fn whole_file_hunk_decision_applies_when_diff3_is_clean() {
    let (a, o, t) = ("A\nB\nC\n", "A1\nB\nC\n", "A\nB\nC1\n");
    assert!(matches!(diff3(a, o, t), MergeResult::Clean(_)));
    let mut hunks = whole_file_conflict(a, o, t);
    hunks[0].decision = Some(Decision::Theirs);
    let anc: Vec<&str> = a.lines().collect();
    let our: Vec<&str> = o.lines().collect();
    let thr: Vec<&str> = t.lines().collect();
    let out = build_resolved_content(&anc, &our, &thr, &hunks, true);
    assert_eq!(out, "A\nB\nC1\n");
}
