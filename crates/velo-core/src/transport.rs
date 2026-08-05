//! Transport abstraction for sync (Phase 2 filesystem + Phase 3 network).
//!
//! A [`Remote`] is a source/sink of history. Two implementations:
//!   * [`LocalRemote`] — a repo reachable by path; read/write its DB directly.
//!   * [`StreamRemote`] — a repo reachable by spawning a subprocess that runs
//!     `velo serve-upload`/`serve-receive` and speaking a small pack protocol
//!     over its stdin/stdout. Used for `ssh://…` (via `ssh`) and `child:…` (a
//!     local subprocess, used by tests and for local streaming).
//!
//! The sync verbs (`clone`/`fetch`/`push`/`pull`) are written against this
//! trait, so they are transport-agnostic.
//!
//! ## Wire protocol (little-endian, length-prefixed)
//! Strings are `u32 len + bytes`. A "refs" block is `u32 count` then that many
//! `(branch, hash)` string pairs.
//!
//! **upload** (server serves a fetch):
//!   `S→C` refs · `C→S` client's "have" hashes then EOF · `S→C` pack then EOF.
//! **receive** (server accepts a push):
//!   `S→C` refs · `C→S` branch, new_tip, pack then EOF · `S→C` status string.
//!
//! The pack is framed by EOF rather than a length prefix, which is why transfer
//! progress is reported as a running byte count with no total: the reader cannot
//! know the size in advance. Giving it one means length-prefixing the pack, and
//! that is a wire-format break — `serve-upload` / `serve-receive` run on the far
//! host, which may be an older build — so it would need a negotiated protocol
//! version. Deliberately not done here.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use crate::commands::bundle::{self, Bundle};
use crate::commands::{all_branch_tips, branch_tip};
use crate::error::{Result, VeloError};
use crate::progress::PhaseGuard;

pub struct RemoteRef {
    pub branch: String,
    pub hash: String,
}

pub enum PushOutcome {
    Ok {
        new_snapshots: usize,
        new_objects: usize,
    },
    Rejected(String),
}

/// A place history can be fetched from and pushed to.
pub trait Remote {
    /// Return the remote's branch tips and a pack of everything reachable from
    /// them that isn't already in `have`.
    ///
    /// `progress` is ticked as bytes move. A transport that doesn't touch a wire
    /// ignores it.
    fn fetch(
        &mut self,
        have: &HashSet<String>,
        progress: &PhaseGuard,
    ) -> Result<(Vec<RemoteRef>, Bundle)>;

    /// Push `branch` at `new_tip`.
    ///
    /// `build` is called with the remote's advertised refs so the caller can
    /// assemble a *minimal* pack (skipping history and objects the remote
    /// already has) without an extra round-trip. The remote then performs the
    /// fast-forward check and imports if it passes.
    fn push(
        &mut self,
        branch: &str,
        new_tip: &str,
        build: &mut dyn FnMut(&[RemoteRef]) -> Result<Bundle>,
        progress: &PhaseGuard,
    ) -> Result<PushOutcome>;
}

/// How much of a pack to move between progress ticks.
///
/// 64 KiB keeps the observer to roughly sixteen calls per megabyte — frequent
/// enough to look live, rare enough that the reporting costs nothing next to the
/// I/O.
const CHUNK: usize = 64 * 1024;

/// How to reach a remote that needs a subprocess.
///
/// None of this is discoverable from inside the library: locating the running
/// binary and reading environment variables are both the caller's business, and
/// a consumer that embeds Velo may have neither a `velo` executable nor an
/// `ssh` on the path. So it is passed in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spawn {
    /// The Velo binary to run for a `child:` URL. Normally the running
    /// executable, which only the caller can locate.
    pub local_bin: PathBuf,
    /// The SSH client to invoke for an `ssh://` URL.
    pub ssh: String,
    /// The Velo binary to run on the far host.
    pub remote_bin: String,
}

