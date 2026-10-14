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
  0  success (including nothing to save, already up to date, nothing to pull)
  1  error
  2  command-line usage error
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
    /// Example
    ///   velo init
    #[command(
        verbatim_doc_comment,
        after_help = "\
NOTES
    · .velo/ is never tracked — it is automatically excluded.
    · The default branch is called 'main'.
    · Edit .veloignore to exclude build artefacts, secrets, etc."
    )]
    Init,

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
    /// Restores the working tree to the latest snapshot on the target
    /// branch.  Aborts if there are unsaved changes unless --force is used.
    ///
    /// Examples
    ///   velo switch feature/auth    # switch (creates if new)
    ///   velo switch main --force    # discard unsaved changes and switch
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
    · Stale file_map rows (snapshot was deleted)
    · Stale index_cache rows (path no longer tracked)
    · Trash entries older than --keep-days days

NOTES
    · Running gc while a merge is in progress is safe.
    · The operation is idempotent — running it twice is harmless."
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
    #[command(verbatim_doc_comment)]
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
    /// A reference server: it handles one request at a time and has NO
    /// authentication and no TLS. Anyone who can reach the port can read
    /// the history and push to it. Put it behind a reverse proxy that
    /// authenticates and terminates TLS for anything public.
    ///
    ///   velo serve-http /srv/project
    ///   velo serve-http /srv/project --listen 0.0.0.0:8417
    #[command(verbatim_doc_comment)]
    ServeHttp {
        #[arg(value_name = "PATH")]
        path: String,
        /// Address to listen on (port 0 picks a free one).
        #[arg(long, value_name = "ADDR", default_value = "127.0.0.1:8417")]
        listen: String,
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
    #[cfg(windows)]
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
            let wrong_subcommand = e.kind() == clap::error::ErrorKind::InvalidSubcommand;
            let _ = e.print();
            if wrong_subcommand {
                if let Some(hint) = git_habit_hint(std::env::args().skip(1)) {
                    eprintln!("{hint}");
                }
            }
            std::process::exit(e.exit_code());
        }
    };
    // The force-flag suggestion depends on which command was run, so it is
    // captured before `cli.command` is consumed.
    let force_form = force_form(&cli.command);

    let code = match run(cli) {
        Ok(exit) => exit.code(),
        Err(e) => {
            eprintln!("{} {}", console::style("error:").red().bold(), e);
            if let Some(hint) = hint_for(&e, force_form) {
                eprintln!("{}", hint);
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
}

impl Exit {
    fn code(&self) -> i32 {
        match self {
            Exit::Success => 0,
            Exit::Conflicts => 3,
            Exit::Diverged => 4,
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

/// What to do about an error, for the kinds that have an obvious next step.
///
/// Core states what went wrong and carries the details; deciding what to suggest
/// is presentation, so it lives here. Centralising it means every command gets
/// the same guidance instead of each one embedding advice in an error string.
fn hint_for(e: &VeloError, force_form: Option<&str>) -> Option<String> {
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
            let recipe = render::sync::recipe(remote, branch);
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
        VeloError::RemoteRefAbsent { remote, .. } => {
            lines.push(format!(
                "  Run velo fetch {} to bring it back, then try again.",
                remote
            ));
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
            let source = std::fs::read_to_string(root.join(".velo/MERGE_HEAD"))
                .ok()
                .and_then(|s| s.trim().split_once(':').map(|(_, r)| r.trim().to_string()))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "incoming".to_string());
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
            Commands::Resolve {
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
            // fsck is read-only unless it's going to repair (which mutates).
            | Commands::Fsck { repair: false }
    )
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

/// Run the reference HTTP server until the process is killed.
///
/// Requests are served one at a time: the core handlers take the repository
/// write lock themselves, and a reference server has no need for more. The only
/// thing written to stdout is the single `listening on` line, so a caller can
/// read it to learn the port chosen for `--listen ...:0`.
fn serve_http(path: &str, listen: &str) -> Result<()> {
    use std::io::Write;
    use tiny_http::{Header, Method, Response, Server};

    let root = Path::new(path);
    // Fail at start-up, not on the first request, if this is not a repository.
    serve::http::refs(root)?;

    let io_err = |e: Box<dyn std::error::Error + Send + Sync>| {
        VeloError::Io(std::io::Error::other(e.to_string()))
    };
    let server = Server::http(listen).map_err(io_err)?;
    let addr = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| VeloError::Io(std::io::Error::other("server has no IP address")))?;
    {
        let mut out = std::io::stdout().lock();
        writeln!(out, "listening on http://{addr}/").map_err(VeloError::Io)?;
        out.flush().map_err(VeloError::Io)?;
    }

    let text = |s: &str| Header::from_bytes("Content-Type", s).expect("static header");
    for mut req in server.incoming_requests() {
        let method = req.method().clone();
        let full = req.url().to_string();
        let route = full.split('?').next().unwrap_or("").to_string();

        let (want, endpoint) = if route.ends_with("/velo/v1/refs") {
            (Method::Get, 0)
        } else if route.ends_with("/velo/v1/upload") {
            (Method::Post, 1)
        } else if route.ends_with("/velo/v1/receive") {
            (Method::Post, 2)
        } else {
            eprintln!("{method} {full} -> 404");
            let _ = req.respond(
                Response::from_string("not found\n")
                    .with_status_code(404)
                    .with_header(text("text/plain")),
            );
            continue;
        };
        if method != want {
            eprintln!("{method} {full} -> 405");
            let _ = req.respond(
                Response::from_string("method not allowed\n")
                    .with_status_code(405)
                    .with_header(text("text/plain")),
            );
            continue;
        }

        let mut body = Vec::new();
        let result = match req.as_reader().read_to_end(&mut body) {
            Err(e) => Err(VeloError::Io(e)),
            Ok(_) => match endpoint {
                0 => serve::http::refs(root),
                1 => serve::http::upload(root, &body),
                _ => serve::http::receive(root, &body),
            },
        };
        match result {
            Ok(bytes) => {
                eprintln!("{method} {full} -> 200 ({} bytes)", bytes.len());
                let _ = req.respond(
                    Response::from_data(bytes).with_header(text("application/octet-stream")),
                );
            }
            Err(e) => {
                eprintln!("{method} {full} -> 500: {e}");
                let _ = req.respond(
                    Response::from_string(e.to_string())
                        .with_status_code(500)
                        .with_header(text("text/plain")),
                );
            }
        }
    }
    Ok(())
}

fn run(cli: Cli) -> Result<Exit> {
    let current_dir = std::env::current_dir().map_err(VeloError::Io)?;

    // Commands that don't require (or create) an enclosing repo run first.
    // The serve-* commands operate on an explicit path and speak a binary
    // protocol on stdout — they must emit nothing else.
    match &cli.command {
        Commands::Init => {
            render::init::print(&commands::init::run(&current_dir)?);
            return Ok(Exit::Success);
        }
        Commands::Clone { url, dir } => {
            let cloned = commands::sync::clone(
                url,
                &spawn_config()?,
                commands::sync::CloneOptions {
                    dir: dir.as_deref().map(Path::new),
                    observer: Some(Box::new(render::progress::Bar::new())),
                    // No cancel: Ctrl-C already ends a terminal process. The flag
                    // is for an embedder whose user closes a window instead.
                    ..Default::default()
                },
            )?;
            render::sync::print_cloned(&cloned);
            return Ok(Exit::Success);
        }
        Commands::ServeUpload { path } => return serve::upload(path).map(|()| Exit::Success),
        Commands::ServeReceive { path } => return serve::receive(path).map(|()| Exit::Success),
        Commands::ServeHttp { path, listen } => {
            return serve_http(path, listen).map(|()| Exit::Success)
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
        Commands::Init
        | Commands::Clone { .. }
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
            let hash = commands::resolve_snapshot_id(&repo, &target)?;
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
                    if let Ok(id) = commands::resolve_snapshot_id(&repo, &bare) {
                        from = Some(id.to_string());
                        branch = None;
                    }
                }
            }
            let branch = branch.map(|b| b.parse::<BranchName>()).transpose()?;
            let from = from
                .map(|t| commands::resolve_snapshot_id(&repo, t.as_str()))
                .transpose()?;
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
            render::history::print(&history, view);
        }

        Commands::Mv { from, to } => {
            render::mv::print(&commands::mv::run(
                write(),
                Path::new(&from),
                Path::new(&to),
            )?);
        }

        Commands::Undo => {
            let outcome = commands::undo::run(write())?;
            // Undo leaves `remote_refs` alone, so a tracking ref may still name
            // the snapshot just removed; say so rather than let it look gone.
            let tracking = commands::sync::tracking_refs_at(
                &repo,
                &outcome.snapshot.parse::<velo_core::SnapshotId>()?,
            )?;
            render::undo::print(&outcome, &tracking);
        }

        Commands::Redo => render::undo::print_redo(&commands::redo::run(write())?),

        Commands::Diff { args, paths } => {
            render::diff::print_comparison(&diffargs::dispatch(&repo, &args, &paths)?);
        }

        Commands::Show { target, paths } => {
            let id = commands::resolve_snapshot_id(&repo, &target)?;
            let paths: Vec<&Path> = paths.iter().map(Path::new).collect();
            render::diff::print_snapshot(&commands::show::run(&repo, &id, &paths)?);
        }

        Commands::CherryPick { target } => {
            let target = commands::resolve_snapshot_id(&repo, &target)?;
            let outcome =
                commands::cherry_pick::run(write(), &target, author::from_env()?.as_ref())?;
            render::cherry_pick::print(&outcome);
            if !outcome.applied.conflicts().is_empty() {
                exit = Exit::Conflicts;
            }
        }

        Commands::Switch { name, force } => {
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
                    .map(|spec| commands::resolve_snapshot_id(&repo, spec))
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
                    commands::resolve_snapshot_id(&repo, source)
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
            StashSub::Push { name } => {
                render::stash::print_pushed(&commands::stash::push(write(), name)?)
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
                .map(|spec| commands::resolve_snapshot_id(&repo, spec))
                .transpose()?;
            let lines = lines.as_deref().map(parse_line_range).transpose()?;
            render::blame::print(&commands::blame::run(
                &repo,
                Path::new(&file),
                commands::blame::Options {
                    at: at.as_ref(),
                    lines,
                    ..Default::default()
                },
            )?);
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
                .map(|spec| commands::resolve_snapshot_id(&repo, spec))
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
                    onto = commands::resolve_snapshot_id(&repo, spec)?;
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

        Commands::Gc { keep_days, dry_run } => {
            render::gc::print(&commands::gc::run(
                write(),
                commands::gc::Options {
                    keep_days,
                    dry_run,
                    ..Default::default()
                },
            )?);
        }

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
                    .map(|spec| commands::resolve_snapshot_id(&repo, spec))
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
            let pushed = commands::sync::push(
                write(),
                &remote,
                branch.as_deref(),
                &spawn_config()?,
                Default::default(),
            );
            match pushed {
                Ok(pushed) => render::sync::print_pushed(&pushed),
                Err(e) => {
                    // The refused push has just refreshed the tracking ref to the
                    // advertised tip. If this repository no longer holds that
                    // snapshot (undone here), a plain merge would fail; say how
                    // to get it back.
                    if let VeloError::NotFastForward { branch, remote } = &e {
                        let spec = format!("{remote}/{branch}");
                        if let Err(VeloError::RemoteRefAbsent { snapshot, .. }) =
                            commands::resolve_snapshot_id(&repo, &spec)
                        {
                            eprintln!(
                                "{spec} points at {}, which this repository no longer has (undone here?); velo fetch brings it back as remotes/{spec}, then velo merge {spec}",
                                render::id::short(snapshot.as_str())
                            );
                        }
                    }
                    return Err(e);
                }
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
