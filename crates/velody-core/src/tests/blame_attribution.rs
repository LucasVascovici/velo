//! Does blame credit the right developer after merges, rebases and cherry-picks?
//!
//! # Verdict
//!
//! The simulation oracle's "blame attribution mismatch" warnings (blame said a
//! word of the commit message, `-`, or another developer for a line the ledger
//! credits) are two different things, and neither is a walk bug:
//!
//! * **A word of the message, or `-`: the oracle's parsing was at fault, and the
//!   CLI text made it easy to be.** The oracle reads `line.split()[3]` as the
//!   author. A row is `hash date time [author] message line text`. The author
//!   cell is shown only when some line of the file has an author, and a line
//!   whose snapshot has none (the initial import, a save made without
//!   `--author`) used to get fourteen blanks. Whitespace splitting drops
//!   blanks, so `[3]` became the first word of the message, and a message such
//!   as `-` shifted it the same way. That is fixed in the renderer: such a row
//!   now prints the fixed placeholder `(unknown)` in the author cell, so the
//!   column count is stable. An author *name* containing a space still spans
//!   two tokens; a parser should not split the text format at all.
//! * **Another developer: replaying credits the replayer.** A rebase or
//!   cherry-pick writes new snapshots, and the author recorded on them is the
//!   one passed to the command, not the original author. Blame then credits
//!   the person who rebased or picked. A three-way merge and a fast-forward do
//!   not replay: the lines keep their original snapshots and authors, and the
//!   merge snapshot owns no line it did not change. A conflict resolved with
//!   `--take theirs` takes the other side's text, and blame follows the text to
//!   the snapshot that last changed it.
//!
//! # What the oracle should parse
//!
//! Do not split the text format. Use the library (`blame::run`, whose
//! `LineOrigin.author` is an `Option<Author>`) or, from the CLI, read the first
//! tokens as `hash date time author` after the placeholder fix and treat
//! `(unknown)` as "no author recorded". For rebased and cherry-picked work,
//! compare against the author the replaying step was given, not the original
//! writer.

use std::fs;
use tempfile::TempDir;

use crate::commands::blame::{self, Blame, LineOrigin};
use crate::commands::resolve::{self, OnEdits, TakeOption};
use crate::commands::{merge, rebase, save, switch};
use crate::{Author, Repo, SnapshotId};

const BASE: &str = "l1\nl2\nl3\nl4\nl5\nl6\n";

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn who(name: &str) -> Author {
    Author::new(name).unwrap()
}

/// Write `body` to `f.txt` and save it as `by`.
fn commit(repo: &Repo, body: &str, msg: &str, by: &Author) -> SnapshotId {
    fs::write(repo.root().join("f.txt"), body).unwrap();
    let g = repo.write().unwrap();
    save::run(
        &g,
        Some(msg),
        save::Options {
            author: Some(by),
            ..Default::default()
        },
    )
    .unwrap()
    .into_result()
    .expect("a snapshot")
    .hash
}

fn go(repo: &Repo, branch: &str) {
    switch::run(&repo.write().unwrap(), branch, false).unwrap();
}

fn bring(repo: &Repo, source: &str) -> merge::Outcome {
    merge::run(&repo.write().unwrap(), merge::Mode::Bring { source }).unwrap()
}

fn blamed(repo: &Repo) -> Blame {
    blame::run(repo, std::path::Path::new("f.txt"), Default::default()).unwrap()
}

fn origin(b: &Blame, line: usize) -> &LineOrigin {
    b.lines[line - 1]
        .origin
        .as_ref()
        .unwrap_or_else(|| panic!("line {line} unattributed"))
}

fn author_of(b: &Blame, line: usize) -> String {
    origin(b, line)
        .author
        .as_ref()
        .map_or("-".to_string(), |a| a.name().to_string())
}

/// `main` has A editing l1 and `feat` has B editing l6, both from a base saved
/// by `dev0`. Returns the two commits; `main` is checked out.
fn diverged() -> (TempDir, Repo, SnapshotId, SnapshotId) {
    let (tmp, repo) = setup();
    commit(&repo, BASE, "base", &who("dev0"));
    go(&repo, "feat");
    let b = commit(
        &repo,
        "l1\nl2\nl3\nl4\nl5\nB6\n",
        "B edits six",
        &who("devB"),
    );
    go(&repo, "main");
    let a = commit(
        &repo,
        "A1\nl2\nl3\nl4\nl5\nl6\n",
        "A edits one",
        &who("devA"),
    );
    (tmp, repo, a, b)
}

