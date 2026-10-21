#![deny(unsafe_code)]

use clap::{builder::styling, Parser, Subcommand};

// All repository logic lives in `velo-core`; this crate is the presentation
// layer — argument parsing, rendering, and exit codes.
mod author;
mod diffargs;
#[cfg(test)]
mod help_check;
mod lockwait;
mod render;

use std::path::{Path, PathBuf};

use velo_core::{commands, error, serve, BranchName, TagName};

use error::{Result, VeloError};
use velo_core::commands::resolve::TakeOption;

/// `--objects` as parsed from the command line; mirrors core's
/// `ObjectLocation`, which carries no clap derive.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum ObjectsArg {
    Files,
    Database,
}

impl From<ObjectsArg> for velo_core::ObjectLocation {
    fn from(a: ObjectsArg) -> Self {
        match a {
            ObjectsArg::Files => velo_core::ObjectLocation::Files,
            ObjectsArg::Database => velo_core::ObjectLocation::Database,
        }
    }
}

/// `--take` as parsed from the command line. Core's `TakeOption` deliberately
/// carries no clap derive, so the CLI owns its own parseable mirror.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum TakeArg {
    Ours,
    Theirs,
    Both,
}

impl From<TakeArg> for TakeOption {
    fn from(a: TakeArg) -> Self {
        match a {
            TakeArg::Ours => TakeOption::Ours,
            TakeArg::Theirs => TakeOption::Theirs,
            TakeArg::Both => TakeOption::Both,
        }
    }
}

// ─── Custom colour scheme for --help output ───────────────────────────────────

fn styles() -> styling::Styles {
    styling::Styles::styled()
        .header(styling::AnsiColor::Yellow.on_default().bold())
        .usage(styling::AnsiColor::Yellow.on_default().bold())
        .literal(styling::AnsiColor::Cyan.on_default().bold())
        .placeholder(styling::AnsiColor::Green.on_default())
        .error(styling::AnsiColor::Red.on_default().bold())
        .valid(styling::AnsiColor::Cyan.on_default())
        .invalid(styling::AnsiColor::Red.on_default())
}

// ─── Root command ─────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    verbatim_doc_comment,
    name    = "velo",
    version = env!("CARGO_PKG_VERSION"),
    styles  = styles(),
    about   = "Velo — fast, safe, intuitive version control.",
    long_about = "\
Velo is a version control system built for everyday developers.
It keeps what Git does right (snapshots, branching, hashing, compression)
and replaces what it gets wrong (staging area, cryptic commands, data loss).

Key differences from Git
  · No staging area — what's on disk is what gets saved
  · Conflicts as data — your files stay valid during a merge
  · Undo/redo — remove or restore snapshots with one command
  · Stash shelves — named, not cryptic stash@{2} indices
  · True 3-way merges — no false conflicts on one-sided changes

Quick start
  velo init
  velo save \"Initial commit\"
  velo status
  velo history",
    after_help = "Run `velo help <COMMAND>` for detailed usage of any command.

EXIT CODES
  0  success (including nothing to save, already up to date, nothing to pull,
     nothing to undo, nothing to redo)
  1  error (including a name that does not resolve)
  2  command-line usage error, and nothing else
  3  conflicts left to resolve (merge, cherry-pick or rebase stopped)
  4  histories diverged (pull, or a push the remote refused)
  5  repository locked by another velo process",
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

// ─── Subcommands ──────────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum Commands {
    /// Initialise a new repository in the current directory.
    ///
    /// Creates a .velo/ directory with an object store and a SQLite database.
    /// A default .veloignore is written if one does not already exist.
    /// Running `init` inside an existing Velo repo is an error.
    ///
    /// By default each object is a file under .velo/objects. With
    /// `--objects database` objects live inside velo.db instead: no file per
    /// object, which is the layout the browser build uses. It can be faster
    /// where every file open is scanned (Windows Defender). The choice is
    /// fixed for the repository's lifetime and cannot be changed later.
    ///
    /// Example
    ///   velo init
    ///   velo init --objects database
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · .velo/ is never tracked — it is automatically excluded.
    · The default branch is called 'main'.
    · Edit .veloignore to exclude build artefacts, secrets, etc."
    )]
    Init {
        /// Where objects are kept: files (default) or database.
        #[arg(long, value_enum, default_value = "files")]
        objects: ObjectsArg,
    },

    /// Snapshot the working directory with a message.
    ///
    /// Every tracked file (respecting .veloignore / .gitignore) is
    /// hashed with BLAKE3 in parallel, compressed with Zstd, and
    /// stored in the content-addressed object store.  Only changed
    /// files produce new objects; unchanged files are referenced by
    /// pointer (delta storage).
    ///
    /// Examples
    ///   velo save "Fix login bug"
    ///   velo save --amend                  # fold changes in, keep the message
    ///   velo save "Better wording" --amend # ...and reword it
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · The message is required, except with --amend, which reuses the
      amended snapshot's message when you don't give a new one.
    · --amend replaces the previous snapshot in-place and keeps the
      same parent, preserving a linear history.  Objects from the
      replaced snapshot are cleaned up by `velo gc`."
    )]
    Save {
        /// Short description of what changed. Optional with --amend, which
        /// then keeps the existing message.
        #[arg(value_name = "MESSAGE")]
        message: Option<String>,

        /// Replace the most recent snapshot on this branch instead of
        /// creating a new one.  Useful to fix a typo or include a
        /// missed file without polluting history.
        #[arg(
            long,
            help = "Amend the most recent snapshot instead of creating a new one"
        )]
        amend: bool,

        /// Only snapshot these paths (relative to repo root).
        /// Other changed files are left as unsaved changes.
        #[arg(last = true, value_name = "PATH")]
        paths: Vec<String>,
    },

    /// Restore the working directory to a past snapshot.
    ///
    /// Accepts a full hash, a unique prefix, or a tag name.
    /// Ghost files (present in the working tree but absent from the
    /// target snapshot) are removed.  Empty directories left behind
    /// are cleaned up automatically.
    ///
    /// Examples
    ///   velo restore abc123ef            # by hash prefix
    ///   velo restore v1.0                # by tag
    ///   velo restore abc123ef --force    # discard unsaved changes
    ///   velo restore abc123ef -- src/    # restore only src/ directory
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Without --force, restore aborts if there are unsaved changes.
    · When paths are given (-- <path>…), only those files are written
      and PARENT is not updated — use this for surgical file-level reverts.
    · Restore via tag: first create a tag with `velo tag <name>`."
    )]
    Restore {
        /// Hash (or prefix), or tag name to restore to.
        #[arg(value_name = "TARGET")]
        target: String,

        /// Discard unsaved changes without prompting.
        #[arg(short, long, help = "Overwrite unsaved changes without prompting")]
        force: bool,

        /// Restore only these paths (relative to repo root).
        /// When set, PARENT is not updated.
        #[arg(last = true, value_name = "PATH")]
        paths: Vec<String>,
    },

    /// Show the working tree status.
    ///
    /// Lists new, modified, and deleted files compared to the last
    /// snapshot.  Files matching .veloignore / .gitignore are excluded.
    /// If a merge is in progress, conflict files are highlighted.
    ///
    /// Examples
    ///   velo status
    ///   velo status -- src/    # only show src/ changes
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Velo uses an mtime+size cache to skip rehashing unchanged files.
      The first call after a large change is slower; subsequent calls
      on an unchanged tree are essentially free (stat only)."
    )]
    Status {
        /// Restrict output to these paths.
        #[arg(last = true, value_name = "PATH")]
        paths: Vec<String>,
    },

    /// Show snapshot history.
    ///
    /// Without flags, shows the ancestry of the current snapshot on
    /// the current branch.  Use --all for cross-branch history.
    ///
    /// Examples
    ///   velo history                          # current branch
    ///   velo history --all                    # all branches
    ///   velo history --branch feature/auth    # specific branch (no switch needed)
    ///   velo history --file src/auth.py       # snapshots that touched this file
    ///   velo history --oneline                # compact format
    ///   velo history --graph                  # ASCII branch graph
    ///   velo history --limit 50               # show up to 50 entries
    #[command(
        verbatim_doc_comment,
        name = "history",
        alias = "log",
        after_help = "\
NOTES
    · --file filters by any path prefix, so --file src/ matches all
      files under src/.
    · --graph is best combined with --oneline for compact output."
    )]
    History {
        /// Show history across all branches (not just the current one).
        #[arg(short, long, help = "Show history across all branches")]
        all: bool,

        /// Maximum number of snapshots to display.
        #[arg(
            short,
            short_alias = 'n',
            long,
            default_value_t = 20,
            value_name = "N",
            help = "Maximum number of entries to show"
        )]
        limit: usize,

        /// Show history for a specific branch without switching to it.
        #[arg(
            short,
            long,
            value_name = "BRANCH",
            help = "Filter to a specific branch"
        )]
        branch: Option<String>,

        /// Ancestry of one snapshot, rather than one branch's snapshots.
        #[arg(
            long,
            value_name = "SNAPSHOT",
            conflicts_with_all = ["all", "branch"],
            help = "Show what led to a snapshot, across branches"
        )]
        from: Option<String>,

        /// Compact one-line format: hash  branch  message.
        #[arg(long, help = "One-line-per-snapshot compact format")]
        oneline: bool,

        /// Draw an ASCII branch/merge graph alongside the log.
        #[arg(long, help = "Show ASCII graph of branch topology")]
        graph: bool,

        /// Show only snapshots that changed the given files or directories.
        ///
        /// Repeatable: a document and its assets are more than one path.
        #[arg(
            long = "file",
            value_name = "PATH",
            help = "Filter to snapshots that modified PATH (repeatable)"
        )]
        file_filter: Vec<String>,

        /// Show only snapshots whose metadata matches.
        ///
        /// Repeatable; every condition must hold.
        #[arg(
            long = "where",
            value_name = "NS:KEY[=VALUE]",
            help = "Filter by snapshot metadata, NS:KEY=VALUE or NS:KEY (repeatable)"
        )]
        meta_filter: Vec<String>,
    },

    /// Remove the most recent snapshot on the current branch.
    ///
    /// The snapshot is moved to a recoverable trash table (not
    /// permanently deleted) and the working tree is rewound to
    /// the previous state.  Use `velo redo` to re-apply it.
    ///
    /// Example
    ///   velo undo
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Undo aborts if there are unsaved changes.
    · Undone snapshots are stored in an internal trash table and can
      be recovered with `velo redo` until `velo gc` purges them.
    · Undoing the very first snapshot clears the working tree."
    )]
    Undo,

    /// Re-apply the most recently undone snapshot.
    ///
    /// Only available after `velo undo` and only until a new `velo save`
    /// invalidates the redo stack.
    ///
    /// Example
    ///   velo redo
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Redo is cleared the moment you run `velo save` — once you
      diverge, there is nothing to redo.
    · Redo refuses if the branch moved since the undo (a pull or
      merge fast-forwarded it); `velo restore <id>` still shows the
      undone snapshot.
    · Redo aborts if there are unsaved changes."
    )]
    Redo,

    /// Show line-level changes — between snapshots, or against the working tree.
    ///
    /// One command covers every comparison. A lone argument is treated as a file
    /// when one exists by that name, otherwise as a snapshot, tag, or branch.
    ///
    /// Examples
    ///   velo diff                        # working tree vs the last snapshot
    ///   velo diff src/auth.py            # just that file
    ///   velo diff v1.0                   # snapshot vs the working tree
    ///   velo diff v1.0 main              # snapshot vs snapshot
    ///   velo diff v1.0..main             # same, range syntax
    ///   velo diff -- src/ tests/         # restrict to paths
    ///   velo diff v1.0 main -- src/      # both at once
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Accepts a full hash, unique prefix, tag, branch, or remote ref (origin/main).
    · Binary files are detected automatically and their diffs are omitted.
    · Diff output uses unified format with 3 lines of context per hunk.
    · To inspect merge conflicts, use `velo conflicts <file>`."
    )]
    Diff {
        /// Snapshot(s) to compare, or a single file. At most two.
        #[arg(value_name = "REF_OR_FILE", num_args = 0..=2)]
        args: Vec<String>,

        /// Restrict the diff to these paths.
        #[arg(last = true, value_name = "PATH")]
        paths: Vec<String>,
    },

    /// Inspect a snapshot without restoring the working tree.
    ///
    /// Prints the snapshot metadata and a full diff vs its parent.
    /// Accepts a hash, prefix, or tag name.
    ///
    /// Examples
    ///   velo show abc123ef          # full diff for this snapshot
    ///   velo show v1.0              # diff for the tagged snapshot
    ///   velo show abc123ef -- src/  # restrict diff to src/
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Nothing on disk is changed — show is entirely read-only.
    · Use `velo restore <target> -- <file>` to pull a single file out
      of a historical snapshot into your working tree."
    )]
    Show {
        /// Hash (or prefix), or tag name of the snapshot to inspect.
        #[arg(value_name = "TARGET")]
        target: String,

        /// Restrict the diff output to this file or directory prefix.
        #[arg(last = true, value_name = "PATH")]
        paths: Vec<String>,
    },

    /// Generate a signing key and print its public half.
    ///
    /// Writes a fresh 32-byte Ed25519 seed, as lowercase hex, to FILE and
    /// prints the matching public key on stdout.  Works anywhere; it does
    /// not need a repository.
    ///
    /// Examples
    ///   velo keygen --out ~/.velo-signing-key
    ///   velo keygen --out key.txt
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · FILE is a secret: velo never stores, uploads or manages it.
      Keep it out of the repository and out of backups you share.
    · An existing FILE is never overwritten.
    · Give the public key to whoever should trust your signatures;
      they put it in the trust file that velo verify reads."
    )]
    Keygen {
        /// File to write the secret key to (created, mode 0600 on Unix).
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
    },

    /// Sign a snapshot with a key file.
    ///
    /// The signature covers the snapshot id, so it vouches for the whole
    /// tree, the parents, the message, the time, the metadata and the
    /// author.  Defaults to the snapshot you are on.
    ///
    /// Examples
    ///   velo sign --key ~/.velo-signing-key
    ///   velo sign v1.0 --key ~/.velo-signing-key
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Create a key with velo keygen.
    · Signing the same snapshot twice with the same key changes nothing.
    · Signatures never change a snapshot's id."
    )]
    Sign {
        /// Hash (or prefix), tag or branch to sign. Defaults to the current snapshot.
        #[arg(value_name = "SNAPSHOT")]
        snapshot: Option<String>,

        /// Secret key file written by velo keygen.
        #[arg(long, value_name = "FILE")]
        key: PathBuf,
    },

    /// Check a snapshot's signatures against a list of trusted keys.
    ///
    /// The trust file holds one public key (64 hex characters) per line;
    /// blank lines and lines starting with # are ignored.  Exits 0 when
    /// a valid signature comes from a trusted key and none is invalid;
    /// otherwise prints why and exits 1.
    ///
    /// Examples
    ///   velo verify --trust trusted.keys
    ///   velo verify v1.0 --trust trusted.keys
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Velo verifies; you decide whom to trust.  There is no key
      registry, revocation or web of trust.
    · Nothing on disk is changed — verify is entirely read-only."
    )]
    Verify {
        /// Hash (or prefix), tag or branch to check. Defaults to the current snapshot.
        #[arg(value_name = "SNAPSHOT")]
        snapshot: Option<String>,

        /// File of trusted public keys, one per line.
        #[arg(long, value_name = "FILE")]
        trust: PathBuf,
    },

    /// Write the repository's audit report as one offline HTML file.
    ///
    /// The report has a verdict from fsck, a timeline per branch (id, time,
    /// author, message, metadata, signature badges, changed files) and the
    /// line provenance of the files those snapshots changed.  --json prints
    /// the same data as JSON instead.
    ///
    /// Examples
    ///   velo report
    ///   velo report --branch main --since 7d -o audit.html
    ///   velo report --json --trust trusted.keys
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · The file is static: no script, no remote resources, a strict Content-Security-Policy.  It is safe to open offline, even when the history it shows is hostile; every message, name and line is escaped.
    · --since takes YYYY-MM-DD, an RFC 3339 timestamp, or an age such as 12h, 7d or 2w.
    · --trust FILE is the key file velo verify reads; without it valid signatures show as untrusted.
    · A repository with integrity problems still gets a report, with a red verdict.
    · Nothing on disk is changed except the report file."
    )]
    Report {
        /// Only this branch (default: every branch).
        #[arg(long, value_name = "BRANCH", help = "Only this branch (default: all)")]
        branch: Option<String>,
        /// Only snapshots from this time on.
        #[arg(
            long,
            value_name = "WHEN",
            value_parser = parse_since_arg,
            help = "Only snapshots since YYYY-MM-DD, an RFC 3339 time, or <n>h|d|w ago"
        )]
        since: Option<i64>,
        /// Where to write the report.
        #[arg(
            short = 'o',
            long = "output",
            value_name = "FILE",
            help = "Output file (default: velo-report.html; with --json, stdout)"
        )]
        output: Option<PathBuf>,
        /// Emit JSON instead of HTML.
        #[arg(long, help = "Emit the same data as JSON")]
        json: bool,
        /// File of trusted public keys, one per line.
        #[arg(
            long,
            value_name = "FILE",
            help = "Trusted public keys, as velo verify --trust"
        )]
        trust: Option<PathBuf>,
        /// Skip the integrity check.
        #[arg(long, help = "Do not run fsck (the verdict reads 'not checked')")]
        no_fsck: bool,
    },

    /// Apply the changes from one snapshot onto the current branch.
    ///
    /// Uses 3-way merge logic: the snapshot's parent acts as the
    /// common ancestor.  Changes that only exist in the cherry-picked
    /// snapshot are applied cleanly.  True conflicts are recorded for
    /// `velo resolve`, just like `velo merge`.
    ///
    /// When there are no conflicts, the result is auto-saved as a new
    /// snapshot so the command is self-contained.
    ///
    /// Example
    ///   velo cherry-pick abc123ef
    #[command(
        verbatim_doc_comment,
        name = "cherry-pick",
        after_help = "\
NOTES
    · Cherry-pick aborts if there are unsaved changes.
    · With conflicts: resolve them, then `velo save \"Apply cherry-pick\"`.
    · Without conflicts: a new snapshot is created automatically."
    )]
    CherryPick {
        /// Hash (or prefix), or tag name of the snapshot to apply.
        #[arg(value_name = "TARGET")]
        target: String,
    },

    /// Switch to a branch (creates it if it does not exist).
    ///
    /// A name that is a near miss of an existing branch (`mian` for `main`)
    /// is not created silently: a terminal is asked, a script is refused with
    /// exit code 2.  --create (-c) creates the branch regardless.
    ///
    /// Restores the working tree to the latest snapshot on the target
    /// branch.  Aborts if there are unsaved changes unless --force is used.
    ///
    /// Examples
    ///   velo switch feature/auth    # switch (creates if new)
    ///   velo switch main --force    # discard unsaved changes and switch
    ///   velo switch --create mian   # create it even though `main` exists
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · New branches inherit the current working tree state.
    · Switch to a deleted branch is not permitted.
    · --force discards unsaved changes — they cannot be recovered."
    )]
    Switch {
        /// Branch name to switch to (or create).
        #[arg(value_name = "NAME")]
        name: String,

        /// Discard unsaved changes without prompting.
        #[arg(short, long, help = "Discard unsaved changes and switch")]
        force: bool,

        /// Create the branch; skips the near-miss check on the name.
        #[arg(short, long, help = "Create the branch (error if it exists)")]
        create: bool,
    },

    /// List branches, or delete one.
    ///
    /// Each branch is shown with its most recent snapshot hash, date,
    /// and message.  The current branch is highlighted with an asterisk.
    ///
    /// Examples
    ///   velo branches
    ///   velo branches --delete feature/old
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Branch deletion is a soft delete — history is preserved in the
      database and purged only by `velo gc`.
    · The current branch and 'main' cannot be deleted.
    · Deleted branches are hidden from all listings."
    )]
    Branches {
        /// Delete this branch (soft delete; history is preserved until gc).
        #[arg(short, long, value_name = "NAME", help = "Delete the named branch")]
        delete: Option<String>,
    },

    /// Create, list, or delete tags.
    ///
    /// Tags are persistent labels pointing to a specific snapshot.
    /// They can be used anywhere a hash is accepted (restore, show,
    /// cherry-pick, history, etc.).
    ///
    /// Examples
    ///   velo tag                         # list all tags
    ///   velo tag v1.0                    # tag the current snapshot
    ///   velo tag v1.0 abc123ef           # tag a specific snapshot
    ///   velo tag v1.0 --force            # overwrite an existing tag
    ///   velo tag --delete v1.0           # delete a tag
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Deleting a tag does not affect the snapshot it pointed to.
    · A tag can point to any snapshot across all branches."
    )]
    Tag {
        /// Tag name to create.
        #[arg(value_name = "NAME", help = "Tag name to create")]
        name: Option<String>,

        /// Snapshot hash, prefix, or existing tag to attach the new tag to.
        /// Defaults to the current snapshot (HEAD) if omitted.
        #[arg(value_name = "TARGET", help = "Snapshot to tag (defaults to HEAD)")]
        snapshot: Option<String>,

        /// Delete the named tag.
        #[arg(
            short,
            long,
            value_name = "NAME",
            conflicts_with = "name",
            help = "Delete a tag by name"
        )]
        delete: Option<String>,

        /// Overwrite an existing tag with the same name.
        #[arg(short, long, help = "Overwrite an existing tag without error")]
        force: bool,
    },

    /// Merge another branch into the current one.
    ///
    /// Velo performs a true 3-way merge using the lowest common
    /// ancestor (LCA) of the two branch tips.  A file modified only
    /// on one side since the ancestor is never flagged as a conflict.
    ///
    /// Conflicts are recorded in the repository and your files are left
    /// valid and runnable during the resolution process.
    ///
    /// Examples
    ///   velo merge feature/payments    # merge into current branch
    ///   velo merge --abort             # discard in-progress merge
    #[command(
        verbatim_doc_comment,
        after_help = "\
CONFLICT RESOLUTION WORKFLOW
    1. velo merge <branch>
    2. velo conflicts <file>          # see both sides of each conflict
    3. velo resolve <file> --take ours|theirs|both
       — or edit the file, then `velo resolve --mark <file>`
    4. velo save \"Merge <branch>\"

