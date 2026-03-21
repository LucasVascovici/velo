use clap::{builder::styling, Parser, Subcommand};

mod commands;
mod db;
mod error;
mod storage;

#[cfg(test)]
mod tests;

use commands::resolve::TakeOption;
use error::{Result, VeloError};

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
    name    = "velo",
    version = "2.0",
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
  velo logs",
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
    #[command(verbatim_doc_comment)]
    #[command(after_help = "\
NOTES
    · .velo/ is never tracked — it is automatically excluded.
    · The default branch is called 'main'.
    · Edit .veloignore to exclude build artefacts, secrets, etc.")]
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
    ///   velo save "Tweak config" --amend
    #[command(verbatim_doc_comment)]
    #[command(after_help = "\
NOTES
    · The message cannot be empty or whitespace-only.
    · --amend replaces the previous snapshot in-place and keeps the
      same parent, preserving a linear history.  Objects from the
      replaced snapshot are cleaned up by `velo gc`.")]
    Save {
        /// Short description of what changed in this snapshot.
        #[arg(value_name = "MESSAGE")]
        message: String,

        /// Replace the most recent snapshot on this branch instead of
        /// creating a new one.  Useful to fix a typo or include a
        /// missed file without polluting history.
        #[arg(
            long,
            help = "Amend the most recent snapshot instead of creating a new one"
        )]
        amend: bool,
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
    #[command(verbatim_doc_comment)]
    #[command(after_help = "\
NOTES
    · Without --force, restore aborts if there are unsaved changes.
    · When paths are given (-- <path>…), only those files are written
      and PARENT is not updated — use this for surgical file-level reverts.
    · Restore via tag: first create a tag with `velo tag <name>`.")]
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
    ///   velo st        # alias
    #[command(verbatim_doc_comment)]
    #[command(
        alias = "st",
        after_help = "\
NOTES
    · Velo uses an mtime+size cache to skip rehashing unchanged files.
      The first call after a large change is slower; subsequent calls
      on an unchanged tree are essentially free (stat only)."
    )]
    Status,

    /// Show snapshot history.
    ///
    /// Without flags, shows the ancestry of the current snapshot on
    /// the current branch.  Use --all for cross-branch history.
    ///
    /// Examples
    ///   velo log                          # current branch
    ///   velo log --all                    # all branches
    ///   velo log --branch feature/auth    # specific branch (no switch needed)
    ///   velo log --file src/auth.py       # snapshots that touched this file
    ///   velo log --oneline                # compact format
    ///   velo log --graph                  # ASCII branch graph
    ///   velo log --limit 50               # show up to 50 entries
    #[command(verbatim_doc_comment)]
    #[command(
        alias = "log",
        after_help = "\
NOTES
    · --file filters by any path prefix, so --file src/ matches all
      files under src/.
    · --graph is best combined with --oneline for compact output."
    )]
    Logs {
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
    #[command(verbatim_doc_comment)]
    #[command(after_help = "\
NOTES
    · Undo aborts if there are unsaved changes.
    · Undone snapshots are stored in an internal trash table and can
      be recovered with `velo redo` until `velo gc` purges them.
    · Undoing the very first snapshot clears the working tree.")]
    Undo,

    /// Re-apply the most recently undone snapshot.
    ///
    /// Only available after `velo undo` and only until a new `velo save`
    /// invalidates the redo stack.
    ///
    /// Example
    ///   velo redo
    #[command(verbatim_doc_comment)]
    #[command(after_help = "\
NOTES
    · Redo is cleared the moment you run `velo save` — once you
      diverge, there is nothing to redo.
    · Redo aborts if there are unsaved changes.")]
    Redo,

    /// Show line-level changes vs the last snapshot.
    ///
    /// Without a file argument, diffs all dirty files in the working
    /// tree.  With --conflict, compares your version of a file against
    /// the incoming .conflict sidecar.
    ///
    /// Examples
    ///   velo diff                         # all changed files
    ///   velo diff src/auth.py             # one file
    ///   velo diff src/auth.py --conflict  # view merge conflict
    #[command(verbatim_doc_comment)]
    #[command(after_help = "\
NOTES
    · Binary files are detected automatically and their diffs are omitted.
    · Diff output uses unified format with ±5 lines of context per hunk.")]
    Diff {
        /// File to diff (relative to repo root). Omit to diff all dirty files.
        #[arg(
            value_name = "FILE",
            help = "File to diff (defaults to all modified files)"
        )]
        file: Option<String>,

        /// Show diff between the working file and its .conflict sidecar.
        #[arg(
            short,
            long,
            help = "Diff against the .conflict sidecar (during a merge)"
        )]
        conflict: bool,
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
    #[command(verbatim_doc_comment)]
    #[command(after_help = "\
NOTES
    · Nothing on disk is changed — show is entirely read-only.
    · Use `velo restore <target> -- <file>` to pull a single file out
      of a historical snapshot into your working tree.")]
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
    #[command(verbatim_doc_comment)]
    #[command(
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
    #[command(verbatim_doc_comment)]
    #[command(after_help = "\
NOTES
    · New branches inherit the current working tree state.
    · Switch to a deleted branch is not permitted.
    · --force discards unsaved changes — they cannot be recovered.")]
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
    ///   velo branch                         # alias
    ///   velo branches --delete feature/old
    #[command(verbatim_doc_comment)]
    #[command(
        alias = "branch",
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
    /// cherry-pick, logs, etc.).
    ///
    /// Examples
    ///   velo tag                         # list all tags
    ///   velo tag v1.0                    # tag the current snapshot
    ///   velo tag v1.0 abc123ef           # tag a specific snapshot
    ///   velo tag v1.0 --force            # overwrite an existing tag
    ///   velo tag --delete v1.0           # delete a tag
    #[command(verbatim_doc_comment)]
    #[command(after_help = "\