impl Spawn {
    /// Defaults suitable for a normal command-line install: `ssh` on the path
    /// here, `velo` on the path there.
    pub fn new(local_bin: impl Into<PathBuf>) -> Self {
        Spawn {
            local_bin: local_bin.into(),
            ssh: "ssh".into(),
            remote_bin: "velo".into(),
        }
    }

    /// Use a different SSH client.
    pub fn ssh(mut self, program: impl Into<String>) -> Self {
        self.ssh = program.into();
        self
    }

    /// Use a different Velo binary on the far host.
    pub fn remote_bin(mut self, program: impl Into<String>) -> Self {
        self.remote_bin = program.into();
        self
    }
}

/// Dispatch a URL to a transport. `ssh://` and `child:` stream over a
/// subprocess; anything else is treated as a local filesystem path.
pub fn open(url: &str, spawn: &Spawn) -> Result<Box<dyn Remote>> {
    if url.starts_with("ssh://") || url.starts_with("child:") {
        Ok(Box::new(StreamRemote {
            url: url.to_string(),
            spawn: spawn.clone(),
        }))
    } else {
        Ok(Box::new(LocalRemote {
            root: crate::commands::remote::resolve_remote_root(url)?,
        }))
    }
}

// ─── Local (direct DB) transport ──────────────────────────────────────────────

pub struct LocalRemote {
    root: PathBuf,
}

impl Remote for LocalRemote {
    fn fetch(
        &mut self,
        have: &HashSet<String>,
        _progress: &PhaseGuard,
    ) -> Result<(Vec<RemoteRef>, Bundle)> {
        // No wire: a local remote reads the other repository's database directly,
        // so there is nothing to report moving.
        // Reading the far repository goes through `Repo` so its schema version is
        // checked too — a remote written by a newer Velo is refused, not misread.
        let repo = crate::Repo::open_and_migrate(&self.root)?;
        let conn = repo.conn();
        let objects = self.root.join(".velo/objects");
        let tips = all_branch_tips(conn);

        let mut snap_set: HashSet<String> = HashSet::new();
        for (_b, tip) in &tips {
            snap_set.extend(bundle::reachable_ancestry(conn, tip));
        }
        for h in have {
            snap_set.remove(h);
        }
        // Skip objects the client already holds via the snapshots it reported.
        let pack = bundle::build_pack_excluding(conn, &objects, &snap_set, have)?;
        let refs = tips
            .into_iter()
            .map(|(branch, hash)| RemoteRef { branch, hash })
            .collect();
        Ok((refs, pack))
    }

    fn push(
        &mut self,
        branch: &str,
        new_tip: &str,
        build: &mut dyn FnMut(&[RemoteRef]) -> Result<Bundle>,
        _progress: &PhaseGuard,
    ) -> Result<PushOutcome> {
        // A local push writes into *another* repository, so it opens that one and
        // takes its lock. The guard is both the lock and the write permission.
        let repo = crate::Repo::open_and_migrate(&self.root)?;
        let guard = repo.write()?;
        let conn = guard.conn();
        let objects = self.root.join(".velo/objects");

        let refs: Vec<RemoteRef> = all_branch_tips(conn)
            .into_iter()
            .map(|(branch, hash)| RemoteRef { branch, hash })
            .collect();
        let pack = build(&refs)?;

        match fast_forward_check(conn, branch, new_tip, &pack) {
            Some(reason) => Ok(PushOutcome::Rejected(reason)),
            None => {
                let (s, o) = bundle::import_pack(&guard, &objects, &pack)?;
                Ok(PushOutcome::Ok {
                    new_snapshots: s,
                    new_objects: o,
                })
            }
        }
    }
}

