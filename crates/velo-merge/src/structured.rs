//! Key-aware merge drivers for JSON, YAML and TOML.
//!
//! Line-based diff3 conflicts when two branches each append a key at the end of
//! the same object, because both edits touch the same lines. These drivers
//! parse all three sides and merge by key instead. Arrays and scalars are
//! atomic: two sides changing the same one differently is a conflict.
//!
//! Conflicts are still reported as line hunks (from diff3, or one whole-file
//! hunk when diff3 sees none) so the resolver and TUI need no new concept.
//!
//! On a clean structural merge the driver prefers diff3's text when that is
//! clean and parses to the same value, because it keeps the user's formatting
//! and comments. Only when diff3 could not produce the structurally correct
//! document is the merged value re-serialised, which can normalise formatting
//! and drops YAML comments.

#[cfg(any(feature = "json", feature = "yaml", feature = "toml"))]
use crate::{compute_conflict_hunks, diff3, whole_file_conflict, MergeDriver, MergeResult};

/// A parsed document type the generic merge can walk.
trait Doc: Sized + Clone + PartialEq {
    type Key: Clone + PartialEq;
    fn parse(text: &str) -> Option<Self>;
    fn serialise(&self) -> Option<String>;
    /// Keys in document order, or `None` when this value is not a map.
    fn keys(&self) -> Option<Vec<Self::Key>>;
    fn get(&self, key: &Self::Key) -> Option<&Self>;
    fn from_entries(entries: Vec<(Self::Key, Self)>) -> Self;
}

/// Three-way merge of one slot, where `None` is "absent". `Err` is a conflict.
fn merge_slot<D: Doc>(a: Option<&D>, o: Option<&D>, t: Option<&D>) -> Result<Option<D>, ()> {
    if o == t {
        return Ok(o.cloned());
    }
    if o == a {
        return Ok(t.cloned());
    }
    if t == a {
        return Ok(o.cloned());
    }
    let (Some(a), Some(o), Some(t)) = (a, o, t) else {
        return Err(());
    };
    let (Some(_), Some(ok), Some(tk)) = (a.keys(), o.keys(), t.keys()) else {
        return Err(());
    };
    // Ours' order first, then keys only theirs has, in theirs' order.
    let mut keys = ok;
    for k in tk {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    let mut out = Vec::new();
    for k in keys {
        if let Some(v) = merge_slot(a.get(&k), o.get(&k), t.get(&k))? {
            out.push((k, v));
        }
    }
    Ok(Some(D::from_entries(out)))
}

fn run<D: Doc>(ancestor: &str, ours: &str, theirs: &str) -> MergeResult {
    let (Some(a), Some(o), Some(t)) = (D::parse(ancestor), D::parse(ours), D::parse(theirs)) else {
        return diff3(ancestor, ours, theirs);
    };
    let Ok(Some(v)) = merge_slot(Some(&a), Some(&o), Some(&t)) else {
        let h = compute_conflict_hunks(ancestor, ours, theirs);
        return MergeResult::Conflicted(if h.is_empty() {
            whole_file_conflict(ancestor, ours, theirs)
        } else {
            h
        });
    };
    let line = diff3(ancestor, ours, theirs);
    if let MergeResult::Clean(text) = &line {
        if D::parse(text).as_ref() == Some(&v) {
            return line;
        }
    }
    match v.serialise() {
        Some(mut s) => {
            if ours.ends_with('\n') && !s.ends_with('\n') {
                s.push('\n');
            }
            MergeResult::Clean(s)
        }
        None => line,
    }
}

#[cfg(feature = "json")]
impl Doc for serde_json::Value {
    type Key = String;
    fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }
    fn serialise(&self) -> Option<String> {
        serde_json::to_string_pretty(self).ok()
    }
    fn keys(&self) -> Option<Vec<String>> {
        self.as_object().map(|m| m.keys().cloned().collect())
    }
    fn get(&self, key: &String) -> Option<&Self> {
        self.as_object().and_then(|m| m.get(key))
    }
    fn from_entries(entries: Vec<(String, Self)>) -> Self {
        serde_json::Value::Object(entries.into_iter().collect())
    }
}

#[cfg(feature = "yaml")]
impl Doc for serde_yaml_ng::Value {
    type Key = serde_yaml_ng::Value;
    fn parse(text: &str) -> Option<Self> {
        serde_yaml_ng::from_str(text).ok()
    }
    fn serialise(&self) -> Option<String> {
        serde_yaml_ng::to_string(self).ok()
    }
    fn keys(&self) -> Option<Vec<Self>> {
        self.as_mapping().map(|m| m.keys().cloned().collect())
    }
    fn get(&self, key: &Self) -> Option<&Self> {
        self.as_mapping().and_then(|m| m.get(key))
    }
    fn from_entries(entries: Vec<(Self, Self)>) -> Self {
        let mut m = serde_yaml_ng::Mapping::new();
        for (k, v) in entries {
            m.insert(k, v);
        }
        serde_yaml_ng::Value::Mapping(m)
    }
}

