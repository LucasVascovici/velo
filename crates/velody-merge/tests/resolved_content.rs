//! `build_resolved_content` must yield exactly the recorded decisions.
//!
//! The resolved text is built from the same line slices the hunks were
//! computed on, so a blank last line in a slice cannot shift a conflict range.

use velody_merge::{build_resolved_content, compute_conflict_hunks, Decision};

/// Resolve every hunk with `decision` and rebuild the file.
fn resolve_all(anc: &str, ours: &str, theirs: &str, decision: &Decision) -> String {
    let mut hunks = compute_conflict_hunks(anc, ours, theirs);
    for h in &mut hunks {
        h.decision = Some(decision.clone());
    }
    let a: Vec<&str> = anc.lines().collect();
    let o: Vec<&str> = ours.lines().collect();
    let t: Vec<&str> = theirs.lines().collect();
    build_resolved_content(&a, &o, &t, &hunks, ours.ends_with('\n'))
}

#[test]
fn changelog_repro_take_ours_is_exactly_ours() {
    let anc = "# Changelog\n\n## Unreleased\n\n";
    let ours = "# Changelog\n\n## Unreleased\n- ours entry\n";
    let theirs = "# Changelog\n\n## Unreleased\n\n- theirs entry\n";
    assert_eq!(resolve_all(anc, ours, theirs, &Decision::Ours), ours);
}

#[test]
fn table_of_edge_shapes() {
    // (ancestor, ours, theirs)
    let cases: &[(&str, &str, &str)] = &[
        // trailing blank line on every side
        ("a\n\n", "a\nx\n\n", "a\ny\n\n"),
        // blank line replaced on one side, kept on the other
        ("a\n\n", "a\nx\n", "a\n\ny\n"),
        // ours side empties the conflict region
        ("a\nb\nc\n", "a\nc\n", "a\nz\nc\n"),
        // theirs side empties the conflict region
        ("a\nb\nc\n", "a\nz\nc\n", "a\nc\n"),
        // no trailing newline anywhere
        ("a\nb", "a\nx", "a\ny"),
    ];
    for (anc, ours, theirs) in cases {
        assert_eq!(
            resolve_all(anc, ours, theirs, &Decision::Ours),
            *ours,
            "ours for {anc:?}"
        );
        assert_eq!(
            resolve_all(anc, ours, theirs, &Decision::Theirs),
            *theirs,
            "theirs for {anc:?}"
        );
    }
}
