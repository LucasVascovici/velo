//! `velo conflicts`: both sides of every conflicting hunk, as text or JSON.
//!
//! The JSON form is the embedder's view of a conflict: everything `velo
//! resolve` shows interactively, with nothing to scrape.

use console::style;
use serde_json::{json, Value};
use velo_core::commands::resolve::ConflictSession;
use velo_merge::Decision;

/// The decision's stable name in JSON, matching what is recorded on disk.
fn decision_name(d: &Decision) -> &'static str {
    match d {
        Decision::Ours => "ours",
        Decision::Theirs => "theirs",
        Decision::BothOursFirst => "both_ours_first",
        Decision::BothTheirsFirst => "both_theirs_first",
        _ => "manual",
    }
}

/// The decision in words, for the text form.
fn decision_text(d: &Decision) -> String {
    match d {
        Decision::Ours => "ours".into(),
        Decision::Theirs => "theirs".into(),
        Decision::BothOursFirst => "both (ours first)".into(),
        Decision::BothTheirsFirst => "both (theirs first)".into(),
        _ => "manual edit".into(),
    }
}

/// Print the conflicts as human-readable text.
pub fn print_text(source: Option<&str>, sessions: &[ConflictSession]) {
    if sessions.is_empty() {
        println!("No conflicts.");
        return;
    }
    let theirs_label = source.unwrap_or("incoming");
    for (i, s) in sessions.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("{}", style(&s.file.path).yellow().bold());
        let n = s.hunks.len();
        for (k, h) in s.hunks.iter().enumerate() {
            println!(
                "\nhunk {}/{} (ancestor lines {}\u{2013}{})",
                k + 1,
                n,
                h.ancestor_start + 1,
                h.ancestor_end
            );
            for l in &h.context_before {
                println!("  {}", style(format!("  {l}")).dim());
            }
            println!("  {}", style("ours (your last saved snapshot):").cyan());
            side(&h.ours);
            println!("  {}", style(format!("theirs ({theirs_label}):")).magenta());
            side(&h.theirs);
            for l in &h.context_after {
                println!("  {}", style(format!("  {l}")).dim());
            }
            if let Some(d) = &h.decision {
                println!("  decision: {}", decision_text(d));
            }
        }
    }
}

fn side(lines: &[String]) {
    if lines.is_empty() {
        println!("    {}", style("(deleted)").dim());
    }
    for l in lines {
        println!("    {l}");
    }
}

/// The conflicts as one JSON document.
pub fn to_json(source: Option<&str>, sessions: &[ConflictSession]) -> Value {
    let files: Vec<Value> = sessions
        .iter()
        .map(|s| {
            let hunks: Vec<Value> = s
                .hunks
                .iter()
                .map(|h| {
                    json!({
                        "id": h.id,
                        "ancestor_start": h.ancestor_start,
                        "ancestor_end": h.ancestor_end,
                        "context_before": h.context_before,
                        "ours": h.ours,
                        "theirs": h.theirs,
                        "context_after": h.context_after,
                        "decision": h.decision.as_ref().map(decision_name),
                    })
                })
                .collect();
            json!({
                "path": s.file.path,
                "ancestor": s.file.ancestor_hash,
                "ours": s.file.our_hash,
                "theirs": s.file.their_hash,
                "hunks": hunks,
            })
        })
        .collect();
    json!({ "source": source, "files": files })
}

/// Print the conflicts as JSON.
pub fn print_json(source: Option<&str>, sessions: &[ConflictSession]) {
    println!(
        "{}",
        serde_json::to_string_pretty(&to_json(source, sessions)).unwrap_or_default()
    );
}
