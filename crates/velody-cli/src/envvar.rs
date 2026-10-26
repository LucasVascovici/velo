//! Environment variables, under their current and their previous names.
//!
//! The project was called Velo until 7.0.0 and its variables began `VELO_`. They
//! begin `VELODY_` now, and the old spelling is still read so that scripts, CI
//! configuration and servers that set `VELO_AUTHOR_NAME`, `VELO_TOKEN` and the rest
//! keep working. The new name wins when both are set. `velody-core` reads no
//! environment at all, so this lives in the binaries.

use std::env::VarError;

/// The previous prefix. Only a variable that starts with `VELODY_` has a legacy twin.
const LEGACY_PREFIX: &str = "VELO_";
const PREFIX: &str = "VELODY_";

/// `name` (spelled `VELODY_...`), else the same variable under its old `VELO_` name.
pub fn var(name: &str) -> Result<String, VarError> {
    var_with(name, |k| std::env::var(k))
}

fn var_with(
    name: &str,
    get: impl Fn(&str) -> Result<String, VarError>,
) -> Result<String, VarError> {
    match get(name) {
        Err(VarError::NotPresent) => match name.strip_prefix(PREFIX) {
            Some(rest) => get(&format!("{LEGACY_PREFIX}{rest}")),
            None => Err(VarError::NotPresent),
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Result<String, VarError> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned().ok_or(VarError::NotPresent)
    }

    #[test]
    fn the_old_name_is_read_when_the_new_one_is_absent() {
        let get = env(&[("VELO_AUTHOR_NAME", "old")]);
        assert_eq!(var_with("VELODY_AUTHOR_NAME", get).unwrap(), "old");
    }

    #[test]
    fn the_new_name_wins_when_both_are_set() {
        let get = env(&[("VELO_TOKEN", "old"), ("VELODY_TOKEN", "new")]);
        assert_eq!(var_with("VELODY_TOKEN", get).unwrap(), "new");
    }

    #[test]
    fn a_name_without_the_prefix_has_no_twin() {
        let get = env(&[("VELO_EDITOR", "old")]);
        assert!(var_with("EDITOR", get).is_err());
        assert!(var_with("VELODY_TOKEN", env(&[])).is_err());
    }
}
