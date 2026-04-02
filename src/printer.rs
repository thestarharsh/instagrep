//! Output formatting engine for search results.
//!
//! Supports all ripgrep output modes: default, heading, JSON Lines, count,
//! quiet, vimgrep, files-only, plus context lines, colors, column numbers, etc.

use regex::bytes::Regex;
use std::collections::VecDeque;
use std::io::Write;
use std::path::Path;

/// When to use colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    Never,
    Auto,
    Always,
}

impl ColorMode {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "never" => Some(Self::Never),
            "auto" => Some(Self::Auto),
            "always" | "ansi" => Some(Self::Always),
            _ => None,
        }
    }

    /// Resolve to a boolean given whether stdout is a terminal.
    pub fn should_color(self, is_tty: bool) -> bool {
        match self {
            Self::Never => false,
            Self::Auto => is_tty,
            Self::Always => true,
        }
    }
}

/// Sort order for output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortMode {
    None,
    Path,
    Modified,
    Accessed,
    Created,
}

impl SortMode {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "path" => Some(Self::Path),
            "modified" => Some(Self::Modified),
            "accessed" => Some(Self::Accessed),
            "created" => Some(Self::Created),
            _ => None,
        }
    }
}

/// All search configuration options.
#[derive(Clone, Debug)]
pub struct SearchConfig {
    // Search behavior
    pub ignore_case: bool,
    pub smart_case: bool,
    pub fixed_strings: bool,
    pub word_regexp: bool,
    pub invert_match: bool,
    pub multiline: bool,
    pub multiline_dotall: bool,
    pub max_count: Option<usize>,
    pub search_binary: bool,

    // Output mode
    pub json: bool,
    pub count: bool,
    pub count_matches: bool,
    pub files_only: bool,
    pub files_without_match: bool,
    pub quiet: bool,
    pub vimgrep: bool,
    pub only_matching: bool,
    pub passthru: bool,
    pub stats: bool,
    pub list_files: bool,

    // Output formatting
    pub color: ColorMode,
    pub heading: Option<bool>, // None = auto (true if tty)
    pub line_number: Option<bool>, // None = auto (true if tty)
    pub column: bool,
    pub byte_offset: bool,
    pub trim: bool,
    pub with_filename: Option<bool>, // None = auto
    pub null_separator: bool,
    pub context_separator: String,
    pub replace: Option<String>,
    pub max_columns: Option<usize>,
    pub max_columns_preview: bool,
    pub pretty: bool,

    // Context
    pub after_context: usize,
    pub before_context: usize,

    // Result limits
    pub max_results: usize, // 0 = unlimited

    // Sort
    pub sort: SortMode,
    pub sort_reverse: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            ignore_case: false,
            smart_case: false,
            fixed_strings: false,
            word_regexp: false,
            invert_match: false,
            multiline: false,
            multiline_dotall: false,
            max_count: None,
            search_binary: false,

            json: false,
            count: false,
            count_matches: false,
            files_only: false,
            files_without_match: false,
            quiet: false,
            vimgrep: false,
            only_matching: false,
            passthru: false,
            stats: false,
            list_files: false,

            color: ColorMode::Auto,
            heading: None,
            line_number: None,
            column: false,
            byte_offset: false,
            trim: false,
            with_filename: None,
            null_separator: false,
            context_separator: "--".to_string(),
            replace: None,
            max_columns: None,
            max_columns_preview: false,
            pretty: false,

            after_context: 0,
            before_context: 0,

            max_results: 0,

            sort: SortMode::None,
            sort_reverse: false,
        }
    }
}

impl SearchConfig {
    /// Build the regex pattern string with flags applied.
    pub fn build_pattern(&self, pattern: &str) -> String {
        let mut pat = pattern.to_string();

        if self.fixed_strings {
            pat = regex::escape(&pat);
        }
        if self.word_regexp {
            pat = format!(r"\b{}\b", pat);
        }

        let mut flags = String::new();
        let use_case_insensitive = self.ignore_case
            || (self.smart_case && pat == pat.to_lowercase() && pat.chars().any(|c| c.is_alphabetic()));
        if use_case_insensitive {
            flags.push('i');
        }
        if self.multiline {
            flags.push('s');
        }
        if self.multiline_dotall {
            flags.push('s');
        }

        if !flags.is_empty() {
            pat = format!("(?{}){}", flags, pat);
        }

        pat
    }

    /// Whether to show filenames.
    pub fn show_filename(&self, _is_tty: bool) -> bool {
        self.with_filename.unwrap_or(true)
    }

    /// Whether to show line numbers.
    pub fn show_line_number(&self, is_tty: bool) -> bool {
        self.line_number.unwrap_or(is_tty) || self.vimgrep
    }

