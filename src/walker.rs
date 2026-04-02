//! File filtering pipeline.
//!
//! Wraps `ignore::WalkBuilder` with all the filtering options (max-depth,
//! follow symlinks, hidden files, gitignore, type filters, glob filters,
//! max-filesize) and combines with index-based candidate filtering.

use crate::types;
use std::path::{Path, PathBuf};

/// Configuration for file walking/filtering.
#[derive(Clone, Debug)]
pub struct WalkConfig {
    pub root: PathBuf,
    pub max_depth: Option<usize>,
    pub follow_symlinks: bool,
    pub hidden: bool,
    pub no_ignore: bool,
    pub no_ignore_vcs: bool,
    pub globs: Vec<String>,
    pub type_include: Vec<String>,
    pub type_exclude: Vec<String>,
    pub max_filesize: Option<u64>,
    pub threads: Option<usize>,
}

impl Default for WalkConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("."),
            max_depth: None,
            follow_symlinks: false,
            hidden: false,
            no_ignore: false,
            no_ignore_vcs: false,
            globs: vec![],
            type_include: vec![],
            type_exclude: vec![],
            max_filesize: None,
            threads: None,
        }
    }
}

/// Collect files using ignore::WalkBuilder with all filters applied.
/// Returns relative paths.
pub fn collect_files(config: &WalkConfig) -> Vec<PathBuf> {
    use ignore::WalkBuilder;

    let mut builder = WalkBuilder::new(&config.root);

    builder
        .hidden(!config.hidden)
        .git_ignore(!config.no_ignore && !config.no_ignore_vcs)
        .git_global(!config.no_ignore && !config.no_ignore_vcs)
        .git_exclude(!config.no_ignore && !config.no_ignore_vcs)
        .require_git(false)
        .follow_links(config.follow_symlinks);

    if config.no_ignore {
        builder.ignore(false);
    }

    if let Some(depth) = config.max_depth {
        builder.max_depth(Some(depth));
    }

    if let Some(threads) = config.threads {
        builder.threads(threads.max(1));
    }

    // Add glob overrides
    if !config.globs.is_empty() {
        let mut overrides = ignore::overrides::OverrideBuilder::new(&config.root);
        for glob in &config.globs {
            // In rg, globs without ! are include, with ! are exclude
            let _ = overrides.add(glob);
        }
        if let Ok(ovr) = overrides.build() {
            builder.overrides(ovr);
        }
    }

    // Add type-based overrides
    if !config.type_include.is_empty() || !config.type_exclude.is_empty() {
        let mut overrides = ignore::overrides::OverrideBuilder::new(&config.root);

        // Include types: add their globs as includes
        for type_name in &config.type_include {
            if let Some(globs) = types::type_globs(type_name) {
                for glob in globs {
                    let _ = overrides.add(glob);
                }
            }
        }

        // Exclude types: add their globs as negated
        for type_name in &config.type_exclude {
            if let Some(globs) = types::type_globs(type_name) {
                for glob in globs {
                    let _ = overrides.add(&format!("!{}", glob));
                }
            }
        }

        if let Ok(ovr) = overrides.build() {
            builder.overrides(ovr);
        }
    }

    // Filter entry to skip .instagrep and .git
    let walker = builder
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            name != ".git"
        })
        .build();

    let max_filesize = config.max_filesize;

    let mut files = Vec::new();
    for entry in walker {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }

        // Max filesize filter
        if let Some(max_size) = max_filesize {
            if let Ok(meta) = entry.metadata() {
                if meta.len() > max_size {
                    continue;
                }
            }
        }

        let abs_path = entry.path();
        let rel_path = abs_path
            .strip_prefix(&config.root)
            .unwrap_or(abs_path)
            .to_path_buf();

        files.push(rel_path);
    }

    files
}

/// Filter a list of candidate paths using walk configuration
/// (type filters, glob filters, max-filesize).
/// Used when we already have candidates from the index.
pub fn filter_candidates(
    candidates: &[PathBuf],
    root: &Path,
    config: &WalkConfig,
) -> Vec<PathBuf> {
    let has_type_include = !config.type_include.is_empty();
    let has_type_exclude = !config.type_exclude.is_empty();

    // Build glob override matcher for candidates
    let glob_matcher = if !config.globs.is_empty() {
        let mut overrides = ignore::overrides::OverrideBuilder::new(root);
        for glob in &config.globs {
            let _ = overrides.add(glob);
        }
        overrides.build().ok()
    } else {
        None
    };

    candidates
        .iter()
        .filter(|p| {
            let path_str = p.to_string_lossy();

            // Type include filter
            if has_type_include && !types::matches_type(&path_str, &config.type_include) {
                return false;
            }

            // Type exclude filter
            if has_type_exclude && types::matches_type_not(&path_str, &config.type_exclude) {
                return false;
            }

            // Glob filter via override matcher
            if let Some(ref matcher) = glob_matcher {
                let full_path = root.join(p);
                match matcher.matched(&full_path, false) {
                    ignore::Match::None => {
                        // If we have include globs (non-negated), this means no match → exclude
                        // If we only have exclude globs, no match means include
                        let has_includes = config.globs.iter().any(|g| !g.starts_with('!'));
                        if has_includes {
                            return false;
                        }
                    }
                    ignore::Match::Ignore(_) => return true,
                    ignore::Match::Whitelist(_) => return true,
                }
            }

            // Max filesize
            if let Some(max_size) = config.max_filesize {
                let full_path = root.join(p);
                if let Ok(meta) = std::fs::metadata(&full_path) {
                    if meta.len() > max_size {
                        return false;
                    }
                }
            }

            // Max depth
            if let Some(max_depth) = config.max_depth {
                let depth = p.components().count();
                if depth > max_depth {
                    return false;
                }
            }

            // Hidden files
            if !config.hidden {
                for component in p.components() {
                    let s = component.as_os_str().to_string_lossy();
                    if s.starts_with('.') && s != "." && s != ".." {
                        return false;
                    }
                }
            }

            true
        })
        .cloned()
        .collect()
}

