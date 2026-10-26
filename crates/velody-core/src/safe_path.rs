//! One definition of "a path velody will store, and a path velody will write".
//!
//! A snapshot id commits to whatever rows it was given, so a hostile source
//! (a crafted bundle, a git stream, a careless caller) can produce a valid
//! snapshot whose tree names `../escape` or `.velo/config`. The id cannot tell
//! us; this module can. Two rules exist, deliberately separate:
//!
//! * the **store rule** ([`check`]) is the same on every platform, so a tree
//!   that one machine accepted is accepted by all of them;
//! * the **platform rule** ([`check_for_this_platform`]) adds what the local
//!   filesystem cannot represent. A Linux repository holding `aux.c` is fine on
//!   Linux and is refused only when written out on Windows.
//!
//! The rule is applied where an entry becomes a tree row (`save_tree`, bundle
//! import) and again, over the whole target tree and before anything is
//! written, by every working-tree writer. Repositories that already hold a
//! refused path stay readable; only writing it out is refused.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::error::Error;

/// Why a path was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathProblem {
    /// The path is empty.
    Empty,
    /// The path starts at the filesystem root.
    Absolute,
    /// The path starts with a drive letter such as `C:`.
    DrivePrefix,
    /// The path has an empty component (`a//b`, or a trailing slash).
    EmptyComponent,
    /// The path has a `.` component.
    DotComponent,
    /// The path has a `..` component.
    DotDotComponent,
    /// The path is inside `.velo`, the repository's own directory.
    RepositoryDirectory,
    /// The path contains a NUL byte.
    NulByte,
    /// A component is a Windows device name such as `CON` or `NUL`.
    WindowsReservedName,
    /// A component holds a character Windows cannot have in a name.
    WindowsInvalidCharacter,
    /// A component ends in a dot or a space, which Windows strips.
    TrailingDotOrSpace,
    /// The path is a file in one entry and a directory in another.
    FileAndDirectory {
        /// The other path involved.
        other: String,
    },
    /// A symlink entry points outside the working tree.
    SymlinkEscapes {
        /// What the link points at.
        target: String,
    },
    /// A directory on the way to the path is a symlink in the working tree.
    ThroughSymlink {
        /// The symlink, relative to the root.
        link: String,
    },
}

impl fmt::Display for PathProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathProblem::Empty => f.write_str("the path is empty"),
            PathProblem::Absolute => f.write_str("the path is absolute"),
            PathProblem::DrivePrefix => f.write_str("the path starts with a drive letter"),
            PathProblem::EmptyComponent => f.write_str("the path has an empty component"),
            PathProblem::DotComponent => f.write_str("the path has a '.' component"),
            PathProblem::DotDotComponent => f.write_str("the path has a '..' component"),
            PathProblem::RepositoryDirectory => {
                f.write_str("the path is inside the repository directory '.velo'")
            }
            PathProblem::NulByte => f.write_str("the path contains a NUL byte"),
            PathProblem::WindowsReservedName => {
                f.write_str("the path names a reserved Windows device")
            }
            PathProblem::WindowsInvalidCharacter => {
                f.write_str("the path has a character Windows does not allow in a name")
            }
            PathProblem::TrailingDotOrSpace => {
                f.write_str("a component of the path ends in a dot or a space")
            }
            PathProblem::FileAndDirectory { other } => {
                write!(f, "it is a file here and a directory in '{other}'")
            }
            PathProblem::SymlinkEscapes { target } => {
                write!(
                    f,
                    "it is a symlink pointing outside the repository ('{target}')"
                )
            }
            PathProblem::ThroughSymlink { link } => {
                write!(f, "it passes through the symlink '{link}'")
            }
        }
    }
}

fn is_repo_dir(component: &str) -> bool {
    component
        .trim_end_matches(['.', ' '])
        .eq_ignore_ascii_case(".velo")
}

