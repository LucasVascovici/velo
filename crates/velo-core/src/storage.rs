use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{Result, VeloError};

/// Monotonic counter giving each temp file a process-unique suffix so parallel
/// writers (e.g. rayon threads storing objects) never collide on a temp name.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `contents` to `path` atomically: write to a sibling temp file first,
/// then rename it over the target. A crash mid-write can only ever leave a
/// stray `*.tmp.*` file (cleaned by the next `gc`), never a truncated target.
/// The rename is atomic within a filesystem on both Unix and Windows.
pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let tmp = temp_sibling(path);
    fs::write(&tmp, contents)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp); // don't leak the temp on failure
            Err(e)
        }
    }
}

fn temp_sibling(target: &Path) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    // `std::process::id()` panics on wasm32-unknown-unknown; one process owns
    // the store there, so a constant pid cannot collide.
    #[cfg(target_family = "wasm")]
    let pid = 0u32;
    #[cfg(not(target_family = "wasm"))]
    let pid = std::process::id();
    let mut name = target
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(format!(".tmp.{pid}.{n}"));
    target.with_file_name(name)
}

/// Threshold above which we use memory-mapped I/O instead of read-into-Vec.
/// Avoids the kernel→userspace copy that `fs::read` incurs on large files.
const MMAP_THRESHOLD: u64 = 256 * 1024; // 256 KB

/// Objects at least this large are stored as content-defined chunks. Not part
/// of the format: a reader handles either form whatever the writer chose.
const CHUNK_THRESHOLD: usize = 1024 * 1024;
const CHUNK_MIN: usize = 16 * 1024;
const CHUNK_AVG: usize = 64 * 1024;
const CHUNK_MAX: usize = 256 * 1024;
/// First bytes of a chunk manifest (a zstd frame never starts with these).
const MANIFEST_MAGIC: &[u8; 8] = b"VELOCHK1";

// ─── File modes ────────────────────────────────────────────────────────────────
// A file's mode is part of its identity in the tree (see `snapshot_id`).
pub const MODE_REGULAR: i64 = 0;
pub const MODE_EXEC: i64 = 1;
pub const MODE_SYMLINK: i64 = 2;

/// Determine a path's mode from the filesystem.
///
/// Symlinks are detected on every platform. The executable bit is only
/// observable on Unix; on other platforms regular files always report
/// `MODE_REGULAR` (callers make the bit "sticky" via the parent tree so it
/// survives edits on Windows).
pub fn capture_mode(path: &Path) -> i64 {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => MODE_SYMLINK,
        Ok(_meta) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if _meta.permissions().mode() & 0o111 != 0 {
                    return MODE_EXEC;
                }
            }
            MODE_REGULAR
        }
        Err(_) => MODE_REGULAR,
    }
}

/// Read a symlink's target as normalised (forward-slash) bytes — the content we
/// store for a symlink object.
pub fn read_symlink_target(path: &Path) -> Result<Vec<u8>> {
    let target = fs::read_link(path).map_err(VeloError::Io)?;
    Ok(crate::db::normalise(&target.to_string_lossy()).into_bytes())
}

/// Hash and store arbitrary bytes verbatim (no CRLF normalisation). Used for
/// symlink targets. Returns the object's BLAKE3 name.
pub fn store_raw(objects_dir: &Path, data: &[u8]) -> Result<String> {
    ObjectStore::at(objects_dir.to_path_buf()).put(data)
}

/// Write object `content` to `dest` honouring `mode`: create a symlink for
/// `MODE_SYMLINK` (falling back to a regular file where symlinks can't be
/// created, e.g. unprivileged Windows), set the executable bit for `MODE_EXEC`
/// on Unix, otherwise write a plain file.
pub fn apply_file(dest: &Path, mode: i64, content: &[u8]) -> Result<()> {
    if mode == MODE_SYMLINK {
        let target = String::from_utf8_lossy(content).to_string();
        // A symlink can't be created over an existing entry.
        let _ = fs::remove_file(dest);
        if create_symlink(&target, dest).is_ok() {
            return Ok(());
        }
        // Fallback: preserve the target text as a regular file so nothing is
        // lost when the platform won't let us make a real link.
        fs::write(dest, content).map_err(VeloError::Io)?;
        return Ok(());
    }

    fs::write(dest, content).map_err(VeloError::Io)?;

    #[cfg(unix)]
    if mode == MODE_EXEC {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(dest).map_err(VeloError::Io)?.permissions();
        perms.set_mode(perms.mode() | 0o755);
        fs::set_permissions(dest, perms).map_err(VeloError::Io)?;
    }
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &str, dest: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, dest)
}

