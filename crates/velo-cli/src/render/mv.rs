//! Render the result of [`commands::mv`](velo_core::commands::mv).

use console::style;
use velo_core::commands::mv::Moved;

pub fn print(moved: &Moved) {
    println!(
        "{} {} {} {}",
        style("Moved").green().bold(),
        style(moved.from.display()).dim(),
        style("→").dim(),
        style(moved.to.display()).cyan()
    );
    if moved.extended_a_pending_move {
        // Otherwise the intermediate name would look like a step in the file's
        // history, when no snapshot ever held it.
        println!(
            "  {}",
            style("(folded into the move already pending for this file)").dim()
        );
    }
    println!(
        "  {}",
        style("Recorded. The next 'velo save' will note the rename.").dim()
    );
}