#[cfg(feature = "toml")]
impl Doc for toml::Value {
    type Key = String;
    fn parse(text: &str) -> Option<Self> {
        text.parse::<toml::Table>().ok().map(toml::Value::Table)
    }
    fn serialise(&self) -> Option<String> {
        toml::to_string_pretty(self).ok()
    }
    fn keys(&self) -> Option<Vec<String>> {
        self.as_table().map(|m| m.keys().cloned().collect())
    }
    fn get(&self, key: &String) -> Option<&Self> {
        self.as_table().and_then(|m| m.get(key))
    }
    fn from_entries(entries: Vec<(String, Self)>) -> Self {
        toml::Value::Table(entries.into_iter().collect())
    }
}

/// Key-aware merge for JSON. Falls back to diff3 if any side does not parse.
#[cfg(feature = "json")]
#[derive(Clone, Copy, Debug, Default)]
pub struct JsonDriver;

#[cfg(feature = "json")]
impl MergeDriver for JsonDriver {
    fn name(&self) -> &str {
        "json"
    }
    fn merge(&self, ancestor: &str, ours: &str, theirs: &str) -> MergeResult {
        run::<serde_json::Value>(ancestor, ours, theirs)
    }
    fn units(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        if serde_json::from_str::<serde_json::Value>(text).is_err() {
            return line_units(text);
        }
        json_units(text)
    }
}

/// Key-aware merge for YAML. Falls back to diff3 if any side does not parse.
/// Re-serialising (only when diff3 cannot give the right document) drops comments.
#[cfg(feature = "yaml")]
#[derive(Clone, Copy, Debug, Default)]
pub struct YamlDriver;

#[cfg(feature = "yaml")]
impl MergeDriver for YamlDriver {
    fn name(&self) -> &str {
        "yaml"
    }
    fn merge(&self, ancestor: &str, ours: &str, theirs: &str) -> MergeResult {
        run::<serde_yaml_ng::Value>(ancestor, ours, theirs)
    }
    fn units(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        if serde_yaml_ng::from_str::<serde_yaml_ng::Value>(text).is_err() {
            return line_units(text);
        }
        let starts: Vec<usize> = text
            .lines()
            .enumerate()
            .filter(|(_, l)| {
                !l.trim().is_empty() && !l.starts_with(char::is_whitespace) && !l.starts_with('#')
            })
            .map(|(i, _)| i)
            .collect();
        let n = text.lines().count();
        crate::ranges_from_starts(n, &starts)
    }
}

/// Key-aware merge for TOML. Falls back to diff3 if any side does not parse.
#[cfg(feature = "toml")]
#[derive(Clone, Copy, Debug, Default)]
pub struct TomlDriver;

#[cfg(feature = "toml")]
impl MergeDriver for TomlDriver {
    fn name(&self) -> &str {
        "toml"
    }
    fn merge(&self, ancestor: &str, ours: &str, theirs: &str) -> MergeResult {
        run::<toml::Value>(ancestor, ours, theirs)
    }
    fn units(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        if text.parse::<toml::Table>().is_err() {
            return line_units(text);
        }
        let lines: Vec<&str> = text.lines().collect();
        let is_header = |l: &str| {
            let t = l.trim();
            t.starts_with('[') && t.split('#').next().unwrap_or("").trim_end().ends_with(']')
        };
        let first = lines
            .iter()
            .position(|l| is_header(l))
            .unwrap_or(lines.len());
        let mut starts: Vec<usize> = (0..first).collect();
        starts.extend((first..lines.len()).filter(|&i| is_header(lines[i])));
        crate::ranges_from_starts(lines.len(), &starts)
    }
}

/// One unit per line: what a document that does not parse falls back to.
fn line_units(text: &str) -> Vec<std::ops::Range<usize>> {
    (0..text.lines().count()).map(|i| i..i + 1).collect()
}

/// Units of a parsed JSON document: the opening line, each top-level member, and
/// the closing line. A member starts at a line indented like the first member
/// line, so nested objects and arrays stay inside their key's unit. A document
/// without that shape (one-liners, bare scalars) keeps line units.
#[cfg(feature = "json")]
fn json_units(text: &str) -> Vec<std::ops::Range<usize>> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(close) = lines.iter().rposition(|l| !l.trim().is_empty()) else {
        return line_units(text);
    };
    let first = (1..close).find(|&i| !lines[i].trim().is_empty());
    let Some(first) =
        first.filter(|_| close >= 2 && lines[close].trim_start().starts_with(['}', ']']))
    else {
        return line_units(text);
    };
    let indent = |l: &str| l.len() - l.trim_start().len();
    let ind = indent(lines[first]);
    let mut starts: Vec<usize> = (first..close)
        .filter(|&i| {
            !lines[i].trim().is_empty()
                && indent(lines[i]) == ind
                && !lines[i].trim_start().starts_with(['}', ']'])
        })
        .collect();
    starts.push(close);
    starts.extend(close + 1..lines.len());
    crate::ranges_from_starts(lines.len(), &starts)
}
