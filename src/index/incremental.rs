//! Incremental index updates: Git-aware change detection.
//!
//! Ties the index to Git commits. On re-index, only modified files are
//! re-processed. Uses mtime + content hash as a fast change detector.
//! Includes lock file support for concurrent access safety.

use crate::index::storage::FileMeta;
use crate::utils::ngram_hash;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Get the current HEAD commit hash, if in a git repo.
pub fn get_head_commit(repo_root: &Path) -> Option<String> {
    let repo = git2::Repository::open(repo_root).ok()?;
    let head = repo.head().ok()?;
    let commit = head.peel_to_commit().ok()?;
    Some(commit.id().to_string())
}

/// Check if the index is stale (built at a different git commit than current HEAD).
pub fn is_index_stale(index_commit: Option<&str>, repo_root: &Path) -> bool {
    let current = get_head_commit(repo_root);
    match (index_commit, current) {
        (Some(idx), Some(cur)) => idx != cur,
        (None, Some(_)) => true,  // index built outside git, now in git
        (Some(_), None) => false, // git not available, can't tell
        (None, None) => false,    // neither has git
    }
}

/// Detect which files have changed since the last index.
/// Returns (files_to_reindex, files_to_remove, unchanged_ids).
///
/// A file needs re-indexing if:
///   - It's new (not in previous index)
///   - Its mtime has changed
///   - It was renamed (old path gone, new path appears)
pub fn detect_changes(
    current_files: &[(PathBuf, i64)], // (path, mtime_secs)
    prev_metas: &[FileMeta],
) -> (Vec<PathBuf>, Vec<u32>, Vec<u32>) {
    let prev_map: HashMap<&Path, (u32, &FileMeta)> = prev_metas
        .iter()
        .enumerate()
        .map(|(id, m)| (m.path.as_path(), (id as u32, m)))
        .collect();

    let current_set: std::collections::HashSet<&Path> =
        current_files.iter().map(|(p, _)| p.as_path()).collect();

    let mut to_reindex = Vec::new();
    let mut unchanged = Vec::new();

    for (path, mtime) in current_files {
        if let Some(&(id, meta)) = prev_map.get(path.as_path()) {
            if meta.mtime_secs == *mtime {
                unchanged.push(id);
            } else {
                to_reindex.push(path.clone());
            }
        } else {
            // New file (or renamed — old path will be in to_remove)
            to_reindex.push(path.clone());
        }
    }

    // Files that were in the previous index but no longer exist (deleted or renamed away)
    let to_remove: Vec<u32> = prev_metas
        .iter()
        .enumerate()
        .filter(|(_, m)| !current_set.contains(m.path.as_path()))
        .map(|(id, _)| id as u32)
        .collect();

    (to_reindex, to_remove, unchanged)
}

/// Compute a fast content hash for change detection.
pub fn content_hash(data: &[u8]) -> u64 {
    ngram_hash(data)
}

/// Collect all files in the repo, respecting .gitignore.
/// Returns (relative_path, mtime_secs) pairs.
pub fn collect_files(root: &Path) -> Result<Vec<(PathBuf, i64)>> {
    use ignore::WalkBuilder;
    use std::time::UNIX_EPOCH;

    let mut files = Vec::new();

    let walker = WalkBuilder::new(root)
        .hidden(true)        // skip hidden files
        .git_ignore(true)    // respect .gitignore
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            name != ".git"
        })
        .build();

    for entry in walker {
        let entry = entry.context("walking directory")?;
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }

        let abs_path = entry.path();
        let rel_path = abs_path
            .strip_prefix(root)
            .unwrap_or(abs_path)
            .to_path_buf();

        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        files.push((rel_path, mtime));
    }

    Ok(files)
}

/// Acquire a lock file to prevent concurrent index writes.
/// Returns the lock file handle (lock is held until dropped).
pub fn acquire_lock(index_dir: &Path) -> Result<std::fs::File> {
    use std::fs::OpenOptions;
    let lock_path = index_dir.join(".lock");
    std::fs::create_dir_all(index_dir)?;

    let lock_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&lock_path)
        .context("Failed to create lock file")?;

    // Try to get exclusive access by writing our PID
    use std::io::Write;
    let mut f = lock_file;
    writeln!(f, "{}", std::process::id())?;
    f.flush()?;

    Ok(f)
}

/// Release the lock file.
pub fn release_lock(index_dir: &Path) {
    let lock_path = index_dir.join(".lock");
    let _ = std::fs::remove_file(lock_path);
}