NOTES
    · Merge aborts if there are unsaved changes.
    · Fast-forward merges (linear ancestry) are handled automatically.
    · --abort restores the working tree to its exact pre-merge state and
      clears all conflict data — works even after all conflicts are resolved."
    )]
    Merge {
        /// Branch to merge into the current branch.
        #[arg(value_name = "BRANCH", help = "Branch to merge in")]
        branch: Option<String>,

        /// Abort an in-progress merge, removing all conflict files.
        #[arg(
            long,
            conflicts_with = "branch",
            help = "Abort the current merge and clean up"
        )]
        abort: bool,
    },

    /// Resolve a merge conflict.
    ///
    /// A merge records a conflict when both branches modified the same
    /// lines since their common ancestor. Your version stays in <file>,
    /// valid and unmarked; see both sides with `velo conflicts <file>`.
    ///
    /// Examples
    ///   velo resolve src/auth.py --take theirs    # accept incoming version
    ///   velo resolve src/auth.py --take ours      # keep current version
    ///   velo resolve src/auth.py --take both      # ours then theirs, per hunk
    ///   velo resolve --mark src/auth.py           # accept your own edit of the file
    ///   velo resolve src/auth.py                  # step through each conflict hunk (needs a terminal)
    ///   velo resolve --all --take theirs          # resolve all conflicts at once
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · After resolving all conflicts, run `velo save \"Merge <branch>\"`.
    · Velo will remind you of remaining conflicts after each resolve.
    · --all requires --take; without it Velo doesn't know which version
      to pick for each file."
    )]
    Resolve {
        /// File to resolve (relative to repo root). Omit when using --all.
        #[arg(value_name = "FILE", help = "File to resolve (omit with --all)")]
        file: Option<String>,

        /// Automatically accept 'ours' or 'theirs' for this file.
        #[arg(
            short,
            long,
            value_enum,
            value_name = "VERSION",
            help = "Which version to keep: ours, theirs, or both (ours then theirs, per hunk)"
        )]
        take: Option<TakeArg>,

        /// Resolve all outstanding conflict files at once.
        #[arg(long, help = "Resolve all conflicts (requires --take)")]
        all: bool,

        /// With --take, replace hand edits; with --mark, accept an edit that
        /// drops lines the other side changed cleanly.
        #[arg(
            long,
            help = "With --take, replace hand edits; with --mark, accept an edit that drops clean changes"
        )]
        force: bool,

        /// Accept your hand edit of the file as the resolution.
        #[arg(
            long,
            requires = "file",
            conflicts_with_all = ["take", "all"],
            help = "Accept your edit of the file as the resolution"
        )]
        mark: bool,
    },

    /// Show both sides of each conflicting hunk.
    ///
    /// Read-only. Prints, for each conflicted file, every hunk with your
    /// side, the incoming side and the lines around it.
    ///
    /// Examples
    ///   velo conflicts                # every conflicted file
    ///   velo conflicts src/auth.py    # one file
    ///   velo conflicts --json         # as data, for scripts
    #[command(verbatim_doc_comment)]
    Conflicts {
        /// File to show (omit for all conflicted files).
        #[arg(value_name = "FILE", help = "File to show (omit for all)")]
        file: Option<String>,

        /// Print as JSON.
        #[arg(long, help = "Print as JSON")]
        json: bool,
    },

    /// Shelve and restore dirty working-tree state.
    ///
    /// Stash shelves let you set aside uncommitted changes without
    /// saving a formal snapshot.  Unlike Git stash, each shelf has
    /// an explicit name — no more cryptic stash@{2} indices.
    ///
    /// Subcommands
    ///   push [NAME]    Shelve current changes (auto-named if NAME is omitted)
    ///   list           List all shelves
    ///   pop [NAME]     Restore the most recent shelf (or named one)
    ///   drop [NAME]    Delete a shelf without restoring it
    ///   show [NAME]    Show what a shelf contains
    ///
    /// Examples
    ///   velo stash push                  # auto-named shelf
    ///   velo stash push "wip: auth"      # named shelf
    ///   velo stash list
    ///   velo stash pop "wip: auth"
    ///   velo stash drop "wip: auth"
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Stashing restores the working tree to the last saved snapshot.
    · Pop aborts if there are unsaved changes.
    · Shelves are stored in the repository database and survive restarts."
    )]
    Stash {
        #[command(subcommand)]
        sub: StashSub,
    },

    /// Show which snapshot last changed each line of a file.
    ///
    /// Walks history backwards and attributes each line to the snapshot that
    /// introduced or last modified it.
    ///
    /// Examples
    ///   velo blame src/auth.py
    ///   velo blame src/auth.py --at v1.0
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · --at accepts a hash prefix, tag, or branch name.
    · Binary files are skipped automatically."
    )]
    Blame {
        /// File to annotate (relative to repo root).
        #[arg(value_name = "FILE")]
        file: String,

        /// Annotate the file as it existed at this snapshot (default: current HEAD).
        #[arg(
            long,
            value_name = "TARGET",
            help = "Snapshot, tag, or branch to inspect"
        )]
        at: Option<String>,

        /// Only these lines, as `N` or `N-M` (1-based, inclusive).
        ///
        /// Also the fast path: the walk stops once the requested lines are
        /// explained, rather than once the whole file is.
        #[arg(
            long,
            value_name = "N[-M]",
            help = "Annotate only these lines, e.g. 40-60"
        )]
        lines: Option<String>,
    },

    /// Move or rename a tracked file, and record that it moved.
    ///
    /// Velo has no staging area — the disk is the snapshot — but a rename is
    /// the one thing the disk cannot express: afterwards the tree looks
    /// exactly as it would if the old file had been deleted and a new one
    /// written.  Recording it here is what lets `velo blame <file>` follow the file
    /// across the move instead of crediting every line to it, and what lets
    /// `velo history --file <file>` reach past it.
    ///
    /// Using `mv` from your shell still works; the move simply reads as a
    /// delete and an add, because by then nothing knows any better.
    ///
    /// Examples
    ///   velo mv notes.txt docs/notes.md
    ///   velo save "move the notes"
    #[command(
        verbatim_doc_comment,
        after_help = "The rename is recorded on the next 'velo save'."
    )]
    Mv {
        /// Path to move.
        #[arg(value_name = "FROM")]
        from: String,

        /// Where to move it.
        #[arg(value_name = "TO")]
        to: String,
    },

    /// Write the history as a `git fast-import` stream.
    ///
    /// The way out: history, file modes and tags go to a stream git can read,
    /// with velo's metadata, rename edges and snapshot ids in `Velo-*`
    /// commit trailers. Writes to stdout unless --output is given.
    ///
    /// Examples
    ///   velo export-git | (mkdir ../out && cd ../out && git init -q && git fast-import)
    ///   velo export-git --output history.fi --branch main
    #[command(verbatim_doc_comment)]
    ExportGit {
        /// Write the stream to this file instead of stdout.
        #[arg(long, short, value_name = "FILE")]
        output: Option<PathBuf>,

        /// Export only this branch (repeatable; default: every branch).
        #[arg(long, value_name = "BRANCH")]
        branch: Vec<String>,
    },

    /// Read a `git fast-export` stream into velo.
    ///
    /// The way back in. A history velo exported keeps its snapshot ids, metadata
    /// and rename edges (they ride in `Velo-*` trailers); a plain git history
    /// imports with its authors, timestamps, branches, tags and merges.
    /// Annotated tags become lightweight tags, and submodules are skipped.
    /// Reads stdin unless --input is given.
    ///
    /// Examples
    ///   git fast-export --all --reencode=yes --signed-tags=strip --tag-of-filtered-object=drop -M | velo import-git
    ///   velo import-git --input history.fe
    #[command(verbatim_doc_comment)]
    ImportGit {
        /// Read the stream from this file instead of stdin.
        #[arg(long, short, value_name = "FILE")]
        input: Option<PathBuf>,
    },

    /// Search tracked files for a pattern.
    ///
    /// Searches the working tree by default.  Use --snapshot to search
    /// inside a stored snapshot without touching disk.
    ///
    /// Examples
    ///   velo grep "TODO"
    ///   velo grep "api_key" -i
    ///   velo grep "def.*login" --snapshot v1.0
    ///   velo grep "error" -l
    ///   velo grep "token" -C 3
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Patterns are treated as regular expressions.
    · Use -l to list only matching file names (no content shown).
    · -C sets how many lines of context to show around each match."
    )]
    Grep {
        /// Pattern to search for (regular expression).
        #[arg(value_name = "PATTERN")]
        pattern: String,

        /// Search inside a stored snapshot instead of the working tree.
        #[arg(
            long,
            short,
            value_name = "TARGET",
            help = "Search inside this snapshot"
        )]
        snapshot: Option<String>,

        /// Case-insensitive matching.
        #[arg(short = 'i', long = "ignore-case", help = "Case-insensitive search")]
        ignore_case: bool,

        /// Only print file names with matches, not the matching lines.
        #[arg(
            short = 'l',
            long = "files-with-matches",
            help = "Print only file names"
        )]
        files_only: bool,

        /// Lines of context to show around each match.
        #[arg(
            short = 'C',
            long = "context",
            default_value_t = 2,
            value_name = "N",
            help = "Lines of context around each match"
        )]
        context: usize,
    },

    /// Collapse the last N snapshots into one.
    ///
    /// Rewrites history by replacing the last N snapshots with a single new
    /// snapshot that has the same files as HEAD and a new message.
    /// The squashed parent becomes the oldest replaced snapshot's parent.
    ///
    /// Examples
    ///   velo squash 3 "Combine auth fixes"
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Squash aborts if there are unsaved changes.
    · Any tags on squashed snapshots are moved to the new snapshot.
    · The originals stop being snapshots: their ids no longer resolve, and `velo undo` does not bring them back. File contents stay in the store until `velo gc`.
    · `velo undo` after a squash removes the squash result and rewinds to the snapshot before the range; `velo redo` restores the squash result."
    )]
    Squash {
        /// Number of snapshots to collapse (must be ≥ 2).
        #[arg(value_name = "N")]
        count: usize,

        /// Message for the new combined snapshot.
        #[arg(value_name = "MESSAGE")]
        message: String,
    },

    /// Replay commits from the current branch on top of another.
    ///
    /// Finds the commits on the current branch that are NOT in the target's
    /// ancestry, then cherry-picks them one by one onto the target.  If a
    /// commit produces a conflict the rebase pauses so you can resolve it.
    ///
    /// Examples
    ///   velo rebase main
    ///   velo rebase main --abort
    ///   velo rebase main --continue
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Rebase rewrites history — the rebased commits get new hashes.
    · Snapshots the target already contains, directly or through a merge,
      are skipped. A merge snapshot on your branch is replayed as one
      change against its first parent.
    · If a conflict occurs: resolve with `velo resolve`, save with
      `velo save`, then continue with `velo rebase --continue`.
    · `velo rebase --abort` restores the original branch state."
    )]
    Rebase {
        /// Branch or snapshot to rebase onto.
        #[arg(value_name = "TARGET")]
        target: Option<String>,

        /// Abort the rebase and restore the original branch.
        #[arg(
            long,
            conflicts_with = "cont",
            help = "Abort and restore original state"
        )]
        abort: bool,

        /// Continue after resolving a conflict.
        #[arg(long = "continue", help = "Continue after resolving conflicts")]
        cont: bool,
    },

    /// Reclaim disk space by removing unreachable objects.
    ///
    /// Objects become orphaned when snapshots are amended or undone.
    /// By default, undone snapshots are kept for 30 days (enabling
    /// redo); gc with --keep-days 0 purges them immediately.
    ///
    /// Examples
    ///   velo gc                   # default: keep undo history for 30 days
    ///   velo gc --keep-days 0     # purge everything immediately
    ///   velo gc --keep-days 90    # keep undo history for 90 days
    ///   velo gc --dry-run         # report what would be removed
    #[command(
        verbatim_doc_comment,
        after_help = "\
WHAT GC CLEANS UP
    · Orphaned objects (no snapshot references them)
    · Stale tree rows (snapshot was deleted)
    · Stale index_cache rows (path no longer tracked)
    · Trash entries older than --keep-days days

