//! Index storage: postings file + mmap-able lookup table.
//!
//! On-disk layout (v2 — varint-compressed posting lists):
//!   ~/.instagrep/indexes/<hash>/
//!     postings.bin  — delta-encoded, varint-compressed posting lists
//!     lookup.bin    — sorted array of (ngram_hash: u64, offset: u64, byte_len: u32)
//!     files.bin     — serialized file metadata (path, mtime, content hash)
//!     meta.bin      — index metadata (git commit, timestamp, version)

use anyhow::{Context, Result};
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

const INDEX_VERSION: u32 = 2; // v2: varint-compressed posting lists
const LOOKUP_ENTRY_SIZE: usize = 20; // 8 (hash) + 8 (offset) + 4 (byte_len)

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileMeta {
    pub path: PathBuf,
    pub mtime_secs: i64,
    pub content_hash: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct IndexMeta {
    pub version: u32,
    pub git_commit: Option<String>,
    pub num_files: u32,
    pub timestamp: u64,
}

// ── Varint encoding/decoding ──────────────────────────────────────────

/// Encode a u32 as a variable-length integer (1-5 bytes).
/// Uses 7 bits of payload per byte, high bit = continuation.
fn encode_varint(mut val: u32, buf: &mut Vec<u8>) {
    while val >= 0x80 {
        buf.push((val as u8) | 0x80);
        val >>= 7;
    }
    buf.push(val as u8);
}

/// Decode a varint from a byte slice. Returns (value, bytes_consumed).
/// On malformed input (>5 continuation bytes), logs a warning and returns
/// what was decoded so far — never panics.
fn decode_varint(bytes: &[u8]) -> (u32, usize) {
    let mut result: u32 = 0;
    let mut shift: u32 = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        result |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 {
            return (result, i + 1);
        }
        shift += 7;
        if shift >= 35 {
            log::warn!("Malformed varint at byte {} (corrupt index?)", i);
            return (result, i + 1);
        }
    }
    (result, bytes.len())
}

