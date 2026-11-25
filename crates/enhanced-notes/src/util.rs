//! Small text helpers shared by every module: normalization, hashing and a
//! script-agnostic similarity measure (character bigram Dice coefficient).

use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Lowercase and collapse whitespace so that cosmetic edits do not change keys.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.extend(ch.to_lowercase());
    }
    out
}

pub fn sha_hex(input: &str, len: usize) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex.truncate(len.min(64));
    hex
}

pub fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x2E80..=0x2FDF   // radicals
        | 0x3000..=0x303F // CJK punctuation
        | 0x3040..=0x30FF // kana
        | 0x3100..=0x312F // bopomofo
        | 0x31A0..=0x31BF
        | 0x3400..=0x4DBF // ext A
        | 0x4E00..=0x9FFF // unified
        | 0xAC00..=0xD7AF // hangul
        | 0xF900..=0xFAFF
        | 0xFF00..=0xFFEF // full-width forms
        | 0x20000..=0x2FA1F)
}

fn bigrams(chars: &[char]) -> HashMap<(char, char), u32> {
    let mut map = HashMap::new();
    for pair in chars.windows(2) {
        *map.entry((pair[0], pair[1])).or_insert(0) += 1;
    }
    map
}

/// Sørensen–Dice similarity over character bigrams of the normalized texts,
/// in `0.0..=1.0`. Works for CJK (no word boundaries needed).
pub fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = normalize(a)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let b: Vec<char> = normalize(b)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a == b {
        return 1.0;
    }
    if a.len() < 2 || b.len() < 2 {
        return 0.0;
    }
    let (ma, mb) = (bigrams(&a), bigrams(&b));
    let overlap: u32 = ma
        .iter()
        .map(|(k, v)| (*v).min(mb.get(k).copied().unwrap_or(0)))
        .sum();
    (2.0 * overlap as f64) / ((a.len() - 1 + b.len() - 1) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_space_and_case() {
        assert_eq!(normalize("  Ship  THE\tthing \n"), "ship the thing");
    }

    #[test]
    fn similarity_is_symmetric_and_script_agnostic() {
        assert!(similarity("Pricing review", "pricing  review!") > 0.8);
        assert!(similarity("定價討論", "定價的討論") > 0.5);
        assert!(similarity("budget", "hiring plan") < 0.3);
        assert_eq!(similarity("a b", "a b"), 1.0);
        assert!((similarity("abcd", "abce") - similarity("abce", "abcd")).abs() < 1e-9);
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(sha_hex("x", 8), sha_hex("x", 8));
        assert_ne!(sha_hex("x", 8), sha_hex("y", 8));
        assert_eq!(sha_hex("x", 8).len(), 8);
    }
}
