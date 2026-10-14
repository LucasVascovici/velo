//! `velo-mcp` binary: argument parsing and the stdio loop.
#![forbid(unsafe_code)]

use std::io::{BufRead, Write};
use std::path::PathBuf;

use clap::Parser;
use velo_core::Repo;
use velo_mcp::{resolve_run_id, Server};

/// A stdio MCP server over a velo repository.
#[derive(Parser)]
#[command(name = "velo-mcp", version)]
struct Args {
    /// Repository to serve (discovered upward from here).
    #[arg(long, default_value = ".", conflicts_with = "store")]
    repo: PathBuf,
    /// Serve a single-file repository with no working tree (store-only mode).
    #[arg(long, value_name = "FILE")]
    store: Option<PathBuf>,
    /// With --store, create the file when it is missing.
    #[arg(long, requires = "store")]
    create: bool,
    /// Run identity recorded on every write (else $VELO_MCP_RUN, else generated).
    #[arg(long)]
    run: Option<String>,
    /// Seconds a write tool waits for the repository lock before it answers
    /// `Locked`, naming the holder.
    #[arg(long, value_name = "SECS", default_value_t = 5.0)]
    lock_wait: f64,
}

fn main() {
    let args = Args::parse();
    let opened = match &args.store {
        Some(file) if args.create && !file.exists() => Repo::create_file(file),
        Some(file) => Repo::open_file_and_migrate(file),
        None => Repo::discover(&args.repo),
    };
    let repo = match opened {
        Ok(r) => r,
        Err(e) => {
            eprintln!("velo-mcp: {e}");
            std::process::exit(1);
        }
    };
    let run = resolve_run_id(args.run, std::env::var("VELO_MCP_RUN").ok());
    if !args.lock_wait.is_finite() || args.lock_wait < 0.0 {
        eprintln!("velo-mcp: --lock-wait must be a number of seconds, zero or more");
        std::process::exit(2);
    }
    let mut server =
        Server::new(repo, run).with_lock_wait(std::time::Duration::from_secs_f64(args.lock_wait));
    if let Ok(name) = std::env::var("VELO_AUTHOR_NAME") {
        server = server.with_author(name, std::env::var("VELO_AUTHOR_EMAIL").ok());
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = server.handle(&line) {
            if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    }
}
