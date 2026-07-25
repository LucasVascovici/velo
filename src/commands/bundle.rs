//! `velo bundle` — offline history transfer.
//!
//! `bundle create <file> [ref]` packs history (all of it, or everything
//! reachable from `ref`) plus every object it references into a single,
//! self-contained, versioned file. `bundle apply <file>` imports that file into
//! another repository, verifying every object and snapshot as it lands.
//!
//! This is Phase 1 of collaboration: no network, no remotes — the primitives
//! (enumerate → serialise → transfer → import → verify) that push/pull are built
//! on. Bundles are always self-contained: reachability walks to the root, so
//! every included snapshot's parents are included too.
//!
//! ## File format (little-endian)
//! ```text
//! magic   : 8 bytes  "VELOBND1"
//! version : u32       (= FORMAT_VERSION)
//! snapshots: u32 count, then per row: 6 length-prefixed strings
//!            (hash, message, branch, parent_hash, merge_parent, created_at)
//! file_map : u32 count, then per row: 3 strings + i64 mode
//!            (snapshot_hash, path, hash, mode)
//! tags     : u32 count, then per row: 2 strings (name, snapshot_hash)
//! objects  : u32 count, then per row: string hash, u32 len, len bytes
//!            (the raw, already-zstd-compressed object file, verbatim)
//! ```

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use console::style;
use rusqlite::params;

use crate::db;
use crate::error::{Result, VeloError};
use crate::storage;

const MAGIC: &[u8; 8] = b"VELOBND1";
const FORMAT_VERSION: u32 = 1;

// ─── In-memory bundle ──────────────────────────────────────────────────────────

pub(crate) struct SnapshotRow {
    pub hash: String,
    pub message: String,
    pub branch: String,
    pub parent_hash: String,
    pub merge_parent: String,
    pub created_at: String,
}

pub(crate) struct FileMapRow {
    pub snapshot_hash: String,
    pub path: String,
    pub hash: String,
    pub mode: i64,
}

/// An in-memory pack of history: the interchange unit for bundles *and* for
/// filesystem remotes (fetch/push build one from a source repo and import it
/// into a destination repo).
pub(crate) struct Bundle {
    pub snapshots: Vec<SnapshotRow>,
    pub file_map: Vec<FileMapRow>,
    pub tags: Vec<(String, String)>,      // (name, snapshot_hash)
    pub objects: Vec<(String, Vec<u8>)>,  // (hash, raw compressed bytes)
}

// ─── create ─────────────────────────────────────────────────────────────────────

pub fn create(root: &Path, file: &str, target: Option<&str>) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    // Which snapshots to include.
    let snap_set: HashSet<String> = match target {
        Some(t) => {
            let tip = crate::commands::resolve_snapshot_id(root, t)?;
            reachable_ancestry(&conn, &tip)
        }
        None => {
            let mut stmt = conn.prepare("SELECT hash FROM snapshots")?;
            let all = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect();
            all
        }
    };

    if snap_set.is_empty() {
        return Err(VeloError::InvalidInput(
            "Nothing to bundle — the repository has no snapshots.".into(),
        ));
    }

    let bundle = build_pack(&conn, &root.join(".velo/objects"), &snap_set)?;
    let encoded = encode(&bundle);
    fs::write(file, &encoded)?;

    println!(
        "{} Bundled {} snapshot(s), {} object(s), {} tag(s) → {} ({:.1} KB)",
        style("✔").green().bold(),
        bundle.snapshots.len(),
        bundle.objects.len(),
        bundle.tags.len(),
        style(file).cyan(),
        encoded.len() as f64 / 1024.0
    );
    Ok(())
}

/// Build a **self-contained** pack for `snap_set` (every referenced object is
/// included). Used by `bundle create`, where the output must stand alone.
pub(crate) fn build_pack(
    conn: &rusqlite::Connection,
    objects_dir: &Path,
    snap_set: &HashSet<String>,
) -> Result<Bundle> {
    build_pack_excluding(conn, objects_dir, snap_set, &HashSet::new())
}