#[cfg(windows)]
fn create_symlink(target: &str, dest: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, dest)
}

#[cfg(not(any(unix, windows)))]
fn create_symlink(_target: &str, _dest: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlinks unsupported on this platform",
    ))
}

/// Hash `file_path` with BLAKE3 and compress it into `objects_dir`.
/// For files ≥ 256 KB the file is memory-mapped to avoid double-buffering.
/// For very large files (≥ 1 MB) blake3's built-in rayon parallelism is used.
pub fn hash_and_compress(file_path: &Path, objects_dir: &Path) -> Result<String> {
    ObjectStore::at(objects_dir.to_path_buf()).put_file(file_path)
}

/// Decompress and return the raw bytes of a stored object.
pub fn read_object(objects_dir: &Path, hash: &str) -> Result<Vec<u8>> {
    ObjectStore::at(objects_dir.to_path_buf()).get(hash)
}

/// The outcome of checking one stored object against its name.
pub(crate) enum Verified {
    Ok,
    Missing,
    Undecodable,
    Mismatch { actual: String },
}

/// Where a repository keeps its objects. Chosen when it is created and
/// recorded in the `settings` table (docs/FORMAT.md 7.2); a repository with no
/// such row predates the choice and keeps them in files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ObjectLocation {
    /// One file per object under `.velo/objects`, large ones as chunks.
    #[default]
    Files,
    /// zstd blobs in the `objects` table of `velo.db`. The only location a
    /// browser build can use, where the SQLite VFS is the one persistent store.
    Database,
}

impl ObjectLocation {
    /// The value stored in the `settings` table.
    pub(crate) fn as_setting(self) -> &'static str {
        match self {
            ObjectLocation::Files => "files",
            ObjectLocation::Database => "database",
        }
    }

    /// Read the recorded location. A missing table or row means `Files`, which
    /// is every repository created before the setting existed.
    pub(crate) fn read(conn: &rusqlite::Connection) -> Result<Self> {
        use rusqlite::OptionalExtension;
        let has_table: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'settings')",
            [],
            |r| r.get(0),
        )?;
        if !has_table {
            return Ok(ObjectLocation::Files);
        }
        let value: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'objects'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match value.as_deref() {
            None | Some("files") => Ok(ObjectLocation::Files),
            Some("database") => Ok(ObjectLocation::Database),
            Some(other) => Err(VeloError::corrupt(format!(
                "this repository keeps its objects in '{}', which this build does not know",
                other
            ))),
        }
    }
}

/// The objects of a repository, wherever they live: the one seam every object
/// access goes through. An enum rather than a trait because the database
/// variant needs the repository's connection and there are exactly two
/// backends; a trait would be wider than that need.
pub(crate) enum ObjectStore<'r> {
    Files(FileObjects),
    Database(&'r rusqlite::Connection),
}

impl ObjectStore<'static> {
    /// A file-backed store over `objects_dir`.
    pub(crate) fn at(objects_dir: PathBuf) -> Self {
        ObjectStore::Files(FileObjects::at(objects_dir))
    }
}

/// Compress `content` as the database backend stores it: one zstd frame.
fn frame_of(content: &[u8]) -> Result<Vec<u8>> {
    zstd::encode_all(content, 1).map_err(VeloError::Io)
}

/// Read a working-tree file as it would be stored (CRLF-normalised) and
/// compute its name and compressed frame. Touches no store, so it can run on
/// many threads while the database connection stays on one.
pub(crate) fn prepare_file(file_path: &Path) -> Result<(String, Vec<u8>)> {
    let data = normalise_crlf(fs::read(file_path).map_err(VeloError::Io)?);
    prepare_bytes(&data)
}

/// The name and compressed frame of already-normalised bytes.
pub(crate) fn prepare_bytes(data: &[u8]) -> Result<(String, Vec<u8>)> {
    Ok((blake3::hash(data).to_hex().to_string(), frame_of(data)?))
}

