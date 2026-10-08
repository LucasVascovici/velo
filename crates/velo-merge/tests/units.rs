//! Units: what diff and blame compare for each driver. Every driver must cover
//! each line of the text exactly once, in order, whatever the input.

use std::ops::Range;
use velo_merge::{unit_texts, LineDriver, MergeDriver, ParagraphDriver};

fn assert_covers(text: &str, units: &[Range<usize>]) {
    let mut next = 0;
    for u in units {
        assert_eq!(u.start, next, "gap or overlap in {units:?}");
        assert!(u.end > u.start, "empty unit in {units:?}");
        next = u.end;
    }
    assert_eq!(next, text.lines().count(), "not covered: {units:?}");
}

#[test]
fn line_units_are_one_per_line() {
    let t = "a\nb\n\nc\n";
    let u = LineDriver.units(t);
    assert_eq!(u, vec![0..1, 1..2, 2..3, 3..4]);
    assert_covers(t, &u);
    assert!(LineDriver.units("").is_empty());
}

#[test]
fn paragraph_units_group_runs() {
    let t = "a\nb\n\n\nc\n\nd\ne\n";
    let u = ParagraphDriver.units(t);
    assert_eq!(u, vec![0..2, 2..4, 4..5, 5..6, 6..8]);
    assert_covers(t, &u);
    assert_eq!(unit_texts(t, &u)[0], "a\nb");
    assert_eq!(ParagraphDriver.name(), "paragraph");
}

#[cfg(feature = "json")]
#[test]
fn json_units_are_top_level_members() {
    use velo_merge::JsonDriver;
    let t = "{\n  \"a\": 1,\n  \"b\": {\n    \"c\": 2\n  },\n  \"d\": [\n    1\n  ]\n}\n";
    let u = JsonDriver.units(t);
    assert_eq!(u, vec![0..1, 1..2, 2..5, 5..8, 8..9]);
    assert_covers(t, &u);
    let bad = "{\n  \"a\": 1,\n";
    let u = JsonDriver.units(bad);
    assert_eq!(u, vec![0..1, 1..2]);
    assert_covers(bad, &u);
    assert_covers("{\"a\":1}", &JsonDriver.units("{\"a\":1}"));
}

#[cfg(feature = "yaml")]
#[test]
fn yaml_units_are_top_level_keys() {
    use velo_merge::YamlDriver;
    let t = "# top\na: 1\nb:\n  - x\n  - y\n\nc: 3\n";
    let u = YamlDriver.units(t);
    assert_eq!(u, vec![0..1, 1..2, 2..6, 6..7]);
    assert_covers(t, &u);
    let bad = "a: [1\nb: 2\n";
    assert_eq!(YamlDriver.units(bad), vec![0..1, 1..2]);
}

#[cfg(feature = "toml")]
#[test]
fn toml_units_are_tables() {
    use velo_merge::TomlDriver;
    let t = "x = 1\ny = 2\n[a]\nk = 1\nj = 2\n[[b]]\nk = 3\n";
    let u = TomlDriver.units(t);
    assert_eq!(u, vec![0..1, 1..2, 2..5, 5..7]);
    assert_covers(t, &u);
    let bad = "x = \n[a\n";
    assert_eq!(TomlDriver.units(bad), vec![0..1, 1..2]);
}
