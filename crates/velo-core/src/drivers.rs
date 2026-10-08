//! Choosing a merge algorithm per path.
//!
//! [`Drivers`] maps path patterns to [`MergeDriver`]s. It is held by the
//! [`Repo`](crate::Repo) handle, as [`Scope`](crate::Scope) is, because which
//! files are JSON is a property of the repository, not of one merge.

use std::sync::Arc;

use globset::{Glob, GlobMatcher};
use velo_merge::{LineDriver, MergeDriver};

use crate::error::{Result, VeloError};

struct Rule {
    pattern: String,
    matcher: GlobMatcher,
    /// Whether the pattern contains a `/`, and so matches the whole path rather
    /// than the file name.
    has_slash: bool,
    driver: Arc<dyn MergeDriver>,
}

/// Path-pattern rules selecting a [`MergeDriver`]; line-based diff3 otherwise.
#[derive(Clone, Default)]
pub struct Drivers {
    rules: Vec<Rule>,
}

impl Clone for Rule {
    fn clone(&self) -> Self {
        Rule {
            pattern: self.pattern.clone(),
            matcher: self.matcher.clone(),
            has_slash: self.has_slash,
            driver: Arc::clone(&self.driver),
        }
    }
}

impl std::fmt::Debug for Drivers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(
                self.rules
                    .iter()
                    .map(|r| format!("{} => {}", r.pattern, r.driver.name())),
            )
            .finish()
    }
}

impl Drivers {
    /// No rules: every path uses line-based diff3.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a rule. The first matching rule wins.
    ///
    /// A pattern with no `/` matches the file name at any depth, like
    /// `.gitignore`; otherwise it matches the whole repo-relative path.
    ///
    /// # Errors
    /// `InvalidInput` if the pattern is not a valid glob.
    pub fn with(mut self, pattern: &str, driver: impl MergeDriver + 'static) -> Result<Self> {
        let glob = Glob::new(pattern)
            .map_err(|e| VeloError::invalid(format!("Bad driver pattern '{pattern}': {e}")))?;
        self.rules.push(Rule {
            pattern: pattern.to_string(),
            matcher: glob.compile_matcher(),
            has_slash: pattern.contains('/'),
            driver: Arc::new(driver),
        });
        Ok(self)
    }

    /// The driver for a repo-relative `path`; [`LineDriver`] when nothing matches.
    pub fn for_path(&self, path: &str) -> &dyn MergeDriver {
        static LINE: LineDriver = LineDriver;
        for r in &self.rules {
            let hit = if r.has_slash {
                r.matcher.is_match(path)
            } else {
                let name = path.rsplit('/').next().unwrap_or(path);
                r.matcher.is_match(name)
            };
            if hit {
                return &*r.driver;
            }
        }
        &LINE
    }
}