impl ObjectStore<'_> {
    /// Hash and store working-tree files in parallel, returning each path with
    /// its object name and mode. A symlink stores its target, not the content
    /// it points at. `check` runs before each file so a caller can cancel.
    ///
    /// The file backend stores from the worker threads. A database connection
    /// cannot be shared between threads, so there the expensive part (read,
    /// hash, compress) runs in parallel and the cheap inserts follow on the
    /// calling thread.
    pub(crate) fn put_paths(
        &self,
        root: &Path,
        rels: Vec<String>,
        progress: &crate::progress::PhaseGuard<'_>,
        check: impl Fn() -> Result<()> + Sync + Send,
    ) -> Result<Vec<(String, String, i64)>> {
        use rayon::prelude::*;
        let locate = |rel: &str| root.join(crate::db::db_to_path(rel));
        match self {
            ObjectStore::Files(f) => rels
                .into_par_iter()
                .inspect(|_| progress.tick())
                .map(|rel| {
                    // Hashing writes objects, which is harmless to abandon: an
                    // object nothing references is what `gc` collects.
                    check()?;
                    let full = locate(&rel);
                    let mode = capture_mode(&full);
                    let hash = if mode == MODE_SYMLINK {
                        f.put(&read_symlink_target(&full)?)?
                    } else {
                        f.put_file(&full)?
                    };
                    Ok((rel, hash, mode))
                })
                .collect(),
            ObjectStore::Database(c) => {
                let prepared: Vec<(String, String, Vec<u8>, i64)> = rels
                    .into_par_iter()
                    .inspect(|_| progress.tick())
                    .map(|rel| {
                        check()?;
                        let full = locate(&rel);
                        let mode = capture_mode(&full);
                        let (hash, frame) = if mode == MODE_SYMLINK {
                            prepare_bytes(&read_symlink_target(&full)?)?
                        } else {
                            prepare_file(&full)?
                        };
                        Ok((rel, hash, frame, mode))
                    })
                    .collect::<Result<_>>()?;
                prepared
                    .into_iter()
                    .map(|(rel, hash, frame, mode)| {
                        db_insert(c, &hash, &frame)?;
                        Ok((rel, hash, mode))
                    })
                    .collect()
            }
        }
    }

    /// Call `work(item, content)` for every item in parallel, where `content`
    /// is the object `hash_of(item)` names. Results come back in item order.
    ///
    /// The database backend cannot read from worker threads, so it fetches a
    /// bounded batch on the calling thread and hands that batch to the pool.
    pub(crate) fn par_with_content<T: Sync, R: Send>(
        &self,
        items: &[T],
        hash_of: impl Fn(&T) -> &str + Sync + Send,
        work: impl Fn(&T, Result<Vec<u8>>) -> Option<R> + Sync + Send,
    ) -> Vec<R> {
        use rayon::prelude::*;
        match self {
            ObjectStore::Files(f) => items
                .par_iter()
                .filter_map(|it| work(it, f.get(hash_of(it))))
                .collect(),
            ObjectStore::Database(_) => {
                const BATCH: usize = 128;
                let mut out = Vec::new();
                for batch in items.chunks(BATCH) {
                    let fetched: Vec<(&T, Result<Vec<u8>>)> =
                        batch.iter().map(|it| (it, self.get(hash_of(it)))).collect();
                    out.extend(
                        fetched
                            .into_par_iter()
                            .filter_map(|(it, content)| work(it, content))
                            .collect::<Vec<R>>(),
                    );
                }
                out
            }
        }
    }

    pub(crate) fn put(&self, normalised: &[u8]) -> Result<String> {
        match self {
            ObjectStore::Files(f) => f.put(normalised),
            ObjectStore::Database(c) => {
                let hash = blake3::hash(normalised).to_hex().to_string();
                if !db_contains(c, &hash)? {
                    db_insert(c, &hash, &frame_of(normalised)?)?;
                }
                Ok(hash)
            }
        }
    }

    pub(crate) fn put_file(&self, file_path: &Path) -> Result<String> {
        match self {
            ObjectStore::Files(f) => f.put_file(file_path),
            ObjectStore::Database(c) => {
                let (hash, frame) = prepare_file(file_path)?;
                db_insert(c, &hash, &frame)?;
                Ok(hash)
            }
        }
    }

    pub(crate) fn get(&self, hash: &str) -> Result<Vec<u8>> {
        match self {
            ObjectStore::Files(f) => f.get(hash),
            ObjectStore::Database(c) => {
                let frame = db_frame(c, hash)?.ok_or_else(|| {
                    VeloError::corrupt(format!(
                        "object '{}' is missing from storage. The repository may be corrupt.",
                        hash
                    ))
                })?;
                let data = zstd::decode_all(&frame[..]).map_err(|_| {
                    VeloError::corrupt(format!("object '{}' could not be decompressed.", hash))
                })?;
                if blake3::hash(&data).to_hex().as_str() != hash {
                    return Err(VeloError::corrupt(format!(
                        "object '{}' does not match its content",
                        hash
                    )));
                }
                Ok(data)
            }
        }
    }

    pub(crate) fn contains(&self, hash: &str) -> bool {
        match self {
            ObjectStore::Files(f) => f.contains(hash),
            ObjectStore::Database(c) => db_contains(c, hash).unwrap_or(false),
        }
    }

    pub(crate) fn compressed(&self, hash: &str) -> Result<Vec<u8>> {
        match self {
            ObjectStore::Files(f) => f.compressed(hash),
            ObjectStore::Database(c) => db_frame(c, hash)?.ok_or_else(|| {
                VeloError::corrupt(format!("object {} is missing — run 'velo fsck'", hash))
            }),
        }
    }

    pub(crate) fn import_compressed(&self, hash: &str, frame: &[u8]) -> Result<bool> {
        match self {
            ObjectStore::Files(f) => f.import_compressed(hash, frame),
            ObjectStore::Database(c) => {
                let decompressed = zstd::decode_all(frame).map_err(|_| {
                    VeloError::corrupt(format!("object {} could not be decompressed", hash))
                })?;
                let actual = blake3::hash(&decompressed).to_hex().to_string();
                if actual != hash {
                    return Err(VeloError::corrupt(format!(
                        "object {} is corrupt (content hashes to {})",
                        hash, actual
                    )));
                }
                db_insert(c, hash, frame)
            }
        }
    }

    pub(crate) fn list(&self) -> Result<Vec<(String, u64)>> {
        match self {
            ObjectStore::Files(f) => f.list(),
            ObjectStore::Database(c) => {
                let mut stmt = c.prepare("SELECT hash, length(data) FROM objects")?;
                let rows =
                    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
                let mut out = Vec::new();
                for row in rows {
                    let (h, n) = row?;
                    out.push((h, n.max(0) as u64));
                }
                Ok(out)
            }
        }
    }

    pub(crate) fn remove(&self, hash: &str) -> Result<()> {
        match self {
            ObjectStore::Files(f) => f.remove(hash),
            ObjectStore::Database(c) => {
                c.execute("DELETE FROM objects WHERE hash = ?", [hash])?;
                Ok(())
            }
        }
    }

    pub(crate) fn verify(&self, hash: &str) -> Verified {
        match self {
            ObjectStore::Files(f) => f.verify(hash),
            ObjectStore::Database(c) => {
                let frame = match db_frame(c, hash) {
                    Ok(Some(frame)) => frame,
                    Ok(None) => return Verified::Missing,
                    Err(_) => return Verified::Undecodable,
                };
                let Ok(data) = zstd::decode_all(&frame[..]) else {
                    return Verified::Undecodable;
                };
                let actual = blake3::hash(&data).to_hex().to_string();
                if actual == hash {
                    Verified::Ok
                } else {
                    Verified::Mismatch { actual }
                }
            }
        }
    }

    // Chunking is a disk-layout optimisation: it lets two large files share
    // the pieces they have in common as separate files. In the database the
    // unit is the SQLite page, which already stores and reuses space its own
    // way, so this backend keeps one whole frame per object and has no chunks.

    pub(crate) fn chunks_of(&self, hash: &str) -> Result<Option<Vec<String>>> {
        match self {
            ObjectStore::Files(f) => f.chunks_of(hash),
            ObjectStore::Database(_) => Ok(None),
        }
    }

    pub(crate) fn list_chunks(&self) -> Result<Vec<(String, u64)>> {
        match self {
            ObjectStore::Files(f) => f.list_chunks(),
            ObjectStore::Database(_) => Ok(Vec::new()),
        }
    }

    pub(crate) fn remove_chunk(&self, chunk: &str) -> Result<()> {
        match self {
            ObjectStore::Files(f) => f.remove_chunk(chunk),
            ObjectStore::Database(_) => Ok(()),
        }
    }

    pub(crate) fn verify_chunk(&self, chunk: &str) -> Verified {
        match self {
            ObjectStore::Files(f) => f.verify_chunk(chunk),
            ObjectStore::Database(_) => Verified::Missing,
        }
    }
}