NOTES
    · Running gc while a merge is in progress is safe.
    · The operation is idempotent — running it twice is harmless.
    · A plain gc then returns freed pages to the operating system, for a repository created by 5.3.0 or later. --vacuum also converts an older one (it needs free disk about the size of velo.db and holds the write lock meanwhile)."
    )]
    Gc {
        /// Keep undone snapshot history for this many days.
        #[arg(
            long,
            default_value_t = 30,
            value_name = "DAYS",
            help = "Retain undo history for N days"
        )]
        keep_days: u32,
        /// Report what would be collected and remove nothing.
        #[arg(long, help = "Report what would be collected, remove nothing")]
        dry_run: bool,
        /// Run a full VACUUM, converting an older repository to incremental
        /// auto_vacuum.
        #[arg(long, help = "Run a full vacuum, converting an older repository")]
        vacuum: bool,
    },

    /// Convert stored full snapshots into deltas, to shrink an existing repository.
    ///
    /// A repository written before deltas keeps every snapshot's whole tree,
    /// and migrating it does not shrink it. Repack rewrites that storage a few
    /// snapshots at a time, each in its own short transaction, so a save made
    /// meanwhile waits only a moment. No snapshot id changes and no history is
    /// rewritten. Interrupting it is safe: run it again to finish.
    ///
    /// Examples
    ///   velo repack --dry-run
    ///   velo repack
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · --dry-run reports what a repack would convert and save, and changes nothing. Its byte figure is an estimate and a lower bound on the saving.
    · The space is returned to the operating system by a vacuum at the end of a real run.
    · Running it twice is harmless: the second run finds nothing to do."
    )]
    Repack {
        /// Report what would be converted and saved, and change nothing.
        #[arg(long, help = "Report what would be converted, change nothing")]
        dry_run: bool,
    },

    /// Thin old snapshots on a branch by age, then gc.
    ///
    /// Keeps every snapshot younger than the first tier, and one per bucket
    /// after that. Signed, tagged, merge, published and checked-out snapshots
    /// always stay. Runs `gc` and an incremental vacuum afterwards, so the
    /// space comes back.
    ///
    /// Examples
    ///   velo retention --dry-run
    ///   velo retention --keep 1d=1h --keep 30d=1d
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · --keep AGE=EVERY: snapshots older than AGE keep one per EVERY. Durations are <n>m, <n>h, <n>d or <n>w. Tiers must ascend in age and EVERY may not shrink.
    · With no --keep: everything for a day, one per hour for a month, one per day after.
    · --dry-run lists what would go and every protected snapshot, and changes nothing.
    · The removed snapshots' ids stop resolving, and `velo undo` does not bring them back.
    · Take a backup first with velo bundle create <file> if the history matters."
    )]
    Retention {
        /// The branch to thin (default: the current branch).
        #[arg(
            long,
            value_name = "BRANCH",
            help = "Branch to thin (default: current)"
        )]
        branch: Option<String>,
        /// A tier, `AGE=EVERY`; repeatable.
        #[arg(
            long = "keep",
            value_name = "AGE=EVERY",
            help = "Older than AGE, keep one snapshot per EVERY (e.g. 1d=1h); repeatable"
        )]
        keep: Vec<String>,
        /// Report what would go and what is protected, and change nothing.
        #[arg(long, help = "Report what would be removed, change nothing")]
        dry_run: bool,
        /// Keep undone snapshot history for this many days (for the gc pass).
        #[arg(
            long,
            default_value_t = 30,
            value_name = "DAYS",
            help = "Retain undo history for N days during gc"
        )]
        keep_days: u32,
    },

    /// Verify repository integrity (read-only).
    ///
    /// Checks that every referenced object exists and re-hashes to its own
    /// name, that snapshot parents resolve, that content-addressed snapshot ids
    /// recompute correctly, and that all refs (PARENT, tags, stash, conflicts)
    /// point at something real. Exits non-zero if any problem is found.
    ///
    /// Examples
    ///   velo fsck
    ///   velo fsck --repair   # also clean up inconsistent in-progress state
    #[command(verbatim_doc_comment)]
    Fsck {
        /// Fix what can be fixed safely: prune orphaned conflict/hunk/shelved-tag
        /// rows and clear a broken (MERGE_HEAD-less) conflict state.
        #[arg(long, help = "Repair safely-fixable inconsistencies")]
        repair: bool,
    },

    /// Pack history into a single file, or apply one (offline transfer).
    ///
    /// A bundle is self-contained: it carries the requested snapshots, every
    /// object they reference, and their tags. Apply it in another repository to
    /// import that history — no network required.
    ///
    /// Examples
    ///   velo bundle create backup.velo          # whole repo
    ///   velo bundle create feature.velo feature # everything reachable from 'feature'
    ///   velo bundle apply backup.velo           # import into this repo
    #[command(verbatim_doc_comment)]
    Bundle {
        #[command(subcommand)]
        cmd: BundleSub,
    },

    /// Copy a repository from a path into a new local repository.
    ///
    /// Imports all history, sets up an 'origin' remote, and checks out the
    /// remote's default branch.
    ///
    /// Example
    ///   velo clone /shared/project        # → ./project
    ///   velo clone /shared/project myproj # → ./myproj
    #[command(verbatim_doc_comment)]
    Clone {
        /// Path to the source repository.
        #[arg(value_name = "URL")]
        url: String,
        /// Directory to create (default: the source's basename).
        #[arg(value_name = "DIR")]
        dir: Option<String>,
    },

    /// Download history from a remote into remotes/<remote>/* tracking branches.
    ///
    /// Read-only with respect to your branches and working tree.
    #[command(verbatim_doc_comment)]
    Fetch {
        /// Remote name (default: origin).
        #[arg(value_name = "REMOTE", default_value = "origin")]
        remote: String,
    },

    /// Send a branch's commits to a remote (fast-forward only).
    ///
    /// Exits with code 4 when the remote refuses the push because it is not a
    /// fast-forward (the remote has commits you lack); `velo sync` fetches,
    /// integrates and pushes in one step.
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · Branches whose snapshots the pushed branch merged are published with
      it, under their own names."
    )]
    Push {
        /// Remote name (default: origin).
        #[arg(value_name = "REMOTE", default_value = "origin")]
        remote: String,
        /// Branch to push (default: the current branch).
        #[arg(value_name = "BRANCH")]
        branch: Option<String>,
    },

    /// Fetch the current branch and fast-forward, or advise a merge if diverged.
    #[command(verbatim_doc_comment)]
    Pull {
        /// Remote name (default: origin).
        #[arg(value_name = "REMOTE", default_value = "origin")]
        remote: String,
    },

    /// Fetch, merge if it is clean, and push: one step to share your work.
    ///
    /// Examples
    ///   velo sync                 # origin, the current branch
    ///   velo sync origin main
    ///
    /// Behind the remote it fast-forwards; ahead it pushes; diverged and clean
    /// it merges, saves the merge and pushes it.
    #[command(
        verbatim_doc_comment,
        after_help = "NOTES
    · A merge with conflicts is never pushed: sync stops with exit code 3 and
      leaves the merge as `velo merge` would. Run `velo resolve`, `velo save`,
      then `velo sync` again.
    · There is no --force: if the remote moves again mid-sync, it exits 4."
    )]
    Sync {
        /// Remote name (default: origin).
        #[arg(value_name = "REMOTE", default_value = "origin")]
        remote: String,
        /// Branch to sync (default: the current branch).
        #[arg(value_name = "BRANCH")]
        branch: Option<String>,
    },

    /// Manage remotes (paths, or ssh://host/path URLs).
    ///
    /// Examples
    ///   velo remote add origin /shared/project
    ///   velo remote add origin ssh://user@host/srv/project
    ///   velo remote
    ///   velo remote remove origin
    #[command(verbatim_doc_comment)]
    Remote {
        #[command(subcommand)]
        cmd: Option<RemoteSub>,
    },

    /// Internal: serve a fetch over stdin/stdout (invoked on the remote host).
    #[command(hide = true)]
    ServeUpload {
        #[arg(value_name = "PATH")]
        path: String,
    },

    /// Internal: serve a push over stdin/stdout (invoked on the remote host).
    #[command(hide = true)]
    ServeReceive {
        #[arg(value_name = "PATH")]
        path: String,
    },

    /// Serve a repository over HTTP, for `velo clone http://...`, `velo push` and `velo pull`.
    ///
    /// A reference server. By default fetch is anonymous and push is
    /// refused; give it a token to accept pushes. There is no TLS: put a
    /// reverse proxy in front of anything public (see "Hosting a remote" in
    /// the README). Pushes stay fast-forward only; there is no --force.
    ///
    /// Tokens: VELO_SERVE_TOKEN is one write token; --token-file holds lines
    /// of `read <token>` or `write <token>`. Once any token is configured,
    /// every request needs one. Clients send theirs in VELO_TOKEN.
    ///
    ///   velo serve-http /srv/project
    ///   VELO_SERVE_TOKEN=s3cret velo serve-http /srv/project --listen 0.0.0.0:8417
    ///   velo serve-http /srv/project --read-only
    ///   velo serve-http /srv/repos --directory --token-file tokens.txt
    ///   velo serve-http /srv/project --allow-anonymous-push
    #[command(verbatim_doc_comment)]
    ServeHttp {
        #[arg(value_name = "PATH")]
        path: String,
        /// Address to listen on (port 0 picks a free one).
        #[arg(long, value_name = "ADDR", default_value = "127.0.0.1:8417")]
        listen: String,
        /// Serve PATH as a directory of repositories, at http://host/<repo>/.
        #[arg(long)]
        directory: bool,
        /// File of `read <token>` / `write <token>` lines; `#` starts a comment.
        #[arg(long, value_name = "FILE")]
        token_file: Option<String>,
        /// Refuse every push, whatever the token.
        #[arg(long)]
        read_only: bool,
        /// With no tokens configured, accept pushes from anyone.
        #[arg(long)]
        allow_anonymous_push: bool,
        /// Largest request body accepted, in bytes.
        #[arg(long, value_name = "N", default_value_t = 1 << 30)]
        max_request_bytes: u64,
        /// Seconds allowed to receive one request body.
        #[arg(long, value_name = "SECS", default_value_t = 300)]
        request_timeout: u64,
        /// Requests served at once; more get 503 busy.
        #[arg(long, value_name = "N", default_value_t = 8)]
        max_connections: usize,
    },
}

// ─── Bundle subcommands ────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum BundleSub {
    /// Create a bundle file from history (all of it, or reachable from a ref).
    Create {
        /// Output file path.
        #[arg(value_name = "FILE")]
        file: String,
        /// Snapshot, tag, or branch to bundle history up to (default: everything).
        #[arg(value_name = "REF")]
        target: Option<String>,
    },
    /// Apply a bundle file into this repository.
    Apply {
        /// Bundle file to import.
        #[arg(value_name = "FILE")]
        file: String,
    },
}

// ─── Remote subcommands ────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum RemoteSub {
    /// Add a remote.
    Add {
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(value_name = "URL")]
        url: String,
    },
    /// Remove a remote.
    Remove {
        #[arg(value_name = "NAME")]
        name: String,
    },
}

// ─── Stash subcommands ────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum StashSub {
    /// Shelve current dirty state onto a named shelf.
    ///
    /// The working tree is restored to the last saved snapshot.
    ///
    /// Examples
    ///   velo stash push
    ///   velo stash push "wip: payments"
    #[command(verbatim_doc_comment)]
    Push {
        /// Name for the shelf. Auto-generated (stash-YYYYMMDD-HHMMSS) if omitted.
        #[arg(value_name = "NAME", help = "Shelf name (auto-generated if omitted)")]
        name: Option<String>,
        /// Git's spelling of the shelf name, accepted as an alias for NAME.
        #[arg(
            short = 'm',
            long = "message",
            value_name = "NAME",
            conflicts_with = "name"
        )]
        message: Option<String>,
    },

    /// List all stash shelves.
    ///
    /// Shows name, source branch, date, and snapshot hash for each shelf.
    ///
    /// Example
    ///   velo stash list
    #[command(verbatim_doc_comment)]
    List,

    /// Restore a shelf and remove it from the list.
    ///
    /// With no name, restores the most recently created shelf.
    ///
    /// Examples
    ///   velo stash pop
    ///   velo stash pop "wip: payments"
    #[command(verbatim_doc_comment)]
    Pop {
        /// Name of the shelf to restore. Defaults to the most recent.
        #[arg(
            value_name = "NAME",
            help = "Shelf to restore (defaults to most recent)"
        )]
        name: Option<String>,
    },

    /// Delete a shelf without restoring its contents.
    ///
    /// With no name, drops the most recently created shelf.
    ///
    /// Examples
    ///   velo stash drop
    ///   velo stash drop "old-experiment"
    #[command(verbatim_doc_comment)]
    Drop {
        /// Name of the shelf to delete. Defaults to the most recent.
        #[arg(
            value_name = "NAME",
            help = "Shelf to delete (defaults to most recent)"
        )]
        name: Option<String>,
    },

    /// Show the diff contained in a shelf without applying it.
    ///
    /// With no name, shows the most recently created shelf.
    ///
    /// Examples
    ///   velo stash show
    ///   velo stash show "wip: payments"
    #[command(verbatim_doc_comment)]
    Show {
        /// Name of the shelf to inspect. Defaults to the most recent.
        #[arg(
            value_name = "NAME",
            help = "Shelf to inspect (defaults to most recent)"
        )]
        name: Option<String>,
    },
}

