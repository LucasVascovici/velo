//! Working out what `velo diff` was asked to compare.
//!
//! ```text
//! velo diff                  working tree vs the last snapshot
//! velo diff <file>           just that file, working tree vs last snapshot
//! velo diff <a>              snapshot <a> vs the working tree
//! velo diff <a> <b>          snapshot <a> vs snapshot <b>
//! velo diff <a>..<b>         same as `velo diff <a> <b>`
//! velo diff -- <paths>       restrict any of the above to paths
//! ```
//!
//! This lived in `velo-core` until it had no business being there: `a..b` syntax,
//! "is this argument a filename or a ref", and reading `.velo/PARENT` for an
//! implicit left-hand side are all facts about a command line. A consumer holding
//! two ids had to format them into a string so core could parse them back.
//!
//! Core now offers `diff::between(repo, a, b, paths)` and this module turns argv
//! into a call to it.

use std::path::Path;

use velo_core::commands::{self, diff::Diff};
use velo_core::{Error, Repo, SnapshotId};

/// Interpret `args` and produce the comparison.
pub fn dispatch(repo: &Repo, args: &[String], paths: &[String]) -> Result<Diff, Error> {
    let paths: Vec<&Path> = paths.iter().map(Path::new).collect();

    match args {
        [] => {
            if paths.is_empty() {
                commands::diff::run(repo, &None)
            } else {
                against_working_tree(repo, &position(repo)?, &paths, None)
            }
        }

        [one] => {
            if let Some((a, b)) = split_range(one) {
                let a = commands::resolve_snapshot_id(repo, a)?;
                let b = commands::resolve_snapshot_id(repo, b)?;
                let mut diff = commands::diff::between(repo, &a, Some(&b), &paths)?;
                relabel(&mut diff, Some(one), None);
                Ok(diff)
            } else if is_path_like(repo, one) {
                // A file: fold it in with any explicit pathspec.
                if paths.is_empty() {
                    commands::diff::run(repo, &Some(one.clone()))
                } else {
                    let mut all = vec![Path::new(one.as_str())];
                    all.extend_from_slice(&paths);
                    against_working_tree(repo, &position(repo)?, &all, None)
                }
            } else {
                // A snapshot / tag / branch: compare it against the working tree.
                let a = commands::resolve_snapshot_id(repo, one).map_err(|_| {
                    Error::invalid(format!(
                        "'{}' is neither a file nor a snapshot, tag, or branch.\n  \
                         To diff a path that doesn't exist any more, use: velo diff -- {}",
                        one, one
                    ))
                })?;
                against_working_tree(repo, &a, &paths, Some(one))
            }
        }

        [a, b] => {
            let a_id = commands::resolve_snapshot_id(repo, a)?;
            let b_id = commands::resolve_snapshot_id(repo, b)?;
            let mut diff = commands::diff::between(repo, &a_id, Some(&b_id), &paths)?;
            relabel(&mut diff, Some(a), Some(b));
            Ok(diff)
        }

        _ => Err(Error::invalid(
            "velo diff takes at most two snapshots. Put file paths after '--'.",
        )),
    }
}

fn against_working_tree(
    repo: &Repo,
    a: &SnapshotId,
    paths: &[&Path],
    label: Option<&str>,
) -> Result<Diff, Error> {
    let mut diff = commands::diff::between(repo, a, None, paths)?;
    relabel(&mut diff, label, None);
    Ok(diff)
}

/// Show what the user typed rather than the id it resolved to.
///
/// Core labels with the abbreviated id, because it has no idea a user wrote
/// `main`. Which words to print is presentation, so it is decided here.
fn relabel(diff: &mut Diff, old: Option<&str>, new: Option<&str>) {
    if let Some(old) = old {
        diff.old_label = format!("{} ({})", old, diff.old_label);
    }
    if let Some(new) = new {
        diff.new_label = format!("{} ({})", new, diff.new_label);
    }
}