fn db_contains(conn: &rusqlite::Connection, hash: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM objects WHERE hash = ?)",
        [hash],
        |r| r.get(0),
    )?)
}

fn db_frame(conn: &rusqlite::Connection, hash: &str) -> Result<Option<Vec<u8>>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row("SELECT data FROM objects WHERE hash = ?", [hash], |r| {
            r.get(0)
        })
        .optional()?)
}

/// `INSERT OR IGNORE`; true when the object was new.
fn db_insert(conn: &rusqlite::Connection, hash: &str, frame: &[u8]) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO objects (hash, data) VALUES (?, ?)",
        rusqlite::params![hash, frame],
    )?;
    Ok(n > 0)
}

/// The file layout: one file per object, large ones as chunks.
#[derive(Clone, Debug)]
pub(crate) struct FileObjects {
    dir: PathBuf,
}

impl FileObjects {
    pub(crate) fn at(objects_dir: PathBuf) -> Self {
        Self { dir: objects_dir }
    }

    fn path(&self, hash: &str) -> PathBuf {
        self.dir.join(hash)
    }

    /// Where chunks live: `.velo/chunks`, a sibling of the objects directory.
    fn chunks_dir(&self) -> PathBuf {
        self.dir
            .parent()
            .map(|p| p.join("chunks"))
            .unwrap_or_else(|| self.dir.join("chunks"))
    }