/// Shared fast-forward gate for a push. Returns `Some(reason)` to reject.
///
/// A push is a fast-forward when the branch has no tip yet (new branch), the tip
/// already equals `new_tip`, or the current tip is an **ancestor** of `new_tip`.
/// Ancestry is walked over the pushed pack *unioned with* what the receiver
/// already has — necessary because a minimal pack omits commits the receiver
/// holds, including (usually) its own current tip.
pub(crate) fn fast_forward_check(
    conn: &rusqlite::Connection,
    branch: &str,
    new_tip: &str,
    pack: &Bundle,
) -> Option<String> {
    // No tip yet → new branch on the remote → fast-forward trivially.
    let old = branch_tip(conn, branch)?;
    if old == new_tip {
        return None; // already up to date
    }

    if reaches(conn, pack, new_tip, &old) {
        return None; // reachable → fast-forward
    }
    Some(format!(
        "'{}' has commits you don't have (non-fast-forward). Pull and reconcile first.",
        branch
    ))
}

/// Is `needle` an ancestor of `from`, walking parent links over `pack` **unioned
/// with** the local database?
///
/// Both halves are required: a minimal pack omits commits the receiver already
/// has (so the walk must fall through to the DB), while the DB lacks the commits
/// still in flight (so the pack must be consulted first).
pub(crate) fn reaches(
    conn: &rusqlite::Connection,
    pack: &Bundle,
    from: &str,
    needle: &str,
) -> bool {
    let pack_parents: std::collections::HashMap<&str, (&str, &str)> = pack
        .snapshots
        .iter()
        .map(|s| {
            (
                s.hash.as_str(),
                (s.parent_hash.as_str(), s.merge_parent.as_str()),
            )
        })
        .collect();

    let mut stack = vec![from.to_string()];
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(h) = stack.pop() {
        if h == needle {
            return true;
        }
        if !seen.insert(h.clone()) {
            continue;
        }
        // Prefer the pack (commits in flight), then fall back to local history.
        let parents = if let Some((p, m)) = pack_parents.get(h.as_str()) {
            Some((p.to_string(), m.to_string()))
        } else {
            conn.query_row(
                "SELECT parent_hash, merge_parent FROM snapshots WHERE hash = ?",
                [&h],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .ok()
        };
        if let Some((p, m)) = parents {
            if !p.is_empty() {
                stack.push(p);
            }
            if !m.is_empty() {
                stack.push(m);
            }
        }
    }
    false
}

// ─── Streaming (subprocess) transport ──────────────────────────────────────────

pub struct StreamRemote {
    url: String,
    spawn: Spawn,
}

impl Remote for StreamRemote {
    fn fetch(
        &mut self,
        have: &HashSet<String>,
        progress: &PhaseGuard,
    ) -> Result<(Vec<RemoteRef>, Bundle)> {
        let mut child = spawn_server(&self.url, "serve-upload", &self.spawn)?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = child.stdout.take().expect("piped stdout");

        // S→C refs
        let refs = read_refs(&mut stdout)?;
        // C→S haves, then EOF (drop stdin)
        for h in have {
            write_string(&mut stdin, h)?;
        }
        stdin.flush().ok();
        drop(stdin);
        // S→C pack, in chunks so the caller sees it arriving. Read to EOF: the
        // sender does not announce a length, so there is no total to report.
        let mut packbytes = Vec::new();
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = stdout.read(&mut buf).map_err(VeloError::Io)?;
            if n == 0 {
                break;
            }
            packbytes.extend_from_slice(&buf[..n]);
            progress.advance(n as u64);
        }
        finish(child)?;
        let bundle = bundle::decode(&packbytes)?;
        Ok((refs, bundle))
    }

    fn push(
        &mut self,
        branch: &str,
        new_tip: &str,
        build: &mut dyn FnMut(&[RemoteRef]) -> Result<Bundle>,
        progress: &PhaseGuard,
    ) -> Result<PushOutcome> {
        let mut child = spawn_server(&self.url, "serve-receive", &self.spawn)?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = child.stdout.take().expect("piped stdout");

        // S→C refs — used to build a minimal pack (no extra round-trip).
        let refs = read_refs(&mut stdout)?;
        let pack = build(&refs)?;
        // C→S branch, new_tip, pack, then EOF
        write_string(&mut stdin, branch)?;
        write_string(&mut stdin, new_tip)?;
        // Written in chunks for the same reason the read is: so a slow link looks
        // alive rather than hung.
        let encoded = bundle::encode(&pack);
        for chunk in encoded.chunks(CHUNK) {
            stdin.write_all(chunk).map_err(VeloError::Io)?;
            progress.advance(chunk.len() as u64);
        }
        stdin.flush().ok();
        drop(stdin);
        // S→C status
        let status = read_string(&mut stdout)?;
        finish(child)?;
        parse_status(&status)
    }
}