/// The snapshot the working tree currently sits on.
fn position(repo: &Repo) -> Result<SnapshotId, Error> {
    let raw = std::fs::read_to_string(repo.root().join(".velo/PARENT")).unwrap_or_default();
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(Error::invalid(
            "Nothing has been saved yet, so there is nothing to compare against.",
        ));
    }
    commands::resolve_snapshot_id(repo, raw)
}

/// Split `a..b` into its ends. `None` when there is no range separator.
fn split_range(spec: &str) -> Option<(&str, &str)> {
    let (a, b) = spec.split_once("..")?;
    if a.is_empty() || b.is_empty() {
        None
    } else {
        Some((a, b))
    }
}

/// Does `arg` name something on disk, or a path the current snapshot tracked?
///
/// Checked before treating it as a ref: a short filename like `a` would
/// otherwise be swallowed by a hash prefix. `--` forces path interpretation.
fn is_path_like(repo: &Repo, arg: &str) -> bool {
    if repo.root().join(arg).exists() {
        return true;
    }
    // A path that was tracked but has since been deleted is still a path.
    match position(repo) {
        Ok(at) => commands::diff::tracks_path(repo, &at, Path::new(arg)),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use velo_testkit::TempRepo;

    /// Every argument form `velo diff` accepts.
    ///
    /// Moved here with `dispatch` itself: these assert how a *command line* is
    /// interpreted, which stopped being velo-core's business.
    #[test]
    fn every_argument_form_is_understood() {
        let t = TempRepo::new();
        t.write("a.py", "l1\nl2\n");
        let h1 = t.save("s1");
        t.write("a.py", "l1\nCHANGED\n");
        let h2 = t.save("s2");
        t.write("a.py", "l1\nWORKING\n");

        let repo = t.repo();
        let args = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();

        // Working tree vs the last snapshot, with and without a pathspec.
        dispatch(repo, &[], &[]).unwrap();
        dispatch(repo, &[], &args(&["a.py"])).unwrap();
        // A lone existing filename is a file, not a ref.
        dispatch(repo, &args(&["a.py"]), &[]).unwrap();
        // A lone ref: snapshot vs working tree, by hash and by name.
        dispatch(repo, &args(&[h1.as_str()]), &[]).unwrap();
        // Two refs, and the equivalent range syntax.
        dispatch(repo, &args(&[h1.as_str(), h2.as_str()]), &[]).unwrap();
        dispatch(repo, &args(&[&format!("{}..{}", h1, h2)]), &[]).unwrap();
        // Two refs restricted to a path.
        dispatch(repo, &args(&[h1.as_str(), h2.as_str()]), &args(&["a.py"])).unwrap();

        // Neither a file nor a ref is a clear error, not a silent empty diff.
        assert!(dispatch(repo, &args(&["no_such_thing"]), &[]).is_err());

        // More than two refs is refused rather than quietly ignoring the rest.
        assert!(dispatch(repo, &args(&["a", "b", "c"]), &[]).is_err());
    }

    /// A filename that could pass for a hash prefix is read as the file.
    ///
    /// Checking the filesystem first is what stops `velo diff a` being swallowed
    /// by a snapshot whose id happens to begin with `a`.
    #[test]
    fn a_bare_filename_wins_over_a_hash_prefix() {
        let t = TempRepo::new();
        t.write("a", "one\n");
        t.save("c1");
        t.write("a", "two\n");

        let diff = dispatch(t.repo(), &["a".to_string()], &[]).unwrap();
        assert!(!diff.is_empty(), "it compared the file, not nothing");
    }

    /// The label shows what the user typed, not the id it resolved to.
    #[test]
    fn labels_show_the_spec_the_user_gave() {
        let t = TempRepo::new();
        t.write("f.txt", "one\n");
        let h1 = t.save("s1");
        t.write("f.txt", "two\n");
        let h2 = t.save("s2");

        let diff = dispatch(t.repo(), &[h1.to_string(), h2.to_string()], &[]).unwrap();
        assert!(
            diff.old_label.starts_with(h1.as_str()),
            "got {}",
            diff.old_label
        );
        // Core labels with the short id; the spec is prepended here.
        assert!(diff.old_label.contains(h1.short()));
    }
}