    fn chunk_path(&self, hex: &str) -> PathBuf {
        self.chunks_dir().join(hex)
    }

    /// Store `normalised` under `hash`: one zstd frame, or a manifest of
    /// deduplicated chunks for large content. Chunks are written first so a
    /// manifest never names a chunk that is not on disk.
    fn store(&self, hash: &str, normalised: &[u8]) -> Result<()> {
        let obj_path = self.path(hash);
        if normalised.len() < CHUNK_THRESHOLD {
            let compressed = zstd::encode_all(normalised, 1).map_err(VeloError::Io)?;
            return write_atomic(&obj_path, &compressed).map_err(VeloError::Io);
        }
        let chunks_dir = self.chunks_dir();
        fs::create_dir_all(&chunks_dir).map_err(VeloError::Io)?;
        let mut manifest = Vec::new();
        manifest.extend_from_slice(MANIFEST_MAGIC);
        manifest.extend_from_slice(&1u32.to_le_bytes());
        manifest.extend_from_slice(&(normalised.len() as u64).to_le_bytes());
        let count_at = manifest.len();
        manifest.extend_from_slice(&0u32.to_le_bytes());
        let mut count: u32 = 0;
        for c in fastcdc::v2020::FastCDC::new(
            normalised,
            CHUNK_MIN as u32,
            CHUNK_AVG as u32,
            CHUNK_MAX as u32,
        ) {
            let data = &normalised[c.offset..c.offset + c.length];
            let digest = blake3::hash(data);
            let path = chunks_dir.join(digest.to_hex().as_str());
            if !path.exists() {
                let frame = zstd::encode_all(data, 1).map_err(VeloError::Io)?;
                write_atomic(&path, &frame).map_err(VeloError::Io)?;
            }
            manifest.extend_from_slice(digest.as_bytes());
            manifest.extend_from_slice(&(c.length as u32).to_le_bytes());
            count += 1;
        }
        manifest[count_at..count_at + 4].copy_from_slice(&count.to_le_bytes());
        write_atomic(&obj_path, &manifest).map_err(VeloError::Io)
    }

    /// Hash and store already-normalised bytes verbatim. Returns the name.
    pub(crate) fn put(&self, normalised: &[u8]) -> Result<String> {
        let hash = blake3::hash(normalised).to_hex().to_string();
        if !self.path(&hash).exists() {
            self.store(&hash, normalised)?;
        }
        Ok(hash)
    }

    /// Hash a working-tree file (CRLF-normalised) and store it.
    pub(crate) fn put_file(&self, file_path: &Path) -> Result<String> {
        let meta = fs::metadata(file_path).map_err(VeloError::Io)?;
        let size = meta.len();

        let hash = if size >= MMAP_THRESHOLD {
            hash_mmap(file_path)?
        } else {
            hash_small(file_path)?
        };

        if !self.path(&hash).exists() {
            // Re-read for compression (mmap again for large files)
            let data = normalise_crlf(if size >= MMAP_THRESHOLD {
                read_mmap(file_path)?
            } else {
                fs::read(file_path).map_err(VeloError::Io)?
            });
            // Atomic writes: a crash can't leave a half-written object under
            // its final content-addressed name (which would corrupt reads forever).
            self.store(&hash, &data)?;
        }
        Ok(hash)
    }

