//! Server side of the streaming sync protocol.
//!
//! These run on the *remote* host (invoked as `velo serve-upload <path>` /
//! `velo serve-receive <path>`, typically over ssh) and speak the binary
//! protocol described in `transport.rs` over stdin/stdout. They must write
//! nothing to stdout except protocol bytes; diagnostics go to stderr.

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::path::PathBuf;

use crate::commands::{all_branch_tips, bundle};
use crate::db;
use crate::error::{Result, VeloError};
use crate::{lock, transport};

fn require_repo(path: &str) -> Result<PathBuf> {
    let root = PathBuf::from(path);
    if root.join(".velo/velo.db").is_file() {
        Ok(root)
    } else {
        Err(VeloError::InvalidInput(format!(
            "'{}' is not a Velo repository.",
            path
        )))
    }
}

/// Serve a fetch: advertise refs, read the client's "have" set, send a pack of
/// everything reachable from our tips that the client lacks.
pub fn upload(path: &str) -> Result<()> {
    let root = require_repo(path)?;
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let objects = root.join(".velo/objects");

    let stdout = io::stdout();
    let mut out = stdout.lock();

    let tips = all_branch_tips(&conn);
    transport::write_refs(&mut out, &tips)?;
    out.flush().map_err(VeloError::Io)?;

    // Read the client's have-set until EOF.
    let stdin = io::stdin();
    let mut inp = stdin.lock();
    let mut have: HashSet<String> = HashSet::new();
    while let Some(h) = transport::read_string_opt(&mut inp)? {
        have.insert(h);
    }

    let mut snap_set: HashSet<String> = HashSet::new();
    for (_b, tip) in &tips {
        snap_set.extend(bundle::reachable_ancestry(&conn, tip));
    }
    for h in &have {
        snap_set.remove(h);
    }
    // Skip objects the client already holds via the snapshots it reported.
    let pack = bundle::build_pack_excluding(&conn, &objects, &snap_set, &have)?;
    out.write_all(&bundle::encode(&pack)).map_err(VeloError::Io)?;
    out.flush().map_err(VeloError::Io)?;
    Ok(())
}

/// Serve a push: advertise refs, read (branch, new_tip, pack), run the
/// fast-forward check, import if it passes, and report status.
pub fn receive(path: &str) -> Result<()> {
    let root = require_repo(path)?;
    // Hold the repo lock for the whole exchange so the push is atomic against
    // other velo processes on this host.
    let _guard = lock::RepoLock::acquire(&root)?;
    let objects = root.join(".velo/objects");

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let stdin = io::stdin();
    let mut inp = stdin.lock();

    let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    // S→C refs.
    let tips = all_branch_tips(&conn);
    transport::write_refs(&mut out, &tips)?;
    out.flush().map_err(VeloError::Io)?;

    // C→S branch, new_tip, then the pack to EOF.
    let branch = transport::read_string(&mut inp)?;
    let new_tip = transport::read_string(&mut inp)?;
    let mut packbytes = Vec::new();
    inp.read_to_end(&mut packbytes).map_err(VeloError::Io)?;
    let pack = bundle::decode(&packbytes)?;

    let status = match transport::fast_forward_check(&conn, &branch, &new_tip, &pack) {
        Some(reason) => format!("REJECT {}", reason),
        None => {
            let (s, o) = bundle::import_pack(&mut conn, &objects, &pack)?;
            format!("OK {} {}", s, o)
        }
    };
    transport::write_string(&mut out, &status)?;
    out.flush().map_err(VeloError::Io)?;
    Ok(())
}
