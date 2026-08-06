use clap::{builder::styling, Parser, Subcommand};

// All repository logic lives in `velo-core`; this crate is the presentation
// layer — argument parsing, rendering, and exit codes.
mod author;
mod diffargs;
mod render;

use std::path::Path;

use velo_core::{commands, error, serve, BranchName, TagName};

use error::{Result, VeloError};
use velo_core::commands::resolve::TakeOption;

/// `--take` as parsed from the command line. Core's `TakeOption` deliberately
/// carries no clap derive, so the CLI owns its own parseable mirror.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum TakeArg {
    Ours,
    Theirs,
}

impl From<TakeArg> for TakeOption {
    fn from(a: TakeArg) -> Self {
        match a {
            TakeArg::Ours => TakeOption::Ours,
            TakeArg::Theirs => TakeOption::Theirs,
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
  · Conflict sidecars — your code stays valid during a merge
  · Undo/redo — remove or restore snapshots with one command
  · Stash shelves — named, not cryptic stash@{2} indices
  · True 3-way merges — no false conflicts on one-sided changes

Quick start
  velo init
  velo save \"Initial commit\"
  velo status
  velo history",
    after_help = "Run `velo help <COMMAND>` for detailed usage of any command.",
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
            long,
            default_value_t = 20,
            value_name = "N",
            help = "Maximum number of entries to show [default: 20]"
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

        /// Compact one-line format: hash  branch  message.
        #[arg(long, help = "One-line-per-snapshot compact format")]
        oneline: bool,

        /// Draw an ASCII branch/merge graph alongside the log.
        #[arg(long, help = "Show ASCII graph of branch topology")]
        graph: bool,

        /// Show only snapshots that touched the given file or directory.
        #[arg(
            long = "file",
            value_name = "PATH",
            help = "Filter to snapshots that modified PATH"
        )]
        file_filter: Option<String>,
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
    · To inspect merge conflicts, use `velo resolve <file>` (interactive TUI)."
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
    /// snapshot are applied cleanly.  True conflicts produce .conflict
    /// sidecars just like `velo merge`.
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
    /// Conflicts are written as .conflict sidecars — your code stays
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
    2. velo diff <file> --conflict    # inspect each conflict
    3. velo resolve <file> --take theirs|ours
       — or edit the file manually, then `velo resolve <file>`
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
    /// Conflict files (<file>.conflict) are created during a merge when
    /// both branches modified the same file since their common ancestor.
    /// Your version is kept as <file>; the incoming version is in
    /// <file>.conflict.
    ///
    /// Examples
    ///   velo resolve src/auth.py --take theirs    # accept incoming version
    ///   velo resolve src/auth.py --take ours      # keep current version
    ///   velo resolve src/auth.py                  # mark manually edited file as resolved
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
            help = "Which version to keep: ours or theirs"
        )]
        take: Option<TakeArg>,

        /// Resolve all outstanding conflict files at once.
        #[arg(long, help = "Resolve all conflicts (requires --take)")]
        all: bool,
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
            help = "Lines of context around each match [default: 2]"
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
    · The squashed objects remain in the store until `velo gc`."
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
        /// Keep undone snapshot history for this many days (default: 30).
        #[arg(
            long,
            default_value_t = 30,
            value_name = "DAYS",
            help = "Retain undo history for N days [default: 30]"
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

fn main() {
    #[cfg(windows)]
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn SetConsoleOutputCP(id: u32) -> i32;
        }
        SetConsoleOutputCP(65001);
    }

    let cli = Cli::parse();
    // The force-flag suggestion depends on which command was run, so it is
    // captured before `cli.command` is consumed.
    let force_form = force_form(&cli.command);

    if let Err(e) = run(cli) {
        eprintln!("{} {}", console::style("error:").red().bold(), e);
        if let Some(hint) = hint_for(&e, force_form) {
            eprintln!("{}", hint);
        }
        std::process::exit(1);
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
            suggest(
                &mut lines,
                &[
                    (
                        &format!("velo pull {}", remote),
                        "bring their commits in first",
                        Tone::Do,
                    ),
                    (
                        &format!("velo push {}", remote),
                        "then send yours",
                        Tone::Alt,
                    ),
                ],
            );
        }
        VeloError::Conflicts { paths } => {
            list_paths(&mut lines, paths);
            suggest(
                &mut lines,
                &[
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
        _ => return None,
    }
    Some(lines.join(
        "
",
    ))
}

/// How prominently to render a suggested command.
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
    guard: &velo_core::WriteGuard,
    file: Option<&str>,
    take: Option<TakeOption>,
    all: bool,
) -> Result<()> {
    use console::style;
    use velo_core::commands::resolve;

    let repo = guard.repo();

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
            for cf in &targets {
                resolve::take_side(guard, cf, side)?;
                println!(
                    "{} Resolved '{}' (took {}).",
                    style("✔").green(),
                    cf.path,
                    side.as_str()
                );
            }
        }
        None => {
            for cf in targets {
                velo_tui::resolve_interactive(guard, cf)?;
            }
        }
    }

    report_remaining_conflicts(repo)
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
            // fsck is read-only unless it's going to repair (which mutates).
            | Commands::Fsck { repair: false }
    )
}

