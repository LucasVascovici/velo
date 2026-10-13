//! `clean_changes_theirs`: the regions only the incoming side changed.

use velo_merge::clean_changes_theirs;

#[test]
fn an_insertion_is_a_clean_change() {
    let c = clean_changes_theirs("a\nb\n", "a\nb\n", "a\nx\nb\n");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].lines, vec!["x".to_string()]);
}

#[test]
fn a_replacement_is_a_clean_change() {
    let c = clean_changes_theirs("a\nb\nc\n", "a\nb\nc\n", "a\nB\nc\n");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].lines, vec!["B".to_string()]);
    assert_eq!((c[0].ancestor_start, c[0].ancestor_end), (1, 2));
}

#[test]
fn a_pure_deletion_is_excluded() {
    assert!(clean_changes_theirs("a\nb\nc\n", "a\nb\nc\n", "a\nc\n").is_empty());
}

#[test]
fn a_file_with_only_conflicts_has_none() {
    assert!(clean_changes_theirs("a\n", "b\n", "c\n").is_empty());
}

#[test]
fn clean_changes_sit_beside_a_conflict() {
    let c = clean_changes_theirs("H\ns\nw\n", "H\ns\nours\nw\n", "H\nnew\ns\ntheirs\nw\n");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].lines, vec!["new".to_string()]);
}