// ─── Entry point ─────────────────────────────────────────────────────────────

/// A nudge for a git habit that clap rejects as an argument rather than as a
/// subcommand. Keyed on the words of the command line that are not flags and
/// on the offending token clap reported, so each row names exactly one habit.
fn clap_habit_hint(args: &[String], offending: &str) -> Option<&'static str> {
    if offending == "-C" {
        return Some("hint: velo has no -C; cd <dir> first, then run velo");
    }
    let words: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with('-'))
        .collect();
    Some(
        match (words.first().copied()?, words.get(1).copied(), offending) {
            ("history", _, tok) if !tok.starts_with('-') && tok.contains(':') => {
                spec_habit_hint(tok, "history")?
            }
            ("history", _, tok) if !tok.starts_with('-') => {
                "hint: velo history --branch <ref> shows another branch's history"
            }
            ("grep", _, "-n" | "--line-number") => {
                "hint: velo grep always shows line numbers; there is no -n"
            }
            ("diff", _, "--stat" | "--numstat" | "--shortstat") => {
                "hint: velo diff has no summary mode; it prints every change, and -- <path> narrows it"
            }
            _ => return None,
        },
    )
}

/// A nudge for a `velo show` target typed out of git habit. These are not
/// argument errors, so they are offered after the lookup itself fails.
fn show_habit_hint(args: &[String]) -> Option<&'static str> {
    let mut words = args.iter().filter(|a| !a.starts_with('-'));
    let cmd = words.next()?.as_str();
    if !matches!(cmd, "show" | "restore" | "diff" | "blame" | "history") {
        return None;
    }
    // `blame --at ref:path` carries the spec behind a flag, so every
    // argument is a candidate there.
    if cmd == "blame" {
        return args.iter().find_map(|a| spec_habit_hint(a, cmd));
    }
    spec_habit_hint(words.next()?, cmd)
}

/// A hint for a snapshot spec typed in git's spelling: `HEAD~N`, `HEAD^` or
/// `ref:path`. Plain `HEAD` is accepted (see `resolve_spec`), so it has none.
fn spec_habit_hint(target: &str, cmd: &str) -> Option<&'static str> {
    if target.starts_with("HEAD~") || target.starts_with("HEAD^") {
        Some(
            "hint: velo has no HEAD~N; name a branch, tag or snapshot id (velo history lists them)",
        )
    } else if target.contains(':') {
        Some(match cmd {
            "restore" => "hint: velo restore <ref> -- <path> pulls a file out of a snapshot; ref:path is git's spelling",
            "diff" => "hint: velo diff <ref> -- <path> narrows a diff to a path; ref:path is git's spelling",
            "history" => "hint: velo history -- <path> follows a path; velo show <ref> -- <path> shows it at a snapshot",
            "blame" => "hint: velo blame <path> --at <ref> takes the path and the ref separately; ref:path is git's spelling",
            _ => "hint: velo show <ref> -- <path> shows a path's changes; velo restore <ref> -- <path> pulls the file out",
        })
    } else {
        None
    }
}

/// Resolve a snapshot spec typed on the command line. `HEAD` (exactly) is
/// git's name for where you are; velo accepts it as the current position,
/// with a note, so the habit costs nothing. Core knows nothing of HEAD.
fn resolve_spec(repo: &velo_core::Repo, spec: &str) -> Result<velo_core::SnapshotId> {
    if spec != "HEAD" {
        return commands::resolve_snapshot_id(repo, spec);
    }
    let root = repo.root();
    let position = std::fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let position = position.trim();
    let id = if position.is_empty() {
        let branch = std::fs::read_to_string(root.join(".velo/HEAD")).map_err(VeloError::Io)?;
        commands::resolve_snapshot_id(repo, branch.trim())?
    } else {
        commands::resolve_snapshot_id(repo, position)?
    };
    eprintln!(
        "note: HEAD means the current position ({}) in velo",
        render::id::short(id.as_ref())
    );
    Ok(id)
}

/// Edit distance counting a swap of two neighbours as one edit.
fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// The existing branch a new name is probably a typo of, if any: within two
/// edits, and neither a prefix of the other (`feat` beside `feature` is a
/// deliberate new name).
fn near_miss<'a>(name: &str, existing: &'a [String]) -> Option<&'a str> {
    existing
        .iter()
        .filter(|b| !b.starts_with("remotes/") && !b.starts_with("_deleted_"))
        .filter(|b| !b.starts_with(name) && !name.starts_with(b.as_str()))
        .map(|b| (damerau_levenshtein(name, b), b))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, b)| b.as_str())
}

/// A nudge for a command people type out of git habit, keyed on the first
/// argument that is not a flag.
fn git_habit_hint(args: impl Iterator<Item = String>) -> Option<&'static str> {
    let word = args.into_iter().find(|a| !a.starts_with('-'))?;
    Some(match word.as_str() {
        "history-all" => "hint: velo history --all",
        "diff-worktree" => "hint: velo diff",
        "commit" => "hint: velo save \"<message>\"",
        "add" => "hint: velo has no staging area — velo save records every change",
        "checkout" => "hint: velo switch <branch>, or velo restore <snapshot>",
        _ => return None,
    })
}

fn main() {
    // Parsing this many subcommands and running the dispatcher in a debug build
    // needs more than the 1 MiB a Windows main thread gets, so run on a thread
    // with room to spare. Exit codes are set by `process::exit` inside.
    let worker = std::thread::Builder::new()
        .name("velo".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(real_main)
        .expect("failed to start the velo worker thread");
    if worker.join().is_err() {
        std::process::exit(101);
    }
}

fn real_main() {
    // SAFETY: `SetConsoleOutputCP` takes a plain integer code page and touches no
    // memory of ours; the declaration matches the kernel32 signature. Failure is
    // harmless (output stays in the old code page), so the result is ignored.
    #[cfg(windows)]
    #[allow(unsafe_code)]
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn SetConsoleOutputCP(id: u32) -> i32;
        }
        SetConsoleOutputCP(65001);
    }

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            use clap::error::{ContextKind, ContextValue, ErrorKind};
            let kind = e.kind();
            let offending = match e.get(ContextKind::InvalidArg) {
                Some(ContextValue::String(s)) => s.clone(),
                Some(ContextValue::Strings(v)) => v.first().cloned().unwrap_or_default(),
                _ => String::new(),
            };
            let _ = e.print();
            let hint = match kind {
                ErrorKind::InvalidSubcommand => git_habit_hint(std::env::args().skip(1)),
                ErrorKind::UnknownArgument
                | ErrorKind::InvalidValue
                | ErrorKind::TooManyValues
                | ErrorKind::ArgumentConflict => {
                    let args: Vec<String> = std::env::args().skip(1).collect();
                    clap_habit_hint(&args, &offending)
                }
                _ => None,
            };
            if let Some(hint) = hint {
                eprintln!("{hint}");
            }
            std::process::exit(e.exit_code());
        }
    };
    // The force-flag suggestion depends on which command was run, so it is
    // captured before `cli.command` is consumed.
    let force_form = force_form(&cli.command);
    let remote = remote_of(&cli.command).map(str::to_string);

    let code = match run(cli) {
        Ok(exit) => exit.code(),
        Err(e) => {
            eprintln!("{} {}", console::style("error:").red().bold(), e);
            if let Some(hint) = hint_for(&e, force_form, remote.as_deref()) {
                eprintln!("{}", hint);
            } else if let Some(hint) =
                show_habit_hint(&std::env::args().skip(1).collect::<Vec<_>>())
            {
                eprintln!("{hint}");
            }
            error_code(&e)
        }
    };
    if code != 0 {
        std::process::exit(code);
    }
}

/// How a command that ran to the end finished. Anything other than `Success`
/// is not an error — the output has already been printed — but a script needs
/// to tell it apart from a clean run.
enum Exit {
    Success,
    /// A merge, cherry-pick or rebase stopped on conflicts.
    Conflicts,
    /// Local and remote histories diverged.
    Diverged,
    /// `velo verify` found no trusted signature, or an invalid one.
    Untrusted,
    /// A refusal made after parsing (a near-miss branch name): an error, exit 1.
    Refused,
}

impl Exit {
    fn code(&self) -> i32 {
        match self {
            Exit::Success => 0,
            Exit::Conflicts => 3,
            Exit::Diverged => 4,
            Exit::Untrusted => 1,
            Exit::Refused => 1,
        }
    }
}

/// The CLI's mapping from a typed error to an exit code; documented in the
/// EXIT CODES block of `velo --help`.
fn error_code(e: &VeloError) -> i32 {
    match e {
        VeloError::Conflicts { .. } => 3,
        VeloError::Diverged { .. } | VeloError::NotFastForward { .. } => 4,
        VeloError::Locked { .. } => 5,
        _ => 1,
    }
}

/// How to re-run this command discarding local changes, for the commands that
/// accept `--force`. `None` for the rest, so a flag is never suggested to a
/// command that would reject it.
fn force_form(command: &Commands) -> Option<&'static str> {
    match command {
        Commands::Switch { .. } => Some("velo switch <branch> --force"),
        Commands::Restore { .. } => Some("velo restore <snapshot> --force"),
        _ => None,
    }
}

/// The remote a network command talks to, so a refusal can name the
/// environment variable that holds that remote's token.
fn remote_of(command: &Commands) -> Option<&str> {
    match command {
        Commands::Push { remote, .. }
        | Commands::Fetch { remote }
        | Commands::Pull { remote, .. } => Some(remote),
        _ => None,
    }
}

/// How to spawn a subprocess remote.
///
/// Core can't work this out for itself — locating the running binary and reading
/// the environment are the caller's job — so the CLI assembles it here.
///
/// * `VELO_SSH` overrides the SSH client.
/// * `VELO_REMOTE_BIN` overrides the `velo` binary invoked on the far host.
fn spawn_config() -> Result<velo_core::transport::Spawn> {
    let local_bin = std::env::current_exe()
        .map_err(|e| VeloError::invalid(format!("cannot locate the velo binary: {}", e)))?;
    let mut spawn = velo_core::transport::Spawn::new(local_bin);
    if let Ok(ssh) = std::env::var("VELO_SSH") {
        spawn = spawn.ssh(ssh);
    }
    if let Ok(bin) = std::env::var("VELO_REMOTE_BIN") {
        spawn = spawn.remote_bin(bin);
    }
    Ok(spawn)
}

/// The bearer token for a remote, from the environment: `VELO_TOKEN_<NAME>`
/// (the name uppercased, every non-alphanumeric character as `_`), then the
/// general `VELO_TOKEN`. Empty values count as unset. Tokens are read only from
/// the environment: one stored in the repository would travel in backups, and
/// one in a URL would land in shell history and `velo remote`'s output.
fn token_for(remote_name: &str) -> Option<String> {
    token_from(|k| std::env::var(k).ok(), remote_name)
}

/// The environment variable that holds the token for one remote: the name
/// uppercased, every non-alphanumeric character replaced with `_`.
fn token_var(name: &str) -> String {
    let mangled: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("VELO_TOKEN_{mangled}")
}

/// The lookup behind [`token_for`], with the environment passed in.
fn token_from(get: impl Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    get(&token_var(name))
        .filter(|t| !t.is_empty())
        .or_else(|| get("VELO_TOKEN").filter(|t| !t.is_empty()))
}

#[cfg(test)]
mod token_tests {
    use super::*;

    #[test]
    fn token_var_uppercases_and_mangles() {
        assert_eq!(token_var("origin"), "VELO_TOKEN_ORIGIN");
        assert_eq!(token_var("my-remote.v2"), "VELO_TOKEN_MY_REMOTE_V2");
    }

    #[test]
    fn token_from_prefers_the_specific_variable() {
        let env = |k: &str| match k {
            "VELO_TOKEN_MY_REMOTE_V2" => Some("specific".to_string()),
            "VELO_TOKEN" => Some("general".to_string()),
            _ => None,
        };
        assert_eq!(token_from(env, "my-remote.v2").as_deref(), Some("specific"));
        assert_eq!(token_from(env, "other").as_deref(), Some("general"));
        assert_eq!(token_from(|_| None, "other"), None);
    }
}

/// Which steps of the reconcile are still ahead, from what the repository holds.
fn recipe_state(repo: &velo_core::Repo, remote: &str, branch: &str) -> render::sync::RecipeState {
    use render::sync::RecipeState;
    if repo.root().join(".velo/MERGE_HEAD").exists() {
        return RecipeState::MergePending;
    }
    let spec = format!("{remote}/{branch}");
    let Ok(tracking) = commands::resolve_snapshot_id(repo, &spec) else {
        return RecipeState::NotFetched;
    };
    let Ok(local) = commands::resolve_snapshot_id(repo, branch) else {
        return RecipeState::Fetched;
    };
    match commands::merge::merge_base(repo, &local, &tracking) {
        Ok(Some(base)) if base == tracking => RecipeState::MergeSaved,
        _ => RecipeState::Fetched,
    }
}

