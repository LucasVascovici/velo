use std::fs;
use std::path::Path;

use console::style;
use rusqlite::params;

use crate::db;
use crate::error::Result;

struct LogEntry {
    hash: String,
    message: String,
    date: String,
    branch: String,
    tag: Option<String>,
}

pub fn run(
    root: &Path,
    all: bool,
    limit: usize,
    filter_branch: Option<&str>,
    oneline: bool,
) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    let current_parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let current_parent = current_parent.trim();
    let branch_raw = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_else(|_| "main".into());
    let branch = branch_raw.trim();
    let sql_limit = limit as i64;

    let mut history: Vec<LogEntry> = Vec::new();

    if all || filter_branch.is_some() {
        let target_branch = filter_branch.unwrap_or("");
        let header = if let Some(b) = filter_branch {
            format!("History for branch '{}'", style(b).cyan().bold())
        } else {
            "Global history (all branches)".to_string()
        };
        println!("\n{}", style(header).bold().underlined());

        // Exclude soft-deleted branches from global view
        let sql = if filter_branch.is_some() {
            "SELECT s.hash, s.message, s.created_at, s.branch, t.name
             FROM snapshots s
             LEFT JOIN tags t ON s.hash = t.snapshot_hash
             WHERE s.branch = ?1
               AND s.branch NOT LIKE '_deleted_%'
             ORDER BY s.created_at DESC, s.rowid DESC LIMIT ?2"
        } else {
            "SELECT s.hash, s.message, s.created_at, s.branch, t.name
             FROM snapshots s
             LEFT JOIN tags t ON s.hash = t.snapshot_hash
             WHERE s.branch NOT LIKE '_deleted_%'
             ORDER BY s.created_at DESC, s.rowid DESC LIMIT ?2"
        };

        let mut stmt = conn.prepare(sql)?;
        let branch_arg = target_branch.to_string();
        let rows = stmt.query_map(params![branch_arg, sql_limit], |r| {
            Ok(LogEntry {
                hash: r.get(0)?,
                message: r.get(1)?,
                date: r.get(2)?,
                branch: r.get(3)?,
                tag: r.get(4)?,
            })
        })?;
        for e in rows {
            history.push(e?);
        }
    } else {
        // Ancestry walk from PARENT on the current branch
        if current_parent.is_empty() {
            println!("No snapshots yet on branch '{}'.", style(branch).cyan());
            return Ok(());
        }
        println!("\nHistory for branch: {}", style(branch).cyan().bold());
        let mut stmt = conn.prepare(
            "WITH RECURSIVE cte(hash, message, created_at, branch, parent_hash) AS (
                SELECT hash, message, created_at, branch, parent_hash
                FROM snapshots WHERE hash = ?1
                UNION ALL
                SELECT s.hash, s.message, s.created_at, s.branch, s.parent_hash
                FROM snapshots s JOIN cte c ON s.hash = c.parent_hash
            )
            SELECT c.hash, c.message, c.created_at, c.branch, t.name
            FROM cte c
            LEFT JOIN tags t ON c.hash = t.snapshot_hash
            LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![current_parent, sql_limit], |r| {
            Ok(LogEntry {
                hash: r.get(0)?,
                message: r.get(1)?,
                date: r.get(2)?,
                branch: r.get(3)?,
                tag: r.get(4)?,
            })
        })?;
        for e in rows {
            history.push(e?);
        }
    }

    if history.is_empty() {
        println!("  {}", style("No snapshots found.").dim());
        return Ok(());
    }

    if oneline {
        print_oneline(&history, current_parent);
    } else {
        print_full(&history, current_parent);
    }

    println!();
    Ok(())
}

fn safe_date(s: &str) -> &str {
    if s.len() >= 19 {
        &s[..19]
    } else {
        s
    }
}

fn print_full(history: &[LogEntry], current_parent: &str) {
    println!(
        "{:<4} {:<12} | {:<15} | {:<19} | {}",
        "", "Hash", "Branch", "Created At", "Message"
    );
    println!("{}", "-".repeat(80));

    for e in history {
        let branch_disp = if e.branch.len() > 15 {
            format!("{}..", &e.branch[..13])
        } else {
            e.branch.clone()
        };

        let tag_str = match &e.tag {
            Some(t) => format!(" {} ", style(format!("[{}]", t)).yellow().bold()),
            None => String::new(),
        };

        let is_current = e.hash == current_parent;
        let prefix = if is_current {
            style("--> ").green().bold().to_string()
        } else {
            "    ".into()
        };

        println!(
            "{}{:<12} | {:<15} | {:<19} | {}{}",
            prefix,
            style(&e.hash).yellow(),
            style(&branch_disp).dim(),
            style(safe_date(&e.date)).dim(),
            tag_str,
            style(&e.message).white()
        );
    }
}

fn print_oneline(history: &[LogEntry], current_parent: &str) {
    for e in history {
        let marker = if e.hash == current_parent { "* " } else { "  " };
        let tag_str = match &e.tag {
            Some(t) => format!(" {}", style(format!("[{}]", t)).yellow()),
            None => String::new(),
        };
        println!(
            "{}{} {}{}  {}",
            marker,
            style(&e.hash).yellow(),
            style(&e.branch).dim(),
            tag_str,
            e.message
        );
    }
}
