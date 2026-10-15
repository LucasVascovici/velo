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
    /// Serve MCP's Streamable HTTP transport on ADDR (e.g. 127.0.0.1:8418)
    /// instead of stdio. Endpoint: /mcp.
    #[arg(long, value_name = "ADDR")]
    http: Option<String>,
    /// With --http, a file of bearer tokens, one per line (else $VELO_MCP_TOKEN).
    #[arg(long, value_name = "FILE", requires = "http")]
    token_file: Option<PathBuf>,
    /// With --http, an extra allowed Origin besides loopback ones (repeatable).
    #[arg(long, value_name = "ORIGIN", requires = "http")]
    allow_origin: Vec<String>,
}

/// Tokens from `--token-file` and `$VELO_MCP_TOKEN`, if any.
fn load_tokens(file: Option<&PathBuf>) -> Result<Option<velo_core::serve::http::Tokens>, String> {
    let mut tokens = velo_core::serve::http::Tokens::new();
    let mut any = false;
    if let Some(f) = file {
        let text = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        for line in text.lines().map(str::trim) {
            if !line.is_empty() && !line.starts_with('#') {
                tokens = tokens.write(line);
                any = true;
            }
        }
        if !any {
            return Err(format!("{}: no tokens in file", f.display()));
        }
    }
    if let Ok(t) = std::env::var("VELO_MCP_TOKEN") {
        if !t.is_empty() {
            tokens = tokens.write(t);
            any = true;
        }
    }
    Ok(any.then_some(tokens))
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
    if let Some(listen) = &args.http {
        let tokens = match load_tokens(args.token_file.as_ref()) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("velo-mcp: {e}");
                std::process::exit(2);
            }
        };
        let cfg = velo_mcp::http::HttpConfig {
            tokens,
            allow_origins: args.allow_origin,
        };
        if let Err(e) = velo_mcp::http::serve(server, listen, cfg) {
            eprintln!("velo-mcp: {e}");
            std::process::exit(2);
        }
        return;
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