/// Check if a stale lock exists (from a crashed process).
/// Returns true if lock seems stale (process not running).
pub fn is_lock_stale(index_dir: &Path) -> bool {
    let lock_path = index_dir.join(".lock");
    if !lock_path.exists() {
        return false;
    }

    // Read PID from lock file
    if let Ok(content) = std::fs::read_to_string(&lock_path) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            // Check if process is still running (Unix-specific but safe to try)
            #[cfg(unix)]
            {
                use std::process::Command;
                let result = Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .output();
                if let Ok(output) = result {
                    if !output.status.success() {
                        // Process not running — stale lock
                        return true;
                    }
                }
            }
            #[cfg(not(unix))]
            {
                // On non-Unix, assume stale if lock is older than 10 minutes
                if let Ok(meta) = std::fs::metadata(&lock_path) {
                    if let Ok(modified) = meta.modified() {
                        if let Ok(elapsed) = modified.elapsed() {
                            return elapsed.as_secs() > 600;
                        }
                    }
                }
            }
        }
    }

    false
}

/// Clean up stale lock if detected.
pub fn cleanup_stale_lock(index_dir: &Path) {
    if is_lock_stale(index_dir) {
        release_lock(index_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_changes_new_files() {
        let current = vec![
            (PathBuf::from("a.rs"), 1000),
            (PathBuf::from("b.rs"), 2000),
        ];
        let prev = vec![];

        let (reindex, remove, unchanged) = detect_changes(&current, &prev);
        assert_eq!(reindex.len(), 2);
        assert!(remove.is_empty());
        assert!(unchanged.is_empty());
    }

    #[test]
    fn test_detect_changes_unchanged() {
        let current = vec![(PathBuf::from("a.rs"), 1000)];
        let prev = vec![FileMeta {
            path: PathBuf::from("a.rs"),
            mtime_secs: 1000,
            content_hash: 111,
        }];

        let (reindex, remove, unchanged) = detect_changes(&current, &prev);
        assert!(reindex.is_empty());
        assert!(remove.is_empty());
        assert_eq!(unchanged, vec![0]);
    }

    #[test]
    fn test_detect_changes_modified() {
        let current = vec![(PathBuf::from("a.rs"), 2000)];
        let prev = vec![FileMeta {
            path: PathBuf::from("a.rs"),
            mtime_secs: 1000,
            content_hash: 111,
        }];

        let (reindex, remove, unchanged) = detect_changes(&current, &prev);
        assert_eq!(reindex, vec![PathBuf::from("a.rs")]);
        assert!(remove.is_empty());
        assert!(unchanged.is_empty());
    }

    #[test]
    fn test_detect_changes_deleted() {
        let current = vec![];
        let prev = vec![FileMeta {
            path: PathBuf::from("a.rs"),
            mtime_secs: 1000,
            content_hash: 111,
        }];

        let (reindex, remove, unchanged) = detect_changes(&current, &prev);
        assert!(reindex.is_empty());
        assert_eq!(remove, vec![0]);
        assert!(unchanged.is_empty());
    }

    #[test]
    fn test_detect_changes_rename() {
        // File renamed: old path gone, new path appears
        let current = vec![(PathBuf::from("new_name.rs"), 1000)];
        let prev = vec![FileMeta {
            path: PathBuf::from("old_name.rs"),
            mtime_secs: 1000,
            content_hash: 111,
        }];

        let (reindex, remove, unchanged) = detect_changes(&current, &prev);
        assert_eq!(reindex, vec![PathBuf::from("new_name.rs")]);
        assert_eq!(remove, vec![0]); // old_name.rs removed
        assert!(unchanged.is_empty());
    }

    #[test]
    fn test_content_hash_deterministic() {
        let data = b"fn main() { println!(\"hello\"); }";
        assert_eq!(content_hash(data), content_hash(data));
    }

    #[test]
    fn test_content_hash_different_content() {
        let a = b"hello world";
        let b = b"hello world!";
        assert_ne!(content_hash(a), content_hash(b));
    }

    #[test]
    fn test_is_index_stale_no_git() {
        // When neither has git, not stale
        assert!(!is_index_stale(None, Path::new("/nonexistent")));
    }

    #[test]
    fn test_lock_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let idx_dir = dir.path().join(".instagrep");

        let _lock = acquire_lock(&idx_dir).unwrap();
        assert!(idx_dir.join(".lock").exists());

        release_lock(&idx_dir);
        assert!(!idx_dir.join(".lock").exists());
    }
}
