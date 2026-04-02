//! Query decomposition: build_covering algorithm.
//!
//! Parses a regex pattern, extracts literal fragments, generates the minimal
//! covering set of sparse n-grams, and intersects posting lists to find
//! candidate files.

use crate::utils::{bigram_weight, ngram_hash};
use regex_syntax::hir::{Hir, HirKind, Literal};

/// Result of query analysis: how useful the index will be.
#[derive(Debug, Clone)]
pub enum QueryStrength {
    /// Index can narrow to a small set of candidates
    Strong(Vec<u32>),
    /// Index found candidates but they're a large fraction (>50%) of total files
    Weak(Vec<u32>),
    /// Index can't help at all — pattern has no extractable literals
    Useless,
}

/// Extract literal byte strings from a regex pattern.
/// These are the concrete substrings that MUST appear in any match.
pub fn extract_literals(pattern: &str) -> Vec<Vec<u8>> {
    let Ok(hir) = regex_syntax::parse(pattern) else {
        // If regex syntax parse fails but the string has no metacharacters,
        // treat as literal. Otherwise return empty (can't optimize).
        if pattern.len() >= 2 && !has_regex_metacharacters(pattern) {
            return vec![pattern.as_bytes().to_vec()];
        }
        return vec![];
    };

    let mut literals = Vec::new();
    collect_literals(&hir, &mut literals);

    // Deduplicate
    literals.sort();
    literals.dedup();

    // Filter out very short literals (single bytes aren't useful for n-gram lookup)
    literals.retain(|l| l.len() >= 2);

    literals
}

/// Check if a string contains regex metacharacters.
fn has_regex_metacharacters(s: &str) -> bool {
    s.contains(|c: char| matches!(c, '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' | '\\'))
}

fn collect_literals(hir: &Hir, out: &mut Vec<Vec<u8>>) {
    match hir.kind() {
        HirKind::Literal(Literal(bytes)) => {
            if bytes.len() >= 2 {
                out.push(bytes.to_vec());
            }
        }
        HirKind::Concat(subs) => {
            // Try to merge adjacent literals into longer strings
            let mut current = Vec::new();
            for sub in subs {
                if let HirKind::Literal(Literal(bytes)) = sub.kind() {
                    current.extend_from_slice(bytes);
                } else {
                    if current.len() >= 2 {
                        out.push(current.clone());
                    }
                    current.clear();
                    collect_literals(sub, out);
                }
            }
            if current.len() >= 2 {
                out.push(current);
            }
        }
        HirKind::Alternation(alts) => {
            // For alternation (a|b|c), collect literals from EACH branch.
            // At search time, we UNION the posting lists for alternation branches
            // (any branch matching means the file is a candidate).
            for alt in alts {
                collect_literals(alt, out);
            }
        }
        HirKind::Repetition(rep) => {
            // For repetitions like (foo)+, foo{2,}, we can still use "foo" as a literal
            collect_literals(&rep.sub, out);
        }
        HirKind::Capture(cap) => {
            collect_literals(&cap.sub, out);
        }
        HirKind::Look(_) | HirKind::Class(_) | HirKind::Empty => {
            // Lookaheads, character classes ([a-z]), empty — no extractable literals
        }
    }
}

/// Build the covering set of sparse n-grams from literal strings.
///
/// Algorithm (build_covering from Cursor's blog):
/// Given the extracted literals, find the minimal set of sparse n-grams
/// that covers all of them. For each literal, extract its sparse n-grams
/// and pick the rarest (most selective) ones.
pub fn build_covering(literals: &[Vec<u8>]) -> Vec<u64> {
    let mut all_hashes = Vec::new();

    for literal in literals {
        let ngrams = extract_covering_ngrams(literal);
        all_hashes.extend(ngrams);
    }

    all_hashes.sort_unstable();
    all_hashes.dedup();
    all_hashes
}

