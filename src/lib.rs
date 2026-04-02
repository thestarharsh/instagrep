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
        return PathBuf::from(custom);
    }

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());

    let base = PathBuf::from(home).join(".instagrep").join("indexes");

    // Hash the canonical path for a stable unique directory name
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let path_str = canonical.to_string_lossy();

    use std::hash::{BuildHasher, Hash, Hasher};
    let build = ahash::RandomState::with_seeds(
        0x517cc1b727220a95,
        0x6c62272e07bb0142,
        0x8db2d5cf3eef2f74,
        0x62d1ce1e6b3b0a5a,
    );
    let mut hasher = build.build_hasher();
    path_str.hash(&mut hasher);
    let hash = hasher.finish();

    let dir_name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "root".to_string());

    base.join(format!("{:016x}-{}", hash, dir_name))
}