    /// Return the full content of an object, reassembling chunked ones.
    ///
    /// A manifest naming an absent chunk yields `MissingObject` carrying the
    /// chunk's hex name; every other malformation is `Corrupt`.
    pub(crate) fn get(&self, hash: &str) -> Result<Vec<u8>> {
        let raw = fs::read(self.path(hash)).map_err(|_| {
            VeloError::corrupt(format!(
                "object '{}' is missing from storage. The repository may be corrupt.",
                hash
            ))
        })?;
        if !raw.starts_with(MANIFEST_MAGIC) {
            return zstd::decode_all(&raw[..]).map_err(|_| {
                VeloError::corrupt(format!("object '{}' could not be decompressed.", hash))
            });
        }
        let (out, total) = self.reassemble(hash, &raw)?;
        if out.len() as u64 != total || blake3::hash(&out).to_hex().as_str() != hash {
            return Err(VeloError::corrupt(format!(
                "object '{}' does not match its reassembled chunks",
                hash
            )));
        }
        Ok(out)
    }

    /// Parse a manifest and concatenate its chunks, checking each chunk but
    /// not the final content hash. Returns the content and the total length
    /// the manifest declares; callers compare both so `get` can say `Corrupt`
    /// while `verify` can report a `Mismatch`.
    fn reassemble(&self, hash: &str, raw: &[u8]) -> Result<(Vec<u8>, u64)> {
        let (total, entries) = parse_manifest(hash, raw)?;
        // Capacity is capped: `total` is untrusted until the hash checks out.
        let mut out = Vec::with_capacity((total as usize).min(1 << 28));
        for (hex, len) in entries {
            let frame = fs::read(self.chunk_path(&hex))
                .map_err(|_| VeloError::MissingObject { hash: hex.clone() })?;
            let data = zstd::decode_all(&frame[..]).map_err(|_| {
                VeloError::corrupt(format!("chunk '{}' could not be decompressed", hex))
            })?;
            if data.len() != len || blake3::hash(&data).to_hex().as_str() != hex {
                return Err(VeloError::corrupt(format!("chunk '{}' is corrupt", hex)));
            }
            out.extend_from_slice(&data);
        }
        Ok((out, total))
    }

    /// The chunk names a manifest object lists, in order (repeats included).
    /// `None` when the object is stored as a plain frame. Only the first bytes
    /// of a plain object are read, so scanning a whole store stays cheap.
    pub(crate) fn chunks_of(&self, hash: &str) -> Result<Option<Vec<String>>> {
        use std::io::Read;
        let mut file = fs::File::open(self.path(hash))?;
        let mut magic = [0u8; 8];
        let mut got = 0;
        while got < magic.len() {
            let n = file.read(&mut magic[got..])?;
            if n == 0 {
                break;
            }
            got += n;
        }
        if got < magic.len() || &magic != MANIFEST_MAGIC {
            return Ok(None);
        }
        let raw = fs::read(self.path(hash))?;
        let (_, entries) = parse_manifest(hash, &raw)?;
        Ok(Some(entries.into_iter().map(|(hex, _)| hex).collect()))
    }