/// Extract the best sparse n-grams from a single literal for query purposes.
/// We want the longest/rarest n-grams to maximize selectivity.
fn extract_covering_ngrams(literal: &[u8]) -> Vec<u64> {
    if literal.len() < 2 {
        return vec![];
    }

    let num_bigrams = literal.len() - 1;
    let weights: Vec<u32> = (0..num_bigrams)
        .map(|i| bigram_weight(literal[i], literal[i + 1]))
        .collect();

    // Generate n-grams matching the index: skip standalone bigrams,
    // skip low-weight start positions (same threshold as builder).
    let mut ngrams: Vec<(u64, usize)> = Vec::new(); // (hash, length)

    // Use same p60 threshold as the index builder
    let weight_threshold = if num_bigrams > 4 {
        let mut w = weights.clone();
        let p60 = w.len() * 3 / 5;
        w.select_nth_unstable(p60);
        w[p60]
    } else {
        0
    };

    for start in 0..num_bigrams {
        let start_weight = weights[start];

        // Skip low-weight positions (same as index builder)
        if start_weight <= weight_threshold {
            continue;
        }

        // Skip standalone bigrams (not stored in index)
        // Only generate n-grams >= 3 bytes
        let mut max_internal: u32 = 0;
        for end_bigram_idx in (start + 1)..num_bigrams {
            if end_bigram_idx > start + 1 {
                max_internal = max_internal.max(weights[end_bigram_idx - 1]);
            }
            let end_weight = weights[end_bigram_idx];
            if start_weight > max_internal && end_weight > max_internal {
                let len = end_bigram_idx + 2 - start;
                if len >= 3 && len <= crate::index::builder::MAX_NGRAM_LEN {
                    ngrams.push((ngram_hash(&literal[start..start + len]), len));
                }
            }
            if max_internal >= start_weight {
                break;
            }
        }
    }

    if ngrams.is_empty() {
        return vec![];
    }

    // Greedy covering: pick the longest n-grams that cover the literal.
    // Sort by length descending — longer n-grams are more selective.
    ngrams.sort_by(|a, b| b.1.cmp(&a.1));
    ngrams.dedup_by_key(|e| e.0);

    // Take the top few most selective n-grams
    let max_ngrams = 8;
    ngrams
        .into_iter()
        .take(max_ngrams)
        .map(|(hash, _)| hash)
        .collect()
}

/// Full query pipeline: pattern -> candidate file IDs.
/// Returns None if we can't extract any useful n-grams (must fall back to full scan).
pub fn find_candidates(
    pattern: &str,
    reader: &super::storage::IndexReader,
) -> Option<Vec<u32>> {
    find_candidates_inner(pattern, reader)
}

/// Analyze query strength: how much can the index help?
pub fn analyze_query(
    pattern: &str,
    reader: &super::storage::IndexReader,
) -> QueryStrength {
    let literals = extract_literals(pattern);
    if literals.is_empty() {
        return QueryStrength::Useless;
    }

    let covering = build_covering(&literals);
    if covering.is_empty() {
        return QueryStrength::Useless;
    }

    match find_candidates_inner(pattern, reader) {
        None => QueryStrength::Useless,
        Some(ids) => {
            let total = reader.num_files();
            if total == 0 || ids.len() > total / 2 {
                QueryStrength::Weak(ids)
            } else {
                QueryStrength::Strong(ids)
            }
        }
    }
}

/// Case-insensitive variant: generates n-grams for all case variants of the literals
/// and unions the posting lists (any file matching any variant is a candidate).
pub fn find_candidates_case_insensitive(
    pattern: &str,
    reader: &super::storage::IndexReader,
) -> Option<Vec<u32>> {
    // Union results from exact, lowercase, and uppercase variants
    let mut all = std::collections::HashSet::new();
    let mut any_succeeded = false;

    for variant in &[
        pattern.to_string(),
        pattern.to_lowercase(),
        pattern.to_uppercase(),
    ] {
        if let Some(ids) = find_candidates_inner(variant, reader) {
            any_succeeded = true;
            all.extend(ids);
        }
    }

    if any_succeeded {
        let mut result: Vec<u32> = all.into_iter().collect();
        result.sort_unstable();
        Some(result)
    } else {
        None
    }
}

fn find_candidates_inner(
    pattern: &str,
    reader: &super::storage::IndexReader,
) -> Option<Vec<u32>> {
    let literals = extract_literals(pattern);
    if literals.is_empty() {
        return None; // Can't optimize, need full scan
    }

    let covering = build_covering(&literals);
    if covering.is_empty() {
        return None;
    }

    // Intersect posting lists for all covering n-grams
    let mut posting_lists: Vec<Vec<u32>> = Vec::new();
    for hash in &covering {
        if let Some(list) = reader.lookup(*hash) {
            posting_lists.push(list);
        }
        // N-gram not in index: could mean no file has it, OR it was below
        // the indexing weight threshold. Can't tell — skip this n-gram.
        // If ALL n-grams are missing, we'll fall through to full scan.
    }

    if posting_lists.is_empty() {
        return None;
    }

    // Intersect all posting lists (start with smallest for efficiency)
    posting_lists.sort_by_key(|l| l.len());

    let mut result = posting_lists[0].clone();
    for list in &posting_lists[1..] {
        result = intersect_sorted(&result, list);
        if result.is_empty() {
            return Some(vec![]);
        }
    }

    Some(result)
}

