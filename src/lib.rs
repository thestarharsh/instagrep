pub mod index;
pub mod printer;
pub mod types;
pub mod utils;
pub mod walker;

use std::path::{Path, PathBuf};

/// Resolve the index directory for a given project root.
///
/// Stores indexes centrally at `~/.instagrep/indexes/<hash>-<name>/`
/// so no files are created inside user projects.
///
///   - macOS/Linux: ~/.instagrep/indexes/<hash>-<name>/
///   - Windows:     %USERPROFILE%\.instagrep\indexes\<hash>-<name>\
///
/// Override with INSTAGREP_CACHE_DIR env var.
pub fn index_dir_for(root: &Path) -> PathBuf {
    if let Ok(custom) = std::env::var("INSTAGREP_CACHE_DIR") {
        // Still need per-project subdirectory — otherwise all projects share one index
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let hash = utils::ngram_hash(canonical.to_string_lossy().as_bytes());
        let dir_name = canonical
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "root".to_string());
        return PathBuf::from(custom).join(format!("{:016x}-{}", hash, dir_name));
    }

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());

    let base = PathBuf::from(home).join(".instagrep").join("indexes");

    // Hash the canonical path for a stable unique directory name
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let path_str = canonical.to_string_lossy();

    // FNV-1a 64-bit — stable across all versions/platforms
    let hash = utils::ngram_hash(path_str.as_bytes());

    let dir_name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "root".to_string());

    base.join(format!("{:016x}-{}", hash, dir_name))
}