/// Delta-encode + varint-compress a sorted list of file IDs.
fn compress_posting_list(ids: &[u32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(ids.len()); // typically smaller than input
    let mut prev = 0u32;
    for &id in ids {
        let delta = id - prev;
        encode_varint(delta, &mut buf);
        prev = id;
    }
    buf
}

/// Decompress a delta-encoded varint posting list back to file IDs.
fn decompress_posting_list(bytes: &[u8]) -> Vec<u32> {
    let mut ids = Vec::new();
    let mut pos = 0;
    let mut current = 0u32;
    while pos < bytes.len() {
        let (delta, consumed) = decode_varint(&bytes[pos..]);
        current += delta;
        ids.push(current);
        pos += consumed;
    }
    ids
}

// ── Writer ────────────────────────────────────────────────────────────

pub struct IndexWriter {
    index_dir: PathBuf,
}

impl IndexWriter {
    pub fn new(index_dir: &Path) -> Result<Self> {
        fs::create_dir_all(index_dir)?;
        Ok(Self {
            index_dir: index_dir.to_path_buf(),
        })
    }

    pub fn write(
        &self,
        file_ngrams: &[Vec<u64>],
        file_metas: &[FileMeta],
        git_commit: Option<String>,
    ) -> Result<()> {
        // Build inverted index: ngram_hash -> list of file IDs
        let mut inverted: HashMap<u64, Vec<u32>> = HashMap::new();
        for (file_id, ngrams) in file_ngrams.iter().enumerate() {
            for &hash in ngrams {
                inverted.entry(hash).or_default().push(file_id as u32);
            }
        }

        // Sort and dedup posting lists
        for list in inverted.values_mut() {
            list.sort_unstable();
            list.dedup();
        }

        // Write compressed postings file
        let postings_path = self.index_dir.join("postings.bin.tmp");
        let mut postings_file = File::create(&postings_path)?;
        let mut lookup_entries: Vec<(u64, u64, u32)> = Vec::with_capacity(inverted.len());
        let mut offset: u64 = 0;

        let mut sorted_ngrams: Vec<u64> = inverted.keys().copied().collect();
        sorted_ngrams.sort_unstable();

        for hash in &sorted_ngrams {
            let posting_list = &inverted[hash];
            let compressed = compress_posting_list(posting_list);
            postings_file.write_all(&compressed)?;
            let byte_len = compressed.len() as u32;
            lookup_entries.push((*hash, offset, byte_len));
            offset += byte_len as u64;
        }
        postings_file.flush()?;
        drop(postings_file);

        // Write lookup table
        let lookup_path = self.index_dir.join("lookup.bin.tmp");
        let mut lookup_file = File::create(&lookup_path)?;
        for (hash, off, byte_len) in &lookup_entries {
            lookup_file.write_all(&hash.to_le_bytes())?;
            lookup_file.write_all(&off.to_le_bytes())?;
            lookup_file.write_all(&byte_len.to_le_bytes())?;
        }
        lookup_file.flush()?;
        drop(lookup_file);

        // Write file metadata
        let files_path = self.index_dir.join("files.bin.tmp");
        let files_data = bincode::serialize(file_metas)?;
        fs::write(&files_path, &files_data)?;

        // Write index metadata
        let meta = IndexMeta {
            version: INDEX_VERSION,
            git_commit,
            num_files: file_metas.len() as u32,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        let meta_path = self.index_dir.join("meta.bin.tmp");
        let meta_data = bincode::serialize(&meta)?;
        fs::write(&meta_path, &meta_data)?;

        // Atomic rename
        fs::rename(postings_path, self.index_dir.join("postings.bin"))?;
        fs::rename(lookup_path, self.index_dir.join("lookup.bin"))?;
        fs::rename(files_path, self.index_dir.join("files.bin"))?;
        fs::rename(meta_path, self.index_dir.join("meta.bin"))?;

        Ok(())
    }
}

// ── Reader ────────────────────────────────────────────────────────────

pub struct IndexReader {
    lookup_mmap: Mmap,
    postings_mmap: Mmap,
    pub file_metas: Vec<FileMeta>,
    pub meta: IndexMeta,
    num_entries: usize,
}

impl IndexReader {
    pub fn open(index_dir: &Path) -> Result<Self> {
        let lookup_file = File::open(index_dir.join("lookup.bin"))
            .context("No index found. Run `instagrep index .` first.")?;
        let postings_file = File::open(index_dir.join("postings.bin"))?;
        let files_data = fs::read(index_dir.join("files.bin"))?;
        let meta_data = fs::read(index_dir.join("meta.bin"))?;

        let lookup_mmap = unsafe { Mmap::map(&lookup_file)? };
        let postings_mmap = unsafe { Mmap::map(&postings_file)? };
        let file_metas: Vec<FileMeta> = bincode::deserialize(&files_data)?;
        let meta: IndexMeta = bincode::deserialize(&meta_data)?;
        let num_entries = lookup_mmap.len() / LOOKUP_ENTRY_SIZE;

        Ok(Self {
            lookup_mmap,
            postings_mmap,
            file_metas,
            meta,
            num_entries,
        })
    }

    /// Binary search the lookup table for an ngram hash.
    /// Decompresses the varint-encoded posting list on the fly.
    pub fn lookup(&self, ngram_hash: u64) -> Option<Vec<u32>> {
        if self.num_entries == 0 {
            return None;
        }

        let mut lo = 0usize;
        let mut hi = self.num_entries;

        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let entry_offset = mid * LOOKUP_ENTRY_SIZE;

            // Safe reads — return None on corrupt/truncated index instead of panicking
            let hash = u64::from_le_bytes(
                self.lookup_mmap.get(entry_offset..entry_offset + 8)?
                    .try_into().ok()?,
            );

            match hash.cmp(&ngram_hash) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => {
                    let postings_offset = u64::from_le_bytes(
                        self.lookup_mmap.get(entry_offset + 8..entry_offset + 16)?
                            .try_into().ok()?,
                    ) as usize;
                    let byte_len = u32::from_le_bytes(
                        self.lookup_mmap.get(entry_offset + 16..entry_offset + 20)?
                            .try_into().ok()?,
                    ) as usize;

                    let compressed = self.postings_mmap
                        .get(postings_offset..postings_offset + byte_len)?;
                    return Some(decompress_posting_list(compressed));
                }
            }
        }

        None
    }

    pub fn num_files(&self) -> usize {
        self.file_metas.len()
    }

    pub fn num_ngrams(&self) -> usize {
        self.num_entries
    }
}

