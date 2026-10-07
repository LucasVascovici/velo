//! The key-aware JSON, YAML and TOML drivers.

use velo_merge::{diff3, MergeDriver, MergeResult};

fn clean(r: MergeResult) -> String {
    match r {
        MergeResult::Clean(s) => s,
        other => panic!("expected a clean merge, got {other:?}"),
    }
}

fn same(a: &MergeResult, b: &MergeResult) -> bool {
    match (a, b) {
        (MergeResult::Clean(x), MergeResult::Clean(y)) => x == y,
        (MergeResult::Conflicted(x), MergeResult::Conflicted(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|(p, q)| (&p.ours, &p.theirs) == (&q.ours, &q.theirs))
        }
        _ => false,
    }
}

/// One suite per format. `$keys` lists a document's top-level keys in order.
macro_rules! suite {
    (
        $m:ident, $feat:literal, $driver:expr, $keys:expr,
        base: $base:expr,
        add_c: $add_c:expr,
        add_d: $add_d:expr,
        both_c: $both_c:expr,
        nested_base: $nb:expr,
        nested_ours: $no:expr,
        nested_theirs: $nt:expr,
        del_b: $del_b:expr,
        broken: $broken:expr,
        wide_base: $wb:expr,
        wide_ours: $wo:expr,
        wide_theirs: $wt:expr,
        wide_expected: $we:expr $(,)?
    ) => {
        #[cfg(feature = $feat)]
        mod $m {
            use super::*;
            fn keys(s: &str) -> Vec<String> {
                let f: fn(&str) -> Vec<String> = $keys;
                f(s)
            }

            #[test]
            fn driver_has_its_name() {
                assert_eq!($driver.name(), $feat);
            }

            #[test]
            fn disjoint_additions_merge_where_diff3_conflicts() {
                assert!(matches!(
                    diff3($base, $add_c, $add_d),
                    MergeResult::Conflicted(_)
                ));
                let out = clean($driver.merge($base, $add_c, $add_d));
                assert_eq!(keys(&out), ["a", "b", "c", "d"]);
            }

            #[test]
            fn same_key_different_value_conflicts() {
                match $driver.merge($base, $add_c, $both_c) {
                    MergeResult::Conflicted(h) => assert!(!h.is_empty()),
                    other => panic!("expected conflict, got {other:?}"),
                }
            }

            #[test]
            fn nested_maps_merge_recursively() {
                let out = clean($driver.merge($nb, $no, $nt));
                for needle in ["p", "q"] {
                    assert!(out.contains(needle), "{needle} missing in:\n{out}");
                }
                assert_eq!(keys(&out), ["n"]);
            }

            #[test]
            fn deletion_beats_an_untouched_key() {
                let out = clean($driver.merge($base, $add_c, $del_b));
                assert_eq!(keys(&out), ["a", "c"]);
            }

            #[test]
            fn invalid_input_falls_back_to_diff3() {
                let r = $driver.merge($base, $broken, $add_d);
                assert!(same(&r, &diff3($base, $broken, $add_d)));
            }

            #[test]
            fn structurally_equal_diff3_text_is_kept_byte_for_byte() {
                let out = clean($driver.merge($wb, $wo, $wt));
                assert_eq!(out, $we);
            }
        }
    };
}

#[cfg(feature = "json")]
fn json_keys(s: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(s).unwrap();
    v.as_object().unwrap().keys().cloned().collect()
}

suite!(
    json, "json", velo_merge::JsonDriver, json_keys,
    base: "{\n  \"a\": 1,\n  \"b\": 2\n}\n",
    add_c: "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}\n",
    add_d: "{\n  \"a\": 1,\n  \"b\": 2,\n  \"d\": 4\n}\n",
    both_c: "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 9\n}\n",
    nested_base: "{\n  \"n\": {\n    \"x\": 1\n  }\n}\n",
    nested_ours: "{\n  \"n\": {\n    \"x\": 1,\n    \"p\": 2\n  }\n}\n",
    nested_theirs: "{\n  \"n\": {\n    \"x\": 1,\n    \"q\": 3\n  }\n}\n",
    del_b: "{\n  \"a\": 1\n}\n",
    broken: "{\n  \"a\": 1,\n",
    wide_base: "{\n    \"a\": 1,\n    \"b\": 2,\n    \"c\": 3\n}\n",
    wide_ours: "{\n    \"a\": 10,\n    \"b\": 2,\n    \"c\": 3\n}\n",
    wide_theirs: "{\n    \"a\": 1,\n    \"b\": 2,\n    \"c\": 30\n}\n",
    wide_expected: "{\n    \"a\": 10,\n    \"b\": 2,\n    \"c\": 30\n}\n",
);

#[cfg(feature = "yaml")]
fn yaml_keys(s: &str) -> Vec<String> {
    let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(s).unwrap();
    v.as_mapping()
        .unwrap()
        .keys()
        .map(|k| k.as_str().unwrap().to_string())
        .collect()
}

suite!(
    yaml, "yaml", velo_merge::YamlDriver, yaml_keys,
    base: "a: 1\nb: 2\n",
    add_c: "a: 1\nb: 2\nc: 3\n",
    add_d: "a: 1\nb: 2\nd: 4\n",
    both_c: "a: 1\nb: 2\nc: 9\n",
    nested_base: "n:\n  x: 1\n",
    nested_ours: "n:\n  x: 1\n  p: 2\n",
    nested_theirs: "n:\n  x: 1\n  q: 3\n",
    del_b: "a: 1\n",
    broken: "a: [1\nb: 2\n",
    wide_base: "a: 1\nb: 2\nc: 3\n",
    wide_ours: "a: 10    # bumped\nb: 2\nc: 3\n",
    wide_theirs: "a: 1\nb: 2\nc: 30\n",
    wide_expected: "a: 10    # bumped\nb: 2\nc: 30\n",
);

#[cfg(feature = "toml")]
fn toml_keys(s: &str) -> Vec<String> {
    let v: ::toml::Table = s.parse().unwrap();
    v.keys().cloned().collect()
}

suite!(
    toml, "toml", velo_merge::TomlDriver, toml_keys,
    base: "a = 1\nb = 2\n",
    add_c: "a = 1\nb = 2\nc = 3\n",
    add_d: "a = 1\nb = 2\nd = 4\n",
    both_c: "a = 1\nb = 2\nc = 9\n",
    nested_base: "[n]\nx = 1\n",
    nested_ours: "[n]\nx = 1\np = 2\n",
    nested_theirs: "[n]\nx = 1\nq = 3\n",
    del_b: "a = 1\n",
    broken: "a = 1\nb = \n",
    wide_base: "a   = 1\nb   = 2\nc   = 3\n",
    wide_ours: "a   = 10\nb   = 2\nc   = 3\n",
    wide_theirs: "a   = 1\nb   = 2\nc   = 30\n",
    wide_expected: "a   = 10\nb   = 2\nc   = 30\n",
);
