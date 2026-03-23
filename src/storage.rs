use std::fs;
use std::path::Path;

use crate::error::{Result, VeloError};

/// Threshold above which we use memory-mapped I/O instead of read-into-Vec.
/// Avoids the kernel→userspace copy that `fs::read` incurs on large files.
const MMAP_THRESHOLD: u64 = 256 * 1024; // 256 KB

/// Hash `file_path` with BLAKE3 and compress it into `objects_dir`.
/// For files ≥ 256 KB the file is memory-mapped to avoid double-buffering.
/// For very large files (≥ 1 MB) blake3's built-in rayon parallelism is used.
pub fn hash_and_compress(file_path: &Path, objects_dir: &Path) -> Result<String> {
    let meta = fs::metadata(file_path).map_err(VeloError::Io)?;
    let size = meta.len();

    let hash = if size >= MMAP_THRESHOLD {
        hash_mmap(file_path, size)?
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
        fs::write(&obj_path, compressed).map_err(VeloError::Io)?;
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
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\r' {
            if i + 1 < data.len() && data[i + 1] == b'\n' {
                i += 1; // skip \r, keep \n
            }
            // bare \r — skip it too
        } else {
            out.push(data[i]);
        }
        i += 1;
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
fn hash_mmap(path: &Path, size: u64) -> Result<String> {
    let file = fs::File::open(path).map_err(VeloError::Io)?;
    // Safety: the file is read-only and we don't modify it during the map's
    // lifetime.  This is the standard pattern for read-only mmaps.
    let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(VeloError::Io)?;

    const PARALLEL_THRESHOLD: u64 = 1024 * 1024; // 1 MB
    let hash = if size >= PARALLEL_THRESHOLD {
        // blake3's update_rayon splits the buffer across the global rayon pool.
        // Note: calling this from inside a rayon par_iter is safe — tasks are
        // queued on the same pool, not deadlocked.
        let mut hasher = blake3::Hasher::new();
        hasher.update_rayon(&mmap);
        hasher.finalize().to_hex().to_string()
    } else {
        blake3::hash(&mmap).to_hex().to_string()
    };
    Ok(hash)
}

fn read_mmap(path: &Path) -> Result<Vec<u8>> {
    let file = fs::File::open(path).map_err(VeloError::Io)?;
    let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(VeloError::Io)?;
    Ok(mmap.to_vec())
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