NOTES
    · Deleting a tag does not affect the snapshot it pointed to.
    · A tag can point to any snapshot across all branches.")]
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
    #[command(verbatim_doc_comment)]
    #[command(after_help = "\
CONFLICT RESOLUTION WORKFLOW
    1. velo merge <branch>
    2. velo diff <file> --conflict    # inspect each conflict
    3. velo resolve <file> --take theirs|ours
       — or edit the file manually, then `velo resolve <file>`
    4. velo save \"Merge <branch>\"

NOTES
    · Merge aborts if there are unsaved changes.
    · Fast-forward merges (linear ancestry) are handled automatically.
    · --abort removes all .conflict files and clears the merge state.")]
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
    ///   velo resolve --all --take theirs           # resolve all conflicts at once
    #[command(verbatim_doc_comment)]
    #[command(after_help = "\
NOTES
    · After resolving all conflicts, run `velo save \"Merge <branch>\"`.
    · Velo will remind you of remaining conflicts after each resolve.
    · --all requires --take; without it Velo doesn't know which version
      to pick for each file.")]
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
        take: Option<TakeOption>,

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
    #[command(verbatim_doc_comment)]
    #[command(after_help = "\
NOTES
    · Stashing restores the working tree to the last saved snapshot.
    · Pop aborts if there are unsaved changes.
    · Shelves are stored in the repository database and survive restarts.")]
    Stash {
        #[command(subcommand)]
        sub: StashSub,
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
    #[command(verbatim_doc_comment)]
    #[command(after_help = "\
WHAT GC CLEANS UP
    · Orphaned objects (no snapshot references them)
    · Stale file_map rows (snapshot was deleted)
    · Stale index_cache rows (path no longer tracked)
    · Trash entries older than --keep-days days

NOTES
    · Running gc while a merge is in progress is safe.
    · The operation is idempotent — running it twice is harmless.")]
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
    List,

    /// Restore a shelf and remove it from the list.
    ///
    /// With no name, restores the most recently created shelf.
    ///
    /// Examples
    ///   velo stash pop
    ///   velo stash pop "wip: payments"
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

    if let Err(e) = run() {
        eprintln!("{} {}", console::style("error:").red().bold(), e);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let current_dir = std::env::current_dir().map_err(VeloError::Io)?;

    if matches!(cli.command, Commands::Init) {
        return commands::init::run(&current_dir);
    }

    let root = commands::find_repo_root(&current_dir).ok_or(VeloError::NotARepo)?;

    match cli.command {
        Commands::Init => unreachable!(),

        Commands::Save { message, amend } => match commands::save::run(&root, &message, amend)? {
            None => {}
            Some(r) => {
                let verb = if amend { "Amended" } else { "Saved" };
                println!(
                    "{} {} {} on {}  ({} new, {} modified, {} deleted)",
                    console::style("✔").green().bold(),
                    verb,
                    console::style(&r.hash).yellow(),
                    console::style(
                        std::fs::read_to_string(root.join(".velo/HEAD"))
                            .unwrap_or_default()
                            .trim()
                            .to_string()
                    )
                    .cyan(),
                    r.new_count,
                    r.modified_count,
                    r.deleted_count,
                );
            }
        },

        Commands::Restore {
            target,
            force,
            paths,
        } => {
            let hash = commands::resolve_snapshot_id(&root, &target)?;
            commands::restore::run(&root, &hash, force, &paths)?;
        }

        Commands::Status => commands::status::run(&root)?,

        Commands::Logs {
            all,
            limit,
            branch,
            oneline,
            graph,
            file_filter,
        } => {
            commands::logs::run(
                &root,
                all,
                limit,
                branch.as_deref(),
                oneline,
                graph,
                file_filter.as_deref(),
            )?;
        }

        Commands::Undo => {
            let msg = commands::undo::run(&root)?;
            println!("{}", msg);
        }

        Commands::Redo => commands::redo::run(&root)?,

        Commands::Diff { file, conflict } => {
            commands::diff::run(&root, &file, conflict)?;
        }

        Commands::Show { target, paths } => {
            let file = paths.into_iter().next();
            commands::show::run(&root, &target, &file)?;
        }

        Commands::CherryPick { target } => {
            commands::cherry_pick::run(&root, &target)?;
        }

        Commands::Switch { name, force } => {
            commands::switch::run(&root, &name, force)?;
        }

        Commands::Branches { delete } => {
            commands::branches::run(&root, delete)?;
        }

        Commands::Tag {
            name,
            snapshot,
            delete,
            force,
        } => {
            commands::tag::run(&root, name, snapshot, delete, force)?;
        }

        Commands::Merge { branch, abort } => {
            commands::merge::run(&root, branch.as_deref(), abort)?;
        }

        Commands::Resolve { file, take, all } => {
            commands::resolve::run(&root, file.as_deref(), take, all)?;
        }

        Commands::Stash { sub } => match sub {
            StashSub::Push { name } => commands::stash::push(&root, name)?,
            StashSub::List => commands::stash::list(&root)?,
            StashSub::Pop { name } => commands::stash::pop(&root, name)?,
            StashSub::Drop { name } => commands::stash::drop_shelf(&root, name)?,
            StashSub::Show { name } => commands::stash::show_shelf(&root, name)?,
        },

        Commands::Gc { keep_days } => {
            commands::gc::run(&root, keep_days)?;
        }
    }

    Ok(())
}
