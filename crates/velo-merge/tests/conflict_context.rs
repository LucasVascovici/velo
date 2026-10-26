//! The context shown around a conflict hunk is text both sides agree on.
//!
//! Run six (team simulation): when both sides moved the same line, the context
//! came from the ancestor, so `velo conflicts` printed the moved line twice
//! under "ours" and under "theirs" although neither side had it twice.

use velo_merge::compute_conflict_hunks;

#[test]
fn context_is_the_merged_text_not_the_ancestor() {
    let ancestor = "# marker\nregister(\"catalog\")\nregister(\"csv\")\nregister(\"bulk\")\n";
    let ours = "# marker\nregister(\"csv\")\nregister(\"bulk\")\nregister(\"catalog\")\nregister(\"wh\")\n";
    let theirs = "# marker\nregister(\"csv\")\nregister(\"bulk\")\nregister(\"catalog\")\nregister(\"low\")\n";
    let hunks = compute_conflict_hunks(ancestor, ours, theirs);
    assert_eq!(hunks.len(), 1);
    let h = &hunks[0];
    // Each side, read as context + its lines + context, is a run of that side's file.
    for (side, text) in [(&h.ours, ours), (&h.theirs, theirs)] {
        let shown: Vec<&str> = h
            .context_before
            .iter()
            .chain(side.iter())
            .chain(h.context_after.iter())
            .map(String::as_str)
            .collect();
        let file: Vec<&str> = text.lines().collect();
        assert!(
            file.windows(shown.len()).any(|w| w == shown.as_slice()),
            "shown {shown:?} is not a run of {file:?}"
        );
    }
}

#[test]
fn context_is_unchanged_when_the_neighbours_were_not_touched() {
    let ancestor = "a\nb\nc\nd\ne\nf\ng\n";
    let ours = "a\nb\nc\nOURS\ne\nf\ng\n";
    let theirs = "a\nb\nc\nTHEIRS\ne\nf\ng\n";
    let h = &compute_conflict_hunks(ancestor, ours, theirs)[0];
    assert_eq!(h.context_before, ["a", "b", "c"]);
    assert_eq!(h.context_after, ["e", "f", "g"]);
}