fn run(cli: Cli) -> Result<()> {
    let current_dir = std::env::current_dir().map_err(VeloError::Io)?;

    // Commands that don't require (or create) an enclosing repo run first.
    // The serve-* commands operate on an explicit path and speak a binary
    // protocol on stdout — they must emit nothing else.
    match &cli.command {
        Commands::Init => {
            render::init::print(&commands::init::run(&current_dir)?);
            return Ok(());
        }
        Commands::Clone { url, dir } => {
            let cloned = commands::sync::clone(
                url,
                dir.as_deref(),
                &spawn_config()?,
                Some(Box::new(render::progress::Bar::new())),
            )?;
            render::sync::print_cloned(&cloned);
            return Ok(());
        }
        Commands::ServeUpload { path } => return serve::upload(path),
        Commands::ServeReceive { path } => return serve::receive(path),
        _ => {}
    }

    let root =
        commands::find_repo_root(&current_dir).ok_or_else(|| not_a_repo_hint(&cli.command))?;

    // One connection for the whole command, and — because this goes through
    // `Repo` rather than opening SQLite directly — the point where a repository
    // written by a newer Velo is refused instead of half-read.
    // Long operations report through this. Inert on a non-TTY, so piped output
    // stays clean.
    let repo = velo_core::Repo::open_and_migrate(&root)?.observing(render::progress::Bar::new());

    // Serialise mutating commands against other velo processes. Read-only
    // commands skip the lock so they never block on a long-running mutation.
    // The guard is held until the command returns, and is what grants write
    // access to core at all.
    let guard = if is_read_only(&cli.command) {
        None
    } else {
        Some(repo.write()?)
    };
    // Mutating arms need the guard; `is_read_only` above decides which is which,
    // so a mismatch here is a programming error rather than a user-visible one.
    let write = || -> &velo_core::WriteGuard<'_> {
        guard
            .as_ref()
            .expect("a mutating command must hold the write guard")
    };

    match cli.command {
        Commands::Init
        | Commands::Clone { .. }
        | Commands::ServeUpload { .. }
        | Commands::ServeReceive { .. } => unreachable!(),

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
            oneline,
            graph,
            file_filter,
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
            let branch = branch.map(|b| b.parse::<BranchName>()).transpose()?;
            let history = commands::history::run(
                &repo,
                commands::history::Options {
                    all,
                    branch: branch.as_ref(),
                    file: file_filter.as_deref(),
                    limit: Some(limit),
                },
            )?;
            render::history::print(&history, view);
        }

        Commands::Undo => {
            render::undo::print(&commands::undo::run(write())?);
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
            render::cherry_pick::print(&commands::cherry_pick::run(
                write(),
                &target,
                author::from_env()?.as_ref(),
            )?);
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
            render::merge::print(&commands::merge::run(write(), mode)?);
        }

        Commands::Resolve { file, take, all } => {
            run_resolve(write(), file.as_deref(), take.map(Into::into), all)?;
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

        Commands::Blame { file, at } => {
            let at = at
                .as_deref()
                .map(|spec| commands::resolve_snapshot_id(&repo, spec))
                .transpose()?;
            render::blame::print(&commands::blame::run(&repo, Path::new(&file), at.as_ref())?);
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
            render::rebase::print(&commands::rebase::run(
                write(),
                mode,
                author::from_env()?.as_ref(),
            )?);
        }

        Commands::Gc { keep_days } => {
            render::gc::print(&commands::gc::run(write(), keep_days)?);
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
            )?);
        }
        Commands::Push { remote, branch } => {
            render::sync::print_pushed(&commands::sync::push(
                write(),
                &remote,
                branch.as_deref(),
                &spawn_config()?,
            )?);
        }
        Commands::Pull { remote } => {
            render::sync::print_pulled(&commands::sync::pull(write(), &remote, &spawn_config()?)?);
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

    Ok(())
}
