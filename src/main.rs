//! instagrep — Instant Grep: blazing-fast regex search via sparse n-gram inverted index.
//!
//! Open-source reimplementation of Cursor's "Fast Regex Search" system.
//! Works as a drop-in replacement for ripgrep with index-accelerated search.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rayon::prelude::*;
use regex::bytes::Regex;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use instagrep::index::{builder, incremental, query, storage};
use instagrep::printer::{self, ColorMode, SearchConfig, SearchStats, SortMode};
use instagrep::types;
use instagrep::utils;
use instagrep::walker::{self, WalkConfig};

#[derive(Parser)]
#[command(
    name = "instagrep",
    about = "Instant Grep — blazing-fast regex search via sparse n-gram inverted index.\n\n\
             Open-source reimplementation of Cursor's Fast Regex Search.\n\
             Works with any AI coding agent as a drop-in search tool.",
    version,
    after_help = "EXAMPLES:\n  \
        instagrep index .                      Build/update index\n  \
        instagrep search \"MAX_FILE_SIZE\"       Regex search\n  \
        instagrep search -w -t rust \"unsafe\"   Word match in Rust files\n  \
        instagrep search --json \"error\"        JSON output for AI agents\n  \
        instagrep search -A5 -B2 \"TODO\"        Context lines\n  \
        instagrep search -c \"fn\"               Count matches per file\n  \
        instagrep status                       Index health\n  \
        instagrep clear                        Reset index"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build or incrementally update the search index
    Index {
        /// Root directory to index
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Force full re-index
        #[arg(long)]
        force: bool,

        /// Number of threads for indexing
        #[arg(short = 'j', long)]
        threads: Option<usize>,

        /// Skip files larger than this during indexing (e.g., 10M, 50M). Default: 50M
        #[arg(long, default_value = "50M")]
        max_filesize: String,
    },

    /// Search for a regex pattern using the index
    Search(SearchArgs),

    /// Show index status and health
    Status {
        /// Root directory
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Clear/reset the index
    Clear {
        /// Root directory
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// List supported file types
    TypeList,

    /// Auto-configure MCP server for Claude Code, Cursor, etc.
    Setup {
        /// Which tool to configure: claude, cursor, cline (default: auto-detect)
        #[arg(default_value = "auto")]
        tool: String,

        /// Remove instagrep MCP config instead of adding it
        #[arg(long)]
        remove: bool,
    },
}

#[derive(Parser, Debug)]
struct SearchArgs {
    /// Regex pattern to search for
    #[arg(required_unless_present_any = ["regexp", "file_patterns"])]
    pattern: Option<String>,

    /// Additional regex pattern(s) to search for (repeatable)
    #[arg(short = 'e', long = "regexp", num_args = 1)]
    regexp: Vec<String>,

    /// Read patterns from file (one per line)
    #[arg(short = 'f', long = "file", value_name = "PATTERNFILE")]
    file_patterns: Option<PathBuf>,

    // === Search behavior ===

    /// Case-insensitive search
    #[arg(short = 'i', long)]
    ignore_case: bool,

    /// Case-sensitive search (default)
    #[arg(short = 's', long)]
    case_sensitive: bool,

    /// Smart case: case-insensitive if pattern is all lowercase
    #[arg(short = 'S', long)]
    smart_case: bool,

    /// Treat pattern as literal string, not regex
    #[arg(short = 'F', long)]
    fixed_strings: bool,

    /// Only match when surrounded by word boundaries
    #[arg(short = 'w', long)]
    word_regexp: bool,

    /// Only match when line matches entirely
    #[arg(short = 'x', long)]
    line_regexp: bool,

    /// Show non-matching lines
    #[arg(short = 'v', long)]
    invert_match: bool,

    /// Enable multiline matching
    #[arg(short = 'U', long)]
    multiline: bool,

    /// Make dot match newlines in multiline mode
    #[arg(long)]
    multiline_dotall: bool,

    /// Limit matches per file
    #[arg(short = 'm', long, value_name = "NUM")]
    max_count: Option<usize>,

    /// Search binary files as text
    #[arg(short = 'a', long)]
    text: bool,

    // === Output modes ===

    /// Output JSON Lines (rg-compatible, one JSON object per match)
    #[arg(long)]
    json: bool,

    /// Show count of matching lines per file
    #[arg(short = 'c', long)]
    count: bool,

    /// Show count of individual matches per file
    #[arg(long)]
    count_matches: bool,

    /// Only print paths of files with matches
    #[arg(short = 'l', long = "files-with-matches")]
    files_with_matches: bool,

    /// Only print paths of files without matches
    #[arg(long)]
    files_without_match: bool,

    /// Suppress all output (exit code indicates match)
    #[arg(short = 'q', long)]
    quiet: bool,

    /// Print only the matched parts of a line
    #[arg(short = 'o', long)]
    only_matching: bool,

    /// Print both matching and non-matching lines
    #[arg(long)]
    passthru: bool,

    /// Print results in vim-compatible format
    #[arg(long)]
    vimgrep: bool,

    /// Replace matches with given text
    #[arg(short = 'r', long, value_name = "TEXT")]
    replace: Option<String>,

    /// Print aggregate statistics
    #[arg(long)]
    stats: bool,

    /// Just list files that would be searched (no searching)
    #[arg(long)]
    files: bool,

    // === Context ===

    /// Show NUM lines after each match
    #[arg(short = 'A', long = "after-context", value_name = "NUM", default_value = "0")]
    after_context: usize,

    /// Show NUM lines before each match
    #[arg(short = 'B', long = "before-context", value_name = "NUM", default_value = "0")]
    before_context: usize,

    /// Show NUM lines before and after each match
    #[arg(short = 'C', long = "context", value_name = "NUM")]
    context: Option<usize>,

    /// String between non-contiguous context blocks
    #[arg(long, default_value = "--")]
    context_separator: String,

    // === Output formatting ===

    /// When to use colors: never, auto, always
    #[arg(long, default_value = "auto", value_name = "WHEN")]
    color: String,

    /// Group matches under file headings
    #[arg(long)]
    heading: bool,

    /// Don't group matches under file headings
    #[arg(long)]
    no_heading: bool,

    /// Show line numbers (default when tty)
    #[arg(short = 'n', long)]
    line_number: bool,

    /// Don't show line numbers
    #[arg(short = 'N', long)]
    no_line_number: bool,

    /// Show column number of first match
    #[arg(long)]
    column: bool,

    /// Show byte offset of each line
    #[arg(short = 'b', long)]
    byte_offset: bool,

    /// Always show file path with matches
    #[arg(short = 'H', long)]
    with_filename: bool,

    /// Never show file path with matches
    #[arg(short = 'I', long)]
    no_filename: bool,

    /// Strip leading whitespace from output lines
    #[arg(long)]
    trim: bool,

    /// Alias for --color always --heading --line-number
    #[arg(short = 'p', long)]
    pretty: bool,

    /// Use NUL byte after file paths in output
    #[arg(short = '0', long)]
    null: bool,

    /// Omit lines longer than NUM bytes
    #[arg(short = 'M', long, value_name = "NUM")]
    max_columns: Option<usize>,

    /// Show preview for lines exceeding max-columns
    #[arg(long)]
    max_columns_preview: bool,

    /// Maximum total results (0 = unlimited)
    #[arg(long, default_value = "0", value_name = "NUM")]
    max_results: usize,

    // === File filtering ===

    /// Include/exclude files by glob (repeatable; prefix ! to negate)
    #[arg(short = 'g', long = "glob", value_name = "GLOB")]
    globs: Vec<String>,

    /// Only search files matching TYPE (repeatable)
    #[arg(short = 't', long = "type", value_name = "TYPE")]
    type_include: Vec<String>,

    /// Exclude files matching TYPE (repeatable)
    #[arg(short = 'T', long = "type-not", value_name = "TYPE")]
    type_exclude: Vec<String>,

    /// Max directory depth to recurse
    #[arg(short = 'd', long, value_name = "NUM")]
    max_depth: Option<usize>,

    /// Ignore files larger than NUM (e.g., 10M, 500K)
    #[arg(long, value_name = "NUM+SUFFIX")]
    max_filesize: Option<String>,

    /// Search hidden files and directories
    #[arg(long)]
    hidden: bool,

    /// Follow symbolic links
    #[arg(short = 'L', long)]
    follow: bool,

    /// Don't respect ignore files (.gitignore, .ignore, etc.)
    #[arg(long)]
    no_ignore: bool,

    /// Don't respect VCS ignore files (.gitignore)
    #[arg(long)]
    no_ignore_vcs: bool,

    /// Sort results (path, modified, accessed, created)
    #[arg(long, value_name = "SORTBY")]
    sort: Option<String>,

    /// Sort results in reverse order
    #[arg(long, value_name = "SORTBY")]
    sortr: Option<String>,

    /// Suppress file I/O error messages
    #[arg(long)]
    no_messages: bool,

    /// Number of threads
    #[arg(short = 'j', long, value_name = "NUM")]
    threads: Option<usize>,

    /// Root directory or file to search (default: current directory)
    #[arg(long, default_value = ".")]
    path: PathBuf,
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .format_timestamp(None)
        .init();

    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Index { path, force, threads, max_filesize } => {
            cmd_index(&path, force, threads, &max_filesize)
        }
        Commands::Search(args) => return cmd_search(args),
        Commands::Status { path } => cmd_status(&path),
        Commands::Clear { path } => cmd_clear(&path),
        Commands::TypeList => {
            print!("{}", types::format_type_list());
            Ok(())
        }
        Commands::Setup { tool, remove } => cmd_setup(&tool, remove),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("instagrep: {:#}", e);
            ExitCode::from(2)
        }
    }
}

