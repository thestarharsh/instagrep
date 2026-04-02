//! Index storage: postings file + mmap-able lookup table.
//!
//! On-disk layout:
//!   .instagrep/
//!     postings.bin  — concatenated posting lists (each entry is a u32 file ID)
//!     lookup.bin    — sorted array of (ngram_hash: u64, offset: u64, length: u32)
//!     files.bin     — serialized file metadata (path, mtime, content hash)
//!     meta.bin      — index metadata (git commit, timestamp, version)

use anyhow::{Context, Result};
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

const INDEX_VERSION: u32 = 1;
const LOOKUP_ENTRY_SIZE: usize = 20; // 8 (hash) + 8 (offset) + 4 (length)

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

/// Builder for creating the index on disk.
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

    /// Write the complete index atomically.
    /// `file_ngrams`: for each file ID, the set of ngram hashes.
    /// `file_metas`: metadata for each file.
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

        // Sort posting lists for better intersection performance
        for list in inverted.values_mut() {
            list.sort_unstable();
            list.dedup();
        }

        // Write postings file and build lookup entries
        let postings_path = self.index_dir.join("postings.bin.tmp");
        let mut postings_file = File::create(&postings_path)?;
        let mut lookup_entries: Vec<(u64, u64, u32)> = Vec::with_capacity(inverted.len());
        let mut offset: u64 = 0;

        // Sort by hash for binary search
        let mut sorted_ngrams: Vec<u64> = inverted.keys().copied().collect();
        sorted_ngrams.sort_unstable();

        for hash in &sorted_ngrams {
            let posting_list = &inverted[hash];
            let bytes: Vec<u8> = posting_list
                .iter()
                .flat_map(|&id| id.to_le_bytes())
                .collect();
            postings_file.write_all(&bytes)?;
            let len = posting_list.len() as u32;
            lookup_entries.push((*hash, offset, len));
            offset += bytes.len() as u64;
        }
        postings_file.flush()?;
        drop(postings_file);

        // Write lookup table
        let lookup_path = self.index_dir.join("lookup.bin.tmp");
        let mut lookup_file = File::create(&lookup_path)?;
        for (hash, off, len) in &lookup_entries {
            lookup_file.write_all(&hash.to_le_bytes())?;
            lookup_file.write_all(&off.to_le_bytes())?;
            lookup_file.write_all(&len.to_le_bytes())?;
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

/// Reader for querying the mmap'd index.
pub struct IndexReader {
    lookup_mmap: Mmap,
    postings_mmap: Mmap,
    pub file_metas: Vec<FileMeta>,
    pub meta: IndexMeta,
    num_entries: usize,
}

impl IndexReader {
    pub fn open(index_dir: &Path) -> Result<Self> {
        let lookup_file =
            File::open(index_dir.join("lookup.bin")).context("No index found. Run `instagrep index .` first.")?;
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
    /// Returns the posting list (file IDs) if found.
    pub fn lookup(&self, ngram_hash: u64) -> Option<Vec<u32>> {
        if self.num_entries == 0 {
            return None;
        }

        // Binary search on sorted lookup table
        let mut lo = 0usize;
        let mut hi = self.num_entries;

        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let entry_offset = mid * LOOKUP_ENTRY_SIZE;
            let hash = u64::from_le_bytes(
                self.lookup_mmap[entry_offset..entry_offset + 8]
                    .try_into()
                    .unwrap(),
            );

            match hash.cmp(&ngram_hash) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => {
                    let postings_offset = u64::from_le_bytes(
                        self.lookup_mmap[entry_offset + 8..entry_offset + 16]
                            .try_into()
                            .unwrap(),
                    ) as usize;
                    let count = u32::from_le_bytes(
                        self.lookup_mmap[entry_offset + 16..entry_offset + 20]
                            .try_into()
                            .unwrap(),
                    ) as usize;

                    let byte_len = count * 4;
                    if postings_offset + byte_len > self.postings_mmap.len() {
                        return None;
                    }

                    let file_ids: Vec<u32> = (0..count)
                        .map(|i| {
                            let off = postings_offset + i * 4;
                            u32::from_le_bytes(
                                self.postings_mmap[off..off + 4].try_into().unwrap(),
                            )
                        })
                        .collect();

                    return Some(file_ids);
                }
            }
        }

        None
    }

    /// Return total number of indexed files.
    pub fn num_files(&self) -> usize {
        self.file_metas.len()
    }

    /// Return total number of distinct n-gram entries.
    pub fn num_ngrams(&self) -> usize {
        self.num_entries
    }
}

/// Check if an index exists at the given directory.
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
}