    /// Whether to use heading mode.
    pub fn use_heading(&self, is_tty: bool) -> bool {
        if self.pretty {
            return true;
        }
        self.heading.unwrap_or(is_tty)
    }

    /// Whether colors are enabled.
    pub fn use_color(&self, is_tty: bool) -> bool {
        if self.pretty {
            return true;
        }
        self.color.should_color(is_tty)
    }

    /// Whether context lines are requested.
    pub fn has_context(&self) -> bool {
        self.after_context > 0 || self.before_context > 0
    }
}

/// Aggregate stats for --stats output.
#[derive(Default)]
pub struct SearchStats {
    pub files_searched: u64,
    pub files_matched: u64,
    pub lines_matched: u64,
    pub matches_found: u64,
    pub bytes_searched: u64,
}

/// Search a single file and write results.
/// Returns (matched: bool, match_count: usize).
pub fn search_file<W: Write>(
    out: &mut W,
    path: &Path,
    content: &[u8],
    re: &Regex,
    config: &SearchConfig,
    is_tty: bool,
    global_match_count: &mut usize,
) -> std::io::Result<(bool, usize)> {
    let use_color = config.use_color(is_tty);
    let show_filename = config.show_filename(is_tty);
    let show_line_num = config.show_line_number(is_tty);
    let use_heading = config.use_heading(is_tty);

    // Binary check
    if !config.search_binary && content.contains(&0) {
        return Ok((false, 0));
    }

    let path_str = path.to_string_lossy();
    let mut file_match_count: usize = 0;
    let mut file_has_match = false;

    // Count-only mode
    if config.count || config.count_matches {
        let mut count = 0usize;
        for line in content.split(|&b| b == b'\n') {
            if config.count_matches {
                count += re.find_iter(line).count();
            } else if re.is_match(line) {
                count += 1;
            }
        }
        if config.invert_match && config.count {
            let total_lines = content.split(|&b| b == b'\n').count();
            let matching = content.split(|&b| b == b'\n').filter(|l| re.is_match(l)).count();
            count = total_lines - matching;
        }
        if show_filename {
            if use_color {
                write!(out, "\x1b[35m{}\x1b[0m:", path_str)?;
            } else {
                write!(out, "{}:", path_str)?;
            }
        }
        writeln!(out, "{}", count)?;
        return Ok((count > 0, count));
    }

    // Quiet mode
    if config.quiet {
        for line in content.split(|&b| b == b'\n') {
            let is_match = re.is_match(line);
            if (is_match && !config.invert_match) || (!is_match && config.invert_match) {
                return Ok((true, 1));
            }
        }
        return Ok((false, 0));
    }

    // Files-only modes
    if config.files_only {
        let has_match = if config.invert_match {
            !content.split(|&b| b == b'\n').any(|l| re.is_match(l))
        } else {
            content.split(|&b| b == b'\n').any(|l| re.is_match(l))
        };
        if has_match {
            write!(out, "{}", path_str)?;
            if config.null_separator {
                write!(out, "\0")?;
            } else {
                writeln!(out)?;
            }
            return Ok((true, 1));
        }
        return Ok((false, 0));
    }

    if config.files_without_match {
        let has_match = content.split(|&b| b == b'\n').any(|l| re.is_match(l));
        if !has_match {
            write!(out, "{}", path_str)?;
            if config.null_separator {
                write!(out, "\0")?;
            } else {
                writeln!(out)?;
            }
            return Ok((true, 1));
        }
        return Ok((false, 0));
    }

    // JSON Lines mode
    if config.json {
        return search_file_json(out, path, content, re, config, global_match_count);
    }

    // Standard line-by-line search with context
    let lines: Vec<&[u8]> = content.split(|&b| b == b'\n').collect();
    let mut before_buf: VecDeque<(usize, &[u8])> = VecDeque::new();
    let mut after_remaining: usize = 0;
    let mut last_printed_line: Option<usize> = None;
    let mut printed_heading = false;

    let mut byte_offset: usize = 0;

    for (line_idx, line) in lines.iter().enumerate() {
        let is_match = re.is_match(line);
        let should_print = if config.invert_match { !is_match } else { is_match };
        let is_passthru = config.passthru && !should_print;

        if should_print || is_passthru {
            // Check max_count per file
            if should_print {
                if let Some(mc) = config.max_count {
                    if file_match_count >= mc {
                        byte_offset += line.len() + 1;
                        continue;
                    }
                }
                // Check global max_results
                if config.max_results > 0 && *global_match_count >= config.max_results {
                    return Ok((file_has_match, file_match_count));
                }
            }

            // Print heading if needed
            if !printed_heading && use_heading && show_filename {
                if use_color {
                    writeln!(out, "\x1b[35m{}\x1b[0m", path_str)?;
                } else {
                    writeln!(out, "{}", path_str)?;
                }
                printed_heading = true;
            }

            // Print context separator if there's a gap
            if config.has_context() || config.passthru {
                if let Some(last) = last_printed_line {
                    if line_idx > last + 1 && !config.passthru {
                        writeln!(out, "{}", config.context_separator)?;
                    }
                }
            }

            // Print before-context lines
            if should_print {
                for (ctx_line_idx, ctx_line) in before_buf.drain(..) {
                    if let Some(last) = last_printed_line {
                        if ctx_line_idx <= last {
                            continue;
                        }
                        if ctx_line_idx > last + 1 {
                            writeln!(out, "{}", config.context_separator)?;
                        }
                    }
                    print_line(
                        out, &path_str, ctx_line_idx, ctx_line, false, re, config,
                        use_color, show_filename && !use_heading, show_line_num,
                        0, // byte_offset not tracked for context lines precisely
                    )?;
                    last_printed_line = Some(ctx_line_idx);
                }
            }

            if config.only_matching && should_print && !config.invert_match {
                // Print each match on its own line
                for mat in re.find_iter(line) {
                    if config.max_results > 0 && *global_match_count >= config.max_results {
                        return Ok((file_has_match, file_match_count));
                    }
                    let matched_bytes = &line[mat.start()..mat.end()];
                    let text = if let Some(ref repl) = config.replace {
                        let replaced = re.replace(matched_bytes, repl.as_bytes());
                        String::from_utf8_lossy(&replaced).into_owned()
                    } else {
                        String::from_utf8_lossy(matched_bytes).into_owned()
                    };

                    if show_filename && !use_heading {
                        if use_color {
                            write!(out, "\x1b[35m{}\x1b[0m:", path_str)?;
                        } else {
                            write!(out, "{}:", path_str)?;
                        }
                    }
                    if show_line_num {
                        if use_color {
                            write!(out, "\x1b[32m{}\x1b[0m:", line_idx + 1)?;
                        } else {
                            write!(out, "{}:", line_idx + 1)?;
                        }
                    }
                    if config.column {
                        if use_color {
                            write!(out, "\x1b[32m{}\x1b[0m:", mat.start() + 1)?;
                        } else {
                            write!(out, "{}:", mat.start() + 1)?;
                        }
                    }
                    writeln!(out, "{}", text)?;
                    *global_match_count += 1;
                }
                file_match_count += 1;
                file_has_match = true;
            } else {
                // Print the line
                print_line(
                    out, &path_str, line_idx, line, should_print, re, config,
                    use_color, show_filename && !use_heading, show_line_num,
                    byte_offset,
                )?;

                if should_print {
                    file_match_count += 1;
                    file_has_match = true;
                    *global_match_count += 1;
                    after_remaining = config.after_context;
                }
            }

            last_printed_line = Some(line_idx);
        } else if after_remaining > 0 {
            // Print after-context line
            if !printed_heading && use_heading && show_filename {
                if use_color {
                    writeln!(out, "\x1b[35m{}\x1b[0m", path_str)?;
                } else {
                    writeln!(out, "{}", path_str)?;
                }
                printed_heading = true;
            }
            print_line(
                out, &path_str, line_idx, line, false, re, config,
                use_color, show_filename && !use_heading, show_line_num,
                byte_offset,
            )?;
            after_remaining -= 1;
            last_printed_line = Some(line_idx);
        } else {
            // Buffer for before-context
            if config.before_context > 0 {
                before_buf.push_back((line_idx, line));
                if before_buf.len() > config.before_context {
                    before_buf.pop_front();
                }
            }
        }

        byte_offset += line.len() + 1; // +1 for newline
    }

    Ok((file_has_match, file_match_count))
}