fn index_dir(root: &Path) -> PathBuf {
    instagrep::index_dir_for(root)
}

fn cmd_index(root: &Path, force: bool, threads: Option<usize>, max_filesize: &str) -> Result<()> {
    let max_size = walker::parse_filesize(max_filesize).unwrap_or(50 * 1024 * 1024);
    let root = std::fs::canonicalize(root).context("Cannot resolve path")?;
    let idx_dir = index_dir(&root);

    // Clean up stale lock from crashed process

    // Acquire lock to prevent concurrent index writes
    let _lock = incremental::acquire_lock(&idx_dir)
        .context("Another instagrep process may be indexing. Use --force to override.")?;

    if let Some(t) = threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(t)
            .build_global()
            .ok();
    }

    let start = std::time::Instant::now();

    let files = incremental::collect_files(&root)?;
    eprintln!("Found {} files", files.len());

    let prev_metas = if !force && storage::index_exists(&idx_dir) {
        match storage::IndexReader::open(&idx_dir) {
            Ok(reader) => reader.file_metas,
            Err(_) => vec![],
        }
    } else {
        vec![]
    };

    let (to_reindex, to_remove, unchanged_ids) =
        incremental::detect_changes(&files, &prev_metas);

    if to_reindex.is_empty() && to_remove.is_empty() && !prev_metas.is_empty() {
        eprintln!("Index is up to date ({} files)", unchanged_ids.len());
        return Ok(());
    }

    eprintln!(
        "Indexing: {} new/modified, {} removed, {} unchanged",
        to_reindex.len(),
        to_remove.len(),
        unchanged_ids.len()
    );

    // Build set of files that need re-indexing
    let reindex_set: std::collections::HashSet<&Path> =
        to_reindex.iter().map(|p| p.as_path()).collect();

    // Process all current files in parallel, but skip n-gram extraction for unchanged ones
    let results: Vec<Option<(storage::FileMeta, Vec<u64>)>> = files
        .par_iter()
        .map(|(rel_path, mtime)| {
            let abs_path = root.join(rel_path);
            let needs_extract = reindex_set.contains(rel_path.as_path());

            // Skip files that are too large
            if let Ok(meta) = std::fs::metadata(&abs_path) {
                if meta.len() > max_size {
                    return None;
                }
            }

            if needs_extract {
                // New or changed file — full extraction
                if utils::is_binary_file(&abs_path) {
                    return None;
                }

                let content = match std::fs::read(&abs_path) {
                    Ok(c) => c,
                    Err(_) => return None,
                };

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
            } else {
                // Unchanged file — still need n-grams for the new index
                // but we can skip binary detection (known good from last index)
                let content = match std::fs::read(&abs_path) {
                    Ok(c) => c,
                    Err(_) => return None,
                };

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
            }
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

    // Release lock
    drop(_lock);

    let elapsed = start.elapsed();
    eprintln!(
        "Indexed {} files in {:.2}s",
        file_metas.len(),
        elapsed.as_secs_f64()
    );

    Ok(())
}

fn cmd_search(args: SearchArgs) -> ExitCode {
    match cmd_search_inner(args) {
        Ok(had_match) => {
            if had_match {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("instagrep: {:#}", e);
            ExitCode::from(2)
        }
    }
}

fn cmd_search_inner(args: SearchArgs) -> Result<bool> {
    let root = std::fs::canonicalize(&args.path).context("Cannot resolve path")?;
    let idx_dir = index_dir(&root);
    let is_tty = io::stdout().is_terminal();

    // Set thread pool
    if let Some(t) = args.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(t)
            .build_global()
            .ok();
    }

    // Collect all patterns
    let mut patterns: Vec<String> = Vec::new();
    if let Some(ref p) = args.pattern {
        patterns.push(p.clone());
    }
    patterns.extend(args.regexp.iter().cloned());
    if let Some(ref pattern_file) = args.file_patterns {
        let content = std::fs::read_to_string(pattern_file)
            .context("Cannot read pattern file")?;
        for line in content.lines() {
            if !line.is_empty() {
                patterns.push(line.to_string());
            }
        }
    }

    // Build search config
    let mut config = SearchConfig {
        ignore_case: args.ignore_case,
        smart_case: args.smart_case && !args.ignore_case && !args.case_sensitive,
        fixed_strings: args.fixed_strings,
        word_regexp: args.word_regexp,
        invert_match: args.invert_match,
        multiline: args.multiline,
        multiline_dotall: args.multiline_dotall,
        max_count: args.max_count,
        search_binary: args.text,

        json: args.json,
        count: args.count,
        count_matches: args.count_matches,
        files_only: args.files_with_matches,
        files_without_match: args.files_without_match,
        quiet: args.quiet,
        vimgrep: args.vimgrep,
        only_matching: args.only_matching,
        passthru: args.passthru,
        stats: args.stats,
        list_files: args.files,

        color: ColorMode::from_str_opt(&args.color).unwrap_or(ColorMode::Auto),
        heading: if args.heading {
            Some(true)
        } else if args.no_heading {
            Some(false)
        } else {
            None
        },
        line_number: if args.line_number {
            Some(true)
        } else if args.no_line_number {
            Some(false)
        } else {
            None
        },
        column: args.column,
        byte_offset: args.byte_offset,
        trim: args.trim,
        with_filename: if args.with_filename {
            Some(true)
        } else if args.no_filename {
            Some(false)
        } else {
            None
        },
        null_separator: args.null,
        context_separator: args.context_separator.clone(),
        replace: args.replace.clone(),
        max_columns: args.max_columns,
        max_columns_preview: args.max_columns_preview,
        pretty: args.pretty,

        // -C sets both, but -A/-B can override with a larger value (matches rg behavior)
        after_context: args.after_context.max(args.context.unwrap_or(0)),
        before_context: args.before_context.max(args.context.unwrap_or(0)),

        max_results: args.max_results,

        sort: SortMode::None,
        sort_reverse: false,
    };

    // Handle sort
    if let Some(ref s) = args.sort {
        config.sort = SortMode::from_str_opt(s).unwrap_or(SortMode::Path);
    }
    if let Some(ref s) = args.sortr {
        config.sort = SortMode::from_str_opt(s).unwrap_or(SortMode::Path);
        config.sort_reverse = true;
    }

    // Handle line_regexp: wrap in ^...$
    let line_regexp = args.line_regexp;

    // Build combined pattern
    let combined_pattern = if patterns.len() == 1 {
        let mut p = patterns[0].clone();
        if line_regexp {
            p = format!("^(?:{})$", p);
        }
        config.build_pattern(&p)
    } else {
        let escaped: Vec<String> = patterns
            .iter()
            .map(|p| {
                let mut pat = p.clone();
                if config.fixed_strings {
                    pat = regex::escape(&pat);
                }
                if config.word_regexp {
                    pat = format!(r"\b{}\b", pat);
                }
                if line_regexp {
                    pat = format!("^(?:{})$", pat);
                }
                pat
            })
            .collect();
        let combined = escaped.join("|");

        let mut flags = String::new();
        // Smart case: check ORIGINAL patterns (not escaped/combined)
        // If any original pattern has uppercase, stay case-sensitive
        let all_lowercase = patterns.iter().all(|p| p == &p.to_lowercase());
        let has_alpha = patterns.iter().any(|p| p.chars().any(|c| c.is_alphabetic()));
        let use_ci = config.ignore_case
            || (config.smart_case && all_lowercase && has_alpha);
        if use_ci {
            flags.push('i');
        }
        if config.multiline {
            flags.push('m'); // (?m) makes ^/$ match line boundaries
        }
        if config.multiline_dotall {
            flags.push('s'); // (?s) makes . match newlines
        }
        if flags.is_empty() {
            combined
        } else {
            format!("(?{}){}", flags, combined)
        }
    };

    let re = Regex::new(&combined_pattern).context("Invalid regex pattern")?;

    // Build walk config
    let walk_config = WalkConfig {
        root: root.clone(),
        max_depth: args.max_depth,
        follow_symlinks: args.follow,
        hidden: args.hidden,
        no_ignore: args.no_ignore,
        no_ignore_vcs: args.no_ignore_vcs,
        globs: args.globs.clone(),
        type_include: args.type_include.clone(),
        type_exclude: args.type_exclude.clone(),
        max_filesize: args.max_filesize.as_deref().and_then(walker::parse_filesize),
        threads: args.threads,
    };

    // Get candidate files
    let use_case_insensitive = config.ignore_case
        || (config.smart_case
            && combined_pattern.to_lowercase() == combined_pattern
            && combined_pattern.chars().any(|c| c.is_alphabetic()));

    let candidate_files: Vec<PathBuf> = get_candidate_files(
        &root,
        &idx_dir,
        &walk_config,
        &patterns,
        args.no_messages,
        use_case_insensitive,
    )?;

    // --files mode: just list files
    if config.list_files {
        let stdout = io::stdout();
        let mut out = io::BufWriter::new(stdout.lock());
        for p in &candidate_files {
            if config.null_separator {
                write!(out, "{}\0", p.display())?;
            } else {
                writeln!(out, "{}", p.display())?;
            }
        }
        out.flush()?;
        return Ok(!candidate_files.is_empty());
    }

    // Sort candidates if requested
    let candidate_files = sort_files(candidate_files, &root, &config);

    // Search
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let mut global_match_count = 0usize;
    let mut any_match = false;
    let mut stats = SearchStats::default();

    // Warn if index is stale
    if let Ok(ref reader) = storage::IndexReader::open(&idx_dir) {
        if incremental::is_index_stale(reader.meta.git_commit.as_deref(), &root) {
            if !args.no_messages {
                eprintln!("instagrep: Index may be stale (git commit changed). Run `instagrep index .` to update.");
            }
        }
    }

    // Cap: don't load files larger than 256MB into memory for search
    const MAX_SEARCH_FILE_SIZE: u64 = 256 * 1024 * 1024;

    for rel_path in &candidate_files {
        let abs_path = root.join(rel_path);

        // Skip files too large for in-memory search
        if let Ok(meta) = std::fs::metadata(&abs_path) {
            if meta.len() > MAX_SEARCH_FILE_SIZE {
                if !args.no_messages {
                    eprintln!("instagrep: {}: file too large ({:.1}MB), skipping",
                        rel_path.display(), meta.len() as f64 / 1_048_576.0);
                }
                continue;
            }
        }

        let content = match std::fs::read(&abs_path) {
            Ok(c) => c,
            Err(e) => {
                if !args.no_messages {
                    eprintln!("instagrep: {}: {}", rel_path.display(), e);
                }
                continue;
            }
        };

        stats.files_searched += 1;
        stats.bytes_searched += content.len() as u64;

        let (had_match, match_count) = printer::search_file(
            &mut out,
            rel_path,
            &content,
            &re,
            &config,
            is_tty,
            &mut global_match_count,
        )?;

        if had_match {
            any_match = true;
            stats.files_matched += 1;
            stats.matches_found += match_count as u64;
            stats.lines_matched += match_count as u64;
        }

        // Quiet mode: bail early on first match
        if config.quiet && any_match {
            break;
        }

        // Global max results
        if config.max_results > 0 && global_match_count >= config.max_results {
            break;
        }
    }

    if config.stats {
        printer::print_stats(&mut out, &stats)?;
    }

    out.flush()?;

    Ok(any_match)
}

/// Get candidate files, using index when available.
fn get_candidate_files(
    root: &Path,
    idx_dir: &Path,
    walk_config: &WalkConfig,
    patterns: &[String],
    no_messages: bool,
    case_insensitive: bool,
) -> Result<Vec<PathBuf>> {
    let reader = storage::IndexReader::open(idx_dir);

    match reader {
        Ok(reader) => {
            // Case-insensitive: skip index pruning, scan all files.
            // The index is byte-exact so it can miss mixed-case variants.
            if case_insensitive {
                let all: Vec<PathBuf> = reader.file_metas.iter().map(|m| m.path.clone()).collect();
                return Ok(walker::filter_candidates(&all, root, walk_config));
            }

            // Try to use index for each pattern and intersect results
            let mut all_candidates: Option<Vec<PathBuf>> = None;

            for pattern in patterns {
                let candidates_result = query::find_candidates(pattern, &reader);
                match candidates_result {
                    Some(ids) => {
                        let paths: Vec<PathBuf> = ids
                            .iter()
                            .map(|&id| reader.file_metas[id as usize].path.clone())
                            .collect();

                        log::info!(
                            "Index narrowed '{}' to {} candidates out of {} files",
                            pattern,
                            paths.len(),
                            reader.num_files()
                        );

                        all_candidates = Some(match all_candidates {
                            Some(existing) => {
                                let path_set: std::collections::HashSet<_> =
                                    paths.iter().collect();
                                existing
                                    .into_iter()
                                    .filter(|p| path_set.contains(p))
                                    .collect()
                            }
                            None => paths,
                        });
                    }
                    None => {
                        // Can't optimize this pattern, use all files
                        log::info!(
                            "No n-grams extractable for '{}', using all files",
                            pattern
                        );
                        all_candidates = Some(
                            reader.file_metas.iter().map(|m| m.path.clone()).collect(),
                        );
                        break;
                    }
                }
            }

            let candidates =
                all_candidates.unwrap_or_else(|| {
                    reader.file_metas.iter().map(|m| m.path.clone()).collect()
                });

            // Apply walk filters to index candidates
            Ok(walker::filter_candidates(&candidates, root, walk_config))
        }
        Err(_) => {
            if !no_messages {
                eprintln!(
                    "instagrep: No index found. Run `instagrep index .` for faster searches."
                );
                eprintln!("instagrep: Falling back to file walk...");
            }
            Ok(walker::collect_files(walk_config))
        }
    }
}

/// Sort files according to config.
fn sort_files(mut files: Vec<PathBuf>, root: &Path, config: &SearchConfig) -> Vec<PathBuf> {
    match config.sort {
        SortMode::None => {}
        SortMode::Path => {
            files.sort();
            if config.sort_reverse {
                files.reverse();
            }
        }
        SortMode::Modified => {
            files.sort_by(|a, b| {
                let ma = std::fs::metadata(root.join(a))
                    .and_then(|m| m.modified())
                    .ok();
                let mb = std::fs::metadata(root.join(b))
                    .and_then(|m| m.modified())
                    .ok();
                ma.cmp(&mb)
            });
            if config.sort_reverse {
                files.reverse();
            }
        }
        SortMode::Accessed => {
            files.sort_by(|a, b| {
                let ma = std::fs::metadata(root.join(a))
                    .and_then(|m| m.accessed())
                    .ok();
                let mb = std::fs::metadata(root.join(b))
                    .and_then(|m| m.accessed())
                    .ok();
                ma.cmp(&mb)
            });
            if config.sort_reverse {
                files.reverse();
            }
        }
        SortMode::Created => {
            files.sort_by(|a, b| {
                let ma = std::fs::metadata(root.join(a))
                    .and_then(|m| m.created())
                    .ok();
                let mb = std::fs::metadata(root.join(b))
                    .and_then(|m| m.created())
                    .ok();
                ma.cmp(&mb)
            });
            if config.sort_reverse {
                files.reverse();
            }
        }
    }
    files
}

fn cmd_status(root: &Path) -> Result<()> {
    let root = std::fs::canonicalize(root).context("Cannot resolve path")?;
    let idx_dir = index_dir(&root);

    if !storage::index_exists(&idx_dir) {
        println!("No index found at {}", idx_dir.display());
        println!("Run `instagrep index .` to build one.");
        return Ok(());
    }

    let reader = storage::IndexReader::open(&idx_dir)?;
    let meta = &reader.meta;

    println!("instagrep index status");
    println!("  Directory:  {}", idx_dir.display());
    println!("  Version:    {}", meta.version);
    println!("  Files:      {}", meta.num_files);
    println!("  N-grams:    {}", reader.num_ngrams());
    println!(
        "  Git commit: {}",
        meta.git_commit.as_deref().unwrap_or("(not a git repo)")
    );
    println!("  Built at:   {} (unix timestamp)", meta.timestamp);

    let mut total_size: u64 = 0;
    for name in &["postings.bin", "lookup.bin", "files.bin", "meta.bin"] {
        if let Ok(m) = std::fs::metadata(idx_dir.join(name)) {
            total_size += m.len();
        }
    }
    println!("  Disk size:  {:.1} MB", total_size as f64 / 1_048_576.0);

    Ok(())
}

fn cmd_clear(root: &Path) -> Result<()> {
    let root = std::fs::canonicalize(root).context("Cannot resolve path")?;
    let idx_dir = index_dir(&root);

    if idx_dir.exists() {
        std::fs::remove_dir_all(&idx_dir)?;
        println!("Index cleared: {}", idx_dir.display());
    } else {
        println!("No index found at {}", idx_dir.display());
    }

    Ok(())
}

fn cmd_setup(tool: &str, remove: bool) -> Result<()> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("HOME not set")?;
    let mcp_binary = which_mcp_binary();

    let mcp_config = serde_json::json!({
        "command": mcp_binary,
    });

    // Global config files for each tool (one install, works in every project)
    let targets: Vec<(&str, PathBuf)> = vec![
        // Claude Code reads ~/.claude.json for global MCP servers
        ("Claude Code", PathBuf::from(&home).join(".claude.json")),
        ("Cursor", PathBuf::from(&home).join(".cursor/mcp.json")),
        ("Cline", PathBuf::from(&home).join(".cline/mcp_settings.json")),
        ("Windsurf", PathBuf::from(&home).join(".windsurf/mcp.json")),
    ];

    let selected: Vec<&(&str, PathBuf)> = match tool {
        "claude" => targets.iter().filter(|(n, _)| *n == "Claude Code").collect(),
        "cursor" => targets.iter().filter(|(n, _)| *n == "Cursor").collect(),
        "cline" => targets.iter().filter(|(n, _)| *n == "Cline").collect(),
        "windsurf" => targets.iter().filter(|(n, _)| *n == "Windsurf").collect(),
        "auto" | _ => {
            // Auto-detect: only configure tools that are installed
            targets.iter().filter(|(_, path)| {
                // Check if the tool's config dir exists
                path.parent().map(|p| p.exists()).unwrap_or(false)
            }).collect()
        }
    };

    if selected.is_empty() {
        println!("No AI tools detected. Specify one: instagrep setup claude|cursor|cline|windsurf");
        return Ok(());
    }

    for (name, path) in &selected {
        if remove {
            remove_mcp_entry(name, path)?;
        } else {
            upsert_mcp_entry(name, path, &mcp_config)?;
        }
    }

    if !remove {
        println!("\nDone. Restart your AI tool to activate instagrep.");
    }
    Ok(())
}

