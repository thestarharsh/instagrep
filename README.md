# instagrep — Instant Grep

**Blazing-fast regex search via sparse n-gram inverted index.**

Open-source reimplementation of Cursor's [Fast Regex Search](https://cursor.com/blog/fast-regex-search) system. Builds a disk-backed sparse n-gram index for massive codebases, then prunes to a tiny set of candidate files before running the real regex engine.

ripgrep-compatible CLI with 50+ flags. Built-in MCP server for AI coding tools. Works on Linux, macOS, and Windows.

> This is the open-source, community version of Cursor's Instant Grep — fully reusable by any AI coding tool.

## Install

```bash
# CLI only
cargo install --git https://github.com/thestarharsh/instagrep

# CLI + MCP server (for AI tool integration)
cargo install --git https://github.com/thestarharsh/instagrep --features mcp

# Auto-configure all detected AI tools (Claude Code, Cursor, Cline, Windsurf)
instagrep setup
```

That's it. Two commands. No config files to edit, no paths to set, no manual indexing.

## Quick Start

```bash
# Search with any regex (auto-indexes on first run)
instagrep search "MAX_FILE_SIZE"
instagrep search "fn\s+\w+_handler"
instagrep search "TODO|FIXME|HACK"

# Or build the index explicitly
instagrep index .
```

## How It Works

```
 "ZX_HANDLE"
      │
      ▼
 Parse regex → extract literal substrings
      │
      ▼
 Find sparse n-grams that cover those literals
      │
      ▼
 Binary search mmap'd lookup table (microseconds)
      │
      ▼
 Intersect posting lists → 2 candidate files (out of 100K)
      │
      ▼
 Run real regex ONLY on those 2 files
      │
      ▼
 Results in <1ms
```

**Sparse n-grams** are variable-length substrings selected at positions where character transitions are rare in source code. Unlike plain trigrams, they're highly discriminative — a rare n-gram like `ZX_H` might appear in only 1 file out of 100,000.

The index is **always safe**: it can produce false positives (extra candidates) but **never false negatives**. The regex engine always confirms matches.

### Index Storage

Indexes are stored centrally — **no files in your project directory**:

```
~/.instagrep/indexes/
  ├── a3f8b2c1-myproject/     ← one folder per project
  │   ├── postings.bin        ← concatenated posting lists (file IDs)
  │   ├── lookup.bin          ← sorted (ngram_hash → offset), mmap'd
  │   ├── files.bin           ← file metadata (path, mtime, content hash)
  │   └── meta.bin            ← index metadata (git commit, timestamp)
  └── bb7da4a4-another-repo/
```

No `.gitignore` needed. Override location with `INSTAGREP_CACHE_DIR` env var.

### Performance

| Metric | `grep` | `rg` (ripgrep) | `instagrep` CLI | `instagrep` MCP |
|--------|--------|----------------|-----------------|-----------------|
| 17 files | 11ms | 13ms | ~10ms | **1ms** |
| 10K files | ~1s | ~200ms | ~5ms | ~3ms |
| 100K files | ~10s | ~2s | ~10ms | ~5ms |
| 1M+ files | minutes | 10-15s | <100ms | <50ms |

MCP server is fastest because the index stays mmap'd in memory between calls — zero startup overhead.

## MCP Server (AI Agent Integration)

instagrep includes a built-in MCP server. Any AI coding tool that supports MCP can use it natively — no shell commands, no JSON parsing, just structured tool calls.

### Setup

```bash
# Install with MCP support
cargo install --git https://github.com/thestarharsh/instagrep --features mcp

# Auto-configure all detected AI tools
instagrep setup
```

`instagrep setup` auto-detects installed tools and writes the MCP config globally. One command, works in every project forever. No `.mcp.json` per project.

To configure a specific tool only:
```bash
instagrep setup claude    # Claude Code only
instagrep setup cursor    # Cursor only
instagrep setup --remove  # Remove from all tools
```

### What Happens Automatically

1. You open a project in Claude Code / Cursor / Cline / Windsurf
2. The tool spawns `instagrep-mcp` in the background
3. **First search**: auto-builds the index (parallel, seconds)
4. **Subsequent searches**: index is mmap'd in memory, microsecond lookups
5. **Git commit changes**: auto-detects stale index, rebuilds incrementally
6. **Switch projects**: new server instance, auto-indexes

You never run `instagrep index`, never pass `--path`, never think about staleness.

### MCP Tools

| Tool | Description |
|------|-------------|
| `search` | Fast indexed regex search. Returns file, line, text, optional context. |
| `index` | Build/update search index. Incremental by default. |
| `status` | Check index health: file count, staleness, disk size. |

### Search Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `pattern` | string | required | Regex pattern |
| `file_type` | string | all | e.g. `"rust"`, `"py"`, `"js"` |
| `glob` | string | none | e.g. `"*.rs"`, `"!*.min.js"` |
| `ignore_case` | bool | false | Case-insensitive search |
| `fixed_strings` | bool | false | Treat pattern as literal |
| `word_regexp` | bool | false | Match whole words only |
| `max_results` | int | 200 | Max matches returned |
| `context_lines` | int | 0 | Lines of context before/after |
| `include_hidden` | bool | false | Include hidden files |

### Example Response