    /// Every chunk file with its on-disk size, for gc. Empty when the
    /// repository has never stored a chunk.
    pub(crate) fn list_chunks(&self) -> Result<Vec<(String, u64)>> {
        let dir = self.chunks_dir();
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push((name, size));
        }
        Ok(out)
    }

    pub(crate) fn remove_chunk(&self, chunk: &str) -> Result<()> {
        fs::remove_file(self.chunk_path(chunk))?;
        Ok(())
    }

    /// Integrity check of one chunk: present, decodable, hashing to its name.
    pub(crate) fn verify_chunk(&self, chunk: &str) -> Verified {
        let Ok(frame) = fs::read(self.chunk_path(chunk)) else {
            return Verified::Missing;
        };
        let Ok(data) = zstd::decode_all(&frame[..]) else {
            return Verified::Undecodable;
        };
        let actual = blake3::hash(&data).to_hex().to_string();
        if actual == chunk {
            Verified::Ok
        } else {
            Verified::Mismatch { actual }
        }
    }

    pub(crate) fn contains(&self, hash: &str) -> bool {
        self.path(hash).exists()
    }

    /// One zstd frame of the full content, as packs and bundles carry it.
    pub(crate) fn compressed(&self, hash: &str) -> Result<Vec<u8>> {
        let raw = fs::read(self.path(hash)).map_err(|_| {
            VeloError::corrupt(format!("object {} is missing — run 'velo fsck'", hash))
        })?;
        if raw.starts_with(MANIFEST_MAGIC) {
            // The wire format is always one frame of the full content.
            let full = self.get(hash)?;
            return zstd::encode_all(&full[..], 1).map_err(VeloError::Io);
        }
        Ok(raw)
    }

    /// Verify a received frame decompresses and hashes to `hash`, then store
    /// it. `Ok(true)` if the object was new.
    pub(crate) fn import_compressed(&self, hash: &str, frame: &[u8]) -> Result<bool> {
        let decompressed = zstd::decode_all(frame).map_err(|_| {
            VeloError::corrupt(format!("object {} could not be decompressed", hash))
        })?;
        let actual = blake3::hash(&decompressed).to_hex().to_string();
        if actual != hash {
            return Err(VeloError::corrupt(format!(
                "object {} is corrupt (content hashes to {})",
                hash, actual
            )));
        }
        if self.path(hash).exists() {
            return Ok(false);
        }
        if decompressed.len() >= CHUNK_THRESHOLD {
            self.store(hash, &decompressed)?;
        } else {
            write_atomic(&self.path(hash), frame)?;
        }
        Ok(true)
    }

    /// Every stored object name with its on-disk size, for gc.
    pub(crate) fn list(&self) -> Result<Vec<(String, u64)>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push((name, size));
        }
        Ok(out)
    }

    pub(crate) fn remove(&self, hash: &str) -> Result<()> {
        fs::remove_file(self.path(hash))?;
        Ok(())
    }

    /// Integrity check for fsck: present, decodable, and hashing to its name.
    pub(crate) fn verify(&self, hash: &str) -> Verified {
        if !self.contains(hash) {
            return Verified::Missing;
        }
        let Ok(raw) = fs::read(self.path(hash)) else {
            return Verified::Undecodable;
        };
        let content = if raw.starts_with(MANIFEST_MAGIC) {
            match self.reassemble(hash, &raw) {
                Ok((bytes, total)) if bytes.len() as u64 == total => Ok(bytes),
                Ok(_) => Err(VeloError::corrupt("manifest length mismatch")),
                Err(e) => Err(e),
            }
        } else {
            self.get(hash)
        };
        match content {
            Err(VeloError::MissingObject { .. }) => Verified::Missing,
            Ok(bytes) => {
                let actual = blake3::hash(&bytes).to_hex().to_string();
                if actual == hash {
                    Verified::Ok
                } else {
                    Verified::Mismatch { actual }
                }
            }
            Err(_) => Verified::Undecodable,
        }
    }
}

