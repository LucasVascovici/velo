//! `velo-mcp` binary: argument parsing and the stdio loop.

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
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    /// Run identity recorded on every write (else $VELO_MCP_RUN, else generated).
    #[arg(long)]
    run: Option<String>,
}

fn main() {
    let args = Args::parse();
    let repo = match Repo::discover(&args.repo) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("velo-mcp: {e}");
            std::process::exit(1);
        }
    };
    let run = resolve_run_id(args.run, std::env::var("VELO_MCP_RUN").ok());
    let mut server = Server::new(repo, run);
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