fn which_mcp_binary() -> String {
    // Prefer absolute path in ~/.cargo/bin (always works regardless of PATH)
    if let Ok(home) = std::env::var("HOME") {
        let cargo_bin = format!("{home}/.cargo/bin/instagrep-mcp");
        if std::path::Path::new(&cargo_bin).exists() {
            return cargo_bin;
        }
    }
    // Fallback: check PATH
    #[cfg(unix)]
    if let Ok(output) = std::process::Command::new("which")
        .arg("instagrep-mcp")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return path;
            }
        }
    }
    #[cfg(windows)]
    if let Ok(output) = std::process::Command::new("where")
        .arg("instagrep-mcp")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Some(first_line) = path.lines().next() {
                if !first_line.is_empty() {
                    return first_line.to_string();
                }
            }
        }
    }
    // Last resort: bare name (hope it's on PATH)
    "instagrep-mcp".to_string()
}

/// Add or update instagrep in an MCP config file.
/// Handles: file doesn't exist, file exists but no mcpServers, file exists with other servers,
/// file exists with old instagrep config (updates command path).
fn upsert_mcp_entry(label: &str, path: &Path, mcp_config: &serde_json::Value) -> Result<()> {
    let mut config: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Ensure mcpServers object exists
    if config.get("mcpServers").is_none() {
        config["mcpServers"] = serde_json::json!({});
    }

    // Check if already configured with same command
    if let Some(existing) = config["mcpServers"].get("instagrep") {
        if existing.get("command") == mcp_config.get("command") {
            println!("  {}: already configured ✓", label);
            return Ok(());
        }
        // Different command path — update it
        config["mcpServers"]["instagrep"] = mcp_config.clone();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(&config)?)?;
        println!("  {}: updated command path", label);
        return Ok(());
    }

    // Add new entry
    config["mcpServers"]["instagrep"] = mcp_config.clone();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&config)?)?;
    println!("  {}: added ✓", label);
    Ok(())
}

/// Remove instagrep from an MCP config file. Preserves other servers.
fn remove_mcp_entry(label: &str, path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(path)?;
    let mut config: serde_json::Value =
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));

    if let Some(servers) = config.get_mut("mcpServers").and_then(|s| s.as_object_mut()) {
        if servers.remove("instagrep").is_some() {
            std::fs::write(path, serde_json::to_string_pretty(&config)?)?;
            println!("  {}: removed", label);
        }
    }

    Ok(())
}