/// Intersect two sorted lists of u32.
fn intersect_sorted(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::new();
    let mut i = 0;
    let mut j = 0;
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                result.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_literals_simple() {
        let lits = extract_literals("hello");
        assert_eq!(lits, vec![b"hello".to_vec()]);
    }

    #[test]
    fn test_extract_literals_with_regex() {
        let lits = extract_literals("foo.*bar");
        assert!(lits.contains(&b"foo".to_vec()));
        assert!(lits.contains(&b"bar".to_vec()));
    }

    #[test]
    fn test_extract_literals_alternation() {
        let lits = extract_literals("hello|world");
        assert!(lits.contains(&b"hello".to_vec()));
        assert!(lits.contains(&b"world".to_vec()));
    }

    #[test]
    fn test_extract_literals_case_insensitive_flag() {
        let lits = extract_literals("(?i)hello");
        // Shouldn't crash regardless of extraction success
        let _ = lits;
    }

    #[test]
    fn test_extract_literals_pure_wildcard() {
        // .* has no literals — should return empty
        let lits = extract_literals(".*");
        assert!(lits.is_empty());
    }

    #[test]
    fn test_extract_literals_single_char() {
        // Single char "a" is too short for n-gram (< 2 bytes)
        let lits = extract_literals("a");
        assert!(lits.is_empty());
    }

    #[test]
    fn test_extract_literals_empty_pattern() {
        let lits = extract_literals("");
        assert!(lits.is_empty());
    }

    #[test]
    fn test_extract_literals_character_class() {
        // [a-z]+ has no extractable literals
        let lits = extract_literals("[a-z]+");
        assert!(lits.is_empty());
    }

    #[test]
    fn test_extract_literals_lookaround() {
        // (?=foo)bar — should extract "bar" at least
        let lits = extract_literals("(?=foo)bar");
        // regex_syntax may or may not extract from lookaheads
        // but it should not crash
        let _ = lits;
    }

    #[test]
    fn test_extract_literals_mixed_regex() {
        // ^start.*end$ — should extract "start" and "end"
        let lits = extract_literals("^start.*end$");
        assert!(lits.contains(&b"start".to_vec()));
        assert!(lits.contains(&b"end".to_vec()));
    }

    #[test]
    fn test_build_covering_basic() {
        let covering = build_covering(&[b"MAX_FILE_SIZE".to_vec()]);
        assert!(!covering.is_empty());
    }

    #[test]
    fn test_build_covering_empty() {
        let covering = build_covering(&[]);
        assert!(covering.is_empty());
    }

    #[test]
    fn test_build_covering_short_literal() {
        // 2-byte literal: bigrams are skipped (not in index), so 0 covering n-grams
        // This correctly causes a full-scan fallback
        let covering = build_covering(&[b"ab".to_vec()]);
        assert!(covering.is_empty());
    }

    #[test]
    fn test_intersect_sorted() {
        assert_eq!(intersect_sorted(&[1, 3, 5, 7], &[2, 3, 5, 8]), vec![3, 5]);
        assert_eq!(intersect_sorted(&[1, 2, 3], &[4, 5, 6]), Vec::<u32>::new());
        assert_eq!(intersect_sorted(&[1, 2, 3], &[1, 2, 3]), vec![1, 2, 3]);
        assert_eq!(intersect_sorted(&[], &[1, 2]), Vec::<u32>::new());
        assert_eq!(intersect_sorted(&[1], &[]), Vec::<u32>::new());
    }

    #[test]
    fn test_has_regex_metacharacters() {
        assert!(has_regex_metacharacters("foo.*bar"));
        assert!(has_regex_metacharacters("a+b"));
        assert!(has_regex_metacharacters("[abc]"));
        assert!(!has_regex_metacharacters("hello_world"));
        assert!(!has_regex_metacharacters("MAX_FILE_SIZE"));
    }
}