/// The store rule: which paths velody will record in a tree, on any platform.
///
/// The input is already normalised (forward slashes).
pub fn check(path: &str) -> Result<(), PathProblem> {
    if path.is_empty() {
        return Err(PathProblem::Empty);
    }
    if path.contains('\0') {
        return Err(PathProblem::NulByte);
    }
    if path.starts_with('/') {
        return Err(PathProblem::Absolute);
    }
    for (i, component) in path.split('/').enumerate() {
        let drive = i == 0
            && component.len() == 2
            && component.ends_with(':')
            && component.as_bytes()[0].is_ascii_alphabetic();
        if drive {
            return Err(PathProblem::DrivePrefix);
        }
        match component {
            "" => return Err(PathProblem::EmptyComponent),
            "." => return Err(PathProblem::DotComponent),
            ".." => return Err(PathProblem::DotDotComponent),
            c if is_repo_dir(c) => return Err(PathProblem::RepositoryDirectory),
            _ => {}
        }
    }
    Ok(())
}

/// The store rule plus whatever the local filesystem cannot hold.
///
/// On Windows that is device names (`CON`, `nul.txt`), the characters
/// `<>:"|?*\` and control characters (`:` also covers alternate data streams),
/// names ending in a dot or space, and the 8.3 alias of `.velo`. Elsewhere it
/// is the store rule alone.
pub fn check_for_this_platform(path: &str) -> Result<(), PathProblem> {
    check(path)?;
    #[cfg(windows)]
    windows_rule(path)?;
    Ok(())
}

#[cfg(windows)]
fn windows_rule(path: &str) -> Result<(), PathProblem> {
    for component in path.split('/') {
        if component
            .chars()
            .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\') || c < ' ')
        {
            return Err(PathProblem::WindowsInvalidCharacter);
        }
        if component.ends_with('.') || component.ends_with(' ') {
            return Err(PathProblem::TrailingDotOrSpace);
        }
        let stem = component
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end_matches(' ')
            .to_ascii_uppercase();
        let numbered = |prefix: &str| {
            stem.strip_prefix(prefix)
                .is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']))
        };
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || numbered("COM")
            || numbered("LPT")
        {
            return Err(PathProblem::WindowsReservedName);
        }
        if component.eq_ignore_ascii_case("velo~1") {
            return Err(PathProblem::RepositoryDirectory);
        }
    }
    Ok(())
}

/// Every path passes [`check`], and no path is also a directory of another.
///
/// Returns the offending path with the problem.
pub(crate) fn check_tree<'a>(
    paths: impl IntoIterator<Item = &'a str>,
) -> Result<(), (String, PathProblem)> {
    let mut all: Vec<&str> = Vec::new();
    for p in paths {
        check(p).map_err(|e| (p.to_string(), e))?;
        all.push(p);
    }
    // Every directory prefix of every path is looked up in the set of paths.
    // A sort-and-compare-neighbours scheme would miss `a`, `a.txt`, `a/b`,
    // because `.` sorts before `/`.
    let set: std::collections::HashSet<&str> = all.iter().copied().collect();
    for p in &all {
        let mut end = 0;
        while let Some(i) = p[end..].find('/') {
            end += i;
            let prefix = &p[..end];
            if set.contains(prefix) {
                return Err((
                    (*p).to_string(),
                    PathProblem::FileAndDirectory {
                        other: prefix.to_string(),
                    },
                ));
            }
            end += 1;
        }
    }
    Ok(())
}

fn unsafe_path(path: &str, problem: PathProblem) -> Error {
    Error::UnsafePath {
        path: path.to_string(),
        problem,
    }
}

/// Join `rel` under `root` for writing, refusing a path the platform rule
/// forbids or one that passes through a symlink in the working tree.
///
/// The symlink walk is git's "beyond a symbolic link" check: a directory that
/// is really a link would carry the write outside the root.
pub(crate) fn join_checked(root: &Path, rel: &str) -> crate::Result<PathBuf> {
    join_checked_replacing(root, rel, &HashSet::new())
}

