//! Render repository initialisation for the terminal.

use console::style;
use velo_core::commands::init::Initialised;

pub fn print(repo: &Initialised) {
    println!(
        "{} Initialized empty Velo repository in {}",
        style("✔").green().bold(),
        style(repo.velo_dir.display()).cyan()
    );
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