#[test]
fn three_way_merge_keeps_each_original_author() {
    let (_tmp, repo, a, b) = diverged();
    assert!(matches!(bring(&repo, "feat"), merge::Outcome::Merged(t) if t.is_clean()));
    let m = commit(
        &repo,
        "A1\nl2\nl3\nl4\nl5\nB6\n",
        "Merge feat",
        &who("devM"),
    );

    let bl = blamed(&repo);
    assert_eq!(author_of(&bl, 1), "devA");
    assert_eq!(origin(&bl, 1).hash, a);
    assert_eq!(author_of(&bl, 6), "devB");
    assert_eq!(origin(&bl, 6).hash, b);
    assert_eq!(author_of(&bl, 3), "dev0");
    assert_eq!(origin(&bl, 6).branch.as_str(), "feat");
    assert!(
        bl.lines
            .iter()
            .all(|l| l.origin.as_ref().unwrap().hash != m),
        "the merge snapshot changed no line, so it owns none"
    );
}

#[test]
fn fast_forward_pull_keeps_authors() {
    let (_tmp, repo) = setup();
    commit(&repo, BASE, "base", &who("dev0"));
    go(&repo, "feat");
    let b = commit(
        &repo,
        "l1\nl2\nl3\nl4\nl5\nB6\n",
        "B edits six",
        &who("devB"),
    );
    go(&repo, "main");
    assert!(matches!(
        bring(&repo, "feat"),
        merge::Outcome::FastForwarded { .. }
    ));
    let bl = blamed(&repo);
    assert_eq!(author_of(&bl, 6), "devB");
    assert_eq!(origin(&bl, 6).hash, b);
    assert_eq!(author_of(&bl, 1), "dev0");
}

#[test]
fn rebase_credits_the_author_the_rebase_was_given() {
    let (_tmp, repo, a, b) = diverged();
    go(&repo, "feat");
    let out = rebase::run(
        &repo.write().unwrap(),
        rebase::Mode::Start { onto: &a },
        Some(&who("devR")),
    )
    .unwrap();
    assert!(matches!(out, rebase::Outcome::Completed { .. }));

    let bl = blamed(&repo);
    // A's line is untouched history; B's line lives in a replayed snapshot,
    // which carries the rebaser, not B.
    assert_eq!(author_of(&bl, 1), "devA");
    assert_eq!(origin(&bl, 1).hash, a);
    assert_eq!(author_of(&bl, 6), "devR");
    assert_ne!(origin(&bl, 6).hash, b, "replay writes a new snapshot");
    assert_eq!(origin(&bl, 6).message, "B edits six");
}

#[test]
fn cherry_pick_credits_the_picker() {
    let (_tmp, repo, a, b) = diverged();
    let out =
        crate::commands::cherry_pick::run(&repo.write().unwrap(), &b, Some(&who("devP"))).unwrap();
    assert!(!out.is_conflicted());

    let bl = blamed(&repo);
    assert_eq!(author_of(&bl, 1), "devA");
    assert_eq!(origin(&bl, 1).hash, a);
    assert_eq!(author_of(&bl, 6), "devP");
    assert_ne!(origin(&bl, 6).hash, b);
}

#[test]
fn conflict_resolved_with_take_theirs_follows_the_taken_text() {
    let (_tmp, repo) = setup();
    commit(&repo, BASE, "base", &who("dev0"));
    go(&repo, "feat");
    let b = commit(
        &repo,
        "l1\nl2\nB3\nl4\nl5\nl6\n",
        "B edits three",
        &who("devB"),
    );
    go(&repo, "main");
    commit(
        &repo,
        "l1\nl2\nA3\nl4\nl5\nl6\n",
        "A edits three",
        &who("devA"),
    );
    assert!(matches!(bring(&repo, "feat"), merge::Outcome::Merged(t) if !t.is_clean()));
    {
        let g = repo.write().unwrap();
        let cf = resolve::get_conflict(&repo, "f.txt").unwrap();
        resolve::take_side(&g, &cf, TakeOption::Theirs, OnEdits::Refuse).unwrap();
    }
    let m = commit(
        &repo,
        "l1\nl2\nB3\nl4\nl5\nl6\n",
        "Merge feat",
        &who("devM"),
    );

    let bl = blamed(&repo);
    assert_eq!(bl.lines[2].text, "B3");
    assert_eq!(author_of(&bl, 3), "devB");
    assert_eq!(origin(&bl, 3).hash, b);
    assert_ne!(origin(&bl, 3).hash, m);
}
