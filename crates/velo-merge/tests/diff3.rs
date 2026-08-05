//! Worked examples for the 3-way merge engine — the hand-written counterpart to
//! the randomised properties in `props.rs`.
//!
//! These cover the whole public surface a resolver needs: `try_auto_merge` for
//! the "no human required" path (this is the entry point `velo-core`'s
//! `reconcile` calls), `compute_conflict_hunks` for what to show when one is,
//! and `build_resolved_content` for applying the decisions that come back.

use velo_merge::{build_resolved_content, compute_conflict_hunks, try_auto_merge, Decision};

#[test]
fn diff3_auto_merges_nonoverlapping_changes() {
    let merged = try_auto_merge(
        "A\nB\nC\nD\nE\n",
        "A_CHANGED\nB\nC\nD\nE\n",
        "A\nB\nC\nD\nE_CHANGED\n",
    );
    assert_eq!(merged.as_deref(), Some("A_CHANGED\nB\nC\nD\nE_CHANGED\n"));
}

#[test]
fn diff3_theirs_only_change_is_applied() {
    // Ours unchanged, theirs changed → auto-merge takes theirs.
    let merged = try_auto_merge("A\nB\nC\n", "A\nB\nC\n", "A\nB_NEW\nC\n");
    assert_eq!(merged.as_deref(), Some("A\nB_NEW\nC\n"));
}

#[test]
fn diff3_overlapping_edits_do_not_auto_merge() {
    // Both change the same line differently → not auto-mergeable.
    let merged = try_auto_merge("A\nB\nC\n", "A\nX\nC\n", "A\nY\nC\n");
    assert!(merged.is_none());

    let hunks = compute_conflict_hunks("A\nB\nC\n", "A\nX\nC\n", "A\nY\nC\n");
    assert_eq!(hunks.len(), 1, "one conflicting hunk expected");
    assert_eq!(hunks[0].ours, vec!["X".to_string()]);
    assert_eq!(hunks[0].theirs, vec!["Y".to_string()]);
}

#[test]
fn diff3_build_resolved_take_theirs_at_conflict_keeps_other_regions() {
    // f.txt: conflict on line 2, theirs-only change on line 5.
    let anc = "A\nB\nC\nD\nE\n";
    let ours = "A\nB_OURS\nC\nD\nE\n";
    let theirs = "A\nB_THEIRS\nC\nD\nE_THEIRS\n";

    let mut hunks = compute_conflict_hunks(anc, ours, theirs);
    assert_eq!(hunks.len(), 1);
    hunks[0].decision = Some(Decision::Theirs);

    let anc_l: Vec<&str> = anc.lines().collect();
    let our_l: Vec<&str> = ours.lines().collect();
    let thr_l: Vec<&str> = theirs.lines().collect();
    let resolved = build_resolved_content(&anc_l, &our_l, &thr_l, &hunks, true);
    assert_eq!(resolved, "A\nB_THEIRS\nC\nD\nE_THEIRS\n");
}

#[test]
fn diff3_identical_edits_on_both_sides_do_not_conflict() {
    // Both sides made the exact same change → no conflict, change applied.
    let merged = try_auto_merge("A\nB\nC\n", "A\nZ\nC\n", "A\nZ\nC\n");
    assert_eq!(merged.as_deref(), Some("A\nZ\nC\n"));
}
