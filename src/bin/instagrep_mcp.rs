//! instagrep MCP Server — Model Context Protocol server for AI coding tools.
//!
//! Exposes instagrep's index-accelerated regex search as MCP tools that any
//! AI coding agent (Claude Code, Cursor, Cline, Codex, etc.) can call natively.
//!
//! Usage:
//!   instagrep-mcp [--path /project/root]

use anyhow::Context;
use rayon::prelude::*;
use regex::bytes::Regex;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    schemars, tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use instagrep::index::{builder, incremental, query, storage};
use instagrep::utils;
use instagrep::walker::{self, WalkConfig};

// ── Tool parameter types ──────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    /// Regex pattern
    pub pattern: String,
    /// Subfolder to search within (e.g. "src/index"). Searches full project if omitted.
    #[serde(default)]
    pub path: Option<String>,
    /// e.g. "rust", "py", "js"
    #[serde(default)]
    pub file_type: Option<String>,
    #[serde(default)]
    pub glob: Option<String>,
    #[serde(default)]
    pub ignore_case: Option<bool>,
    #[serde(default)]
    pub fixed_strings: Option<bool>,
    #[serde(default)]
    pub word_regexp: Option<bool>,
    /// Default: 200
    #[serde(default)]
    pub max_results: Option<usize>,
    #[serde(default)]
    pub context_lines: Option<usize>,
    #[serde(default)]
    pub include_hidden: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IndexParams {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub force: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StatusParams {
    #[serde(default)]
    pub path: Option<String>,
}

// ── Result types ──────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct SearchMatch {
    file: String,
    line: usize,
    text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    context_before: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    context_after: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SearchResult {
    matches: Vec<SearchMatch>,
    stats: SearchResultStats,
}

#[derive(Debug, Serialize)]
struct SearchResultStats {
    total_files: usize,
    candidates: usize,
    files_searched: usize,
    matches_found: usize,
    elapsed_ms: u64,
}

#[derive(Debug, Serialize)]
struct IndexResult {
    files_indexed: usize,
    elapsed_secs: f64,
    index_size_mb: f64,
}

#[derive(Debug, Serialize)]
struct StatusResult {
    has_index: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_files: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_ngrams: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_stale: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disk_size_mb: Option<f64>,
}

// ── MCP Server ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct InstaGrepServer {
    default_root: PathBuf,
    tool_router: ToolRouter<Self>,
}

impl InstaGrepServer {
    pub fn new(root: PathBuf) -> Self {
        Self {
            default_root: root,
            tool_router: Self::tool_router(),
        }
    }

    fn idx_dir(root: &Path) -> PathBuf {
        instagrep::index_dir_for(root)
    }

    fn disk_size(idx_dir: &Path) -> f64 {
        let mut total: u64 = 0;
        for name in &["postings.bin", "lookup.bin", "files.bin", "meta.bin"] {
            if let Ok(m) = std::fs::metadata(idx_dir.join(name)) {
                total += m.len();
            }
        }
        total as f64 / 1_048_576.0
    }