/// What to do about an error, for the kinds that have an obvious next step.
///
/// Core states what went wrong and carries the details; deciding what to suggest
/// is presentation, so it lives here. Centralising it means every command gets
/// the same guidance instead of each one embedding advice in an error string.
fn hint_for(e: &VeloError, force_form: Option<&str>, remote: Option<&str>) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    match e {
        VeloError::DirtyWorkingTree { paths } => {
            list_paths(&mut lines, paths);
            lines.push("  Save or shelve them first:".into());
            let mut options = vec![
                (
                    "velo save \"<message>\"",
                    "keep the changes as a snapshot",
                    Tone::Do,
                ),
                (
                    "velo stash push",
                    "set them aside to reapply later",
                    Tone::Alt,
                ),
            ];
            // Only offered when the command that failed actually takes it —
            // naming a flag it would reject is worse than saying nothing. Added
            // to the same list so every description lines up.
            if let Some(form) = force_form {
                options.push((form, "discard them", Tone::Destructive));
            }
            suggest(&mut lines, &options);
        }
        VeloError::SnapshotHasDependents { branches, .. } => {
            for (name, count) in branches {
                let label = match name.as_str() {
                    "_stash" => "a stash shelf".to_string(),
                    other => match other.strip_prefix("_deleted_") {
                        Some(gone) => format!("deleted branch '{}'", gone),
                        None => format!("branch '{}'", other),
                    },
                };
                lines.push(format!("  {} snapshot(s) on {}", count, label));
            }
            lines.push("  Instead:".into());
            let first = branches
                .iter()
                .map(|(b, _)| b.as_str())
                .find(|b| !b.starts_with('_'));
            let mut options: Vec<(String, &str, Tone)> = vec![(
                format!("velo restore {} && velo save \"<message>\"", "<parent>"),
                "go back on this branch and keep the snapshot for the others",
                Tone::Do,
            )];
            if let Some(b) = first {
                options.push((
                    format!("velo switch {}", b),
                    "see the work that builds on it",
                    Tone::Alt,
                ));
            }
            let borrowed: Vec<(&str, &str, Tone)> = options
                .iter()
                .map(|(c, d, t)| (c.as_str(), *d, *t))
                .collect();
            suggest(&mut lines, &borrowed);
        }
        VeloError::WouldOverwriteEdits { paths } => {
            list_paths(&mut lines, paths);
            lines.push("  Taking a side would replace those edits:".into());
            let file = paths
                .first()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<file>".into());
            let force = format!("velo resolve {} --take <side> --force", file);
            let diff = format!("velo diff {}", file);
            let mark = format!("velo resolve --mark {}", file);
            suggest(
                &mut lines,
                &[
                    (&mark, "keep your edit as the resolution", Tone::Do),
                    (&diff, "see your edits", Tone::Do),
                    (&force, "replace them", Tone::Destructive),
                ],
            );
        }
        VeloError::MissingCleanChanges {
            path,
            missing,
            sample,
        } => {
            lines.push("  Lines the other side changed cleanly are not in your file:".into());
            for l in sample {
                lines.push(format!("    + {l}"));
            }
            if *missing > sample.len() {
                lines.push(format!("    … and {} more", missing - sample.len()));
            }
            let file = path.display().to_string();
            let again = format!("velo resolve --mark {}", file);
            let both = format!("velo resolve {} --take both --force", file);
            let drop = format!("velo resolve --mark {} --force", file);
            suggest(
                &mut lines,
                &[
                    (&again, "add them and run this again", Tone::Do),
                    (
                        &both,
                        "replace your edit with both sides plus the clean changes",
                        Tone::Destructive,
                    ),
                    (&drop, "accept your edit and drop them", Tone::Destructive),
                ],
            );
        }
        VeloError::FormatTooOld { found, .. } => {
            lines.push(format!(
                "  This repository was written by Velo before format v2 (found v{}).",
                found
            ));
            lines.push(
                "  Its snapshot ids were built by a different recipe, so there is nothing".into(),
            );
            lines.push("  to migrate in place. Move the work across instead:".into());
            suggest(
                &mut lines,
                &[
                    (
                        "velo init <new-dir>",
                        "start a v2 repository elsewhere",
                        Tone::Do,
                    ),
                    (
                        "cp -r <files> <new-dir> && velo save \"<message>\"",
                        "copy the working tree in and record it",
                        Tone::Alt,
                    ),
                ],
            );
        }
        VeloError::OperationInProgress { what } => {
            lines.push("  Finish it or undo it:".into());
            let abort = format!("velo {} --abort", what);
            suggest(
                &mut lines,
                &[
                    ("velo save \"<message>\"", "record the result", Tone::Do),
                    (&abort, "throw the attempt away", Tone::Destructive),
                ],
            );
        }
        VeloError::NotFastForward { branch, remote } => {
            lines.push(format!(
                "  '{}' on '{}' moved on while you were working.",
                console::style(branch).cyan(),
                console::style(remote).cyan()
            ));
            let state = std::env::current_dir()
                .ok()
                .and_then(|d| commands::find_repo_root(&d))
                .and_then(|root| velo_core::Repo::open(&root).ok())
                .map_or(render::sync::RecipeState::NotFetched, |repo| {
                    recipe_state(&repo, remote, branch)
                });
            let recipe = render::sync::recipe(remote, branch, state);
            let steps: Vec<(&str, &str, Tone)> = recipe
                .iter()
                .map(|(cmd, why)| (cmd.as_str(), *why, Tone::Do))
                .collect();
            suggest(&mut lines, &steps);
        }
        VeloError::Conflicts { paths } => {
            list_paths(&mut lines, paths);
            suggest(
                &mut lines,
                &[
                    ("velo conflicts <file>", "see both sides", Tone::Do),
                    (
                        "velo resolve --mark <file>",
                        "after editing it yourself",
                        Tone::Alt,
                    ),
                    (
                        "velo resolve <file>",
                        "walk through them hunk by hunk",
                        Tone::Alt,
                    ),
                    (
                        "velo resolve --all --take theirs",
                        "take one side everywhere",
                        Tone::Do,
                    ),
                ],
            );
        }
        VeloError::Locked { holder, .. } => match holder {
            Some(h) => {
                let live = match lockwait::pid_alive(h.pid) {
                    Some(true) => {
                        "; that process is still running \u{2014} wait for it to finish".to_string()
                    }
                    Some(false) => format!(
                        "; process {} is no longer running, so that record is stale \u{2014} retry",
                        h.pid
                    ),
                    None => String::new(),
                };
                lines.push(format!(
                    "  started {} ago{}",
                    lockwait::age(h.started_at_ms),
                    live
                ));
            }
            None => lines.push("  another velo process is writing; retry in a moment".into()),
        },
        VeloError::RedoBranchMoved { snapshot, .. } => {
            lines.push(
                "  The branch moved after the undo, so the undone snapshot is no longer its child."
                    .into(),
            );
            lines.push(format!(
                "  velo restore {} still shows it.",
                snapshot.short()
            ));
        }
        VeloError::RemoteRefAbsent { remote, .. } => {
            lines.push(format!(
                "  Run velo fetch {} to bring it back, then try again.",
                remote
            ));
        }
        VeloError::AccessDenied { .. } => {
            let var = remote.map(token_var);
            let sent = match remote {
                Some(r) => token_for(r).is_some(),
                None => std::env::var("VELO_TOKEN").is_ok_and(|v| !v.is_empty()),
            };
            let names = match &var {
                Some(v) => format!("{v} (or VELO_TOKEN)"),
                None => "VELO_TOKEN_<REMOTE> (the remote's name, uppercased) or VELO_TOKEN".into(),
            };
            if sent {
                lines.push(format!(
                    "  The server rejected the token in {names}, or the token lacks permission for this."
                ));
                lines.push(
                    "  Check the value, or ask the server's owner for a token that may write."
                        .into(),
                );
            } else {
                lines.push(format!(
                    "  No token was sent. Set {names} to a bearer token for this remote."
                ));
                lines.push("  The server accepts pushes only with a token: its owner runs `velo serve-http` with VELO_SERVE_TOKEN=<token> or --token-file FILE and gives you one.".into());
            }
            lines.push("  Never put the token in the URL.".into());
        }
        _ => return None,
    }
    Some(lines.join(
        "
",
    ))
}

/// How prominently to render a suggested command.
#[derive(Clone, Copy)]
enum Tone {
    /// The usual next step.
    Do,
    /// A reasonable alternative.
    Alt,
    /// Throws work away — deliberately understated.
    Destructive,
}

/// Render suggestions as an aligned bullet list.
///
/// The commands vary in length, so the descriptions are padded to a common
/// column rather than separated by a fixed run of spaces.
fn suggest(lines: &mut Vec<String>, items: &[(&str, &str, Tone)]) {
    use console::style;
    let width = items.iter().map(|(cmd, _, _)| cmd.len()).max().unwrap_or(0);
    for (cmd, description, tone) in items {
        // `console` forwards the formatter to the inner value, so padding is
        // measured on the text and not on the escape codes around it.
        let rendered = match tone {
            Tone::Do => style(format!("{:<width$}", cmd)).green().to_string(),
            Tone::Alt => style(format!("{:<width$}", cmd)).cyan().to_string(),
            Tone::Destructive => style(format!("{:<width$}", cmd)).dim().to_string(),
        };
        lines.push(format!("    · {}  {}", rendered, description));
    }
}

/// Append up to five paths, noting how many were left out.
fn list_paths(lines: &mut Vec<String>, paths: &[std::path::PathBuf]) {
    const SHOWN: usize = 5;
    for p in paths.iter().take(SHOWN) {
        lines.push(format!("  {}", console::style(p.display()).yellow()));
    }
    if paths.len() > SHOWN {
        lines.push(format!("  … and {} more", paths.len() - SHOWN));
    }
}

/// Guidance for running a command outside any repository. The right next step
/// depends on what was attempted: sync commands usually mean the user wants a
/// working copy (`clone`) rather than a brand-new repository (`init`).
fn not_a_repo_hint(cmd: &Commands) -> VeloError {
    let hint = match cmd {
        // Sync commands operate between repositories — both ends must be one.
        Commands::Remote { .. }
        | Commands::Push { .. }
        | Commands::Pull { .. }
        | Commands::Sync { .. }
        | Commands::Fetch { .. } => {
            "  Sync commands run inside a repository — this folder isn't one yet.\n  \
             · velo clone <url> [dir]   get a working copy of an existing repository\n  \
             · velo init                start a new repository here, then 'velo remote add origin <url>'"
        }
        Commands::Bundle { .. } => {
            "  A bundle is imported into a repository, so create one first:\n  \
             · velo init                then 'velo bundle apply <file>'"
        }
        _ => {
            "  · velo init                start tracking this folder\n  \
             · velo clone <url> [dir]   copy an existing repository"
        }
    };
    VeloError::invalid(format!(
        "Not a Velo repository (no .velo found here or above).
{}",
        hint
    ))
}

/// Drive `velo resolve`: core supplies the conflict data and applies decisions,
/// `velo-tui` provides the interactive navigator. This wiring lives in the CLI so
/// neither of those crates needs to know about the other.
fn run_resolve(
    repo: &velo_core::Repo,
    guard: Option<&velo_core::WriteGuard>,
    file: Option<&str>,
    take: Option<TakeOption>,
    all: bool,
    force: bool,
) -> Result<()> {
    use console::style;
    use velo_core::commands::resolve;

    if !resolve::merge_active(repo) {
        if all {
            println!("{}", style("No conflicts to resolve.").dim());
            return Ok(());
        }
        return Err(VeloError::invalid(
            "No merge in progress. Nothing to resolve.",
        ));
    }
    if !all && file.is_none() {
        return Err(VeloError::invalid(
            "Specify a file, or use --all.\n  \
             Example: velo resolve src/auth.py\n  \
             Example: velo resolve --all --take theirs",
        ));
    }

    let targets = if all {
        resolve::list_conflicts(repo)?
    } else {
        vec![resolve::get_conflict(repo, file.unwrap())?]
    };
    if targets.is_empty() {
        println!("{}", style("No conflict files found.").dim());
        return Ok(());
    }

    match take {
        Some(side) => {
            let on_edits = if force {
                resolve::OnEdits::Replace
            } else {
                resolve::OnEdits::Refuse
            };
            let guard = guard.expect("--take must hold the write guard");
            let taken = resolve::take_all(guard, &targets, side, on_edits)?;
            let root = guard.root();
            let branch = std::fs::read_to_string(root.join(".velo/HEAD"))
                .unwrap_or_default()
                .trim()
                .to_string();
            let source =
                resolve::merge_source(guard.repo()).unwrap_or_else(|| "incoming".to_string());
            for t in &taken {
                let (meaning, lost, other) = match t.side {
                    TakeOption::Ours => (
                        format!("your last saved snapshot on {}", branch),
                        "theirs",
                        source.as_str(),
                    ),
                    TakeOption::Both => (
                        format!("ours then {}", source),
                        "neither side",
                        source.as_str(),
                    ),
                    _ => (source.clone(), "ours", "ours"),
                };
                println!(
                    "{} Resolved '{}' — took {} for {} conflict(s) ({})",
                    style("✔").green(),
                    t.path,
                    t.side.as_str(),
                    t.conflicts,
                    meaning
                );
                let mut parts = Vec::new();
                if t.dropped_lines > 0 {
                    parts.push(format!(
                        "discarded {} line(s) from {}",
                        t.dropped_lines, lost
                    ));
                }
                if t.kept_clean_lines > 0 {
                    parts.push(format!(
                        "kept {} clean line(s) from {}",
                        t.kept_clean_lines, other
                    ));
                }
                if !parts.is_empty() {
                    println!("  {}", parts.join(", "));
                }
                if t.replaced_edit_lines > 0 {
                    println!(
                        "  {} replaced your unsaved edits in {} ({} lines)",
                        style("!").yellow().bold(),
                        t.path,
                        t.replaced_edit_lines
                    );
                }
            }
        }
        None => {
            // The TUI needs a real terminal at both ends; without one it
            // would hang on a key read. The check is the CLI's, so core stays
            // free of environment reads. The session takes the write lock
            // itself, one decision at a time.
            use std::io::IsTerminal;
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                return Err(VeloError::invalid(
                    "interactive resolve needs a terminal; use --take ours|theirs|both, \
                     or edit the file and run velo resolve --mark <file>",
                ));
            }
            for cf in targets {
                velo_tui::resolve_interactive(repo, cf)?;
            }
        }
    }

    report_remaining_conflicts(repo)
}

/// `velo resolve --mark <file>`: accept the hand-edited file as the resolution.
fn run_mark(guard: &velo_core::WriteGuard, file: &str, force: bool) -> Result<()> {
    use console::style;
    use velo_core::commands::resolve;

    let repo = guard.repo();
    if !resolve::merge_active(repo) {
        return Err(VeloError::invalid(
            "No merge in progress. Nothing to resolve.",
        ));
    }
    let cf = resolve::get_conflict(repo, file)?;
    let on_clean = if force {
        resolve::OnCleanChanges::Drop
    } else {
        resolve::OnCleanChanges::Refuse
    };
    let m = resolve::mark_with(guard, &cf, on_clean)?;
    println!(
        "{} Accepted your edit of '{}' ({} lines; {} line(s) added or removed vs ours, {} vs theirs).",
        style("✔").green(),
        m.path,
        m.lines,
        m.differs_from_ours,
        m.differs_from_theirs
    );
    if !m.dropped_clean_changes.is_empty() {
        let source = resolve::merge_source(repo).unwrap_or_else(|| "incoming".into());
        println!(
            "  {} dropped {} line(s) from {} that merged cleanly:",
            style("!").yellow().bold(),
            m.dropped_clean_changes.len(),
            source
        );
        for l in m.dropped_clean_changes.iter().take(5) {
            println!("    {l}");
        }
        if m.dropped_clean_changes.len() > 5 {
            println!("    … and {} more", m.dropped_clean_changes.len() - 5);
        }
    }
    report_remaining_conflicts(repo)
}

/// `velo conflicts`: show both sides of each hunk, as text or JSON.
fn run_conflicts(repo: &velo_core::Repo, file: Option<&str>, json: bool) -> Result<()> {
    use velo_core::commands::resolve;

    let targets = match file {
        Some(f) if resolve::merge_active(repo) => vec![resolve::get_conflict(repo, f)?],
        Some(_) => Vec::new(),
        None => resolve::list_conflicts(repo)?,
    };
    let source = resolve::merge_source(repo);
    let mut sessions = Vec::with_capacity(targets.len());
    let mut missing = Vec::with_capacity(targets.len());
    for cf in targets {
        missing.push(resolve::missing_clean_changes(repo, &cf)?);
        sessions.push(resolve::open_session(repo, cf)?);
    }
    if json {
        render::conflicts::print_json(source.as_deref(), &sessions, &missing);
    } else {
        render::conflicts::print_text(source.as_deref(), &sessions, &missing);
    }
    Ok(())
}

/// After resolving, tell the user what is left and what to do next.
fn report_remaining_conflicts(repo: &velo_core::Repo) -> Result<()> {
    use console::style;
    use velo_core::commands::resolve;

    let remaining = resolve::conflict_count(repo);
    if remaining == 0 {
        // MERGE_HEAD deliberately survives until `velo save`, so the user can
        // still abort the whole merge after resolving everything.
        println!(
            "\n{} All conflicts resolved! Run {} to finalise.",
            style("✔").green().bold(),
            style("velo save \"Merge <branch>\"").yellow().bold()
        );
        println!(
            "  {} to cancel the merge entirely.",
            style("velo merge --abort").dim()
        );
    } else {
        println!(
            "\n{} {} conflict file(s) still unresolved.",
            style("!").yellow().bold(),
            remaining
        );
        for cf in resolve::list_conflicts(repo)? {
            println!("  {}", style(&cf.path).yellow());
        }
    }
    Ok(())
}

