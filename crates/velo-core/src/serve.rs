//! Server side of the streaming sync protocol.
//!
//! These run on the *remote* host (invoked as `velo serve-upload <path>` /
//! `velo serve-receive <path>`, typically over ssh) and speak the binary
//! protocol described in `transport.rs` over stdin/stdout. They must write
//! nothing to stdout except protocol bytes; diagnostics go to stderr.
//!
//! The [`http`] submodule exposes the same operations as plain
//! bytes-in/bytes-out functions, so a service can mount them in any web
//! framework without velo-core depending on one. Both front ends share the
//! private helpers below, so there is one implementation of "which pack does
//! this client need" and "is this push a fast-forward".

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::commands::{all_branch_tips, bundle};
use crate::error::{Result, VeloError};
use crate::transport;

fn require_repo(path: &str) -> Result<PathBuf> {
    let root = PathBuf::from(path);
    check_repo(&root)?;
    Ok(root)
}

fn check_repo(root: &Path) -> Result<()> {
    if root.join(".velo/velo.db").is_file() {
        Ok(())
    } else {
        Err(VeloError::invalid(format!(
            "'{}' is not a Velo repository.",
            root.display()
        )))
    }
}

/// The pack a client holding `have` needs from a repository whose branch tips
/// are `tips`: everything reachable from a tip that the client lacks, minus
/// objects it already holds through the snapshots it reported.
fn pack_for(
    conn: &rusqlite::Connection,
    objects: &crate::storage::ObjectStore,
    tips: &[(String, String)],
    have: &HashSet<String>,
) -> Result<bundle::Bundle> {
    let mut snap_set: HashSet<String> = HashSet::new();
    for (_b, tip) in tips {
        snap_set.extend(bundle::reachable_ancestry(conn, tip));
    }
    for h in have {
        snap_set.remove(h);
    }
    bundle::build_pack_excluding(conn, objects, &snap_set, have)
}

/// Run the fast-forward check and import if it passes, returning the status
/// string the client expects (`OK <snapshots> <objects>` or `REJECT <reason>`).
fn apply_push(
    guard: &crate::WriteGuard,
    branch: &str,
    new_tip: &str,
    pack: &bundle::Bundle,
) -> Result<String> {
    Ok(
        match transport::fast_forward_check(guard.conn(), branch, new_tip, pack) {
            Some(reason) => format!("REJECT {}", reason),
            None => {
                let (s, o) = bundle::import_pack(guard, pack)?;
                guard.repo().emit_imported(s);
                format!("OK {} {}", s, o)
            }
        },
    )
}

/// Serve a fetch: advertise refs, read the client's "have" set, send a pack of
/// everything reachable from our tips that the client lacks.
pub fn upload(path: &str) -> Result<()> {
    let root = require_repo(path)?;
    let repo = crate::Repo::open_and_migrate(&root)?;
    let conn = repo.conn();

    let stdout = io::stdout();
    let mut out = stdout.lock();

    let tips = all_branch_tips(conn);
    transport::write_refs(&mut out, &tips)?;
    out.flush().map_err(VeloError::Io)?;

    // Read the client's have-set until EOF.
    let stdin = io::stdin();
    let mut inp = stdin.lock();
    let mut have: HashSet<String> = HashSet::new();
    while let Some(h) = transport::read_string_opt(&mut inp)? {
        have.insert(h);
    }

    let pack = pack_for(conn, &repo.objects(), &tips, &have)?;
    out.write_all(&bundle::encode(&pack))
        .map_err(VeloError::Io)?;
    out.flush().map_err(VeloError::Io)?;
    Ok(())
}

