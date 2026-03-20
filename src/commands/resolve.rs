use std::fs;
use std::path::Path;

use console::style;

use crate::error::{Result, VeloError};

/// Clap-compatible enum for the `--take` option.
#[derive(clap::ValueEnum, Clone, Debug, PartialEq)]
pub enum TakeOption {
    Ours,
    Theirs,
}

pub fn run(root: &Path, file: Option<&str>, take: Option<TakeOption>, all: bool) -> Result<()> {
    // ── Validate arguments ────────────────────────────────────────────────────
    if !all && file.is_none() {
        return Err(VeloError::InvalidInput(
            "Specify a file to resolve, or use --all.  Example: velo resolve app.py --take theirs"
                .into(),
        ));
    }

    // ── Gather the list of conflict files to resolve ──────────────────────────
    let files: Vec<String> = if all {
        let cf = crate::commands::get_conflict_files(root);
        if cf.is_empty() {
            println!("{}", style("No conflict files found.").dim());
            return Ok(());
        }
        // Conflict paths from get_conflict_files already end with ".conflict".
        // resolve_one expects the *original* file path without the suffix.
        cf.iter()
            .map(|p| p.trim_end_matches(".conflict").to_string())
            .collect()
    } else {
        vec![file.unwrap().to_string()]
    };

    for f in &files {
        resolve_one(root, f, take.clone())?;
    }

    // ── Check remaining conflicts ─────────────────────────────────────────────
    let remaining = crate::commands::get_conflict_files(root);
    if remaining.is_empty() {
        println!(
            "\n{} All conflicts resolved! Run {} to finalise.",
            style("✔").green().bold(),
            style("velo save \"Finish merge\"").yellow().bold()
        );
        // Clean up MERGE_HEAD
        let _ = fs::remove_file(root.join(".velo/MERGE_HEAD"));
    } else {
        println!(
            "\n{} {} conflict(s) still unresolved:",
            style("!").yellow().bold(),
            remaining.len()
        );
        for r in &remaining {
            println!("  {}", style(r.trim_end_matches(".conflict")).yellow());
        }
    }

    Ok(())
}

fn resolve_one(root: &Path, file_path: &str, take: Option<TakeOption>) -> Result<()> {
    let current = root.join(file_path);
    let conflict = root.join(format!("{}.conflict", file_path));

    if !conflict.exists() {
        return Err(VeloError::InvalidInput(format!(
            "No conflict file found for '{}'. Is there an active merge?",
            file_path
        )));
    }

    match take {
        Some(TakeOption::Theirs) => {
            // Cross-platform: copy then remove (avoids rename failures on Windows
            // when the destination is in use).
            fs::copy(&conflict, &current)?;
            fs::remove_file(&conflict)?;
            println!(
                "{} Kept {} for '{}'.",
                style("✔").green(),
                style("THEIRS").green().bold(),
                file_path
            );
        }
        Some(TakeOption::Ours) => {
            fs::remove_file(&conflict)?;
            println!(
                "{} Kept {} for '{}'.",
                style("✔").green(),
                style("OURS").cyan().bold(),
                file_path
            );
        }
        None => {
            // Manual resolution: the user edited the file themselves; just
            // remove the conflict marker.
            fs::remove_file(&conflict)?;
            println!(
                "{} Conflict cleared for '{}' (manual resolution).",
                style("✔").green(),
                file_path
            );
        }
    }

    Ok(())
}
