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
    let hash = blake3::hash(data).to_hex().to_string();
    let obj_path = objects_dir.join(&hash);
    if !obj_path.exists() {
        let compressed = zstd::encode_all(data, 1).map_err(VeloError::Io)?;
        write_atomic(&obj_path, &compressed).map_err(VeloError::Io)?;
    }
    Ok(hash)
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
    let meta = fs::metadata(file_path).map_err(VeloError::Io)?;
    let size = meta.len();

    let hash = if size >= MMAP_THRESHOLD {
        hash_mmap(file_path)?
    } else {
        hash_small(file_path)?
    };

    let obj_path = objects_dir.join(&hash);
    if !obj_path.exists() {
        // Re-read for compression (mmap again for large files)
        let data = normalise_crlf(if size >= MMAP_THRESHOLD {
            read_mmap(file_path)?
        } else {
            fs::read(file_path).map_err(VeloError::Io)?
        });
        let compressed = zstd::encode_all(&data[..], 1) // level 1: fast save
            .map_err(VeloError::Io)?;
        // Atomic write: a crash can't leave a half-written object under its
        // final content-addressed name (which would corrupt reads forever).
        write_atomic(&obj_path, &compressed).map_err(VeloError::Io)?;
    }
    Ok(hash)
}

/// Decompress and return the raw bytes of a stored object.
pub fn read_object(objects_dir: &Path, hash: &str) -> Result<Vec<u8>> {
    let obj_path = objects_dir.join(hash);
    let compressed = fs::read(&obj_path).map_err(|_| {
        VeloError::CorruptRepo(format!(
            "object '{}' is missing from storage. The repository may be corrupt.",
            hash
        ))
    })?;
    zstd::decode_all(&compressed[..]).map_err(|_| {
        VeloError::CorruptRepo(format!("object '{}' could not be decompressed.", hash))
    })
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
    let file = fs::File::open(path).map_err(VeloError::Io)?;
    // Safety: the file is read-only and we don't modify it during the map's
    // lifetime.  This is the standard pattern for read-only mmaps.
    let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(VeloError::Io)?;
    let data = normalise_crlf(mmap.to_vec());

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

fn read_mmap(path: &Path) -> Result<Vec<u8>> {
    let file = fs::File::open(path).map_err(VeloError::Io)?;
    let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(VeloError::Io)?;
    Ok(mmap.to_vec())
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
