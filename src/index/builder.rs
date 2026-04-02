//! Sparse n-gram extraction (build_all algorithm).
//!
//! A "sparse n-gram" is a variable-length substring selected deterministically
//! where edge bigram weights are greater than all internal bigram weights.
//! This naturally selects substrings anchored at rare character transitions,
//! making them highly discriminative for filtering.

use crate::utils::{bigram_weight, ngram_hash};

/// Maximum length of a sparse n-gram (in bytes).
/// Longer n-grams have diminishing selectivity returns and bloat the index.
/// 128 bytes handles even long identifiers and string literals.
pub const MAX_NGRAM_LEN: usize = 128;

/// Extract all valid sparse n-grams from a byte slice (one file's content).
///
/// Algorithm (build_all from Cursor's blog):
/// For each position i in the text, try to extend a substring starting at i.
/// A sparse n-gram [i..j] is valid when:
///   - weight(text[i], text[i+1]) > weight(text[k], text[k+1]) for all i < k < j-1
///   - weight(text[j-2], text[j-1]) > weight(text[k], text[k+1]) for all i < k < j-2
///
/// In other words, both the first and last bigram of the n-gram must have
/// higher weight than all internal bigrams. This ensures we pick substrings
/// that start and end at "rare" character transitions.
///
/// Returns deduplicated (hash, ngram_bytes) pairs.
pub fn extract_sparse_ngrams(content: &[u8]) -> Vec<u64> {
    if content.len() < 2 {
        return vec![];
    }

    let mut hashes = Vec::new();

    // Precompute bigram weights
    let num_bigrams = content.len() - 1;
    let weights: Vec<u32> = (0..num_bigrams)
        .map(|i| bigram_weight(content[i], content[i + 1]))
        .collect();

    // For each starting position, find all valid sparse n-grams
    for start in 0..num_bigrams {
        let start_weight = weights[start];

        // The n-gram must be at least 2 bytes (one bigram).
        // A single bigram [start..start+2] is always valid (no internal bigrams).
        hashes.push(ngram_hash(&content[start..start + 2]));

        // Try extending: track the minimum edge condition
        // We need start_weight > all internal bigrams AND end_weight > all internal bigrams
        let mut max_internal: u32 = 0;

        for end_bigram_idx in (start + 1)..num_bigrams {
            // The internal bigrams are start+1 .. end_bigram_idx-1
            // When end_bigram_idx == start + 1, there are no internal bigrams yet
            if end_bigram_idx > start + 1 {
                // The new internal bigram is at end_bigram_idx - 1
                max_internal = max_internal.max(weights[end_bigram_idx - 1]);
            }

            let end_weight = weights[end_bigram_idx];

            // Check sparse n-gram condition
            if start_weight > max_internal && end_weight > max_internal {
                let ngram = &content[start..end_bigram_idx + 2];
                // Cap n-gram length to avoid very long ones (diminishing returns)
                if ngram.len() <= MAX_NGRAM_LEN {
                    hashes.push(ngram_hash(ngram));
                }
            }

            // Optimization: if an internal bigram exceeds start_weight,
            // no further extensions from this start can satisfy the condition
            if max_internal >= start_weight {
                break;
            }
        }
    }

    // Deduplicate
    hashes.sort_unstable();
    hashes.dedup();
    hashes
}

/// Extract sparse n-grams and return them as (hash, ngram_string) for debugging/testing.
pub fn extract_sparse_ngrams_debug(content: &[u8]) -> Vec<(u64, Vec<u8>)> {
    if content.len() < 2 {
        return vec![];
    }

    let mut results: Vec<(u64, Vec<u8>)> = Vec::new();
    let num_bigrams = content.len() - 1;
    let weights: Vec<u32> = (0..num_bigrams)
        .map(|i| bigram_weight(content[i], content[i + 1]))
        .collect();

    for start in 0..num_bigrams {
        let start_weight = weights[start];
        results.push((ngram_hash(&content[start..start + 2]), content[start..start + 2].to_vec()));

        let mut max_internal: u32 = 0;
        for end_bigram_idx in (start + 1)..num_bigrams {
            if end_bigram_idx > start + 1 {
                max_internal = max_internal.max(weights[end_bigram_idx - 1]);
            }
            let end_weight = weights[end_bigram_idx];
            if start_weight > max_internal && end_weight > max_internal {
                let ngram = &content[start..end_bigram_idx + 2];
                if ngram.len() <= MAX_NGRAM_LEN {
                    results.push((ngram_hash(ngram), ngram.to_vec()));
                }
            }
            if max_internal >= start_weight {
                break;
            }
        }
    }

    results.sort_by_key(|&(h, _)| h);
    results.dedup_by_key(|e| e.0);
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_basic() {
        let content = b"hello world";
        let ngrams = extract_sparse_ngrams(content);
        assert!(!ngrams.is_empty(), "should extract some n-grams");
    }

    #[test]
    fn test_extract_short_content() {
        assert!(extract_sparse_ngrams(b"").is_empty());
        assert!(extract_sparse_ngrams(b"a").is_empty());
        let two = extract_sparse_ngrams(b"ab");
        assert_eq!(two.len(), 1, "two bytes = one bigram");
    }

    #[test]
    fn test_extract_deterministic() {
        let content = b"MAX_FILE_SIZE = 1024";
        let a = extract_sparse_ngrams(content);
        let b = extract_sparse_ngrams(content);
        assert_eq!(a, b, "extraction must be deterministic");
    }

    #[test]
    fn test_extract_debug_captures_rare_bigrams() {
        let content = b"ZX_HANDLE";
        let ngrams = extract_sparse_ngrams_debug(content);
        // Should capture ZX as a bigram at minimum
        let has_zx = ngrams.iter().any(|(_, ng)| ng == b"ZX");
        assert!(has_zx, "should capture the rare ZX bigram");
    }

    #[test]
    fn test_ngrams_are_deduplicated() {
        let content = b"abcabc"; // repeated pattern
        let ngrams = extract_sparse_ngrams(content);
        let mut sorted = ngrams.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(ngrams.len(), sorted.len(), "should be deduplicated");
    }
}