/// Serve a push: advertise refs, read (branch, new_tip, pack), run the
/// fast-forward check, import if it passes, and report status.
pub fn receive(path: &str) -> Result<()> {
    let root = require_repo(path)?;
    let repo = crate::Repo::open_and_migrate(&root)?;
    // Hold the repo lock for the whole exchange so the push is atomic against
    // other velo processes on this host. The guard is also what lets us import.
    let guard = repo.write()?;

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let stdin = io::stdin();
    let mut inp = stdin.lock();

    // S→C refs.
    let tips = all_branch_tips(guard.conn());
    transport::write_refs(&mut out, &tips)?;
    out.flush().map_err(VeloError::Io)?;

    // C→S branch, new_tip, then the pack to EOF.
    let branch = transport::read_string(&mut inp)?;
    let new_tip = transport::read_string(&mut inp)?;
    let mut packbytes = Vec::new();
    inp.read_to_end(&mut packbytes).map_err(VeloError::Io)?;
    let pack = bundle::decode(&packbytes)?;

    let status = apply_push(&guard, &branch, &new_tip, &pack)?;
    transport::write_string(&mut out, &status)?;
    out.flush().map_err(VeloError::Io)?;
    Ok(())
}

/// The server side of the HTTP transport, as framework-agnostic handlers.
///
/// Each function is one endpoint: bytes in, bytes out, no sockets and no HTTP
/// library. A hosting service mounts them under any prefix it likes:
///
/// | Method | Path               | Handler           |
/// |--------|--------------------|-------------------|
/// | GET    | `/velo/v1/refs`    | [`http::refs`]    |
/// | POST   | `/velo/v1/upload`  | [`http::upload`]  |
/// | POST   | `/velo/v1/receive` | [`http::receive`] |
///
/// An `Err` is a server-side failure (map it to a 4xx/5xx); a refused push is
/// *not* an error, it is an `Ok` carrying a `REJECT` status, exactly as over
/// ssh. There is no authentication here: put a reverse proxy in front.
pub mod http {
    use std::collections::HashSet;
    use std::io::Cursor;
    use std::path::Path;

    use super::{apply_push, check_repo, pack_for};
    use crate::commands::{all_branch_tips, bundle};
    use crate::error::Result;
    use crate::transport;

    /// `GET /velo/v1/refs`: the branch tips, as a refs block.
    pub fn refs(root: &Path) -> Result<Vec<u8>> {
        check_repo(root)?;
        let repo = crate::Repo::open_and_migrate(root)?;
        let mut out = Vec::new();
        transport::write_refs(&mut out, &all_branch_tips(repo.conn()))?;
        Ok(out)
    }

    /// `POST /velo/v1/upload`: serve a fetch.
    ///
    /// `body` is the client's have-ids as length-prefixed strings. The response
    /// is a refs block followed by the pack.
    pub fn upload(root: &Path, body: &[u8]) -> Result<Vec<u8>> {
        check_repo(root)?;
        let repo = crate::Repo::open_and_migrate(root)?;
        let conn = repo.conn();

        let mut inp = Cursor::new(body);
        let mut have: HashSet<String> = HashSet::new();
        while let Some(h) = transport::read_string_opt(&mut inp)? {
            have.insert(h);
        }

        let tips = all_branch_tips(conn);
        let mut out = Vec::new();
        transport::write_refs(&mut out, &tips)?;
        let pack = pack_for(conn, &repo.objects(), &tips, &have)?;
        out.extend_from_slice(&bundle::encode(&pack));
        Ok(out)
    }

    /// `POST /velo/v1/receive`: serve a push.
    ///
    /// `body` is the branch and new tip (length-prefixed strings) followed by
    /// the pack. The response is a length-prefixed status string, `OK n m` or
    /// `REJECT reason`. The repository write lock is held throughout.
    pub fn receive(root: &Path, body: &[u8]) -> Result<Vec<u8>> {
        check_repo(root)?;
        let repo = crate::Repo::open_and_migrate(root)?;
        let guard = repo.write()?;

        let mut inp = Cursor::new(body);
        let branch = transport::read_string(&mut inp)?;
        let new_tip = transport::read_string(&mut inp)?;
        let at = inp.position() as usize;
        let pack = bundle::decode(&body[at..])?;

        let status = apply_push(&guard, &branch, &new_tip, &pack)?;
        let mut out = Vec::new();
        transport::write_string(&mut out, &status)?;
        Ok(out)
    }
}