pub fn index_exists(index_dir: &Path) -> bool {
    index_dir.join("lookup.bin").exists()
        && index_dir.join("postings.bin").exists()
        && index_dir.join("files.bin").exists()
        && index_dir.join("meta.bin").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_varint_roundtrip() {
        for val in [0, 1, 127, 128, 255, 256, 16383, 16384, u32::MAX] {
            let mut buf = Vec::new();
            encode_varint(val, &mut buf);
            let (decoded, consumed) = decode_varint(&buf);
            assert_eq!(decoded, val, "varint roundtrip failed for {}", val);
            assert_eq!(consumed, buf.len());
        }
    }

    #[test]
    fn test_posting_list_compression_roundtrip() {
        let ids = vec![0, 5, 10, 100, 500, 10000];
        let compressed = compress_posting_list(&ids);
        let decompressed = decompress_posting_list(&compressed);
        assert_eq!(decompressed, ids);
        // Compressed should be much smaller than 6 * 4 = 24 bytes
        assert!(compressed.len() < 24, "compressed={} should be < 24", compressed.len());
    }

    #[test]
    fn test_posting_list_single_entry() {
        let ids = vec![42];
        let compressed = compress_posting_list(&ids);
        let decompressed = decompress_posting_list(&compressed);
        assert_eq!(decompressed, ids);
    }

    #[test]
    fn test_posting_list_empty() {
        let ids: Vec<u32> = vec![];
        let compressed = compress_posting_list(&ids);
        assert!(compressed.is_empty());
        let decompressed = decompress_posting_list(&compressed);
        assert!(decompressed.is_empty());
    }

    #[test]
    fn test_roundtrip_write_read() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join(".instagrep");

        let writer = IndexWriter::new(&index_dir).unwrap();

        let file_ngrams = vec![
            vec![100, 200, 300],
            vec![200, 400],
            vec![100, 400, 500],
        ];
        let file_metas = vec![
            FileMeta {
                path: PathBuf::from("a.rs"),
                mtime_secs: 1000,
                content_hash: 111,
            },
            FileMeta {
                path: PathBuf::from("b.rs"),
                mtime_secs: 2000,
                content_hash: 222,
            },
            FileMeta {
                path: PathBuf::from("c.rs"),
                mtime_secs: 3000,
                content_hash: 333,
            },
        ];

        writer.write(&file_ngrams, &file_metas, Some("abc123".into())).unwrap();

        let reader = IndexReader::open(&index_dir).unwrap();
        assert_eq!(reader.num_files(), 3);
        assert_eq!(reader.meta.version, INDEX_VERSION);
        assert_eq!(reader.meta.git_commit.as_deref(), Some("abc123"));

        // Hash 200 should appear in files 0 and 1
        let files = reader.lookup(200).unwrap();
        assert!(files.contains(&0));
        assert!(files.contains(&1));

        // Hash 500 should appear only in file 2
        let files = reader.lookup(500).unwrap();
        assert_eq!(files, vec![2]);

        // Non-existent hash
        assert!(reader.lookup(999).is_none());
    }

    #[test]
    fn test_compression_ratio() {
        // Simulate a posting list with 100 sequential file IDs
        let ids: Vec<u32> = (0..100).collect();
        let compressed = compress_posting_list(&ids);
        let raw_size = ids.len() * 4; // 400 bytes uncompressed
        // Delta of 1 each → varint of 1 → 1 byte each = ~100 bytes
        assert!(
            compressed.len() < raw_size / 3,
            "compressed {} should be < {} (1/3 of raw)",
            compressed.len(),
            raw_size / 3
        );
    }
}