fn print_line<W: Write>(
    out: &mut W,
    path_str: &str,
    line_idx: usize,
    line: &[u8],
    is_match: bool,
    re: &Regex,
    config: &SearchConfig,
    use_color: bool,
    show_filename: bool,
    show_line_num: bool,
    byte_offset: usize,
) -> std::io::Result<()> {
    let separator = if is_match { ":" } else { "-" };

    if show_filename {
        if use_color {
            write!(out, "\x1b[35m{}\x1b[0m{}", path_str, separator)?;
        } else {
            write!(out, "{}{}", path_str, separator)?;
        }
    }

    if show_line_num {
        if use_color {
            write!(out, "\x1b[32m{}\x1b[0m{}", line_idx + 1, separator)?;
        } else {
            write!(out, "{}{}", line_idx + 1, separator)?;
        }
    }

    if config.column && is_match {
        if let Some(m) = re.find(line) {
            if use_color {
                write!(out, "\x1b[32m{}\x1b[0m{}", m.start() + 1, separator)?;
            } else {
                write!(out, "{}{}", m.start() + 1, separator)?;
            }
        }
    }

    if config.byte_offset {
        if use_color {
            write!(out, "\x1b[32m{}\x1b[0m{}", byte_offset, separator)?;
        } else {
            write!(out, "{}{}", byte_offset, separator)?;
        }
    }

    let line_text = String::from_utf8_lossy(line);
    let display_text = if config.trim {
        line_text.trim_start()
    } else {
        line_text.trim_end()
    };

    // Max columns
    if let Some(max_cols) = config.max_columns {
        if display_text.len() > max_cols {
            if config.max_columns_preview {
                write!(out, "{}", &display_text[..max_cols])?;
                writeln!(out, " [... {} more bytes]", display_text.len() - max_cols)?;
            } else {
                writeln!(out, "[Omitted long line with {} bytes]", display_text.len())?;
            }
            return Ok(());
        }
    }

    if is_match && use_color && !config.invert_match {
        if let Some(ref repl) = config.replace {
            let replaced = re.replace_all(line, repl.as_bytes());
            let s = String::from_utf8_lossy(&replaced);
            let s = if config.trim { s.trim_start().to_string() } else { s.trim_end().to_string() };
            writeln!(out, "{}", s)?;
        } else {
            // Highlight matches
            let mut last_end = 0;
            let trimmed = if config.trim {
                let trim_offset = line.len() - line_text.trim_start().len();
                trim_offset
            } else {
                0
            };
            for mat in re.find_iter(line) {
                let start = mat.start();
                let end = mat.end();
                if start >= trimmed {
                    let pre = String::from_utf8_lossy(&line[last_end.max(trimmed)..start]);
                    write!(out, "{}", pre)?;
                } else if last_end < trimmed {
                    // skip trimmed prefix
                }
                let matched = String::from_utf8_lossy(&line[start.max(trimmed)..end]);
                write!(out, "\x1b[1;31m{}\x1b[0m", matched)?;
                last_end = end;
            }
            let rest = String::from_utf8_lossy(&line[last_end..]);
            writeln!(out, "{}", rest.trim_end())?;
        }
    } else if let Some(ref repl) = config.replace {
        if is_match {
            let replaced = re.replace_all(line, repl.as_bytes());
            let s = String::from_utf8_lossy(&replaced);
            writeln!(out, "{}", s.trim_end())?;
        } else {
            writeln!(out, "{}", display_text)?;
        }
    } else {
        writeln!(out, "{}", display_text)?;
    }

    Ok(())
}

