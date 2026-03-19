use std::fs;
use std::path::Path;
use crate::error::{Result, VeloError};

/// Hash and compress a file, writing the object to `objects_dir` if it does
/// not already exist.  Returns the full BLAKE3 hex digest.
pub fn hash_and_compress(file_path: &Path, objects_dir: &Path) -> Result<String> {
    let data = fs::read(file_path)
        .map_err(|e| VeloError::Io(e))?;
    let hash = blake3::hash(&data).to_hex().to_string();
    let obj_path = objects_dir.join(&hash);

    if !obj_path.exists() {
        let compressed = zstd::encode_all(&data[..], 3)
            .map_err(|e| VeloError::Io(e))?;
        fs::write(&obj_path, compressed)
            .map_err(|e| VeloError::Io(e))?;
    }
    Ok(hash)
}

/// Decompress and return the raw bytes of an object.
/// Returns a descriptive error if the object is missing or corrupt.
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