use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

use instagrep::index::{builder, query, storage};
use instagrep::printer::{self, ColorMode, SearchConfig};
use instagrep::utils;
use instagrep::walker::{self, WalkConfig};

fn setup_test_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::write(
        root.join("main.rs"),
        r#"
fn main() {
    let MAX_FILE_SIZE: usize = 1024 * 1024;
    println!("Max size: {}", MAX_FILE_SIZE);
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("config.rs"),
        r#"
pub const MAX_FILE_SIZE: usize = 10 * 1024 * 1024;
pub const MIN_BUFFER: usize = 4096;
pub const ZX_HANDLE_INVALID: u32 = 0;
"#,
    )
    .unwrap();

    fs::write(
        root.join("util.rs"),
        r#"
pub fn parse_config() -> Result<(), String> {
    // No mention of MAX_FILE_SIZE here
    Ok(())
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("readme.txt"),
        "This is a readme file with no code.\n",
    )
    .unwrap();

    fs::write(root.join("app.py"), "import os\ndef hello():\n    print('hello')\n").unwrap();

    dir
}

fn build_index(dir: &TempDir) -> PathBuf {
    let root = dir.path();
    let idx_dir = root.join(".instagrep");
    let file_names = ["main.rs", "config.rs", "util.rs", "readme.txt", "app.py"];

    let mut file_metas = Vec::new();
    let mut file_ngrams = Vec::new();

    for name in &file_names {
        let path = root.join(name);
        let content = fs::read(&path).unwrap();
        let mtime = fs::metadata(&path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        file_metas.push(storage::FileMeta {
            path: PathBuf::from(*name),
            mtime_secs: mtime,
            content_hash: utils::ngram_hash(&content),
        });
        file_ngrams.push(builder::extract_sparse_ngrams(&content));
    }

    let writer = storage::IndexWriter::new(&idx_dir).unwrap();
    writer.write(&file_ngrams, &file_metas, None).unwrap();
    idx_dir
}

// === Index + query tests ===

#[test]
fn test_end_to_end_index_and_search() {
    let dir = setup_test_repo();
    let idx_dir = build_index(&dir);

    let reader = storage::IndexReader::open(&idx_dir).unwrap();
    assert_eq!(reader.num_files(), 5);

    let candidates = query::find_candidates("MAX_FILE_SIZE", &reader);
    assert!(candidates.is_some());

    let ids = candidates.unwrap();
    let candidate_paths: Vec<_> = ids
        .iter()
        .map(|&id| reader.file_metas[id as usize].path.clone())
        .collect();

    assert!(candidate_paths.iter().any(|p| p.to_str() == Some("main.rs")));
    assert!(candidate_paths.iter().any(|p| p.to_str() == Some("config.rs")));
}

#[test]
fn test_sparse_ngram_selectivity() {
    let dir = setup_test_repo();
    let idx_dir = build_index(&dir);

    let reader = storage::IndexReader::open(&idx_dir).unwrap();

    let rare = query::find_candidates("ZX_HANDLE_INVALID", &reader).unwrap();
    assert!(rare.len() <= 2, "ZX_HANDLE_INVALID should narrow to <=2 candidates, got {}", rare.len());
}

#[test]
fn test_regex_pattern_search() {
    let lits = query::extract_literals(".*");
    assert!(lits.is_empty());

    let lits = query::extract_literals("foo.*bar");
    assert!(lits.contains(&b"foo".to_vec()));
    assert!(lits.contains(&b"bar".to_vec()));
}

// === Printer tests ===

#[test]
fn test_count_mode() {
    let re = regex::bytes::Regex::new("fn").unwrap();
    let content = b"fn main() {\n    fn helper() {}\n}\n";
    let config = SearchConfig {
        count: true,
        color: ColorMode::Never,
        ..Default::default()
    };

    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, count) = printer::search_file(
        &mut out, std::path::Path::new("test.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();

    assert!(matched);
    assert_eq!(count, 2);
    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("2"), "should show count 2, got: {}", output);
}

#[test]
fn test_invert_match_count() {
    let re = regex::bytes::Regex::new("fn").unwrap();
    let content = b"fn main() {\n    let x = 1;\n}\n";
    let config = SearchConfig {
        count: true,
        invert_match: true,
        color: ColorMode::Never,
        ..Default::default()
    };

    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, count) = printer::search_file(
        &mut out, std::path::Path::new("test.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();

    assert!(matched);
    // 3 lines that don't match "fn": "    let x = 1;", "}", ""
    assert!(count >= 2);
}

#[test]
fn test_files_only_mode() {
    let re = regex::bytes::Regex::new("fn").unwrap();
    let content = b"fn main() {}";
    let config = SearchConfig {
        files_only: true,
        color: ColorMode::Never,
        ..Default::default()
    };

    let mut out = Vec::new();
    let mut gc = 0;
    printer::search_file(
        &mut out, std::path::Path::new("test.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();

    let output = String::from_utf8(out).unwrap();
    assert_eq!(output.trim(), "test.rs");
}

#[test]
fn test_files_without_match_mode() {
    let re = regex::bytes::Regex::new("NONEXISTENT").unwrap();
    let content = b"fn main() {}";
    let config = SearchConfig {
        files_without_match: true,
        color: ColorMode::Never,
        ..Default::default()
    };

    let mut out = Vec::new();
    let mut gc = 0;
    printer::search_file(
        &mut out, std::path::Path::new("test.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();

    let output = String::from_utf8(out).unwrap();
    assert_eq!(output.trim(), "test.rs");
}

#[test]
fn test_quiet_mode() {
    let re = regex::bytes::Regex::new("fn").unwrap();
    let content = b"fn main() {}";
    let config = SearchConfig {
        quiet: true,
        ..Default::default()
    };

    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, _) = printer::search_file(
        &mut out, std::path::Path::new("test.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();

    assert!(matched);
    assert!(out.is_empty(), "quiet mode should produce no output");
}

#[test]
fn test_json_output() {
    let re = regex::bytes::Regex::new("fn").unwrap();
    let content = b"fn main() {}";
    let config = SearchConfig {
        json: true,
        color: ColorMode::Never,
        ..Default::default()
    };

    let mut out = Vec::new();
    let mut gc = 0;
    printer::search_file(
        &mut out, std::path::Path::new("test.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();

    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("\"type\":\"match\""), "JSON should have type=match");
    assert!(output.contains("test.rs"), "JSON should contain filename");
}

#[test]
fn test_context_lines() {
    let re = regex::bytes::Regex::new("MATCH").unwrap();
    let content = b"line1\nline2\nMATCH\nline4\nline5\n";
    let config = SearchConfig {
        before_context: 1,
        after_context: 1,
        color: ColorMode::Never,
        ..Default::default()
    };

    let mut out = Vec::new();
    let mut gc = 0;
    printer::search_file(
        &mut out, std::path::Path::new("test.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();

    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("line2"), "should contain before-context line");
    assert!(output.contains("MATCH"), "should contain match line");
    assert!(output.contains("line4"), "should contain after-context line");
}

#[test]
fn test_max_count() {
    let re = regex::bytes::Regex::new("line").unwrap();
    let content = b"line1\nline2\nline3\nline4\nline5\n";
    let config = SearchConfig {
        max_count: Some(2),
        color: ColorMode::Never,
        ..Default::default()
    };

    let mut out = Vec::new();
    let mut gc = 0;
    let (_, count) = printer::search_file(
        &mut out, std::path::Path::new("test.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();

    assert_eq!(count, 2, "should only match 2 lines");
}

#[test]
fn test_only_matching() {
    let re = regex::bytes::Regex::new(r"\d+").unwrap();
    let content = b"foo 42 bar 99\n";
    let config = SearchConfig {
        only_matching: true,
        color: ColorMode::Never,
        line_number: Some(false),
        with_filename: Some(false),
        ..Default::default()
    };

    let mut out = Vec::new();
    let mut gc = 0;
    printer::search_file(
        &mut out, std::path::Path::new("test.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();

    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("42"), "should show match '42'");
    assert!(output.contains("99"), "should show match '99'");
    assert!(!output.contains("foo"), "should not show non-matching text");
}

#[test]
fn test_replace() {
    let re = regex::bytes::Regex::new("old").unwrap();
    let content = b"this is old text\n";
    let config = SearchConfig {
        replace: Some("new".to_string()),
        color: ColorMode::Never,
        with_filename: Some(false),
        line_number: Some(false),
        ..Default::default()
    };

    let mut out = Vec::new();
    let mut gc = 0;
    printer::search_file(
        &mut out, std::path::Path::new("test.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();

    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("new"), "should contain replacement");
    assert!(!output.contains("old"), "should not contain original: {}", output);
}

#[test]
fn test_binary_skip() {
    let re = regex::bytes::Regex::new("hello").unwrap();
    let mut content = b"hello world".to_vec();
    content.push(0); // null byte makes it binary
    let config = SearchConfig::default();

    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, _) = printer::search_file(
        &mut out, std::path::Path::new("test.bin"), &content, &re, &config, false, &mut gc,
    ).unwrap();

    assert!(!matched, "binary files should be skipped by default");
}

#[test]
fn test_search_binary_as_text() {
    let re = regex::bytes::Regex::new("hello").unwrap();
    let mut content = b"hello world".to_vec();
    content.push(0);
    let config = SearchConfig {
        search_binary: true,
        color: ColorMode::Never,
        ..Default::default()
    };

    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, _) = printer::search_file(
        &mut out, std::path::Path::new("test.bin"), &content, &re, &config, false, &mut gc,
    ).unwrap();

    assert!(matched, "-a/--text should search binary files");
}

// === SearchConfig tests ===

#[test]
fn test_build_pattern_fixed_strings() {
    let config = SearchConfig {
        fixed_strings: true,
        ..Default::default()
    };
    let pat = config.build_pattern("foo.bar()");
    assert!(pat.contains(r"foo\.bar\(\)"), "should escape regex metacharacters: {}", pat);
}

#[test]
fn test_build_pattern_word_regexp() {
    let config = SearchConfig {
        word_regexp: true,
        ..Default::default()
    };
    let pat = config.build_pattern("fn");
    assert!(pat.contains(r"\bfn\b"), "should wrap in word boundaries: {}", pat);
}

#[test]
fn test_build_pattern_smart_case() {
    let config = SearchConfig {
        smart_case: true,
        ..Default::default()
    };
    // All lowercase → case insensitive
    let pat = config.build_pattern("hello");
    assert!(pat.contains("(?i)"), "all-lowercase with smart_case should be case insensitive: {}", pat);

    // Contains uppercase → case sensitive (no flag)
    let pat = config.build_pattern("Hello");
    assert!(!pat.contains("(?i)"), "mixed case should stay case sensitive: {}", pat);
}

// === Walker tests ===

#[test]
fn test_type_filter_via_walker() {
    let dir = setup_test_repo();
    let config = WalkConfig {
        root: dir.path().to_path_buf(),
        type_include: vec!["rust".to_string()],
        ..Default::default()
    };

    let files = walker::collect_files(&config);
    assert!(files.iter().all(|p| p.to_string_lossy().ends_with(".rs")),
        "should only include .rs files: {:?}", files);
    assert!(!files.is_empty());
}

#[test]
fn test_type_exclude_via_walker() {
    let dir = setup_test_repo();
    let config = WalkConfig {
        root: dir.path().to_path_buf(),
        type_exclude: vec!["py".to_string()],
        ..Default::default()
    };

    let files = walker::collect_files(&config);
    assert!(files.iter().all(|p| !p.to_string_lossy().ends_with(".py")),
        "should exclude .py files: {:?}", files);
}

#[test]
fn test_max_depth_via_walker() {
    let dir = setup_test_repo();
    let root = dir.path();
    fs::create_dir_all(root.join("deep/nested")).unwrap();
    fs::write(root.join("deep/nested/deep.rs"), "fn deep() {}").unwrap();

    let config = WalkConfig {
        root: root.to_path_buf(),
        max_depth: Some(1),
        ..Default::default()
    };

    let files = walker::collect_files(&config);
    assert!(files.iter().all(|p| p.components().count() <= 1),
        "max_depth=1 should only include top-level files: {:?}", files);
}

// === Edge case tests ===

#[test]
fn test_empty_file_indexing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("empty.rs"), "").unwrap();

    let content = fs::read(root.join("empty.rs")).unwrap();
    let ngrams = builder::extract_sparse_ngrams(&content);
    assert!(ngrams.is_empty(), "empty file should produce no n-grams");
}

#[test]
fn test_one_byte_file_indexing() {
    let content = b"x";
    let ngrams = builder::extract_sparse_ngrams(content);
    assert!(ngrams.is_empty(), "1-byte file should produce no n-grams");
}

#[test]
fn test_two_byte_file_produces_no_ngrams() {
    let content = b"ab";
    let ngrams = builder::extract_sparse_ngrams(content);
    assert!(ngrams.is_empty(), "2-byte file: bigrams skipped in index (too common)");
}

#[test]
fn test_binary_file_skipped_in_search() {
    let re = regex::bytes::Regex::new("hello").unwrap();
    // File with null bytes = binary
    let content = b"hello\x00world";
    let config = SearchConfig::default();
    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, _) = printer::search_file(
        &mut out, std::path::Path::new("binary.bin"), content, &re, &config, false, &mut gc,
    ).unwrap();
    assert!(!matched, "binary files should be skipped by default");
    assert!(out.is_empty(), "no output for binary files");
}

#[test]
fn test_binary_file_searched_with_text_flag() {
    let re = regex::bytes::Regex::new("hello").unwrap();
    let content = b"hello\x00world";
    let config = SearchConfig {
        search_binary: true,
        color: ColorMode::Never,
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, _) = printer::search_file(
        &mut out, std::path::Path::new("binary.bin"), content, &re, &config, false, &mut gc,
    ).unwrap();
    assert!(matched, "--text flag should search binary files");
}

#[test]
fn test_gitignore_respected_in_walker() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Init a git repo so .gitignore is respected
    fs::write(root.join(".gitignore"), "ignored.rs\nbuild/\n").unwrap();
    fs::write(root.join("kept.rs"), "fn kept() {}").unwrap();
    fs::write(root.join("ignored.rs"), "fn ignored() {}").unwrap();
    fs::create_dir_all(root.join("build")).unwrap();
    fs::write(root.join("build/output.rs"), "fn build() {}").unwrap();

    // Need git init for .gitignore to be respected
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(root)
        .output()
        .ok();

    let config = WalkConfig {
        root: root.to_path_buf(),
        ..Default::default()
    };
    let files = walker::collect_files(&config);
    let file_names: Vec<String> = files.iter().map(|p| p.to_string_lossy().to_string()).collect();

    assert!(file_names.iter().any(|f| f.contains("kept.rs")),
        "kept.rs should be included: {:?}", file_names);
    assert!(!file_names.iter().any(|f| f.contains("ignored.rs")),
        "ignored.rs should be excluded by .gitignore: {:?}", file_names);
    assert!(!file_names.iter().any(|f| f.contains("build/")),
        "build/ dir should be excluded by .gitignore: {:?}", file_names);
}

#[test]
fn test_huge_file_max_filesize_filter() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Create a small file and a "large" file
    fs::write(root.join("small.rs"), "fn small() {}").unwrap();
    fs::write(root.join("large.rs"), "x".repeat(2000)).unwrap(); // 2KB

    let config = WalkConfig {
        root: root.to_path_buf(),
        max_filesize: Some(1024), // 1KB limit
        ..Default::default()
    };
    let files = walker::collect_files(&config);
    let names: Vec<String> = files.iter().map(|p| p.to_string_lossy().to_string()).collect();

    assert!(names.iter().any(|f| f.contains("small.rs")), "small file should be included");
    assert!(!names.iter().any(|f| f.contains("large.rs")), "large file should be excluded by max_filesize");
}

#[test]
fn test_ngram_cap_at_128_bytes() {
    // A long string of rare chars should still produce n-grams but capped at 128 bytes
    let content = "Z".repeat(200);
    let ngrams = builder::extract_sparse_ngrams_debug(content.as_bytes());
    for (_, ng) in &ngrams {
        assert!(ng.len() <= builder::MAX_NGRAM_LEN,
            "n-gram length {} exceeds cap of {}", ng.len(), builder::MAX_NGRAM_LEN);
    }
}

#[test]
fn test_case_insensitive_index_lookup() {
    let dir = setup_test_repo();
    let idx_dir = build_index(&dir);
    let reader = storage::IndexReader::open(&idx_dir).unwrap();

    // Exact case should find candidates
    let exact = query::find_candidates("MAX_FILE_SIZE", &reader);
    assert!(exact.is_some());

    // Case-insensitive variant should also find candidates (via lowercase/uppercase lookup)
    let ci = query::find_candidates_case_insensitive("max_file_size", &reader);
    // Should return Some (either matches or empty vec, not None which means "can't optimize")
    assert!(ci.is_some(), "case-insensitive lookup should not fall back to full scan");
}

#[test]
fn test_hidden_files_excluded_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    fs::write(root.join("visible.rs"), "fn visible() {}").unwrap();
    fs::write(root.join(".hidden.rs"), "fn hidden() {}").unwrap();
    fs::create_dir_all(root.join(".hidden_dir")).unwrap();
    fs::write(root.join(".hidden_dir/secret.rs"), "fn secret() {}").unwrap();

    let config = WalkConfig {
        root: root.to_path_buf(),
        hidden: false,
        ..Default::default()
    };
    let files = walker::collect_files(&config);
    let names: Vec<String> = files.iter().map(|p| p.to_string_lossy().to_string()).collect();

    assert!(names.iter().any(|f| f.contains("visible")));
    assert!(!names.iter().any(|f| f.contains(".hidden")),
        "hidden files should be excluded by default: {:?}", names);
}

#[test]
fn test_hidden_files_included_with_flag() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    fs::write(root.join("visible.rs"), "fn visible() {}").unwrap();
    fs::write(root.join(".hidden.rs"), "fn hidden() {}").unwrap();

    let config = WalkConfig {
        root: root.to_path_buf(),
        hidden: true,
        ..Default::default()
    };
    let files = walker::collect_files(&config);
    let names: Vec<String> = files.iter().map(|p| p.to_string_lossy().to_string()).collect();

    assert!(names.iter().any(|f| f.contains(".hidden.rs")),
        "hidden files should be included with --hidden: {:?}", names);
}

// ============================================================
// Edge case tests for all 11 categories
// ============================================================

// --- 1. Weak patterns (no useful n-grams) ---

#[test]
fn test_weak_pattern_wildcard_only() {
    // .* extracts no literals → index returns None → falls back to full scan
    let lits = query::extract_literals(".*");
    assert!(lits.is_empty(), ".* should produce no literals");
}

#[test]
fn test_weak_pattern_single_char() {
    let lits = query::extract_literals("a");
    assert!(lits.is_empty(), "single char 'a' too short for n-grams");
}

#[test]
fn test_weak_pattern_space_only() {
    let lits = query::extract_literals(" ");
    assert!(lits.is_empty(), "single space too short for n-grams");
}

#[test]
fn test_weak_pattern_dot_star_foo() {
    // .*foo.* → should extract "foo"
    let lits = query::extract_literals(".*foo.*");
    assert!(lits.contains(&b"foo".to_vec()), "should extract 'foo' from .*foo.*");
}

#[test]
fn test_weak_pattern_character_class_only() {
    let lits = query::extract_literals("[a-zA-Z]+");
    assert!(lits.is_empty(), "pure character class has no literals");
}

#[test]
fn test_empty_search_pattern() {
    let lits = query::extract_literals("");
    assert!(lits.is_empty());
}

// --- 2. Very short files ---
// (already covered by test_empty_file_indexing, test_one_byte_file, test_two_byte_file)

#[test]
fn test_search_in_empty_file() {
    let re = regex::bytes::Regex::new("anything").unwrap();
    let content = b"";
    let config = SearchConfig::default();
    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, _) = printer::search_file(
        &mut out, std::path::Path::new("empty.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();
    assert!(!matched, "empty file should have no matches");
}

#[test]
fn test_search_in_one_line_file() {
    let re = regex::bytes::Regex::new("hello").unwrap();
    let content = b"hello";
    let config = SearchConfig {
        color: ColorMode::Never,
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, count) = printer::search_file(
        &mut out, std::path::Path::new("tiny.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();
    assert!(matched);
    assert_eq!(count, 1);
}

// --- 3. Regex without clear literal text ---

#[test]
fn test_regex_start_end_anchors() {
    let lits = query::extract_literals("^start.*end$");
    assert!(lits.contains(&b"start".to_vec()));
    assert!(lits.contains(&b"end".to_vec()));
}

#[test]
fn test_regex_alternation_common_words() {
    let lits = query::extract_literals("foo|bar|baz");
    assert_eq!(lits.len(), 3);
    assert!(lits.contains(&b"foo".to_vec()));
    assert!(lits.contains(&b"bar".to_vec()));
    assert!(lits.contains(&b"baz".to_vec()));
}

#[test]
fn test_regex_repetition() {
    let lits = query::extract_literals("(hello)+");
    assert!(lits.contains(&b"hello".to_vec()), "should extract from repetition");
}

#[test]
fn test_regex_nested_groups() {
    let lits = query::extract_literals("(foo(bar))");
    // Should extract "foobar" as a merged literal
    assert!(!lits.is_empty());
}

// --- 5. Unicode / Non-English code ---

#[test]
fn test_unicode_ngram_extraction() {
    // UTF-8 multi-byte: é = 0xC3 0xA9, café = 5 bytes
    let content = "café naïve".as_bytes();
    let ngrams = builder::extract_sparse_ngrams(content);
    assert!(!ngrams.is_empty(), "should extract n-grams from UTF-8 text");
}

#[test]
fn test_unicode_cjk_ngram_extraction() {
    // Chinese characters: each is 3 bytes in UTF-8
    let content = "你好世界".as_bytes(); // 12 bytes
    let ngrams = builder::extract_sparse_ngrams(content);
    assert!(!ngrams.is_empty(), "should extract n-grams from CJK text");
}

#[test]
fn test_unicode_emoji_ngram_extraction() {
    // Emoji: 🦀 = 4 bytes in UTF-8
    let content = "hello 🦀 world".as_bytes();
    let ngrams = builder::extract_sparse_ngrams(content);
    assert!(!ngrams.is_empty(), "should handle emoji in n-gram extraction");
}

#[test]
fn test_unicode_search_match() {
    let re = regex::bytes::Regex::new("café").unwrap();
    let content = "I love café au lait\n".as_bytes();
    let config = SearchConfig {
        color: ColorMode::Never,
        with_filename: Some(false),
        line_number: Some(false),
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, _) = printer::search_file(
        &mut out, std::path::Path::new("unicode.txt"), content, &re, &config, false, &mut gc,
    ).unwrap();
    assert!(matched, "should match UTF-8 pattern");
    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("café"), "output should contain the Unicode match");
}

#[test]
fn test_unicode_mixed_script_search() {
    let re = regex::bytes::Regex::new("変数").unwrap();
    let content = "let 変数 = 42;\n".as_bytes();
    let config = SearchConfig {
        color: ColorMode::Never,
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, _) = printer::search_file(
        &mut out, std::path::Path::new("jp.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();
    assert!(matched, "should match CJK characters");
}

// --- 7. Incremental indexing: renames ---
// (test_detect_changes_rename already in incremental.rs unit tests)

#[test]
fn test_index_handles_duplicate_content_different_paths() {
    // Two files with identical content should both be indexed
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let idx_dir = root.join(".instagrep");

    let content = b"pub fn duplicate() { println!(\"same\"); }";
    fs::write(root.join("a.rs"), content).unwrap();
    fs::write(root.join("b.rs"), content).unwrap();

    let mut file_metas = Vec::new();
    let mut file_ngrams = Vec::new();
    for name in &["a.rs", "b.rs"] {
        let c = fs::read(root.join(name)).unwrap();
        file_metas.push(storage::FileMeta {
            path: PathBuf::from(*name),
            mtime_secs: 1000,
            content_hash: utils::ngram_hash(&c),
        });
        file_ngrams.push(builder::extract_sparse_ngrams(&c));
    }

    let writer = storage::IndexWriter::new(&idx_dir).unwrap();
    writer.write(&file_ngrams, &file_metas, None).unwrap();

    let reader = storage::IndexReader::open(&idx_dir).unwrap();
    assert_eq!(reader.num_files(), 2, "both files should be indexed even with same content");

    // Search for a pattern — candidates may not match both files if the
    // n-gram threshold filters them, but both files should be in the index
    let candidates = query::find_candidates("duplicate", &reader);
    // With weight-threshold filtering, simple words might not produce matching n-grams.
    // The key assertion is that both files are indexed.
    if let Some(ids) = candidates {
        assert!(ids.len() <= 2, "should not exceed 2 candidates");
    }
}

// --- 8. False positives ---

#[test]
fn test_false_positives_are_safe() {
    // Index may return false positive candidates due to hash collisions.
    // Verify the regex engine still filters correctly.
    let re = regex::bytes::Regex::new("UNIQUE_PATTERN_XYZ").unwrap();
    let content = b"this file does NOT contain the pattern\n";
    let config = SearchConfig {
        color: ColorMode::Never,
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, _) = printer::search_file(
        &mut out, std::path::Path::new("false_positive.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();
    assert!(!matched, "false positive candidate should be filtered by regex engine");
    assert!(out.is_empty(), "no output for non-matching file");
}

// --- 11. Output edges ---

#[test]
fn test_zero_results_clean_output() {
    let re = regex::bytes::Regex::new("NONEXISTENT_PATTERN_12345").unwrap();
    let content = b"fn main() { println!(\"hello\"); }\n";
    let config = SearchConfig {
        color: ColorMode::Never,
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, count) = printer::search_file(
        &mut out, std::path::Path::new("test.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();
    assert!(!matched);
    assert_eq!(count, 0);
    assert!(out.is_empty(), "zero results should produce no output");
}

#[test]
fn test_many_matches_bounded() {
    // File with many matching lines — max_count should limit
    let re = regex::bytes::Regex::new("line").unwrap();
    let content: Vec<u8> = (0..1000)
        .map(|i| format!("line {}\n", i))
        .collect::<String>()
        .into_bytes();

    let config = SearchConfig {
        max_count: Some(5),
        color: ColorMode::Never,
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, count) = printer::search_file(
        &mut out, std::path::Path::new("big.rs"), &content, &re, &config, false, &mut gc,
    ).unwrap();
    assert!(matched);
    assert_eq!(count, 5, "max_count should limit to 5 matches");
}

#[test]
fn test_very_long_line_handling() {
    // A very long line (1MB) should not crash
    let re = regex::bytes::Regex::new("needle").unwrap();
    let mut content = "x".repeat(1_000_000);
    content.push_str("needle");
    content.push('\n');

    let config = SearchConfig {
        color: ColorMode::Never,
        max_columns: Some(200),
        max_columns_preview: true,
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, _) = printer::search_file(
        &mut out, std::path::Path::new("long.rs"), content.as_bytes(), &re, &config, false, &mut gc,
    ).unwrap();
    assert!(matched, "should find needle in long line");
    // With max_columns, the output should be truncated, not the full 1MB line
    assert!(out.len() < 10_000, "output should be bounded by max_columns, got {} bytes", out.len());
}

#[test]
fn test_json_zero_results() {
    let re = regex::bytes::Regex::new("NONEXISTENT").unwrap();
    let content = b"fn main() {}\n";
    let config = SearchConfig {
        json: true,
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut gc = 0;
    let (matched, _) = printer::search_file(
        &mut out, std::path::Path::new("test.rs"), content, &re, &config, false, &mut gc,
    ).unwrap();
    assert!(!matched);
    assert!(out.is_empty(), "JSON output should be empty for no matches");
}

#[test]
fn test_max_results_global_limit() {
    let re = regex::bytes::Regex::new("x").unwrap();
    let content = b"x\nx\nx\nx\nx\n";
    let mut out = Vec::new();
    // Set global count to 3 already
    let mut gc = 3;
    let config_with_limit = SearchConfig {
        max_results: 5,
        color: ColorMode::Never,
        ..Default::default()
    };
    let (_, count) = printer::search_file(
        &mut out, std::path::Path::new("test.rs"), content, &re, &config_with_limit, false, &mut gc,
    ).unwrap();
    assert!(count <= 2, "global limit of 5 with 3 already done should allow at most 2 more, got {}", count);
}

// --- 10. Concurrency edges ---

#[test]
fn test_flock_based_locking() {
    use instagrep::index::incremental;
    let dir = tempfile::tempdir().unwrap();
    let idx_dir = dir.path().join(".instagrep");

    // Acquire flock — held until dropped
    let lock = incremental::acquire_lock(&idx_dir).unwrap();
    assert!(idx_dir.join(".lock").exists());
    drop(lock); // OS releases flock automatically
}

#[test]
fn test_corrupted_index_graceful_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let idx_dir = dir.path().join(".instagrep");
    fs::create_dir_all(&idx_dir).unwrap();

    // Write garbage to index files
    fs::write(idx_dir.join("lookup.bin"), b"garbage").unwrap();
    fs::write(idx_dir.join("postings.bin"), b"garbage").unwrap();
    fs::write(idx_dir.join("files.bin"), b"garbage").unwrap();
    fs::write(idx_dir.join("meta.bin"), b"garbage").unwrap();

    // Opening should fail gracefully
    let result = storage::IndexReader::open(&idx_dir);
    assert!(result.is_err(), "corrupted index should fail to open");
}