/// [`join_checked`] for a write that follows the removal of `replaced`: a
/// symlink in `replaced` is about to be deleted, so a path below it is judged
/// as if the link were already gone. This is how a restore turns a tracked
/// symlink `a` into a directory holding `a/b`.
pub(crate) fn join_checked_replacing(
    root: &Path,
    rel: &str,
    replaced: &HashSet<String>,
) -> crate::Result<PathBuf> {
    check_for_this_platform(rel).map_err(|p| unsafe_path(rel, p))?;
    let full = root.join(crate::db::db_to_path(rel));
    let mut current = root.to_path_buf();
    let mut walked = String::new();
    let components: Vec<&str> = rel.split('/').collect();
    for component in &components[..components.len() - 1] {
        current.push(component);
        if !walked.is_empty() {
            walked.push('/');
        }
        walked.push_str(component);
        match std::fs::symlink_metadata(&current) {
            // Nothing below a link that is being removed will exist.
            Ok(m) if m.file_type().is_symlink() && replaced.contains(&walked) => break,
            Ok(m) if m.file_type().is_symlink() => {
                return Err(unsafe_path(
                    rel,
                    PathProblem::ThroughSymlink { link: walked },
                ));
            }
            Ok(_) => {}
            // Nothing below a missing directory exists, so nothing can be a link.
            Err(_) => break,
        }
    }
    Ok(full)
}

/// Refuse a symlink whose target is absolute or climbs above the root when
/// resolved from the link's own directory.
pub(crate) fn check_symlink_target(rel: &str, target: &str) -> Result<(), PathProblem> {
    let escapes = || PathProblem::SymlinkEscapes {
        target: target.to_string(),
    };
    let bytes = target.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if target.starts_with('/') || target.starts_with('\\') || drive {
        return Err(escapes());
    }
    let mut depth = rel.split('/').count() as i64 - 1;
    for part in target.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return Err(escapes());
                }
            }
            _ => depth += 1,
        }
    }
    Ok(())
}

/// Validate everything a working-tree writer is about to do, before it does
/// any of it.
///
/// `tree` is the whole target tree (for file/directory exclusivity); `writing`
/// is what will actually be written, as `(path, object, mode)`. Symlink
/// targets live in the object, so `read` fetches it.
pub(crate) fn validate_write_set<'a>(
    root: &Path,
    tree: impl IntoIterator<Item = &'a str>,
    writing: impl IntoIterator<Item = (&'a str, &'a str, i64)>,
    read: impl Fn(&str) -> crate::Result<Vec<u8>>,
) -> crate::Result<()> {
    validate_write_set_replacing(root, tree, writing, read, &HashSet::new())
}

/// [`validate_write_set`] for a write that follows the removal of `replaced`
/// (see [`join_checked_replacing`]).
pub(crate) fn validate_write_set_replacing<'a>(
    root: &Path,
    tree: impl IntoIterator<Item = &'a str>,
    writing: impl IntoIterator<Item = (&'a str, &'a str, i64)>,
    read: impl Fn(&str) -> crate::Result<Vec<u8>>,
    replaced: &HashSet<String>,
) -> crate::Result<()> {
    check_tree(tree).map_err(|(p, e)| unsafe_path(&p, e))?;
    for (rel, hash, mode) in writing {
        join_checked_replacing(root, rel, replaced)?;
        if mode == crate::storage::MODE_SYMLINK {
            let target = read(hash)?;
            let target = String::from_utf8_lossy(&target);
            check_symlink_target(rel, &target).map_err(|e| unsafe_path(rel, e))?;
        }
    }
    Ok(())
}

/// Check the paths a bundle or importer is about to turn into rows.
pub(crate) fn check_rows<'a>(paths: impl IntoIterator<Item = &'a str>) -> crate::Result<()> {
    check_tree(paths).map_err(|(p, e)| unsafe_path(&p, e))
}

/// Check one path, naming it in the error.
pub(crate) fn check_named(path: &str) -> crate::Result<()> {
    check(path).map_err(|e| unsafe_path(path, e))
}