/// JSON Lines output (one JSON object per match line).
fn search_file_json<W: Write>(
    out: &mut W,
    path: &Path,
    content: &[u8],
    re: &Regex,
    config: &SearchConfig,
    global_match_count: &mut usize,
) -> std::io::Result<(bool, usize)> {
    let path_str = path.to_string_lossy();
    let mut file_match_count = 0usize;
    let mut file_has_match = false;
    let mut byte_offset = 0usize;

    for (line_idx, line) in content.split(|&b| b == b'\n').enumerate() {
        let is_match = re.is_match(line);
        let should_print = if config.invert_match { !is_match } else { is_match };

        if should_print {
            if let Some(mc) = config.max_count {
                if file_match_count >= mc {
                    byte_offset += line.len() + 1;
                    continue;
                }
            }
            if config.max_results > 0 && *global_match_count >= config.max_results {
                return Ok((file_has_match, file_match_count));
            }

            let line_str = String::from_utf8_lossy(line);
            let mut obj = serde_json::json!({
                "type": "match",
                "data": {
                    "path": { "text": path_str },
                    "lines": { "text": line_str.trim_end() },
                    "line_number": line_idx + 1,
                    "absolute_offset": byte_offset,
                }
            });

            // Add submatches
            if !config.invert_match {
                let submatches: Vec<serde_json::Value> = re
                    .find_iter(line)
                    .map(|m| {
                        serde_json::json!({
                            "match": { "text": String::from_utf8_lossy(&line[m.start()..m.end()]) },
                            "start": m.start(),
                            "end": m.end(),
                        })
                    })
                    .collect();
                obj["data"]["submatches"] = serde_json::Value::Array(submatches);
            }

            writeln!(out, "{}", obj)?;

            file_match_count += 1;
            file_has_match = true;
            *global_match_count += 1;
        }

        byte_offset += line.len() + 1;
    }

    Ok((file_has_match, file_match_count))
}

/// Print aggregate stats.
pub fn print_stats<W: Write>(out: &mut W, stats: &SearchStats) -> std::io::Result<()> {
    writeln!(out)?;
    writeln!(out, "{} files searched", stats.files_searched)?;
    writeln!(out, "{} files contained matches", stats.files_matched)?;
    writeln!(out, "{} lines contained matches", stats.lines_matched)?;
    writeln!(out, "{} matches found", stats.matches_found)?;
    writeln!(
        out,
        "{:.1} MB searched",
        stats.bytes_searched as f64 / 1_048_576.0
    )?;
    Ok(())
}