/// Intersect index candidates with walk results for maximum filtering.
pub fn candidates_with_index(
    index_candidates: &[PathBuf],
    walk_config: &WalkConfig,
    root: &Path,
) -> Vec<PathBuf> {
    filter_candidates(index_candidates, root, walk_config)
}

/// Parse a human-readable filesize string (e.g., "10M", "500K", "1024").
/// Returns bytes.
pub fn parse_filesize(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (num_str, multiplier) = if s.ends_with('K') || s.ends_with('k') {
        (&s[..s.len() - 1], 1024u64)
    } else if s.ends_with('M') || s.ends_with('m') {
        (&s[..s.len() - 1], 1024 * 1024)
    } else if s.ends_with('G') || s.ends_with('g') {
        (&s[..s.len() - 1], 1024 * 1024 * 1024)
    } else if s.ends_with("KB") || s.ends_with("kb") {
        (&s[..s.len() - 2], 1024)
    } else if s.ends_with("MB") || s.ends_with("mb") {
        (&s[..s.len() - 2], 1024 * 1024)
    } else if s.ends_with("GB") || s.ends_with("gb") {
        (&s[..s.len() - 2], 1024 * 1024 * 1024)
    } else {
        (s, 1u64)
    };

    num_str.trim().parse::<u64>().ok().map(|n| n * multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_filesize() {
        assert_eq!(parse_filesize("1024"), Some(1024));
        assert_eq!(parse_filesize("10K"), Some(10 * 1024));
        assert_eq!(parse_filesize("10k"), Some(10 * 1024));
        assert_eq!(parse_filesize("5M"), Some(5 * 1024 * 1024));
        assert_eq!(parse_filesize("1G"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_filesize("10MB"), Some(10 * 1024 * 1024));
        assert_eq!(parse_filesize(""), None);
        assert_eq!(parse_filesize("abc"), None);
    }

    #[test]
    fn test_filter_hidden_files() {
        let candidates = vec![
            PathBuf::from("src/main.rs"),
            PathBuf::from(".hidden/file.rs"),
            PathBuf::from("src/.secret.rs"),
        ];

        let config = WalkConfig {
            hidden: false,
            ..Default::default()
        };

        let filtered = filter_candidates(&candidates, Path::new("/tmp"), &config);
        assert_eq!(filtered, vec![PathBuf::from("src/main.rs")]);
    }

    #[test]
    fn test_filter_hidden_files_included() {
        let candidates = vec![
            PathBuf::from("src/main.rs"),
            PathBuf::from(".hidden/file.rs"),
        ];

        let config = WalkConfig {
            hidden: true,
            ..Default::default()
        };

        let filtered = filter_candidates(&candidates, Path::new("/tmp"), &config);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_filter_max_depth() {
        let candidates = vec![
            PathBuf::from("main.rs"),
            PathBuf::from("src/lib.rs"),
            PathBuf::from("src/index/builder.rs"),
        ];

        let config = WalkConfig {
            max_depth: Some(1),
            ..Default::default()
        };

        let filtered = filter_candidates(&candidates, Path::new("/tmp"), &config);
        assert_eq!(filtered, vec![PathBuf::from("main.rs")]);
    }

    #[test]
    fn test_filter_type_include() {
        let candidates = vec![
            PathBuf::from("main.rs"),
            PathBuf::from("script.py"),
            PathBuf::from("readme.md"),
        ];

        let config = WalkConfig {
            type_include: vec!["rust".to_string()],
            ..Default::default()
        };

        let filtered = filter_candidates(&candidates, Path::new("/tmp"), &config);
        assert_eq!(filtered, vec![PathBuf::from("main.rs")]);
    }

    #[test]
    fn test_filter_type_exclude() {
        let candidates = vec![
            PathBuf::from("main.rs"),
            PathBuf::from("script.py"),
            PathBuf::from("readme.md"),
        ];

        let config = WalkConfig {
            type_exclude: vec!["py".to_string()],
            ..Default::default()
        };

        let filtered = filter_candidates(&candidates, Path::new("/tmp"), &config);
        assert_eq!(
            filtered,
            vec![PathBuf::from("main.rs"), PathBuf::from("readme.md")]
        );
    }
}