fn parse_status(status: &str) -> Result<PushOutcome> {
    // "OK <snaps> <objs>" or "REJECT <reason…>"
    if let Some(rest) = status.strip_prefix("OK ") {
        let mut it = rest.split_whitespace();
        let s = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        let o = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        Ok(PushOutcome::Ok {
            new_snapshots: s,
            new_objects: o,
        })
    } else if let Some(reason) = status.strip_prefix("REJECT ") {
        Ok(PushOutcome::Rejected(reason.to_string()))
    } else {
        Err(VeloError::invalid(format!(
            "Unexpected response from remote: {}",
            status
        )))
    }
}

/// Build and spawn the server subprocess for `url` running `op`
/// (`serve-upload` / `serve-receive`).
fn spawn_server(url: &str, op: &str, spawn: &Spawn) -> Result<Child> {
    let (program, args) = command_for(url, op, spawn)?;
    Command::new(&program)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| VeloError::invalid(format!("Failed to start '{}': {}", program, e)))
}

/// Translate a streaming URL into a command to spawn.
///   `ssh://[user@]host[:port]/path` → `ssh [-p port] host velo <op> /path`
///   `child:PATH`                    → `<this binary> <op> PATH`
fn command_for(url: &str, op: &str, spawn: &Spawn) -> Result<(String, Vec<String>)> {
    if let Some(path) = url.strip_prefix("child:") {
        return Ok((
            spawn.local_bin.to_string_lossy().into_owned(),
            vec![op.to_string(), path.to_string()],
        ));
    }
    if let Some(rest) = url.strip_prefix("ssh://") {
        // Split host[:port] from the path at the first '/'.
        let (hostport, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]), // keep the leading '/'
            None => {
                return Err(VeloError::invalid(
                    "ssh URL must include a path: ssh://host/path",
                ))
            }
        };
        let mut args: Vec<String> = Vec::new();
        // Optional :port
        let host = if let Some((h, port)) = split_port(hostport) {
            args.push("-p".into());
            args.push(port.to_string());
            h.to_string()
        } else {
            hostport.to_string()
        };
        args.push(host);
        args.push(spawn.remote_bin.clone());
        args.push(op.to_string());
        args.push(path.to_string());
        return Ok((spawn.ssh.clone(), args));
    }
    Err(VeloError::invalid(format!("Not a streaming URL: {}", url)))
}

/// Split a trailing `:port` off `[user@]host` if present and numeric.
fn split_port(hostport: &str) -> Option<(&str, u16)> {
    let idx = hostport.rfind(':')?;
    let (host, port) = (&hostport[..idx], &hostport[idx + 1..]);
    port.parse::<u16>().ok().map(|p| (host, p))
}

fn finish(mut child: Child) -> Result<()> {
    let status = child.wait().map_err(VeloError::Io)?;
    if status.success() {
        Ok(())
    } else {
        Err(VeloError::invalid(
            "Remote velo process exited with an error (see messages above).",
        ))
    }
}

// ─── Protocol I/O primitives (shared with serve.rs) ─────────────────────────────

pub(crate) fn write_string<W: Write>(w: &mut W, s: &str) -> Result<()> {
    w.write_all(&(s.len() as u32).to_le_bytes())
        .map_err(VeloError::Io)?;
    w.write_all(s.as_bytes()).map_err(VeloError::Io)?;
    Ok(())
}