/// Build a pack for `snap_set`, omitting objects the peer demonstrably already
/// has — i.e. any object referenced by a snapshot in `peer_has`.
///
/// This is the minimal-transfer path for `fetch`/`push`. Without it, a one-line
/// change in a 1000-file project ships all 1000 objects, because a snapshot's
/// file_map references its *whole* tree, not just what changed.
///
/// Correctness note: this trusts that a peer holding a snapshot also holds that
/// snapshot's objects — the invariant `velo fsck` enforces. `bundle create`
/// deliberately passes an empty `peer_has` so bundles stay self-contained.
pub(crate) fn build_pack_excluding(
    conn: &rusqlite::Connection,
    objects_dir: &Path,
    snap_set: &HashSet<String>,
    peer_has: &HashSet<String>,
) -> Result<Bundle> {
    let mut snapshots = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT hash, message, branch, parent_hash, merge_parent, created_at FROM snapshots",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SnapshotRow {
                hash: r.get(0)?,
                message: r.get(1)?,
                branch: r.get(2)?,
                parent_hash: r.get(3)?,
                merge_parent: r.get(4)?,
                created_at: r.get::<_, String>(5).unwrap_or_default(),
            })
        })?;
        for row in rows.flatten() {
            if snap_set.contains(&row.hash) {
                snapshots.push(row);
            }
        }
    }

    // One pass over file_map: collect the rows we're sending, the objects they
    // need, and (separately) the objects the peer already holds via `peer_has`.
    let mut file_map = Vec::new();
    let mut object_hashes: HashSet<String> = HashSet::new();
    let mut peer_objects: HashSet<String> = HashSet::new();
    {
        let mut stmt = conn.prepare("SELECT snapshot_hash, path, hash, mode FROM file_map")?;
        let rows = stmt.query_map([], |r| {
            Ok(FileMapRow {
                snapshot_hash: r.get(0)?,
                path: r.get(1)?,
                hash: r.get(2)?,
                mode: r.get(3)?,
            })
        })?;
        for row in rows.flatten() {
            if snap_set.contains(&row.snapshot_hash) {
                object_hashes.insert(row.hash.clone());
                file_map.push(row);
            } else if peer_has.contains(&row.snapshot_hash) {
                peer_objects.insert(row.hash);
            }
        }
    }
    for h in &peer_objects {
        object_hashes.remove(h);
    }

    let mut tags = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT name, snapshot_hash FROM tags")?;
        let rows =
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for (name, hash) in rows.flatten() {
            if snap_set.contains(&hash) {
                tags.push((name, hash));
            }
        }
    }

    let mut objects = Vec::with_capacity(object_hashes.len());
    for h in &object_hashes {
        let bytes = fs::read(objects_dir.join(h)).map_err(|_| {
            VeloError::CorruptRepo(format!("object {} is missing — run 'velo fsck'", h))
        })?;
        objects.push((h.clone(), bytes));
    }

    Ok(Bundle { snapshots, file_map, tags, objects })
}

// ─── apply ──────────────────────────────────────────────────────────────────────

pub fn apply(root: &Path, file: &str) -> Result<()> {
    let raw = fs::read(file).map_err(|e| {
        VeloError::InvalidInput(format!("Cannot read bundle '{}': {}", file, e))
    })?;
    let bundle = decode(&raw)?;

    let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let objects_dir = root.join(".velo/objects");
    let (new_snaps, new_objects) = import_pack(&mut conn, &objects_dir, &bundle)?;

    if new_snaps == 0 && new_objects == 0 {
        println!(
            "{} Already up to date — nothing new in this bundle.",
            style("✔").green()
        );
    } else {
        println!(
            "{} Imported {} snapshot(s) and {} object(s).",
            style("✔").green().bold(),
            new_snaps,
            new_objects
        );
        println!(
            "  Use {} to see them, or {} to check into one.",
            style("velo history --all").cyan(),
            style("velo switch <branch>").cyan()
        );
    }
    Ok(())
}