/// Warn, on stderr, when a history-rewriting command is about to replace
/// snapshots that a remote-tracking ref already contains.
///
/// Advisory only: the command proceeds either way, and any failure to work it
/// out (no snapshots yet, an unresolvable target) silently means no warning.
fn warn_if_rewriting_pushed(repo: &velo_core::Repo, root: &Path, cmd: &Commands) {
    let check = || -> Option<String> {
        let parent_of = |id: &velo_core::SnapshotId| -> Option<velo_core::SnapshotId> {
            commands::show::run(repo, id, &[]).ok()?.parent
        };
        let position = std::fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
        let position = position.trim();
        let tip = if position.is_empty() {
            let branch = std::fs::read_to_string(root.join(".velo/HEAD")).ok()?;
            commands::resolve_snapshot_id(repo, branch.trim()).ok()?
        } else {
            commands::resolve_snapshot_id(repo, position).ok()?
        };
        let base = match cmd {
            Commands::Save { amend: true, .. } | Commands::Undo => parent_of(&tip),
            Commands::Squash { count, .. } => {
                let mut base = Some(tip.clone());
                for _ in 0..*count {
                    base = parent_of(&base?);
                }
                base
            }
            Commands::Rebase {
                target: Some(spec),
                abort: false,
                cont: false,
            } => Some(commands::resolve_snapshot_id(repo, spec).ok()?),
            _ => return None,
        };
        commands::sync::published(repo, &tip, base.as_ref())
            .ok()
            .flatten()
    };
    if let Some(label) = check() {
        eprintln!(
            "warning: this rewrites snapshot(s) already pushed to {label}; anyone who pulled them will diverge from you."
        );
    }
}

/// Does this command hold the repo lock for its whole run?
///
/// Interactive `resolve` does not: a human can leave the TUI open for an hour,
/// so it takes the lock per decision instead.
fn needs_write_guard(cmd: &Commands) -> bool {
    !is_read_only(cmd)
        && !matches!(
            cmd,
            // Repack takes the write lock itself, one chunk at a time, so a
            // guard held across the command would make it wait on itself.
            Commands::Repack { .. }
                | Commands::Resolve {
                    take: None,
                    mark: false,
                    ..
                }
        )
}

/// Commands that never mutate the repository — they skip the repo lock so a
/// long-running write (e.g. `gc`) never blocks a `status` or `history`.
fn is_read_only(cmd: &Commands) -> bool {
    matches!(
        cmd,
        Commands::Status { .. }
            | Commands::History { .. }
            | Commands::Diff { .. }
            | Commands::Show { .. }
            | Commands::Blame { .. }
            | Commands::Grep { .. }
            | Commands::Conflicts { .. }
            | Commands::ExportGit { .. }
            | Commands::Verify { .. }
            | Commands::Report { .. }
            // fsck is read-only unless it's going to repair (which mutates).
            | Commands::Fsck { repair: false }
    )
}

/// Write a fresh signing key to `out` and print the public half.
///
/// The randomness is the operating system's, read here because core takes no
/// entropy source. The file is created exclusively, so an existing key is
/// never overwritten, and with mode 0600 on Unix so it is never briefly
/// readable by others.
fn keygen(out: &Path) -> Result<()> {
    use std::io::Write;

    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed)
        .map_err(|e| VeloError::Io(std::io::Error::other(format!("no system randomness: {e}"))))?;
    let key = velo_core::sign::SigningKey::from_seed(seed);
    let hex: String = seed.iter().map(|b| format!("{b:02x}")).collect();

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(out).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            VeloError::invalid(format!(
                "{} already exists; velo keygen will not overwrite a key. Choose another --out.",
                out.display()
            ))
        } else {
            VeloError::Io(e)
        }
    })?;
    file.write_all(format!("{hex}\n").as_bytes())
        .map_err(VeloError::Io)?;
    file.sync_all().map_err(VeloError::Io)?;

    render::sign::print_keygen(&key.public_key().to_string(), out);
    Ok(())
}

/// The snapshot named on the command line, or the one the working tree is on.
fn resolve_or_current(
    repo: &velo_core::Repo,
    root: &Path,
    spec: Option<&str>,
) -> Result<velo_core::SnapshotId> {
    match spec {
        Some(spec) => commands::resolve_snapshot_id(repo, spec),
        None => {
            let branch = std::fs::read_to_string(root.join(".velo/HEAD")).map_err(VeloError::Io)?;
            commands::resolve_snapshot_id(repo, branch.trim())
        }
    }
}

fn read_signing_key(path: &Path) -> Result<velo_core::sign::SigningKey> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| VeloError::invalid(format!("cannot read key file {}: {e}", path.display())))?;
    velo_core::sign::SigningKey::from_hex(text.trim())
}

/// A trust file: one public key per line, `#` comments and blank lines allowed.
fn read_trust_file(path: &Path) -> Result<velo_core::sign::TrustedKeys> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        VeloError::invalid(format!("cannot read trust file {}: {e}", path.display()))
    })?;
    let mut keys = velo_core::sign::TrustedKeys::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        keys.insert(line.parse()?);
    }
    Ok(keys)
}

/// A duration such as `90m`, `12h`, `1d` or `2w`, in milliseconds.
fn parse_duration_ms(text: &str) -> Option<i64> {
    let unit = text.chars().last()?;
    let per: i64 = match unit {
        'm' => 60_000,
        'h' => 3_600_000,
        'd' => 86_400_000,
        'w' => 7 * 86_400_000,
        _ => return None,
    };
    let n: i64 = text[..text.len() - 1].parse().ok()?;
    n.checked_mul(per)
}

/// `--since`: `YYYY-MM-DD`, an RFC 3339 timestamp, or `<n>h|d|w` ago, as epoch
/// milliseconds.
fn parse_since_at(text: &str, now_ms: i64) -> std::result::Result<i64, String> {
    let bad = || {
        format!(
            "invalid --since '{text}': expected YYYY-MM-DD, an RFC 3339 timestamp, or an age like 12h, 7d or 2w"
        )
    };
    if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        let midnight = date.and_hms_opt(0, 0, 0).ok_or_else(bad)?;
        return Ok(midnight.and_utc().timestamp_millis());
    }
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(text) {
        return Ok(t.timestamp_millis());
    }
    if text.ends_with(['h', 'd', 'w']) {
        if let Some(age) = parse_duration_ms(text) {
            return now_ms.checked_sub(age).ok_or_else(bad);
        }
    }
    Err(bad())
}

/// The clap value parser for `--since`; a bad value is a usage error.
fn parse_since_arg(text: &str) -> std::result::Result<i64, String> {
    parse_since_at(text, chrono::Utc::now().timestamp_millis())
}

/// `AGE=EVERY`, e.g. `30d=1d`, to a retention tier.
fn parse_keep(spec: &str) -> Result<commands::retention::Tier> {
    let bad = || {
        VeloError::invalid(format!(
            "invalid --keep value '{spec}': expected AGE=EVERY with durations like 90m, 12h, 1d or 2w (e.g. 30d=1d)"
        ))
    };
    let (age, every) = spec.split_once('=').ok_or_else(bad)?;
    Ok(commands::retention::Tier {
        older_than_ms: parse_duration_ms(age).ok_or_else(bad)?,
        keep_one_per_ms: parse_duration_ms(every).ok_or_else(bad)?,
    })
}

/// `N` or `N-M`, 1-based and inclusive, to a half-open range.
///
/// Parsed here rather than in core: this is argv shorthand, and core takes a
/// `Range<usize>` that says what it means.
fn parse_line_range(spec: &str) -> Result<std::ops::Range<usize>> {
    let bad = || {
        VeloError::invalid(format!(
            "'{}' is not a line range. Use N or N-M, e.g. 40-60.",
            spec
        ))
    };
    let (start, end) = match spec.split_once('-') {
        Some((a, b)) => (a.trim(), b.trim()),
        None => (spec.trim(), spec.trim()),
    };
    let start: usize = start.parse().map_err(|_| bad())?;
    let end: usize = end.parse().map_err(|_| bad())?;
    if start == 0 || end < start {
        return Err(bad());
    }
    // Inclusive on the command line, half-open in the API.
    Ok(start..end + 1)
}

/// The CLI's access policy, a small [`serve::http::Authorize`] over `Tokens`.
///
/// Which variant is active is decided once at start-up from the flags and
/// `VELO_SERVE_TOKEN`; see [`serve_http`] for the rules.
enum ServePolicy {
    /// Tokens are configured: every request needs a valid one.
    Tokens(serve::http::Tokens),
    /// No tokens: fetch is anonymous, push is refused as unauthenticated.
    AnonymousRead,
    /// Everything is allowed (the pre-5.2 behaviour, or a read-only mirror).
    Open,
}

impl serve::http::Authorize for ServePolicy {
    fn authorize(
        &self,
        repository: &str,
        operation: serve::http::Operation,
        bearer: Option<&str>,
    ) -> serve::http::Access {
        use serve::http::Access;
        match self {
            ServePolicy::Tokens(t) => t.authorize(repository, operation, bearer),
            ServePolicy::AnonymousRead if operation.is_write() => Access::Unauthenticated,
            ServePolicy::AnonymousRead | ServePolicy::Open => Access::Allow,
        }
    }
}

/// Parse a `--token-file`: lines of `read <token>` or `write <token>`, with
/// `#` comments and blank lines allowed.
fn parse_token_file(path: &str) -> Result<serve::http::Tokens> {
    let text = std::fs::read_to_string(path).map_err(VeloError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.permissions().mode() & 0o077 != 0 {
                eprintln!(
                    "warning: token file '{path}' is readable by other users; run chmod 600 on it"
                );
            }
        }
    }
    let mut tokens = serve::http::Tokens::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.split_once(char::is_whitespace) {
            Some(("read", t)) if !t.trim().is_empty() => tokens = tokens.read(t.trim()),
            Some(("write", t)) if !t.trim().is_empty() => tokens = tokens.write(t.trim()),
            _ => {
                return Err(VeloError::invalid(format!(
                    "{path}:{}: expected 'read <token>' or 'write <token>'",
                    i + 1
                )))
            }
        }
    }
    Ok(tokens)
}

/// Options of `velo serve-http`, gathered from the flags.
struct ServeHttpOptions<'a> {
    path: &'a str,
    listen: &'a str,
    directory: bool,
    token_file: Option<&'a str>,
    read_only: bool,
    allow_anonymous_push: bool,
    max_request_bytes: u64,
    request_timeout: u64,
    max_connections: usize,
}

/// A refusal made by the front end itself, in the core's `velo: <kind>: <detail>` form.
fn front_refusal(
    status: u16,
    kind: &str,
    detail: &str,
) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    tiny_http::Response::from_data(format!("velo: {kind}: {detail}\n").into_bytes())
        .with_status_code(status)
        .with_header(
            tiny_http::Header::from_bytes("Content-Type", "text/plain").expect("static header"),
        )
}

/// Why reading a request body stopped early.
enum BodyError {
    TooLarge,
    Timeout,
    Io(std::io::Error),
}

/// Read a body in chunks, refusing past `max` bytes (covers chunked bodies
/// that carry no Content-Length) or once `timeout` has elapsed. A socket read
/// that never returns is not interrupted: tiny_http has no read timeout, so
/// that case is bounded only by the proxy in front.
fn read_body(
    reader: &mut dyn std::io::Read,
    max: u64,
    timeout: std::time::Duration,
) -> std::result::Result<Vec<u8>, BodyError> {
    let start = std::time::Instant::now();
    let mut limited = std::io::Read::take(reader, max.saturating_add(1));
    let mut body = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        if start.elapsed() > timeout {
            return Err(BodyError::Timeout);
        }
        let n = std::io::Read::read(&mut limited, &mut chunk).map_err(BodyError::Io)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
        if body.len() as u64 > max {
            return Err(BodyError::TooLarge);
        }
    }
    Ok(body)
}

/// Serve one received request: size and time limits, then the core service.
fn serve_one(
    mut req: tiny_http::Request,
    service: &serve::http::Service,
    limits: (u64, std::time::Duration),
) {
    let (max, timeout) = limits;
    let method = req.method().to_string();
    let full = req.url().to_string();
    let too_large = || {
        front_refusal(
            413,
            "too-large",
            &format!("request body exceeds {max} bytes"),
        )
    };

    if req.body_length().is_some_and(|n| n as u64 > max) {
        eprintln!("{method} {full} -> 413");
        let _ = req.respond(too_large());
        return;
    }
    let body = match read_body(req.as_reader(), max, timeout) {
        Ok(b) => b,
        Err(BodyError::TooLarge) => {
            eprintln!("{method} {full} -> 413");
            let _ = req.respond(too_large());
            return;
        }
        Err(BodyError::Timeout) => {
            eprintln!("{method} {full} -> 408");
            let _ = req.respond(front_refusal(
                408,
                "timeout",
                &format!("request not received within {} seconds", timeout.as_secs()),
            ));
            return;
        }
        Err(BodyError::Io(e)) => {
            eprintln!("{method} {full} -> 400: {e}");
            let _ = req.respond(front_refusal(
                400,
                "error",
                "the request body could not be read",
            ));
            return;
        }
    };
    // The Authorization header is passed through but never logged.
    let auth = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str().to_string());
    let resp = service.handle(&serve::http::Request::new(
        &method,
        &full,
        auth.as_deref(),
        &body,
    ));
    eprintln!(
        "{method} {full} -> {} ({} bytes)",
        resp.status,
        resp.body.len()
    );
    let mut out = tiny_http::Response::from_data(resp.body)
        .with_status_code(resp.status)
        .with_header(
            tiny_http::Header::from_bytes("Content-Type", resp.content_type)
                .expect("static header"),
        );
    if resp.www_authenticate {
        out = out.with_header(
            tiny_http::Header::from_bytes("WWW-Authenticate", "Bearer").expect("static header"),
        );
    }
    let _ = req.respond(out);
}

/// Releases one active-request slot when dropped, panic or not.
struct ActiveSlot<'a>(&'a std::sync::atomic::AtomicUsize);

