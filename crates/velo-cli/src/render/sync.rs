//! Render sync outcomes for the terminal.

use console::style;
use velo_core::commands::sync::{
    BranchCreated, Cloned, Fetched, Pulled, PushReport, Pushed, Synced,
};

/// The steps after a refused push, in order. One definition so `push`, `pull`
/// and `status` can never give different advice. `pull` and `push` carry the
/// remote name only when it is not the default one.
pub fn recipe(remote: &str, branch: &str) -> Vec<(String, &'static str)> {
    let arg = if remote == "origin" {
        String::new()
    } else {
        format!(" {remote}")
    };
    vec![
        (
            format!("velo sync{arg}"),
            "fetch, merge if clean, push (or by hand:)",
        ),
        (format!("velo pull{arg}"), "bring their snapshots in"),
        (
            format!("velo merge {remote}/{branch}"),
            "combine them with yours",
        ),
        (
            format!("velo save \"Merge {remote}/{branch}\""),
            "record the merge",
        ),
        (format!("velo push{arg}"), "publish"),
    ]
}

/// Print [`recipe`] as an aligned list.
pub fn print_recipe(remote: &str, branch: &str) {
    let steps = recipe(remote, branch);
    let width = steps.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
    for (cmd, why) in &steps {
        println!("    · {}  {}", style(format!("{cmd:<width$}")).cyan(), why);
    }
}

pub fn print_cloned(cloned: &Cloned) {
    println!(
        "{} Cloned {} snapshot(s), {} object(s), {} branch(es) into {}",
        style("✔").green().bold(),
        cloned.snapshots,
        cloned.objects,
        cloned.branches,
        style(cloned.into.display()).cyan()
    );
    println!(
        "  Checked out {} — from {}",
        style(&cloned.branch).cyan().bold(),
        style(&cloned.url).dim()
    );
}

pub fn print_fetched(fetched: &Fetched) {
    println!(
        "{} Fetched from '{}' — {} new snapshot(s), {} object(s).",
        style("✔").green().bold(),
        fetched.remote,
        fetched.snapshots,
        fetched.objects
    );
    for r in &fetched.refs {
        println!(
            "  {}/{}  →  {}",
            fetched.remote,
            style(&r.branch).cyan(),
            style(super::id::short(&r.hash)).yellow()
        );
    }
    // Fetch deliberately leaves local branches and the working tree alone, which
    // surprises people expecting `pull`.
    if fetched.snapshots > 0 {
        println!(
            "  {} your branches are untouched — {} to incorporate the changes.",
            style("note:").dim(),
            style("velo pull").cyan()
        );
    }
}

pub fn print_push_report(report: &PushReport) {
    print_pushed(&report.pushed);
    if let Pushed::Sent { branch, .. } = &report.pushed {
        if !report.also_published.is_empty() {
            println!(
                "  also published: {} (merged into {}); the remote now lists them as branches",
                report.also_published.join(", "),
                branch
            );
        }
    }
}

pub fn print_synced(synced: &Synced) {
    match synced {
        Synced::Nothing { branch, remote } => println!(
            "{} '{}' is already up to date with '{}'.",
            style("✔").green(),
            branch,
            remote
        ),
        Synced::Pushed { report } => print_push_report(report),
        Synced::FastForwarded { branch, to, .. } => println!(
            "{} Fast-forwarded '{}' to {}.",
            style("✔").green().bold(),
            branch,
            style(super::id::short(to.as_str())).yellow()
        ),
        Synced::Merged {
            branch,
            remote,
            merge,
            report,
        } => {
            println!(
                "{} merged {}/{} ({}) and pushed",
                style("✔").green().bold(),
                remote,
                branch,
                style(super::id::short(merge.as_str())).yellow()
            );
            print_push_report(report);
        }
        Synced::Conflicted {
            branch,
            remote,
            paths,
        } => {
            println!(
                "{} '{}' and '{}/{}' conflict; nothing was pushed. Conflicting files:",
                style("!").yellow().bold(),
                branch,
                remote,
                branch
            );
            for p in paths {
                println!("  {} {}", style("[Conflict]").red().bold(), p.display());
            }
            println!("  Next steps:");
            println!(
                "    · {}  resolve each file (or --take ours / --take theirs)",
                style("velo resolve <file>").cyan()
            );
            println!("    · {}  record the merge", style("velo save").cyan());
            println!("    · {}  publish it", style("velo sync").cyan());
        }
        // `Synced` is non-exhaustive; a future outcome falls back to its debug form.
        other => println!("{other:?}"),
    }
}

pub fn print_pushed(pushed: &Pushed) {
    match pushed {
        Pushed::AlreadyUpToDate { branch, remote } => println!(
            "{} '{}' is already up to date on '{}'.",
            style("✔").green(),
            branch,
            remote
        ),
        Pushed::Sent {
            branch,
            remote,
            snapshots,
            objects,
            created,
        } => {
            println!(
                "{} Pushed '{}' to '{}' — {} snapshot(s), {} object(s).",
                style("✔").green().bold(),
                branch,
                remote,
                snapshots,
                objects
            );
            if let Some(created) = created {
                let what = match created {
                    BranchCreated::RemoteWasEmpty => "was empty; it now has its first history",
                    BranchCreated::BranchWasMissing => {
                        "did not have this branch; it was created there"
                    }
                };
                println!(
                    "  {} '{}' {}. Others can now {}.",
                    style("note:").dim(),
                    remote,
                    what,
                    style("velo clone <url> / velo pull").cyan()
                );
            }
            println!(
                "  {} the remote's working tree is unchanged; it updates on its next {}.",
                style("note:").dim(),
                style("velo pull").cyan()
            );
        }
    }
}

pub fn print_pulled(pulled: &Pulled) {
    match pulled {
        Pulled::AlreadyUpToDate { branch, remote } => println!(
            "{} '{}' is already up to date with '{}'.",
            style("✔").green(),
            branch,
            remote
        ),
        Pulled::FastForwarded { branch, to, .. } => println!(
            "{} Fast-forwarded '{}' to {}.",
            style("✔").green().bold(),
            branch,
            style(super::id::short(to)).yellow()
        ),
        Pulled::Ahead { remote, ahead, .. } => println!(
            "{} Nothing to pull; {} local snapshot(s) not yet on '{}'. Run {}.",
            style("✔").green(),
            ahead,
            remote,
            style("velo push").cyan()
        ),
        Pulled::Diverged {
            branch,
            remote,
            ahead,
            behind,
        } => {
            println!(
                "{} '{}' and '{}/{}' have diverged: {} local and {} remote snapshot(s).",
                style("!").yellow().bold(),
                branch,
                remote,
                branch,
                ahead,
                behind
            );
            println!("  Reconcile with:");
            print_recipe(remote, branch);
        }
        // `Pulled` is non-exhaustive; a future outcome falls back to its debug form.
        other => println!("{other:?}"),
    }
}