/// Import a pack into a repository: write+verify its objects (dedup by name),
/// then insert unknown snapshots/file_map/tags in one transaction, verifying
/// each snapshot's id against its content. Idempotent. Returns
/// `(new_snapshots, new_objects)`. Shared by `bundle apply`, `fetch`, `push`,
/// and `clone`.
pub(crate) fn import_pack(
    conn: &mut rusqlite::Connection,
    objects_dir: &Path,
    bundle: &Bundle,
) -> Result<(usize, usize)> {
    // ── Write + verify objects (dedup by name) ────────────────────────────────
    let mut new_objects = 0usize;
    for (hash, compressed) in &bundle.objects {
        // Verify *received* data: it must decompress and re-hash to its own name.
        let decompressed = zstd::decode_all(&compressed[..]).map_err(|_| {
            VeloError::CorruptRepo(format!("object {} could not be decompressed", hash))
        })?;
        let actual = blake3::hash(&decompressed).to_hex().to_string();
        if &actual != hash {
            return Err(VeloError::CorruptRepo(format!(
                "object {} is corrupt (content hashes to {})",
                hash, actual
            )));
        }
        let obj_path = objects_dir.join(hash);
        if !obj_path.exists() {
            storage::write_atomic(&obj_path, compressed)?;
            new_objects += 1;
        }
    }

    let existing: HashSet<String> = {
        let mut stmt = conn.prepare("SELECT hash FROM snapshots")?;
        let set = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        set
    };

    // Group file_map rows by snapshot for per-snapshot insertion + id checks.
    let mut fm_by_snap: HashMap<&str, Vec<&FileMapRow>> = HashMap::new();
    for row in &bundle.file_map {
        fm_by_snap.entry(row.snapshot_hash.as_str()).or_default().push(row);
    }

    let tx = conn.transaction()?;
    let mut new_snaps = 0usize;
    {
        let mut ins_snap = tx.prepare(
            "INSERT INTO snapshots (hash, message, branch, parent_hash, merge_parent, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )?;
        let mut ins_fm = tx.prepare(
            "INSERT INTO file_map (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, ?)",
        )?;

        for s in &bundle.snapshots {
            if existing.contains(&s.hash) {
                continue; // already have it — idempotent
            }
            // Verify the snapshot id matches its content before trusting it.
            let tree: Vec<(String, String, i64)> = fm_by_snap
                .get(s.hash.as_str())
                .map(|rows| rows.iter().map(|r| (r.path.clone(), r.hash.clone(), r.mode)).collect())
                .unwrap_or_default();
            let recomputed = crate::commands::snapshot_id(
                &tree,
                &s.parent_hash,
                &s.merge_parent,
                &s.message,
                &s.created_at,
            );
            if recomputed != s.hash {
                return Err(VeloError::CorruptRepo(format!(
                    "snapshot {} does not match its content (recomputed {})",
                    s.hash, recomputed
                )));
            }

            ins_snap.execute(params![
                s.hash,
                s.message,
                s.branch,
                s.parent_hash,
                s.merge_parent,
                s.created_at
            ])?;
            if let Some(rows) = fm_by_snap.get(s.hash.as_str()) {
                for r in rows {
                    ins_fm.execute(params![r.snapshot_hash, r.path, r.hash, r.mode])?;
                }
            }
            new_snaps += 1;
        }

        // Tags: don't clobber a local tag of the same name.
        let mut ins_tag =
            tx.prepare("INSERT OR IGNORE INTO tags (name, snapshot_hash) VALUES (?, ?)")?;
        for (name, hash) in &bundle.tags {
            ins_tag.execute(params![name, hash])?;
        }
    }
    tx.commit()?;

    Ok((new_snaps, new_objects))
}

// ─── Reachability ────────────────────────────────────────────────────────────────

/// All snapshots reachable from `tip` by walking `parent_hash` and
/// `merge_parent` to the root. Guarantees the resulting set is self-contained
/// (every included snapshot's parents are also included).
pub(crate) fn reachable_ancestry(conn: &rusqlite::Connection, tip: &str) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut stack = vec![tip.to_string()];
    while let Some(h) = stack.pop() {
        if h.is_empty() || seen.contains(&h) {
            continue;
        }
        seen.insert(h.clone());
        if let Ok((parent, merge)) = conn.query_row(
            "SELECT parent_hash, merge_parent FROM snapshots WHERE hash = ?",
            [&h],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        ) {
            stack.push(parent);
            stack.push(merge);
        }
    }
    seen
}

