//! Where the CLI finds out who is saving.
//!
//! `velo-core` reads no process environment — that is a stated boundary, enforced
//! by lints — so the author has to be discovered out here and passed in.
//!
//! There is no config file yet. Environment variables are the smallest thing that
//! works, they are what CI and scripts already reach for, and they cost nothing to
//! replace later with a config file that falls back to them.
//!
//! An absent author is not an error. Velo has always recorded snapshots without
//! one, and refusing to save until someone sets a variable would be a poor trade
//! for a tool that is useful single-player.

use velo_core::Author;

/// The author's name.
const NAME: &str = "VELO_AUTHOR_NAME";
/// The author's email, optional even when a name is set.
const EMAIL: &str = "VELO_AUTHOR_EMAIL";

/// Who to record for a snapshot, or `None` when nothing says.
///
/// A malformed value is reported rather than silently dropped: someone who set
/// the variable meant it, and quietly ignoring it would produce history missing
/// exactly the attribution they asked for.
pub fn from_env() -> Result<Option<Author>, velo_core::Error> {
    let name = match std::env::var(NAME) {
        Ok(name) if !name.trim().is_empty() => name,
        _ => return Ok(None),
    };
    let name = name.trim();

    match std::env::var(EMAIL) {
        Ok(email) if !email.trim().is_empty() => Author::with_email(name, email.trim()).map(Some),
        _ => Author::new(name).map(Some),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Environment variables are process-global, so these run as one test rather
    /// than racing each other across threads.
    #[test]
    fn reads_name_and_optional_email() {
        // Safety: single-threaded within this test, and every branch restores.
        std::env::remove_var(NAME);
        std::env::remove_var(EMAIL);
        assert_eq!(from_env().unwrap(), None, "nothing set means no author");

        std::env::set_var(NAME, "Ada Lovelace");
        let author = from_env().unwrap().unwrap();
        assert_eq!(author.name(), "Ada Lovelace");
        assert_eq!(author.email(), None);
        assert_eq!(author.to_string(), "Ada Lovelace");

        std::env::set_var(EMAIL, "ada@example.com");
        let author = from_env().unwrap().unwrap();
        assert_eq!(author.email(), Some("ada@example.com"));
        assert_eq!(author.to_string(), "Ada Lovelace <ada@example.com>");

        // Whitespace is trimmed rather than rejected — a trailing space in a
        // shell export is a typo, not an intent.
        std::env::set_var(NAME, "  Ada Lovelace  ");
        assert_eq!(from_env().unwrap().unwrap().name(), "Ada Lovelace");

        // An empty variable is the same as unset.
        std::env::set_var(NAME, "   ");
        assert_eq!(from_env().unwrap(), None);

        // A value that cannot be an author is reported, not dropped.
        std::env::set_var(NAME, "bad\nname");
        assert!(from_env().is_err());

        std::env::remove_var(NAME);
        std::env::remove_var(EMAIL);
    }
}