    fn do_search(&self, params: &SearchParams) -> anyhow::Result<SearchResult> {
        let start = std::time::Instant::now();
        // Always use default_root for indexing — path is a subfolder filter
        let root = std::fs::canonicalize(&self.default_root)
            .context("Cannot resolve project root")?;
        let idx_dir = Self::idx_dir(&root);
        let subfolder = params.path.as_deref().filter(|p| !p.is_empty());
        let max_results = params.max_results.unwrap_or(200);
        let context_lines = params.context_lines.unwrap_or(0);
        let ignore_case = params.ignore_case.unwrap_or(false);

        // Build regex
        let mut pat = params.pattern.clone();
        if params.fixed_strings.unwrap_or(false) {
            pat = regex::escape(&pat);
        }
        if params.word_regexp.unwrap_or(false) {
            pat = format!(r"\b{}\b", pat);
        }
        if ignore_case {
            pat = format!("(?i){}", pat);
        }
        let re = Regex::new(&pat).context("Invalid regex pattern")?;

        // Get candidates from index
        let reader = storage::IndexReader::open(&idx_dir).ok();
        let total_files = reader.as_ref().map(|r| r.num_files()).unwrap_or(0);

        let candidate_files: Vec<PathBuf> = if let Some(ref reader) = reader {
            if ignore_case {
                // Case-insensitive: skip index pruning, scan all files.
                // The index is byte-exact so it can miss mixed-case variants.
                // Better to be correct than fast.
                reader.file_metas.iter().map(|m| m.path.clone()).collect()
            } else {
                match query::find_candidates(&params.pattern, reader) {
                    Some(ids) => ids
                        .iter()
                        .map(|&id| reader.file_metas[id as usize].path.clone())
                        .collect(),
                    None => reader.file_metas.iter().map(|m| m.path.clone()).collect(),
                }
            }
        } else {
            walker::collect_files(&WalkConfig {
                root: root.clone(),
                hidden: params.include_hidden.unwrap_or(false),
                ..Default::default()
            })
        };

        let num_candidates = candidate_files.len();

        // Apply filters
        let walk_config = WalkConfig {
            root: root.clone(),
            hidden: params.include_hidden.unwrap_or(false),
            type_include: params
                .file_type
                .as_ref()
                .map(|t| vec![t.clone()])
                .unwrap_or_default(),
            globs: params
                .glob
                .as_ref()
                .map(|g| vec![g.clone()])
                .unwrap_or_default(),
            ..Default::default()
        };
        let filtered = walker::filter_candidates(&candidate_files, &root, &walk_config);

        // Apply subfolder filter if path was specified
        let filtered: Vec<PathBuf> = if let Some(sub) = subfolder {
            filtered
                .into_iter()
                .filter(|p| p.starts_with(sub))
                .collect()
        } else {
            filtered
        };

        // Search
        let mut matches = Vec::new();
        let mut files_searched = 0usize;

        for rel_path in &filtered {
            if matches.len() >= max_results {
                break;
            }
            let abs_path = root.join(rel_path);
            let content = match std::fs::read(&abs_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if content.contains(&0) {
                continue;
            }
            files_searched += 1;

            let lines: Vec<&[u8]> = content.split(|&b| b == b'\n').collect();
            for (line_idx, line) in lines.iter().enumerate() {
                if matches.len() >= max_results {
                    break;
                }
                if re.is_match(line) {
                    let text = String::from_utf8_lossy(line).trim_end().to_string();
                    let mut context_before = Vec::new();
                    let mut context_after = Vec::new();
                    if context_lines > 0 {
                        let s = line_idx.saturating_sub(context_lines);
                        for i in s..line_idx {
                            context_before
                                .push(String::from_utf8_lossy(lines[i]).trim_end().to_string());
                        }
                        let e = (line_idx + 1 + context_lines).min(lines.len());
                        for i in (line_idx + 1)..e {
                            context_after
                                .push(String::from_utf8_lossy(lines[i]).trim_end().to_string());
                        }
                    }
                    matches.push(SearchMatch {
                        file: rel_path.to_string_lossy().to_string(),
                        line: line_idx + 1,
                        text,
                        context_before,
                        context_after,
                    });
                }
            }
        }

        let elapsed = start.elapsed();
        let matches_found = matches.len();

        Ok(SearchResult {
            matches,
            stats: SearchResultStats {
                total_files: if total_files > 0 { total_files } else { filtered.len() },
                candidates: num_candidates,
                files_searched,
                matches_found,
                elapsed_ms: elapsed.as_millis() as u64,
            },
        })
    }

    fn do_index(&self, params: &IndexParams) -> anyhow::Result<IndexResult> {
        // Always index from project root — one index per project
        let root = std::fs::canonicalize(&self.default_root)
            .context("Cannot resolve project root")?;
        let idx_dir = Self::idx_dir(&root);
        let force = params.force.unwrap_or(false);

        let _lock = incremental::acquire_lock(&idx_dir)?;

        let start = std::time::Instant::now();
        let files = incremental::collect_files(&root)?;

        let prev_metas = if !force && storage::index_exists(&idx_dir) {
            storage::IndexReader::open(&idx_dir)
                .map(|r| r.file_metas)
                .unwrap_or_default()
        } else {
            vec![]
        };

        let (to_reindex, to_remove, unchanged_ids) =
            incremental::detect_changes(&files, &prev_metas);

        if to_reindex.is_empty() && to_remove.is_empty() && !prev_metas.is_empty() {
            return Ok(IndexResult {
                files_indexed: unchanged_ids.len(),
                elapsed_secs: start.elapsed().as_secs_f64(),
                index_size_mb: Self::disk_size(&idx_dir),
            });
        }

        let max_size: u64 = 50 * 1024 * 1024;
        let results: Vec<Option<(storage::FileMeta, Vec<u64>)>> = files
            .par_iter()
            .map(|(rel_path, mtime)| {
                let abs_path = root.join(rel_path);
                if let Ok(meta) = std::fs::metadata(&abs_path) {
                    if meta.len() > max_size {
                        return None;
                    }
                }
                if utils::is_binary_file(&abs_path) {
                    return None;
                }
                let content = std::fs::read(&abs_path).ok()?;
                if content.contains(&0) {
                    return None;
                }
                let hash = incremental::content_hash(&content);
                let ngrams = builder::extract_sparse_ngrams(&content);
                Some((
                    storage::FileMeta {
                        path: rel_path.clone(),
                        mtime_secs: *mtime,
                        content_hash: hash,
                    },
                    ngrams,
                ))
            })
            .collect();

        let mut file_metas = Vec::new();
        let mut file_ngrams = Vec::new();
        for result in results.into_iter().flatten() {
            file_metas.push(result.0);
            file_ngrams.push(result.1);
        }

        let git_commit = incremental::get_head_commit(&root);
        let writer = storage::IndexWriter::new(&idx_dir)?;
        writer.write(&file_ngrams, &file_metas, git_commit)?;

        drop(_lock);

        Ok(IndexResult {
            files_indexed: file_metas.len(),
            elapsed_secs: start.elapsed().as_secs_f64(),
            index_size_mb: Self::disk_size(&idx_dir),
        })
    }

    fn do_status(&self, _params: &StatusParams) -> StatusResult {
        let root = match std::fs::canonicalize(&self.default_root) {
            Ok(r) => r,
            Err(_) => {
                return StatusResult {
                    has_index: false,
                    num_files: None,
                    num_ngrams: None,
                    git_commit: None,
                    is_stale: None,
                    disk_size_mb: None,
                }
            }
        };
        let idx_dir = Self::idx_dir(&root);

        if !storage::index_exists(&idx_dir) {
            return StatusResult {
                has_index: false,
                num_files: None,
                num_ngrams: None,
                git_commit: None,
                is_stale: None,
                disk_size_mb: None,
            };
        }

        match storage::IndexReader::open(&idx_dir) {
            Ok(reader) => {
                let stale =
                    incremental::is_index_stale(reader.meta.git_commit.as_deref(), &root);
                StatusResult {
                    has_index: true,
                    num_files: Some(reader.meta.num_files),
                    num_ngrams: Some(reader.num_ngrams()),
                    git_commit: reader.meta.git_commit.clone(),
                    is_stale: Some(stale),
                    disk_size_mb: Some(Self::disk_size(&idx_dir)),
                }
            }
            Err(_) => StatusResult {
                has_index: false,
                num_files: None,
                num_ngrams: None,
                git_commit: None,
                is_stale: None,
                disk_size_mb: None,
            },
        }
    }
}

// ── MCP Tool Definitions ──────────────────────────────────────────────

#[tool_router]
impl InstaGrepServer {
    #[tool(
        name = "search",
        description = "Fast indexed regex search. Auto-indexes on first call. Just search — never call index first."
    )]
    async fn search_tool(&self, Parameters(params): Parameters<SearchParams>) -> String {
        // Auto-index if no index exists, auto-reindex if stale
        // Always index from project root, never from a subfolder
        let root_path = &self.default_root;
        let idx_dir = Self::idx_dir(root_path);
        let needs_index = if !storage::index_exists(&idx_dir) {
            true
        } else if let Ok(reader) = storage::IndexReader::open(&idx_dir) {
            incremental::is_index_stale(reader.meta.git_commit.as_deref(), root_path)
        } else {
            true
        };
        if needs_index {
            let _ = self.do_index(&IndexParams {
                path: None,
                force: Some(false),
            });
        }

        match self.do_search(&params) {
            Ok(result) => serde_json::to_string_pretty(&result).unwrap_or_else(|e| {
                format!("{{\"error\": \"serialization failed: {}\"}}", e)
            }),
            Err(e) => serde_json::json!({
                "error": format!("{:#}", e),
                "matches": [],
                "stats": {"total_files":0,"candidates":0,"files_searched":0,"matches_found":0}
            })
            .to_string(),
        }
    }

    #[tool(
        name = "index",
        description = "Rebuild search index. Only needed if search results seem stale. Search auto-indexes — you rarely need this."
    )]
    async fn index_tool(&self, Parameters(params): Parameters<IndexParams>) -> String {
        match self.do_index(&params) {
            Ok(result) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e)),
            Err(e) => serde_json::json!({"error": format!("{:#}", e)}).to_string(),
        }
    }

    #[tool(
        name = "status",
        description = "Check index health. Search auto-indexes — you don't need to check status before searching."
    )]
    async fn status_tool(&self, Parameters(params): Parameters<StatusParams>) -> String {
        let result = self.do_status(&params);
        serde_json::to_string_pretty(&result)
            .unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
    }
}

// ── ServerHandler ─────────────────────────────────────────────────────

#[tool_handler]
impl ServerHandler for InstaGrepServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
    }
}

// ── Entry point ───────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--path" if i + 1 < args.len() => {
                root = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--help" | "-h" => {
                eprintln!("instagrep-mcp — MCP server for instant regex search");
                eprintln!();
                eprintln!("Usage: instagrep-mcp [--path <dir>]");
                eprintln!("  Defaults to cwd. Auto-indexes on first search.");
                eprintln!();
                eprintln!("Tools: instagrep_search, instagrep_index, instagrep_status");
                eprintln!();
                eprintln!("Add to any MCP-compatible tool:");
                eprintln!(
                    "  {{\"mcpServers\": {{\"instagrep\": {{\"command\": \"instagrep-mcp\"}}}}}}"
                );
                std::process::exit(0);
            }
            _ => i += 1,
        }
    }

    eprintln!("instagrep-mcp: starting with root={}", root.display());

    let server = InstaGrepServer::new(root);
    let transport = (tokio::io::stdin(), tokio::io::stdout());

    let service = rmcp::serve_server(server, transport).await?;
    service.waiting().await?;

    Ok(())
}