/// Read exactly `buf.len()` bytes, or fewer only at a clean EOF. Returns the
/// number of bytes actually read.
fn fill<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]).map_err(VeloError::Io)? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// Read a length-prefixed string, or `None` at a clean stream EOF.
pub(crate) fn read_string_opt<R: Read>(r: &mut R) -> Result<Option<String>> {
    let mut lenb = [0u8; 4];
    match fill(r, &mut lenb)? {
        0 => return Ok(None),
        4 => {}
        _ => return Err(proto_err()),
    }
    let len = u32::from_le_bytes(lenb) as usize;
    let mut buf = vec![0u8; len];
    if fill(r, &mut buf)? != len {
        return Err(proto_err());
    }
    Ok(Some(String::from_utf8(buf).map_err(|_| proto_err())?))
}

pub(crate) fn read_string<R: Read>(r: &mut R) -> Result<String> {
    read_string_opt(r)?.ok_or_else(proto_err)
}

pub(crate) fn write_refs<W: Write>(w: &mut W, refs: &[(String, String)]) -> Result<()> {
    w.write_all(&(refs.len() as u32).to_le_bytes())
        .map_err(VeloError::Io)?;
    for (branch, hash) in refs {
        write_string(w, branch)?;
        write_string(w, hash)?;
    }
    Ok(())
}

fn read_refs<R: Read>(r: &mut R) -> Result<Vec<RemoteRef>> {
    let mut lenb = [0u8; 4];
    if fill(r, &mut lenb)? != 4 {
        return Err(proto_err());
    }
    let count = u32::from_le_bytes(lenb);
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let branch = read_string(r)?;
        let hash = read_string(r)?;
        out.push(RemoteRef { branch, hash });
    }
    Ok(out)
}

fn proto_err() -> VeloError {
    VeloError::invalid("Malformed data from remote (protocol error).")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_port_parses_trailing_numeric_port() {
        assert_eq!(split_port("host:2222"), Some(("host", 2222)));
        assert_eq!(split_port("user@host:22"), Some(("user@host", 22)));
        assert_eq!(split_port("host"), None);
        assert_eq!(split_port("user@host"), None);
    }

    fn spawn_cfg() -> Spawn {
        Spawn::new("/usr/local/bin/velo")
    }

    #[test]
    fn ssh_url_builds_expected_command() {
        let (prog, args) =
            command_for("ssh://user@host/srv/repo", "serve-upload", &spawn_cfg()).unwrap();
        assert_eq!(prog, "ssh");
        assert_eq!(args, vec!["user@host", "velo", "serve-upload", "/srv/repo"]);
    }

    #[test]
    fn ssh_url_with_port_uses_dash_p() {
        let (prog, args) =
            command_for("ssh://host:2222/repo", "serve-receive", &spawn_cfg()).unwrap();
        assert_eq!(prog, "ssh");
        assert_eq!(
            args,
            vec!["-p", "2222", "host", "velo", "serve-receive", "/repo"]
        );
    }

    #[test]
    fn ssh_url_without_path_is_error() {
        assert!(command_for("ssh://host", "serve-upload", &spawn_cfg()).is_err());
    }

    #[test]
    fn child_url_runs_the_configured_binary() {
        // No `current_exe()` guesswork: the caller says which binary to run, so
        // the test can assert the exact command.
        let (prog, args) = command_for("child:/some/path", "serve-upload", &spawn_cfg()).unwrap();
        assert_eq!(prog, "/usr/local/bin/velo");
        assert_eq!(args, vec!["serve-upload", "/some/path"]);
    }

    #[test]
    fn spawn_overrides_are_honoured() {
        let cfg = Spawn::new("/bin/velo")
            .ssh("/opt/bin/ssh")
            .remote_bin("/srv/velo");
        let (prog, args) = command_for("ssh://host/repo", "serve-upload", &cfg).unwrap();
        assert_eq!(prog, "/opt/bin/ssh");
        assert_eq!(args, vec!["host", "/srv/velo", "serve-upload", "/repo"]);
    }
}
