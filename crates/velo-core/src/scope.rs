//! Deciding which files in a directory the repository is interested in.
//!
//! By default that is everything except what `.veloignore` and `.gitignore`
//! exclude. Both are *files in the user's directory*, which is fine for a person
//! running `velo` and wrong for an application: writing `.veloignore` into
//! someone's workspace so your own cache directory is skipped is putting a file
//! in their folder for your benefit, and it shows up in their editor, their
//! backups and their other version control.
//!
//! [`Scope`] says the same things in memory instead.
//!
//! # Two shapes of the same question
//!
//! - **Ignore rules** subtract: everything is tracked *except* these.
//! - **Roots** restrict: *only* these are tracked.
//!
//! They compose — roots first, then ignores within them — and both use
//! gitignore-style patterns, which is what `.veloignore` already speaks, so there
//! is one syntax to learn rather than two.
//!
//! ```
//! # fn main() -> Result<(), velo_core::Error> {
//! use velo_core::Scope;
//!
//! // An editor keeping its cache out of the user's history, without writing
//! // anything to their folder.
//! let scope = Scope::new().ignore(".myapp-cache/**")?.ignore("*.tmp")?;
//!
//! // Or: only ever track one subtree.
//! let prompts = Scope::new().only("prompts/**")?;
//! # let _ = (scope, prompts);
//! # Ok(()) }
//! ```

use std::path::Path;

use ignore::overrides::{Override, OverrideBuilder};

use crate::error::{Result, VeloError};

/// Which paths a repository handle considers part of the working tree.
///
/// Applied on top of `.veloignore` and `.gitignore`, never instead of them: a
/// user's own rules are theirs, and an application quietly overriding them would
/// be worse than the file it was trying to avoid writing.
#[derive(Clone, Debug, Default)]
pub struct Scope {
    /// Patterns to exclude, gitignore syntax.
    ignores: Vec<String>,
    /// Patterns to restrict to. Empty means "everything".
    only: Vec<String>,
}

impl Scope {
    /// Track everything, as `velo` itself does.
    pub fn new() -> Self {
        Self::default()
    }

    /// Exclude paths matching `pattern`, in gitignore syntax.
    ///
    /// # Errors
    /// If the pattern is malformed — reported now rather than silently matching
    /// nothing later, which is the failure mode that wastes an afternoon.
    pub fn ignore(mut self, pattern: &str) -> Result<Self> {
        validate(pattern)?;
        self.ignores.push(pattern.to_string());
        Ok(self)
    }

    /// Track **only** paths matching `pattern`. Repeatable; the effect is a union.
    ///
    /// "Only track `prompts/**`" without writing a `.veloignore` that excludes
    /// everything else by hand.
    pub fn only(mut self, pattern: &str) -> Result<Self> {
        validate(pattern)?;
        self.only.push(pattern.to_string());
        Ok(self)
    }

    /// Whether anything has been narrowed at all.
    pub fn is_default(&self) -> bool {
        self.ignores.is_empty() && self.only.is_empty()
    }

    /// Exclusions, for the walker's override layer.
    ///
    /// Only ever `!`-prefixed patterns. An override made purely of negations
    /// reports `None` for everything it does not match, so the walker's own
    /// `.veloignore` and `.gitignore` handling still decides those — which is
    /// what makes a scope subtract without ever adding.
    pub(crate) fn exclusions(&self, root: &Path) -> Result<Option<Override>> {
        if self.ignores.is_empty() {
            return Ok(None);
        }
        let mut builder = OverrideBuilder::new(root);
        for pattern in &self.ignores {
            builder.add(&format!("!{}", pattern)).map_err(|e| {
                VeloError::invalid(format!("bad ignore pattern '{}': {}", pattern, e))
            })?;
        }
        Ok(Some(builder.build().map_err(|e| {
            VeloError::invalid(format!("ignore rules could not be compiled: {}", e))
        })?))
    }

    /// Restriction, applied to the walk's *results*.
    ///
    /// Deliberately **not** handed to the walker as an override. The `ignore`
    /// crate treats a matching positive pattern as a whitelist that outranks
    /// `.veloignore`, so `only("**")` would quietly re-include a file the user
    /// had excluded — an application overriding a person's own rules, which is
    /// worse than the file it was trying to avoid writing. Filtering results
    /// instead can only ever remove.
    pub(crate) fn restriction(&self, root: &Path) -> Result<Option<Override>> {
        if self.only.is_empty() {
            return Ok(None);
        }
        let mut builder = OverrideBuilder::new(root);
        for pattern in &self.only {
            builder.add(pattern).map_err(|e| {
                VeloError::invalid(format!("bad scope pattern '{}': {}", pattern, e))
            })?;
        }
        Ok(Some(builder.build().map_err(|e| {
            VeloError::invalid(format!("scope could not be compiled: {}", e))
        })?))
    }
}

/// Whether `path` survives a compiled restriction.
pub(crate) fn permitted(restriction: Option<&Override>, path: &Path) -> bool {
    match restriction {
        None => true,
        Some(o) => o.matched(path, false).is_whitelist(),
    }
}

/// Catch the patterns that are obviously not going to do what was meant.
fn validate(pattern: &str) -> Result<()> {
    if pattern.trim().is_empty() {
        return Err(VeloError::invalid("An ignore pattern cannot be empty."));
    }
    // A leading `!` in gitignore means "re-include"; accepting it here would flip
    // the meaning of `ignore()` and `only()` under the caller's feet.
    if pattern.starts_with('!') {
        return Err(VeloError::invalid(format!(
            "'{}' starts with '!'. Use `only` to include rather than a negated ignore.",
            pattern
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_scope_narrows_nothing() {
        let scope = Scope::new();
        assert!(scope.is_default());
        assert!(scope.exclusions(Path::new(".")).unwrap().is_none());
        assert!(scope.restriction(Path::new(".")).unwrap().is_none());
    }

    #[test]
    fn patterns_that_cannot_work_are_refused_when_given() {
        assert!(Scope::new().ignore("").is_err());
        assert!(Scope::new().ignore("   ").is_err());
        // `!` would invert the meaning of the method that accepted it.
        assert!(Scope::new().ignore("!keep.txt").is_err());
        assert!(Scope::new().only("!skip.txt").is_err());
    }

    #[test]
    fn ignores_and_roots_compile_separately() {
        let scope = Scope::new()
            .only("prompts/**")
            .unwrap()
            .ignore("prompts/scratch/**")
            .unwrap();
        assert!(!scope.is_default());
        // Two matchers, because they are applied at different points: exclusions
        // go to the walker, the restriction filters its results.
        assert!(scope.exclusions(Path::new(".")).unwrap().is_some());
        assert!(scope.restriction(Path::new(".")).unwrap().is_some());
    }
}