```json
{
  "matches": [
    {
      "file": "src/lib.rs",
      "line": 42,
      "text": "pub unsafe fn allocate(size: usize) -> *mut u8 {",
      "context_before": ["/// Allocates raw memory."],
      "context_after": ["    let layout = Layout::from_size_align(size, 8).unwrap();"]
    }
  ],
  "stats": {
    "total_files": 50000,
    "candidates": 12,
    "files_searched": 12,
    "matches_found": 1,
    "elapsed_ms": 3
  }
}
```

## CLI Reference

### Commands

| Command | Description |
|---------|-------------|
| `instagrep search [FLAGS] PATTERN` | Search with regex |
| `instagrep index [PATH]` | Build/update index |
| `instagrep status [PATH]` | Show index health |
| `instagrep clear [PATH]` | Reset index |
| `instagrep setup [TOOL]` | Configure MCP for AI tools |
| `instagrep type-list` | List all built-in file types |

### Search Flags (ripgrep-compatible)

#### Search Behavior
| Flag | Description |
|------|-------------|
| `-i, --ignore-case` | Case-insensitive search |
| `-s, --case-sensitive` | Case-sensitive (default) |
| `-S, --smart-case` | Case-insensitive if pattern is all lowercase |
| `-F, --fixed-strings` | Treat pattern as literal, not regex |
| `-w, --word-regexp` | Match whole words only |
| `-x, --line-regexp` | Match entire lines only |
| `-v, --invert-match` | Show non-matching lines |
| `-U, --multiline` | Enable multiline matching |
| `-e, --regexp PATTERN` | Specify pattern (repeatable) |
| `-f, --file PATTERNFILE` | Read patterns from file |
| `-m, --max-count NUM` | Limit matches per file |
| `-a, --text` | Search binary files as text |

#### Output Modes
| Flag | Description |
|------|-------------|
| `--json` | JSON Lines output (rg-compatible) |
| `-c, --count` | Count matching lines per file |
| `-l, --files-with-matches` | Print only paths with matches |
| `--files-without-match` | Print only paths without matches |
| `-q, --quiet` | No output; exit code indicates match |
| `-o, --only-matching` | Print only matched parts |
| `--vimgrep` | Vim-compatible output |
| `-r, --replace TEXT` | Replace matches in output |
| `--stats` | Print aggregate statistics |

#### Context Lines
| Flag | Description |
|------|-------------|
| `-A, --after-context NUM` | Lines after each match |
| `-B, --before-context NUM` | Lines before each match |
| `-C, --context NUM` | Lines before and after |

#### File Filtering
| Flag | Description |
|------|-------------|
| `-g, --glob GLOB` | Include/exclude by glob (repeatable; `!` to negate) |
| `-t, --type TYPE` | Only search TYPE files (e.g., `rust`, `py`, `js`) |
| `-T, --type-not TYPE` | Exclude TYPE files |
| `-d, --max-depth NUM` | Max directory depth |
| `--max-filesize SIZE` | Ignore files larger than SIZE (e.g., `10M`) |
| `--hidden` | Include hidden files/dirs |
| `-L, --follow` | Follow symbolic links |
| `--no-ignore` | Don't respect .gitignore |

#### Output Formatting
| Flag | Description |
|------|-------------|
| `--color WHEN` | `never`, `auto`, `always` |
| `--heading / --no-heading` | Group matches under filename |
| `-n / -N` | Show / hide line numbers |
| `--column` | Show column number |
| `-b, --byte-offset` | Show byte offset |
| `-H / -I` | Show / hide filename |
| `--trim` | Strip leading whitespace |
| `-p, --pretty` | Alias for `--color always --heading -n` |
| `--sort SORTBY` | Sort by `path`, `modified`, `accessed`, `created` |
| `-j, --threads NUM` | Number of threads |

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Match found |
| 1 | No matches |
| 2 | Error |

## Usage Examples

```bash
# Context lines
instagrep search -A5 -B2 "TODO"

# Word boundary + type filter
instagrep search -w "unsafe" -t rust

# Count matches
instagrep search -c "import"

# Fixed string (not regex)
instagrep search -F "fn main()"

# Case-insensitive
instagrep search -i "readme"

# Multiple patterns
instagrep search -e "TODO" -e "FIXME" -e "HACK"

# Glob filtering
instagrep search -g "*.rs" -g "!*test*" "fn"

# Vim integration
instagrep search --vimgrep "pattern"

# Quiet mode (just check exit code)
instagrep search -q "pattern" && echo "found" || echo "not found"
```

## Architecture

```
src/
├── main.rs               CLI binary (clap, 50+ flags, setup command)
├── bin/
│   └── instagrep_mcp.rs  MCP server binary (rmcp, tokio, auto-index)
├── lib.rs                Library root + central index path resolution
├── printer.rs            Output engine (color, context, JSON, count, etc.)
├── walker.rs             File filtering (types, globs, depth, gitignore)
├── types.rs              Built-in file type definitions (~100 types)
├── utils.rs              Bigram weights, hashing, binary detection
└── index/
    ├── builder.rs        Sparse n-gram extraction (build_all)
    ├── query.rs          Regex → n-gram decomposition (build_covering)
    ├── storage.rs        Postings + mmap lookup table
    └── incremental.rs    Git-aware change detection, lock files
```

## Development

```bash
cargo test                           # 104 tests
cargo build --release                # CLI only
cargo build --release --features mcp # CLI + MCP server
```

## License

MIT
