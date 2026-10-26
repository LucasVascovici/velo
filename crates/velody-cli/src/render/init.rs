//! Render repository initialisation for the terminal.

use console::style;
use velody_core::commands::init::Initialised;

pub fn print(repo: &Initialised, objects: velody_core::ObjectLocation) {
    println!(
        "{} Initialized empty Velody repository in {}",
        style("✔").green().bold(),
        style(repo.velody_dir.display()).cyan()
    );
    if objects == velody_core::ObjectLocation::Database {
        println!("  Objects are kept in velo.db (no file per object).");
    }
    // The old message claimed a default .veloignore was written even when one
    // already existed and was left untouched.
    if repo.wrote_veloignore {
        println!(
            "  Default .veloignore written. Branch: {}",
            style(&repo.branch).cyan().bold()
        );
    } else {
        println!(
            "  Kept the existing .veloignore. Branch: {}",
            style(&repo.branch).cyan().bold()
        );
    }
}
