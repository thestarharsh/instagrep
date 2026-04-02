use std::path::Path;

/// Returns true if the file appears to be binary (contains null bytes in first 8KB).
pub fn is_binary_file(path: &Path) -> bool {
    use std::fs::File;
    use std::io::Read;

    let Ok(mut f) = File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 8192];
    let Ok(n) = f.read(&mut buf) else {
        return false;
    };
    buf[..n].contains(&0)
}

/// Deterministic bigram weight function.
/// Lower weight = more common (less useful for filtering).
/// Higher weight = rarer (better for narrowing candidates).
///
/// Uses a hardcoded frequency model based on common source code patterns.
/// No external data files needed at runtime.
pub fn bigram_weight(a: u8, b: u8) -> u32 {
    // Character class scoring: rare chars get higher base scores
    fn char_class_score(c: u8) -> u32 {
        match c {
            b'Q' | b'X' | b'Z' | b'q' | b'x' | b'z' => 90,
            b'J' | b'K' | b'V' | b'W' | b'Y' => 70,
            b'A'..=b'Z' => 50, // other uppercase
            b'0'..=b'9' => 30,
            b'a'..=b'z' => 20, // lowercase common
            b'_' => 25,
            b' ' | b'\t' | b'\n' | b'\r' => 5, // whitespace is extremely common
            b'(' | b')' | b'{' | b'}' | b'[' | b']' => 15,
            b';' | b',' | b'.' | b':' => 10,
            b'=' | b'+' | b'-' | b'*' | b'/' => 12,
            b'<' | b'>' | b'!' | b'&' | b'|' | b'^' | b'~' => 35,
            b'@' | b'#' | b'$' | b'%' | b'`' => 60,
            b'"' | b'\'' => 8,
            _ => 40, // other bytes
        }
    }

    let base = char_class_score(a) + char_class_score(b);

    // Cross-class bonus: uppercase+symbol, digit+uppercase, etc. are rarer
    let cross_bonus = match (a, b) {
        (b'A'..=b'Z', b'!' | b'@' | b'#' | b'$' | b'%' | b'^' | b'&' | b'*')
        | (b'!' | b'@' | b'#' | b'$' | b'%' | b'^' | b'&' | b'*', b'A'..=b'Z') => 40,
        (b'A'..=b'Z', b'0'..=b'9') | (b'0'..=b'9', b'A'..=b'Z') => 20,
        (b'A'..=b'Z', b'A'..=b'Z') => 15, // consecutive uppercase
        _ => 0,
    };

    // Use a deterministic hash mix to add variety (avoids all "Aa" bigrams scoring the same)
    let hash_mix = ((a as u32).wrapping_mul(31).wrapping_add(b as u32)) & 0xF;

    base + cross_bonus + hash_mix
}

/// Hash an n-gram to a 64-bit key for the lookup table.
pub fn ngram_hash(ngram: &[u8]) -> u64 {
    use std::hash::{BuildHasher, Hash, Hasher};
    let build = ahash::RandomState::with_seeds(
        0x517cc1b727220a95,
        0x6c62272e07bb0142,
        0x8db2d5cf3eef2f74,
        0x62d1ce1e6b3b0a5a,
    );
    let mut hasher = build.build_hasher();
    ngram.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bigram_weight_rare_chars_score_high() {
        // ZX should score higher than "th" (very common in English/code)
        let rare = bigram_weight(b'Z', b'X');
        let common = bigram_weight(b't', b'h');
        assert!(rare > common, "ZX={rare} should be > th={common}");
    }

    #[test]
    fn test_bigram_weight_whitespace_is_low() {
        let ws = bigram_weight(b' ', b' ');
        let code = bigram_weight(b'Q', b'_');
        assert!(ws < code);
    }

    #[test]
    fn test_ngram_hash_deterministic() {
        let h1 = ngram_hash(b"hello");
        let h2 = ngram_hash(b"hello");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_ngram_hash_different_inputs() {
        let h1 = ngram_hash(b"hello");
        let h2 = ngram_hash(b"world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_is_binary_file() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();

        let text_path = dir.path().join("text.txt");
        std::fs::write(&text_path, "hello world\n").unwrap();
        assert!(!is_binary_file(&text_path));

        let bin_path = dir.path().join("binary.bin");
        let mut f = std::fs::File::create(&bin_path).unwrap();
        f.write_all(&[0x89, 0x50, 0x4E, 0x47, 0x00, 0x00]).unwrap();
        assert!(is_binary_file(&bin_path));
    }
}