impl Drop for ActiveSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Run the reference HTTP server until the process is killed.
///
/// Known limit: tiny_http reads and discards the unread remainder of a request
/// body when it drops the request, even after a 413, allocating a buffer of
/// the declared Content-Length. A panic from that (an absurd length) is caught
/// per request, but a merely huge length can make the allocator abort the
/// process, and tiny_http offers no way to answer without the drain. The
/// reverse proxy's `client_max_body_size` (or equivalent) is therefore
/// required, not optional, when this server faces untrusted clients.
///
/// A front end over the core `Service`: it owns the socket, the connection
/// pool, and the body size and time limits; routing, authorization and
/// read-only are the service's. Policy: with tokens (a `--token-file` or
/// `VELO_SERVE_TOKEN`) every request needs a valid one; with none, fetch is
/// anonymous and push is refused unless `--allow-anonymous-push`. The only
/// thing written to stdout is the single `listening on` line, so a caller can
/// read it to learn the port chosen for `--listen ...:0`.
fn serve_http(o: ServeHttpOptions<'_>) -> Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let root = Path::new(o.path);
    if !o.directory {
        // Fail at start-up, not on the first request, if this is not a repository.
        serve::http::refs(root)?;
    } else if !root.is_dir() {
        return Err(VeloError::invalid(format!(
            "'{}' is not a directory",
            o.path
        )));
    }

    let mut tokens = match o.token_file {
        Some(f) => parse_token_file(f)?,
        None => serve::http::Tokens::new(),
    };
    let mut have_tokens = o.token_file.is_some();
    if let Ok(t) = std::env::var("VELO_SERVE_TOKEN") {
        if !t.is_empty() {
            tokens = tokens.write(t);
            have_tokens = true;
        }
    }
    let (policy, described) = if have_tokens {
        (
            ServePolicy::Tokens(tokens),
            "a bearer token is required for every request",
        )
    } else if o.read_only || o.allow_anonymous_push {
        (
            ServePolicy::Open,
            if o.read_only {
                "anonymous fetch; push refused (read-only)"
            } else {
                "anonymous fetch and anonymous push (no authentication)"
            },
        )
    } else {
        (
            ServePolicy::AnonymousRead,
            "anonymous fetch; push refused (set VELO_SERVE_TOKEN or pass --token-file to enable it)",
        )
    };
    let mut described = described.to_string();
    if have_tokens && o.read_only {
        described.push_str("; read-only, pushes are refused");
    }
    eprintln!("velo serve-http: {described}");

    let limits = velo_core::Limits::default().with_max_pack_bytes(o.max_request_bytes);
    let base = if o.directory {
        serve::http::Service::directory(root)
    } else {
        serve::http::Service::single(root)
    };
    let service = Arc::new(
        base.authorize_with(policy)
            .read_only(o.read_only)
            .limits(limits),
    );

    let io_err = |e: Box<dyn std::error::Error + Send + Sync>| {
        VeloError::Io(std::io::Error::other(e.to_string()))
    };
    let server = Arc::new(tiny_http::Server::http(o.listen).map_err(io_err)?);
    let addr = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| VeloError::Io(std::io::Error::other("server has no IP address")))?;
    {
        let mut out = std::io::stdout().lock();
        writeln!(out, "listening on http://{addr}/").map_err(VeloError::Io)?;
        out.flush().map_err(VeloError::Io)?;
    }

    // `max_connections` workers serve requests; one extra thread exists only
    // to answer 503 when they are all busy, since tiny_http would otherwise
    // queue the request past its timeout.
    let max_conn = o.max_connections.max(1);
    let active = Arc::new(AtomicUsize::new(0));
    let limits = (
        o.max_request_bytes,
        std::time::Duration::from_secs(o.request_timeout),
    );
    let mut threads = Vec::new();
    for _ in 0..=max_conn {
        let (server, service, active) = (server.clone(), service.clone(), active.clone());
        threads.push(std::thread::spawn(move || {
            while let Ok(req) = server.recv() {
                if active.fetch_add(1, Ordering::SeqCst) >= max_conn {
                    active.fetch_sub(1, Ordering::SeqCst);
                    eprintln!("{} {} -> 503", req.method(), req.url());
                    // Dropping a Request drains its unread body in tiny_http,
                    // which blocks on a stalled client. Do that off this
                    // thread so it goes straight back to `recv()`.
                    std::thread::spawn(move || {
                        let _ = req.respond(front_refusal(
                            503,
                            "busy",
                            "all connections are in use; retry shortly",
                        ));
                    });
                    continue;
                }
                // The guard frees the slot even if the request panics, and
                // catch_unwind keeps the worker alive: tiny_http's drain of an
                // oversized body can panic (capacity overflow) on drop.
                let _slot = ActiveSlot(&active);
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    serve_one(req, &service, limits);
                }));
            }
        }));
    }
    for t in threads {
        let _ = t.join();
    }
    Ok(())
}