/// Parse a manifest into its declared total length and `(chunk hex, length)`
/// entries, rejecting anything malformed as `Corrupt`.
fn parse_manifest(hash: &str, raw: &[u8]) -> Result<(u64, Vec<(String, usize)>)> {
    let bad = |why: &str| VeloError::corrupt(format!("manifest of object '{}' {}", hash, why));
    let rest = raw
        .get(MANIFEST_MAGIC.len()..)
        .ok_or_else(|| bad("is truncated"))?;
    if rest.len() < 16 {
        return Err(bad("is truncated"));
    }
    let version = u32::from_le_bytes(rest[0..4].try_into().unwrap());
    if version != 1 {
        return Err(bad("has an unknown version"));
    }
    let total = u64::from_le_bytes(rest[4..12].try_into().unwrap());
    let count = u32::from_le_bytes(rest[12..16].try_into().unwrap()) as usize;
    let entries = &rest[16..];
    if entries.len() != count.checked_mul(36).ok_or_else(|| bad("is malformed"))? {
        return Err(bad("has the wrong length for its chunk count"));
    }
    Ok((
        total,
        entries
            .as_chunks::<36>()
            .0
            .iter()
            .map(|e| {
                let hex = blake3::Hash::from_bytes(e[..32].try_into().unwrap())
                    .to_hex()
                    .to_string();
                let len = u32::from_le_bytes(e[32..36].try_into().unwrap()) as usize;
                (hex, len)
            })
            .collect(),
    ))
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// Normalise CRLF → LF in a byte buffer.
/// Text files on Windows often use \r\n. We always store and hash LF-normalised
/// content so that files saved on Windows compare correctly to files saved on Unix.
/// Binary files (containing a NUL byte) are returned unchanged.
#[inline]
pub fn normalise_crlf(data: Vec<u8>) -> Vec<u8> {
    if data.contains(&0u8) {
        // Binary file — do not touch
        return data;
    }
    if !data.contains(&b'\r') {
        return data;
    }
    // Drop every carriage return, keeping all other bytes. This turns "\r\n"
    // into "\n" and removes bare "\r". (The previous hand-rolled index walk
    // advanced past the "\n" after a "\r", silently deleting line breaks — so
    // CRLF files were stored collapsed onto a single line.)
    let mut out = Vec::with_capacity(data.len());
    for &byte in &data {
        if byte != b'\r' {
            out.push(byte);
        }
    }
    out
}

/// Hash a small file by reading it fully into a Vec then hashing.
fn hash_small(path: &Path) -> Result<String> {
    let data = normalise_crlf(fs::read(path).map_err(VeloError::Io)?);
    Ok(blake3::hash(&data).to_hex().to_string())
}

/// Hash a large file via memory-mapped I/O.
/// For files ≥ 1 MB uses blake3's rayon parallel hasher.
///
/// The content is CRLF-normalised *before* hashing so that the hash matches the
/// normalised bytes that `hash_and_compress` actually stores, and so it agrees
/// with `hash_small`/`fast_hash`. Without this, a large (≥256 KB) text file with
/// `\r\n` line endings would hash differently here than everywhere else — making
/// it appear permanently "modified" on Windows and breaking content-addressing.
fn hash_mmap(path: &Path) -> Result<String> {
    let data = normalise_crlf(read_mmap(path)?);

    const PARALLEL_THRESHOLD: usize = 1024 * 1024; // 1 MB
    let hash = if data.len() >= PARALLEL_THRESHOLD {
        // blake3's update_rayon splits the buffer across the global rayon pool.
        // Note: calling this from inside a rayon par_iter is safe — tasks are
        // queued on the same pool, not deadlocked.
        let mut hasher = blake3::Hasher::new();
        hasher.update_rayon(&data);
        hasher.finalize().to_hex().to_string()
    } else {
        blake3::hash(&data).to_hex().to_string()
    };
    Ok(hash)
}

#[cfg(not(target_family = "wasm"))]
fn read_mmap(path: &Path) -> Result<Vec<u8>> {
    let file = fs::File::open(path).map_err(VeloError::Io)?;
    // Safety: the file is read-only and we don't modify it during the map's
    // lifetime.  This is the standard pattern for read-only mmaps.
    let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(VeloError::Io)?;
    Ok(mmap.to_vec())
}

/// There is no mmap on wasm; a plain read gives the same bytes.
#[cfg(target_family = "wasm")]
fn read_mmap(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(VeloError::Io)
}

/// Mode-aware content hash for dirty checks: a symlink hashes to its target,
/// everything else to its (CRLF-normalised) file content.
pub fn hash_for(path: &Path, mode: i64) -> String {
    if mode == MODE_SYMLINK {
        match read_symlink_target(path) {
            Ok(target) => blake3::hash(&target).to_hex().to_string(),
            Err(_) => String::new(),
        }
    } else {
        fast_hash(path)
    }
}

/// Fast content hash used during dirty-checks.
/// Uses the same mmap strategy as `hash_and_compress` but skips compression.
pub fn fast_hash(path: &Path) -> String {
    // Always normalise CRLF for consistent hashing across platforms.
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size >= MMAP_THRESHOLD {
        let data = read_mmap(path).unwrap_or_default();
        let data = normalise_crlf(data);
        blake3::hash(&data).to_hex().to_string()
    } else {
        hash_small(path).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, FileObjects) {
        let dir = tempfile::tempdir().unwrap();
        let store = FileObjects::at(dir.path().to_path_buf());
        (dir, store)
    }

    #[test]
    fn put_get_and_compressed_round_trip() {
        let (_d, s) = store();
        let h = s.put(b"hello objects").unwrap();
        assert!(s.contains(&h));
        assert_eq!(s.get(&h).unwrap(), b"hello objects");
        let frame = s.compressed(&h).unwrap();
        assert_eq!(zstd::decode_all(&frame[..]).unwrap(), b"hello objects");
        assert!(matches!(s.verify(&h), Verified::Ok));
    }

    #[test]
    fn import_rejects_wrong_name_and_verify_flags_tamper() {
        let (_d, s) = store();
        let frame = zstd::encode_all(&b"content"[..], 1).unwrap();
        let wrong = blake3::hash(b"other").to_hex().to_string();
        let err = s.import_compressed(&wrong, &frame).unwrap_err();
        assert!(matches!(err, VeloError::Corrupt { .. }));

        let h = s.put(b"original").unwrap();
        let tampered = zstd::encode_all(&b"tampered"[..], 1).unwrap();
        fs::write(s.path(&h), tampered).unwrap();
        assert!(matches!(s.verify(&h), Verified::Mismatch { .. }));
    }
}
