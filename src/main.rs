use clap::{Parser, Subcommand};

mod db;
mod error;
mod storage;
mod commands;

#[cfg(test)]
mod tests;

use commands::resolve::TakeOption;
use error::{Result, VeloError};

#[derive(Parser)]
#[command(name = "velo", version = "2.0")]
#[command(
    about = "Velo — a fast, safe, and intuitive version control system.",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialise a new Velo repository in the current directory.
    Init,

    /// Save a snapshot of the working directory.
    Save {
        /// Short description of this snapshot.
        message: String,
    },

    /// Restore the working directory to a snapshot (hash, prefix, or tag).
    Restore {
        target: String,
        /// Discard unsaved changes without prompting.
        #[arg(short, long)]
        force: bool,
    },

    /// Show the status of the working directory.
    #[command(alias = "st")]
    Status,

    /// Show snapshot history.
    #[command(alias = "log")]
    Logs {
        /// Show history across all branches.
        #[arg(short, long)]
        all: bool,
        /// Maximum number of snapshots to show.
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
        /// Show history for a specific branch (without switching to it).
        #[arg(short, long)]
        branch: Option<String>,
        /// Compact one-line format.
        #[arg(long)]
        oneline: bool,
    },

    /// Remove the most recent snapshot on the current branch.
    Undo,

    /// Re-apply the most recently undone snapshot.
    Redo,

    /// Show changes between the working tree and the last snapshot.
    Diff {
        /// Show diff for a specific file only.
        file: Option<String>,
        /// Show diff against the conflict version of a file.
        #[arg(short, long)]
        conflict: bool,
    },

    /// Switch to a branch (creates it if it doesn't exist).
    Switch {
        name: String,
        /// Discard unsaved changes without prompting.
        #[arg(short, long)]
        force: bool,
    },

    /// List branches, or delete one with --delete.
    #[command(alias = "branch")]
    Branches {
        /// Delete a branch (soft delete; history is preserved).
        #[arg(short, long)]
        delete: Option<String>,
    },

    /// Manage tags.  With no arguments, lists all tags.
    Tag {
        /// Tag name to create.
        name: Option<String>,
        /// Snapshot hash or tag to attach the new tag to (defaults to HEAD).
        snapshot: Option<String>,
        /// Delete a tag by name.
        #[arg(short, long, conflicts_with = "name")]
        delete: Option<String>,
        /// Overwrite an existing tag with the same name.
        #[arg(short, long)]
        force: bool,
    },

    /// Merge another branch into the current one.
    Merge {
        /// Branch to merge in.
        branch: Option<String>,
        /// Abort an in-progress merge, discarding all conflict files.
        #[arg(long, conflicts_with = "branch")]
        abort: bool,
    },

    /// Resolve a merge conflict.
    Resolve {
        /// File to resolve (omit when using --all).
        file: Option<String>,
        /// Accept 'ours' or 'theirs' version automatically.
        #[arg(short, long, value_enum)]
        take: Option<TakeOption>,
        /// Resolve all conflicts at once with the chosen --take strategy.
        #[arg(long)]
        all: bool,
    },

    /// Remove orphaned objects and old trash entries to reclaim disk space.
    Gc {
        /// Keep undo history for this many days before permanent deletion (default: 30).
        #[arg(long, default_value_t = 30)]
        keep_days: u32,
    },
}

fn main() {
    // On Windows the default console code page is often CP1252, which mangles
    // UTF-8 output (✔ → âœ", · → Â·, … → â€¦).  Switching to CP 65001 (UTF-8)
    // before any output is written fixes this without any extra dependencies.
    #[cfg(windows)]
    // SAFETY: SetConsoleOutputCP is idempotent and safe to call at any time.
    unsafe {
        #[link(name = "kernel32")]
        extern "system" { fn SetConsoleOutputCP(id: u32) -> i32; }
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

    // `init` is the only command that doesn't require an existing repo.
    if matches!(cli.command, Commands::Init) {
        return commands::init::run(&current_dir);
    }

    let root = commands::find_repo_root(&current_dir)
        .ok_or(VeloError::NotARepo)?;

    match cli.command {
        Commands::Init => unreachable!(),

        Commands::Save { message } => {
            match commands::save::run(&root, &message)? {
                None => {} // "nothing to save" message printed inside
                Some(r) => {
                    println!(
                        "{} Saved {} on {}  ({} new, {} modified, {} deleted)",
                        console::style("✔").green().bold(),
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
            }
        }

        Commands::Restore { target, force } => {
            let hash = commands::resolve_snapshot_id(&root, &target)?;
            commands::restore::run(&root, &hash, force)?;
        }

        Commands::Status => commands::status::run(&root)?,

        Commands::Logs { all, limit, branch, oneline } => {
            commands::logs::run(&root, all, limit, branch.as_deref(), oneline)?;
        }

        Commands::Undo => {
            let msg = commands::undo::run(&root)?;
            println!("{}", msg);
        }

        Commands::Redo => commands::redo::run(&root)?,

        Commands::Diff { file, conflict } => {
            commands::diff::run(&root, &file, conflict)?;
        }

        Commands::Switch { name, force } => {
            commands::switch::run(&root, &name, force)?;
        }

        Commands::Branches { delete } => {
            commands::branches::run(&root, delete)?;
        }

        Commands::Tag { name, snapshot, delete, force } => {
            commands::tag::run(&root, name, snapshot, delete, force)?;
        }

        Commands::Merge { branch, abort } => {
            commands::merge::run(&root, branch.as_deref(), abort)?;
        }

        Commands::Resolve { file, take, all } => {
            commands::resolve::run(&root, file.as_deref(), take, all)?;
        }

        Commands::Gc { keep_days } => {
            commands::gc::run(&root, keep_days)?;
        }
    }

    Ok(())
}