// ─── Encoding ────────────────────────────────────────────────────────────────────

pub(crate) fn encode(b: &Bundle) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    put_u32(&mut out, FORMAT_VERSION);

    put_u32(&mut out, b.snapshots.len() as u32);
    for s in &b.snapshots {
        put_str(&mut out, &s.hash);
        put_str(&mut out, &s.message);
        put_str(&mut out, &s.branch);
        put_str(&mut out, &s.parent_hash);
        put_str(&mut out, &s.merge_parent);
        put_str(&mut out, &s.created_at);
    }

    put_u32(&mut out, b.file_map.len() as u32);
    for r in &b.file_map {
        put_str(&mut out, &r.snapshot_hash);
        put_str(&mut out, &r.path);
        put_str(&mut out, &r.hash);
        out.extend_from_slice(&r.mode.to_le_bytes());
    }

    put_u32(&mut out, b.tags.len() as u32);
    for (name, hash) in &b.tags {
        put_str(&mut out, name);
        put_str(&mut out, hash);
    }

    put_u32(&mut out, b.objects.len() as u32);
    for (hash, data) in &b.objects {
        put_str(&mut out, hash);
        put_u32(&mut out, data.len() as u32);
        out.extend_from_slice(data);
    }
    out
}

pub(crate) fn decode(data: &[u8]) -> Result<Bundle> {
    let mut r = Reader { data, pos: 0 };
    let magic = r.bytes(8)?;
    if magic != MAGIC {
        return Err(VeloError::InvalidInput(
            "Not a Velo bundle (bad magic).".into(),
        ));
    }
    let version = r.u32()?;
    if version != FORMAT_VERSION {
        return Err(VeloError::InvalidInput(format!(
            "Unsupported bundle format version {} (this velo understands {}).",
            version, FORMAT_VERSION
        )));
    }

    let n = r.u32()?;
    let mut snapshots = Vec::with_capacity(n as usize);
    for _ in 0..n {
        snapshots.push(SnapshotRow {
            hash: r.string()?,
            message: r.string()?,
            branch: r.string()?,
            parent_hash: r.string()?,
            merge_parent: r.string()?,
            created_at: r.string()?,
        });
    }

    let n = r.u32()?;
    let mut file_map = Vec::with_capacity(n as usize);
    for _ in 0..n {
        file_map.push(FileMapRow {
            snapshot_hash: r.string()?,
            path: r.string()?,
            hash: r.string()?,
            mode: r.i64()?,
        });
    }

    let n = r.u32()?;
    let mut tags = Vec::with_capacity(n as usize);
    for _ in 0..n {
        tags.push((r.string()?, r.string()?));
    }

    let n = r.u32()?;
    let mut objects = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let hash = r.string()?;
        let len = r.u32()? as usize;
        let data = r.bytes(len)?.to_vec();
        objects.push((hash, data));
    }

    Ok(Bundle { snapshots, file_map, tags, objects })
}

fn put_u32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}
fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

/// Bounds-checked reader — every malformed bundle surfaces as an error, never a panic.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(len).ok_or_else(truncated)?;
        if end > self.data.len() {
            return Err(truncated());
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
    fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i64(&mut self) -> Result<i64> {
        let b = self.bytes(8)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(b);
        Ok(i64::from_le_bytes(arr))
    }
    fn string(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        let b = self.bytes(len)?;
        String::from_utf8(b.to_vec())
            .map_err(|_| VeloError::InvalidInput("Bundle contains invalid UTF-8.".into()))
    }
}

fn truncated() -> VeloError {
    VeloError::InvalidInput("Bundle is truncated or corrupt.".into())
}