fn run(cli: Cli) -> Result<Exit> {
    let current_dir = std::env::current_dir().map_err(VeloError::Io)?;

    // Commands that don't require (or create) an enclosing repo run first.
    // The serve-* commands operate on an explicit path and speak a binary
    // protocol on stdout — they must emit nothing else.
    match &cli.command {
        Commands::Init { objects } => {
            let location: velo_core::ObjectLocation = (*objects).into();
            let options = velo_core::InitOptions::new().objects(location);
            render::init::print(&commands::init::run_with(&current_dir, options)?, location);
            return Ok(Exit::Success);
        }
        Commands::Clone { url, dir } => {
            let cloned = commands::sync::clone_with_token(
                url,
                &spawn_config()?,
                commands::sync::CloneOptions {
                    dir: dir.as_deref().map(Path::new),
                    observer: Some(Box::new(render::progress::Bar::new())),
                    // No cancel: Ctrl-C already ends a terminal process. The flag
                    // is for an embedder whose user closes a window instead.
                    ..Default::default()
                },
                token_for("origin").as_deref(),
            )?;
            render::sync::print_cloned(&cloned);
            return Ok(Exit::Success);
        }
        Commands::Keygen { out } => {
            keygen(out)?;
            return Ok(Exit::Success);
        }
        Commands::ServeUpload { path } => return serve::upload(path).map(|()| Exit::Success),
        Commands::ServeReceive { path } => return serve::receive(path).map(|()| Exit::Success),
        Commands::ServeHttp {
            path,
            listen,
            directory,
            token_file,
            read_only,
            allow_anonymous_push,
            max_request_bytes,
            request_timeout,
            max_connections,
        } => {
            return serve_http(ServeHttpOptions {
                path,
                listen,
                directory: *directory,
                token_file: token_file.as_deref(),
                read_only: *read_only,
                allow_anonymous_push: *allow_anonymous_push,
                max_request_bytes: *max_request_bytes,
                request_timeout: *request_timeout,
                max_connections: *max_connections,
            })
            .map(|()| Exit::Success)
        }
        _ => {}
    }

    let root =
        commands::find_repo_root(&current_dir).ok_or_else(|| not_a_repo_hint(&cli.command))?;

    // One connection for the whole command, and — because this goes through
    // `Repo` rather than opening SQLite directly — the point where a repository
    // written by a newer Velo is refused instead of half-read.
    // Long operations report through this. Inert on a non-TTY, so piped output
    // stays clean.
    let mut exit = Exit::Success;
    let repo = velo_core::Repo::open_and_migrate(&root)?
        .lock_label(lockwait::command_label())
        .observing(render::progress::Bar::new())
        .merging(
            velo_core::Drivers::new()
                .with("*.json", velo_merge::JsonDriver)?
                .with("*.yaml", velo_merge::YamlDriver)?
                .with("*.yml", velo_merge::YamlDriver)?
                .with("*.toml", velo_merge::TomlDriver)?,
        );
    // Bearer tokens for HTTP remotes come from the environment, looked up by
    // the remote's name. The provider is handed a URL, so map URLs to names once.
    let remote_names: Vec<(String, String)> = commands::remote::list(&repo)
        .map(|l| l.into_iter().map(|r| (r.url, r.name)).collect())
        .unwrap_or_default();
    let repo = repo.credentials(move |url| {
        remote_names
            .iter()
            .find(|(u, _)| u == url)
            .and_then(|(_, name)| token_for(name))
    });

    // Serialise mutating commands against other velo processes. Read-only
    // commands skip the lock so they never block on a long-running mutation.
    // The guard is held until the command returns, and is what grants write
    // access to core at all.
    let guard = if needs_write_guard(&cli.command) {
        Some(lockwait::acquire(&repo)?)
    } else {
        None
    };
    // Mutating arms need the guard; `is_read_only` above decides which is which,
    // so a mismatch here is a programming error rather than a user-visible one.
    let write = || -> &velo_core::WriteGuard<'_> {
        guard
            .as_ref()
            .expect("a mutating command must hold the write guard")
    };

    warn_if_rewriting_pushed(&repo, &root, &cli.command);

    match cli.command {
        Commands::Init { .. }
        | Commands::Clone { .. }
        | Commands::Keygen { .. }
        | Commands::ServeUpload { .. }
        | Commands::ServeReceive { .. }
        | Commands::ServeHttp { .. } => unreachable!(),

        Commands::Save {
            message,
            amend,
            paths,
        } => {
            let paths: Vec<&Path> = paths.iter().map(Path::new).collect();
            let outcome = commands::save::run(
                write(),
                message.as_deref(),
                commands::save::Options {
                    amend,
                    paths: &paths,
                    author: author::from_env()?.as_ref(),
                    ..Default::default()
                },
            )?;
            let branch = std::fs::read_to_string(root.join(".velo/HEAD"))
                .unwrap_or_default()
                .trim()
                .to_string();
            render::save::print(&outcome, &branch, amend);
            if matches!(outcome, commands::save::Outcome::NothingToSave) {
                if let Some(o) = commands::redo::peek(&repo)? {
                    println!(
                        "  Your last velo undo removed {} \"{}\" — velo redo brings it back.",
                        console::style(render::id::short(&o.snapshot)).yellow(),
                        o.message
                    );
                }
            }
        }

        Commands::Restore {
            target,
            force,
            paths,
        } => {
            let hash = resolve_spec(&repo, &target)?;
            let paths: Vec<&Path> = paths.iter().map(Path::new).collect();
            render::restore::print(&commands::restore::run(
                write(),
                &hash,
                commands::restore::Options {
                    force,
                    paths: &paths,
                    ..Default::default()
                },
            )?);
        }

        Commands::Status { paths } => render::status::print(&commands::status::run(&repo, &paths)?),

        Commands::History {
            all,
            limit,
            branch,
            from,
            oneline,
            graph,
            file_filter,
            meta_filter,
        } => {
            // The flags choose a presentation; core just returns the entries.
            let view = if graph {
                render::history::View::Graph
            } else if oneline {
                render::history::View::Oneline
            } else {
                render::history::View::Full
            };
            // Parsed here, at the argv boundary, so a malformed branch name is
            // a clear error rather than a query that quietly matches nothing.
            // A remote-tracking branch (`origin/main`, or `remotes/origin/main`
            // as `--all` prints it) is not a local branch, so it is shown as
            // the ancestry of the snapshot it points at.
            let mut from = from;
            let mut branch = branch;
            if let Some(b) = branch.clone() {
                let bare = b.strip_prefix("remotes/").unwrap_or(&b).to_string();
                let is_local = commands::branches::list(&repo)?
                    .iter()
                    .any(|x| x.name.as_str() == b);
                if !is_local && bare.contains('/') {
                    if let Ok(id) = resolve_spec(&repo, &bare) {
                        from = Some(id.to_string());
                        branch = None;
                    }
                }
            }
            let branch = branch.map(|b| b.parse::<BranchName>()).transpose()?;
            let from = from.map(|t| resolve_spec(&repo, t.as_str())).transpose()?;
            let file_paths: Vec<&Path> = file_filter.iter().map(Path::new).collect();
            let meta = meta_filter
                .iter()
                .map(|w| parse_where(w))
                .collect::<Result<Vec<_>>>()?;
            let history = commands::history::run(
                &repo,
                commands::history::Options {
                    all,
                    branch: branch.as_ref(),
                    from: from.as_ref(),
                    paths: &file_paths,
                    meta: &meta,
                    limit: Some(limit),
                },
            )?;
            let signed = velo_core::sign::signed_snapshots(&repo)?;
            render::history::print(&history, view, &signed);
        }

        Commands::Mv { from, to } => {
            render::mv::print(&commands::mv::run(
                write(),
                Path::new(&from),
                Path::new(&to),
            )?);
        }

        Commands::Undo => {
            let outcome = match commands::undo::run(write()) {
                Ok(o) => o,
                // Nothing to undo is not a failure: say so and exit 0.
                Err(e @ VeloError::NothingToUndo { .. }) => {
                    println!("{e}");
                    return Ok(Exit::Success);
                }
                Err(e) => return Err(e),
            };
            // Undo leaves `remote_refs` alone, so a tracking ref may still name
            // the snapshot just removed; say so rather than let it look gone.
            let tracking = commands::sync::tracking_refs_at(
                &repo,
                &outcome.snapshot.parse::<velo_core::SnapshotId>()?,
            )?;
            render::undo::print(&outcome, &tracking);
        }

        Commands::Redo => match commands::redo::run(write()) {
            Ok(o) => render::undo::print_redo(&o),
            Err(e @ VeloError::NothingToRedo { .. }) => println!("{e}"),
            Err(e) => return Err(e),
        },

        Commands::Diff { args, paths } => {
            render::diff::print_comparison(&diffargs::dispatch(&repo, &args, &paths)?);
        }

        Commands::Show { target, paths } => {
            let id = resolve_spec(&repo, &target)?;
            let paths: Vec<&Path> = paths.iter().map(Path::new).collect();
            let signed = velo_core::sign::signed_snapshots(&repo)?;
            render::diff::print_snapshot(
                &commands::show::run(&repo, &id, &paths)?,
                signed.contains(&id),
            );
        }

        Commands::Sign { snapshot, key } => {
            let id = resolve_or_current(&repo, &root, snapshot.as_deref())?;
            let key = read_signing_key(&key)?;
            let sig = velo_core::sign::sign(write(), &id, &key)?;
            render::sign::print_signed(&id, &sig.public_key);
        }

        Commands::Verify { snapshot, trust } => {
            let id = resolve_or_current(&repo, &root, snapshot.as_deref())?;
            let policy = read_trust_file(&trust)?;
            let verification = velo_core::sign::verify(&repo, &id, &policy)?;
            render::sign::print_verification(&verification);
            if !verification.is_trusted() {
                exit = Exit::Untrusted;
            }
        }

        Commands::Report {
            branch,
            since,
            output,
            json,
            trust,
            no_fsck,
        } => {
            let branch = branch.map(|b| b.parse::<BranchName>()).transpose()?;
            let policy = trust.as_deref().map(read_trust_file).transpose()?;
            let mut options = commands::report::Options::new().fsck(!no_fsck);
            if let Some(b) = branch.as_ref() {
                options = options.branch(b);
            }
            if let Some(ms) = since {
                options = options.since_ms(ms);
            }
            if let Some(p) = policy.as_ref() {
                options = options.trust(p);
            }
            let report = commands::report::build(&repo, options)?;
            let generated = render::report::format_ms(chrono::Utc::now().timestamp_millis());
            let repo_name = root.display().to_string();
            let write_file = |path: &Path, text: &str| {
                std::fs::write(path, text).map_err(|e| {
                    VeloError::invalid(format!("cannot write {}: {e}", path.display()))
                })
            };
            if json {
                let value = render::report::json(&report, &repo_name, &generated);
                let text = serde_json::to_string_pretty(&value)
                    .map_err(|e| VeloError::invalid(format!("cannot encode report: {e}")))?;
                match output {
                    Some(path) => {
                        write_file(&path, &text)?;
                        render::report::print_written(&path);
                    }
                    None => println!("{text}"),
                }
            } else {
                let path = output.unwrap_or_else(|| PathBuf::from("velo-report.html"));
                let view = render::report::view(&report, &repo_name, &generated);
                write_file(&path, &render::report::html(&view))?;
                render::report::print_written(&path);
            }
        }

        Commands::CherryPick { target } => {
            let target = resolve_spec(&repo, &target)?;
            let outcome =
                commands::cherry_pick::run(write(), &target, author::from_env()?.as_ref())?;
            render::cherry_pick::print(&outcome);
            if !outcome.applied.conflicts().is_empty() {
                exit = Exit::Conflicts;
            }
        }

        Commands::Switch {
            name,
            force,
            create,
        } => {
            let existing: Vec<String> = commands::branches::list(&repo)?
                .iter()
                .map(|b| b.name.as_str().to_string())
                .collect();
            let exists = existing.contains(&name);
            if create && exists {
                return Err(VeloError::invalid(format!(
                    "branch '{name}' already exists; switch to it with: velo switch {name}"
                )));
            }
            let mut name = name;
            if !create && !exists {
                if let Some(close) = near_miss(&name, &existing) {
                    use std::io::IsTerminal;
                    let close = close.to_string();
                    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                        print!(
                            "No branch '{name}'. Did you mean '{close}'? \
                             [y = switch to {close} / c = create {name} / N = cancel] "
                        );
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                        let mut answer = String::new();
                        let _ = std::io::stdin().read_line(&mut answer);
                        match answer.trim().to_lowercase().as_str() {
                            "y" => name = close,
                            "c" => {}
                            _ => {
                                println!("Cancelled.");
                                return Ok(Exit::Success);
                            }
                        }
                    } else {
                        eprintln!(
                            "{} no branch '{name}'; did you mean '{close}'? \
                             To create it: velo switch --create {name}",
                            console::style("error:").red().bold()
                        );
                        return Ok(Exit::Refused);
                    }
                }
            }
            render::switch::print(&commands::switch::run(write(), &name, force)?);
        }

        Commands::Branches { delete } => match delete {
            Some(name) => {
                let name: BranchName = name.parse()?;
                commands::branches::delete(write(), &name)?;
                render::branches::print_deleted(&name);
            }
            None => render::branches::print_list(&commands::branches::list(&repo)?),
        },

        Commands::Tag {
            name,
            snapshot,
            delete,
            force,
        } => {
            // Three operations behind one subcommand: delete wins, then
            // create, then the bare listing.
            if let Some(name) = delete {
                let name: TagName = name.parse()?;
                commands::tag::delete(write(), &name)?;
                render::tag::print_deleted(&name);
            } else if let Some(name) = name {
                let name: TagName = name.parse()?;
                // The user types a spec; resolving it here means `tag::create`
                // takes the id it actually needs.
                let target = snapshot
                    .as_deref()
                    .map(|spec| resolve_spec(&repo, spec))
                    .transpose()?;
                let created = commands::tag::create(write(), &name, target.as_ref(), force)?;
                render::tag::print_created(&created);
            } else {
                render::tag::print_list(&commands::tag::list(&repo)?);
            }
        }

        Commands::Merge { branch, abort } => {
            let mode = match (abort, branch.as_deref()) {
                (true, _) => commands::merge::Mode::Abort,
                (false, Some(source)) => commands::merge::Mode::Bring { source },
                (false, None) => {
                    return Err(error::Error::invalid(
                        "Specify a branch to merge: velo merge <branch>",
                    ))
                }
            };
            let outcome = commands::merge::run(write(), mode)?;
            // A fast-forward records the source tip as its second parent, and the
            // merge does not move the source, so its tip is that parent.
            let second_parent = match (&outcome, branch.as_deref()) {
                (commands::merge::Outcome::FastForwarded { .. }, Some(source)) => {
                    resolve_spec(&repo, source)
                        .ok()
                        .map(|id| (source, id.as_str().to_string()))
                }
                _ => None,
            };
            render::merge::print(
                &outcome,
                second_parent.as_ref().map(|(b, id)| (*b, id.as_str())),
            );
            if let commands::merge::Outcome::Merged(result) = &outcome {
                if !result.conflicts().is_empty() {
                    exit = Exit::Conflicts;
                } else if result.is_clean() && result.applied_nothing() {
                    // The trees were already equal but the histories diverged:
                    // there is nothing for a human to review, so record the
                    // merge now. Otherwise the next push is refused again.
                    let source = branch.as_deref().unwrap_or_default();
                    let saved = commands::save::run(
                        write(),
                        Some(&format!("Merge {source}")),
                        commands::save::Options {
                            author: author::from_env()?.as_ref(),
                            ..Default::default()
                        },
                    )?;
                    if let Some(r) = saved.result() {
                        println!(
                            "{} Recorded merge {} with no content change — the trees were already equal, so this only joins the histories. velo push will now fast-forward.",
                            console::style("✔").green(),
                            console::style(render::id::short(&r.hash)).yellow()
                        );
                    }
                }
            }
        }

        Commands::Resolve {
            file,
            take,
            all,
            force,
            mark,
        } => {
            if force && !mark && take.is_none() {
                return Err(VeloError::invalid("--force needs --take or --mark"));
            }
            if mark {
                run_mark(write(), file.as_deref().unwrap_or_default(), force)?;
            } else {
                // Interactive resolve holds no guard here; the TUI takes the
                // lock for one decision at a time.
                let guard = if take.is_some() { Some(write()) } else { None };
                run_resolve(
                    &repo,
                    guard,
                    file.as_deref(),
                    take.map(Into::into),
                    all,
                    force,
                )?;
            }
        }

        Commands::Conflicts { file, json } => {
            run_conflicts(&repo, file.as_deref(), json)?;
        }

        Commands::Stash { sub } => match sub {
            StashSub::Push { name, message } => {
                render::stash::print_pushed(&commands::stash::push(write(), name.or(message))?)
            }
            StashSub::List => render::stash::print_list(&commands::stash::list(&repo)?),
            StashSub::Pop { name } => {
                render::stash::print_popped(&commands::stash::pop(write(), name)?)
            }
            StashSub::Drop { name } => {
                render::stash::print_dropped(&commands::stash::drop_shelf(write(), name)?)
            }
            StashSub::Show { name } => {
                render::stash::print_shelf(&commands::stash::show_shelf(&repo, name)?)
            }
        },

        Commands::Blame { file, at, lines } => {
            let at = at
                .as_deref()
                .map(|spec| resolve_spec(&repo, spec))
                .transpose()?;
            let lines = lines.as_deref().map(parse_line_range).transpose()?;
            let asked = lines.is_some();
            let blame = commands::blame::run(
                &repo,
                Path::new(&file),
                commands::blame::Options {
                    at: at.as_ref(),
                    lines,
                    ..Default::default()
                },
            )?;
            render::blame::print(&blame);
            if asked && blame.lines.is_empty() {
                // A range past the end of the file selects nothing; say how long
                // the file is so the silence is not mistaken for a failure.
                if let Ok(bytes) = repo.read_file_at(&blame.snapshot, &blame.path) {
                    let nl = bytes.iter().filter(|b| **b == b'\n').count();
                    let n = nl + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
                    eprintln!("file has {n} lines");
                }
            }
        }

        Commands::ExportGit { output, branch } => {
            let branches: Vec<BranchName> = branch
                .iter()
                .map(|b| b.parse::<BranchName>())
                .collect::<std::result::Result<_, _>>()?;
            let refs: Vec<&BranchName> = branches.iter().collect();
            let options = commands::export::Options {
                branches: &refs,
                ..Default::default()
            };
            match output {
                Some(path) => {
                    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
                    commands::export::git_fast_import(&repo, &mut file, options)?;
                }
                None => {
                    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
                    commands::export::git_fast_import(&repo, &mut out, options)?;
                }
            }
        }

        Commands::ImportGit { input } => {
            let imported = match input {
                Some(path) => {
                    let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
                    commands::import::git_fast_export(write(), &mut file, Default::default())?
                }
                None => {
                    let mut stdin = std::io::stdin().lock();
                    commands::import::git_fast_export(write(), &mut stdin, Default::default())?
                }
            };
            let mut line = format!(
                "Imported {} commit(s), {} branch(es), {} tag(s).",
                imported.commits, imported.branches, imported.tags
            );
            if imported.skipped_submodules > 0 {
                line.push_str(&format!(
                    " Skipped {} submodule(s).",
                    imported.skipped_submodules
                ));
            }
            if imported.mismatched_ids > 0 {
                line.push_str(&format!(
                    " {} snapshot id(s) differ from their Velo-Snapshot trailer.",
                    imported.mismatched_ids
                ));
            }
            println!("{line}");
        }

        Commands::Grep {
            pattern,
            snapshot,
            ignore_case,
            files_only,
            context,
        } => {
            let snapshot = snapshot
                .as_deref()
                .map(|spec| resolve_spec(&repo, spec))
                .transpose()?;
            let results = commands::grep::run(
                &repo,
                &pattern,
                commands::grep::Options {
                    snapshot: snapshot.as_ref(),
                    case_insensitive: ignore_case,
                    names_only: files_only,
                    context,
                },
            )?;
            render::grep::print(&results, files_only);
        }

        Commands::Squash { count, message } => {
            render::squash::print(&commands::squash::run(write(), count, &message)?);
        }

        Commands::Rebase {
            target,
            abort,
            cont,
        } => {
            // The three real modes, chosen once here rather than encoded in a
            // pair of booleans the core has to disentangle.
            let onto;
            let mode = match (abort, cont, target.as_deref()) {
                (true, _, _) => commands::rebase::Mode::Abort,
                (false, true, _) => commands::rebase::Mode::Continue,
                (false, false, Some(spec)) => {
                    onto = resolve_spec(&repo, spec)?;
                    commands::rebase::Mode::Start { onto: &onto }
                }
                (false, false, None) => {
                    return Err(error::Error::invalid(
                        "Specify a target branch, or use --abort / --continue.",
                    ))
                }
            };
            let outcome = commands::rebase::run(write(), mode, author::from_env()?.as_ref())?;
            render::rebase::print(&outcome);
            if matches!(outcome, commands::rebase::Outcome::Paused { .. }) {
                exit = Exit::Conflicts;
            }
        }

        Commands::Gc {
            keep_days,
            dry_run,
            vacuum,
        } => {
            if dry_run && vacuum {
                return Err(VeloError::invalid(
                    "--vacuum cannot be combined with --dry-run: a vacuum changes the database",
                ));
            }
            render::gc::print(&commands::gc::run(
                write(),
                commands::gc::Options {
                    keep_days,
                    dry_run,
                    ..Default::default()
                },
            )?);
            if !dry_run {
                let mode = if vacuum {
                    commands::gc::Vacuum::Full
                } else {
                    commands::gc::Vacuum::Incremental
                };
                render::gc::print_vacuum(&commands::gc::vacuum(write(), mode)?);
            }
        }

        Commands::Repack { dry_run } => {
            let bar = render::progress::Bar::new();
            let done = lockwait::retry_locked(|| {
                let mut options = commands::repack::Options::default();
                options.dry_run = dry_run;
                options.observer = Some(&bar);
                commands::repack::run(&repo, options)
            })?;
            render::repack::print(&done);
        }

        Commands::Retention {
            branch,
            keep,
            dry_run,
            keep_days,
        } => run_retention(write(), &root, branch.as_deref(), &keep, dry_run, keep_days)?,

        Commands::Fsck { repair } => {
            let report = if repair {
                commands::fsck::repair(write())?
            } else {
                commands::fsck::check(&repo)?
            };
            render::fsck::print(&report);
            // Corruption must be visible to scripts, so it sets the exit code.
            // Cruft alone does not — it is cleanable, not damage.
            if !report.is_healthy() {
                if report.problems.iter().all(|p| p.is_remote_tracking()) {
                    return Err(VeloError::invalid(format!(
                        "{} problem(s), all in remote-tracking history (remotes/\u{2026}); your own branches are intact \u{2014} run velo fetch to refresh",
                        report.problems.len()
                    )));
                }
                return Err(VeloError::corrupt(format!(
                    "{} integrity problem(s) found",
                    report.problems.len()
                )));
            }
        }

        Commands::Bundle { cmd } => match cmd {
            BundleSub::Create { file, target } => {
                let target = target
                    .as_deref()
                    .map(|spec| resolve_spec(&repo, spec))
                    .transpose()?;
                render::bundle::print_created(&commands::bundle::create(
                    &repo,
                    Path::new(&file),
                    target.as_ref(),
                )?);
            }
            BundleSub::Apply { file } => {
                render::bundle::print_applied(&commands::bundle::apply(write(), Path::new(&file))?);
            }
        },

        Commands::Fetch { remote } => {
            render::sync::print_fetched(&commands::sync::fetch(
                write(),
                &remote,
                &spawn_config()?,
                Default::default(),
            )?);
        }
        Commands::Push { remote, branch } => {
            let pushed = commands::sync::push_report(
                write(),
                &remote,
                branch.as_deref(),
                &spawn_config()?,
                Default::default(),
            );
            match pushed {
                Ok(report) => render::sync::print_push_report(&report),
                Err(e) => {
                    // The refused push has just refreshed the tracking ref to the
                    // advertised tip, whose snapshots were usually never fetched.
                    // Say that; mention an undo only when the id is in the trash.
                    if let VeloError::NotFastForward { branch, remote } = &e {
                        let spec = format!("{remote}/{branch}");
                        if let Err(VeloError::RemoteRefAbsent { snapshot, .. }) =
                            resolve_spec(&repo, &spec)
                        {
                            let short = render::id::short(snapshot.as_str());
                            if commands::is_trashed(&repo, snapshot.as_str()).unwrap_or(false) {
                                eprintln!(
                                    "{spec} is at {short}, which was undone here; velo sync (or velo fetch, then velo merge {spec}) brings it back"
                                );
                            } else {
                                eprintln!(
                                    "{spec} is at {short}, not fetched yet; velo sync (or velo fetch, then velo merge {spec})"
                                );
                            }
                        }
                    }
                    return Err(e);
                }
            }
        }
        Commands::Sync { remote, branch } => {
            let author = author::from_env()?;
            let synced =
                commands::sync::sync(write(), &remote, branch.as_deref(), &spawn_config()?, {
                    let options = commands::sync::SyncOptions::default();
                    match author.as_ref() {
                        Some(a) => options.author(a),
                        None => options,
                    }
                })?;
            render::sync::print_synced(&synced);
            if matches!(synced, commands::sync::Synced::Conflicted { .. }) {
                exit = Exit::Conflicts;
            }
        }
        Commands::Pull { remote } => {
            let pulled =
                commands::sync::pull(write(), &remote, &spawn_config()?, Default::default())?;
            render::sync::print_pulled(&pulled);
            if matches!(pulled, commands::sync::Pulled::Diverged { .. }) {
                exit = Exit::Diverged;
            }
        }
        Commands::Remote { cmd } => match cmd {
            None => render::remote::print_list(&commands::remote::list(&repo)?),
            Some(RemoteSub::Add { name, url }) => {
                render::remote::print_added(&commands::remote::add(write(), &name, &url)?)
            }
            Some(RemoteSub::Remove { name }) => {
                commands::remote::remove(write(), &name)?;
                render::remote::print_removed(&name);
            }
        },
    }

    Ok(exit)
}

/// Parse `NS:KEY=VALUE` or `NS:KEY` into a metadata filter.
///
/// Split on the first `:` and then the first `=`, so a value may itself contain
/// either character.
fn parse_where(text: &str) -> Result<commands::history::MetaFilter<'_>> {
    let shape = "Expected --where NAMESPACE:KEY=VALUE or NAMESPACE:KEY";
    let Some((namespace, rest)) = text.split_once(':') else {
        return Err(VeloError::invalid(format!(
            "'{text}' is not a metadata filter. {shape}."
        )));
    };
    let (key, value) = match rest.split_once('=') {
        Some((key, value)) => (key, Some(value)),
        None => (rest, None),
    };
    if namespace.is_empty() || key.is_empty() {
        return Err(VeloError::invalid(format!(
            "'{text}' has an empty namespace or key. {shape}."
        )));
    }
    Ok(match value {
        Some(value) => commands::history::MetaFilter::equals(namespace, key, value),
        None => commands::history::MetaFilter::has(namespace, key),
    })
}

/// `velo retention`: thin a branch, then gc and vacuum.
///
/// Kept out of `run` because that function's frame is already large and a
/// debug build on Windows overflows the main thread's stack if it grows.
#[inline(never)]
fn run_retention(
    guard: &velo_core::WriteGuard<'_>,
    root: &Path,
    branch: Option<&str>,
    keep: &[String],
    dry_run: bool,
    keep_days: u32,
) -> Result<()> {
    let branch: BranchName = match branch {
        Some(b) => b.parse()?,
        None => std::fs::read_to_string(root.join(".velo/HEAD"))
            .map_err(|e| VeloError::invalid(format!("cannot read .velo/HEAD: {e}")))?
            .trim()
            .parse()?,
    };
    let policy = if keep.is_empty() {
        commands::retention::Policy::default()
    } else {
        commands::retention::Policy {
            tiers: keep
                .iter()
                .map(|k| parse_keep(k))
                .collect::<Result<Vec<_>>>()?,
        }
    };
    let report = commands::retention::apply(
        guard,
        &branch,
        &policy,
        commands::retention::Options {
            dry_run,
            ..Default::default()
        },
    )?;
    render::retention::print(&report, dry_run);
    if !dry_run && report.removed > 0 {
        render::gc::print(&commands::gc::run(
            guard,
            commands::gc::Options {
                keep_days,
                ..Default::default()
            },
        )?);
        render::gc::print_vacuum(&commands::gc::vacuum(
            guard,
            commands::gc::Vacuum::Incremental,
        )?);
    }
    Ok(())
}
