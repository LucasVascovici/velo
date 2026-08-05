//! Behaviour a real consumer depends on.
//!
//! These run against a temporary directory and use nothing but the public API of
//! `prompt-registry`, which in turn uses nothing but the public API of
//! `velo-core`.

use prompt_registry::Registry;
use tempfile::TempDir;
use velo_core::commands;

fn registry() -> (TempDir, Registry) {
    let tmp = TempDir::new().unwrap();
    let reg = Registry::create(tmp.path()).unwrap();
    (tmp, reg)
}

#[test]
fn a_published_prompt_can_be_read_back() {
    let (_tmp, mut reg) = registry();
    let v1 = reg
        .publish("summarise", "Summarise the following:", "claude-opus-5")
        .unwrap();
    assert_eq!(
        reg.get_at(&v1, "summarise").unwrap(),
        "Summarise the following:"
    );
    assert_eq!(reg.latest("summarise").unwrap(), "Summarise the following:");
}

#[test]
fn an_older_version_stays_readable_after_republishing() {
    let (_tmp, mut reg) = registry();
    let v1 = reg.publish("greet", "Hello.", "claude-opus-5").unwrap();
    let v2 = reg.publish("greet", "Hi there.", "claude-opus-5").unwrap();

    assert_ne!(v1, v2);
    assert_eq!(reg.get_at(&v1, "greet").unwrap(), "Hello.");
    assert_eq!(reg.get_at(&v2, "greet").unwrap(), "Hi there.");
    assert_eq!(reg.latest("greet").unwrap(), "Hi there.");
}

#[test]
fn publishing_one_prompt_does_not_disturb_the_others() {
    let (_tmp, mut reg) = registry();
    reg.publish("a", "prompt a", "claude-opus-5").unwrap();
    reg.publish("b", "prompt b", "claude-opus-5").unwrap();
    reg.publish("a", "prompt a v2", "claude-opus-5").unwrap();

    assert_eq!(reg.latest("a").unwrap(), "prompt a v2");
    assert_eq!(reg.latest("b").unwrap(), "prompt b");

    let mut names = reg.list().unwrap();
    names.sort();
    assert_eq!(names, ["a", "b"]);
}

#[test]
fn identical_content_republished_is_a_distinct_version() {
    let (_tmp, mut reg) = registry();
    let v1 = reg.publish("same", "body", "claude-opus-5").unwrap();
    let v2 = reg.publish("same", "body", "claude-opus-5").unwrap();
    // The tree is identical, but the parent and timestamp differ, so the ids do.
    assert_ne!(v1, v2, "a republish is its own version");
}

#[test]
fn versions_are_filtered_by_metadata_not_by_message_text() {
    let (_tmp, mut reg) = registry();
    reg.publish("alpha", "one", "claude-opus-5").unwrap();
    reg.publish("beta", "two", "claude-haiku-4-5").unwrap();
    reg.publish("alpha", "three", "claude-opus-5").unwrap();

    let alpha = reg.versions("alpha").unwrap();
    assert_eq!(alpha.len(), 2, "two publishes touched alpha");
    assert!(alpha
        .iter()
        .all(|v| v.model.as_deref() == Some("claude-opus-5")));

    let beta = reg.versions("beta").unwrap();
    assert_eq!(beta.len(), 1);
    assert_eq!(beta[0].model.as_deref(), Some("claude-haiku-4-5"));
}

#[test]
fn a_release_tag_resolves_to_its_version() {
    let (_tmp, mut reg) = registry();
    let v1 = reg.publish("greet", "Hello.", "claude-opus-5").unwrap();
    reg.publish("greet", "Hi there.", "claude-opus-5").unwrap();

    reg.release("v1.0", &v1).unwrap();
    assert_eq!(reg.get("v1.0", "greet").unwrap(), "Hello.");
    // And a hash prefix works the same way.
    assert_eq!(reg.get(&v1.as_str()[..12], "greet").unwrap(), "Hello.");
}

#[test]
fn a_registry_survives_being_closed_and_reopened() {
    let tmp = TempDir::new().unwrap();
    let v1 = {
        let mut reg = Registry::create(tmp.path()).unwrap();
        reg.publish("keep", "durable", "claude-opus-5").unwrap()
    };

    let reg = Registry::open(tmp.path()).unwrap();
    assert_eq!(reg.latest("keep").unwrap(), "durable");
    assert_eq!(reg.get_at(&v1, "keep").unwrap(), "durable");
}

#[test]
fn an_empty_registry_answers_without_erroring() {
    let (_tmp, reg) = registry();
    assert!(reg.list().unwrap().is_empty());
    assert!(reg.versions("anything").unwrap().is_empty());
    assert!(reg.latest("anything").is_err());
}

#[test]
fn a_missing_prompt_is_reported_as_such() {
    let (_tmp, mut reg) = registry();
    reg.publish("present", "here", "claude-opus-5").unwrap();
    assert!(reg.latest("absent").is_err());
    assert!(reg.publish("", "x", "m").is_err());
    assert!(reg.publish("has/slash", "x", "m").is_err());
}

#[test]
fn the_repository_stays_verifiable() {
    let tmp = TempDir::new().unwrap();
    let mut reg = Registry::create(tmp.path()).unwrap();
    for i in 0..5 {
        reg.publish("churn", &format!("body {}", i), "claude-opus-5")
            .unwrap();
    }
    drop(reg);

    // fsck recomputes every snapshot id, so this proves the metadata we attached
    // is hashed into the ids exactly as the receiver of a bundle would recompute.
    let repo = velo_core::Repo::open_and_migrate(tmp.path()).unwrap();
    let report = commands::fsck::check(&repo).unwrap();
    assert!(report.is_healthy(), "registry writes must survive fsck");
}

/// The natural guess for "give me everything" silently returns nothing.
///
/// Kept as a test rather than a note, so that if `history::run` ever grows a
/// sensible unlimited value this starts failing and gets revisited.
#[test]
fn history_limit_zero_returns_nothing_not_everything() {
    let tmp = TempDir::new().unwrap();
    let mut reg = Registry::create(tmp.path()).unwrap();
    reg.publish("a", "one", "claude-opus-5").unwrap();
    reg.publish("a", "two", "claude-opus-5").unwrap();
    drop(reg);

    let repo = velo_core::Repo::open_and_migrate(tmp.path()).unwrap();
    let none = commands::history::run(&repo, false, 0, Some("registry"), None).unwrap();
    assert!(
        none.entries.is_empty(),
        "limit 0 means LIMIT 0 in SQL, i.e. no rows — not 'unlimited'"
    );

    let all = commands::history::run(&repo, false, usize::MAX, Some("registry"), None).unwrap();
    assert_eq!(all.entries.len(), 2, "usize::MAX is the way to say 'all'");
}
