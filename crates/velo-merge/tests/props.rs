//! Property tests for the 3-way merge engine (randomised, with shrinking).
//!
//! These live outside `src` so they exercise `velo_merge` exactly as a consumer
//! does — through the public `diff3` entry point, with no access to the internal
//! segmentation. They previously ran inside `velo-core`'s suite, which left the
//! engine's own crate unverified.

use proptest::prelude::*;
use proptest::test_runner::TestCaseResult;
use velo_merge::{diff3, try_auto_merge, MergeResult};

/// "Merged cleanly, here is the text" — `None` when a human has to decide.
///
/// `MergeResult` is `#[non_exhaustive]`, so a wildcard arm is required from
/// outside the crate; today it covers only `Conflicted`.
fn merged(result: MergeResult) -> Option<String> {
    match result {
        MergeResult::Clean(text) => Some(text),
        _ => None,
    }
}

/// Join lines into a file body (trailing newline, like a real file).
fn body(lines: &[String]) -> String {
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

/// A non-empty run of short lowercase lines (never collides with the uppercase
/// ANCHOR markers used to separate edit regions).
fn lines() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec("[a-z]{1,5}", 1..6)
}

/// Assert the crate's two clean-merge entry points agree on one input triple.
fn agree(ancestor: &str, ours: &str, theirs: &str) -> TestCaseResult {
    prop_assert_eq!(
        merged(diff3(ancestor, ours, theirs)),
        try_auto_merge(ancestor, ours, theirs)
    );
    Ok(())
}

fn cat(a: &[String], b: &[String], c: &[String]) -> Vec<String> {
    let mut v = Vec::with_capacity(a.len() + b.len() + c.len());
    v.extend_from_slice(a);
    v.extend_from_slice(b);
    v.extend_from_slice(c);
    v
}

proptest! {
    /// If our side made no change, the merge must equal theirs exactly.
    #[test]
    fn ours_unchanged_yields_theirs(anc in lines(), theirs in lines()) {
        let a = body(&anc);
        let t = body(&theirs);
        prop_assert_eq!(merged(diff3(&a, &a, &t)), Some(t));
    }

    /// Symmetric: if their side made no change, the merge equals ours.
    #[test]
    fn theirs_unchanged_yields_ours(anc in lines(), ours in lines()) {
        let a = body(&anc);
        let o = body(&ours);
        prop_assert_eq!(merged(diff3(&a, &o, &a)), Some(o));
    }

    /// Both sides making the identical change is not a conflict.
    #[test]
    fn identical_change_on_both_sides(anc in lines(), both in lines()) {
        let a = body(&anc);
        let b = body(&both);
        prop_assert_eq!(merged(diff3(&a, &b, &b)), Some(b));
    }

    /// No silent data loss: when ours edits only a prefix region and theirs
    /// edits only a disjoint suffix region (separated by a stable anchor block
    /// neither touches), the merge must keep BOTH edits.
    #[test]
    fn disjoint_edits_preserve_both_sides(
        pre_a in lines(), suf_a in lines(),
        pre_o in lines(), suf_t in lines(),
    ) {
        let anchor: Vec<String> = (0..4).map(|i| format!("ANCHOR{i}")).collect();
        let ancestor = body(&cat(&pre_a, &anchor, &suf_a));
        let ours     = body(&cat(&pre_o, &anchor, &suf_a)); // changed prefix only
        let theirs   = body(&cat(&pre_a, &anchor, &suf_t)); // changed suffix only
        let expected = body(&cat(&pre_o, &anchor, &suf_t)); // both, no loss

        prop_assert_eq!(merged(diff3(&ancestor, &ours, &theirs)), Some(expected));
    }

    /// The merge is a pure function of its inputs.
    #[test]
    fn deterministic(anc in lines(), ours in lines(), theirs in lines()) {
        let (a, o, t) = (body(&anc), body(&ours), body(&theirs));
        prop_assert_eq!(merged(diff3(&a, &o, &t)), merged(diff3(&a, &o, &t)));
    }

    /// `try_auto_merge` is `diff3` with the outcome flattened to an `Option`, so
    /// this holds by construction today. It stays because that is a property of
    /// the *implementation*, not the signatures: the two were parallel copies of
    /// the same walk once, and re-inlining one would restore a blind spot the
    /// rest of this file cannot see — every property above enters through
    /// `diff3`, while `velo-core`'s `reconcile` calls only `try_auto_merge`.
    ///
    /// Random triples nearly always conflict, which would only ever exercise the
    /// `None` side, so each of the clean shapes above is checked too.
    #[test]
    fn diff3_and_try_auto_merge_agree(
        anc in lines(), ours in lines(), theirs in lines(), suf_t in lines(),
    ) {
        let (a, o, t) = (body(&anc), body(&ours), body(&theirs));
        agree(&a, &o, &t)?; // usually conflicting → both None
        agree(&a, &a, &t)?; // ours unchanged
        agree(&a, &o, &a)?; // theirs unchanged
        agree(&a, &o, &o)?; // identical change on both sides

        // Disjoint edits either side of a stable anchor block.
        let anchor: Vec<String> = (0..4).map(|i| format!("ANCHOR{i}")).collect();
        agree(
            &body(&cat(&anc, &anchor, &theirs)),
            &body(&cat(&ours, &anchor, &theirs)),
            &body(&cat(&anc, &anchor, &suf_t)),
        )?;
    }
}
