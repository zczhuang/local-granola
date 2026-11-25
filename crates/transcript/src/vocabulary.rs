//! Custom-vocabulary correction for ASR output.
//!
//! Parakeet (and other engines without a prompt) mis-hear names and jargon:
//! "anar log" for "Anarlog", "quick books" for "QuickBooks". [`correct_words`]
//! pulls such spans toward the user's terms using normalized edit distance over
//! one to three consecutive words.
//!
//! # Invariants
//!
//! Like [`crate::TranscriptPostprocessor`], the output has exactly the same
//! words as the input: same count, same `id`, timing, channel, speaker and
//! state. Only `text` changes.
//!
//! When a span of `n` words is corrected to a term:
//! * if the term has exactly `n` whitespace-separated tokens, token `i` goes to
//!   word `i` (timing stays word-accurate);
//! * otherwise the whole term goes on the first word of the span, any trailing
//!   punctuation of the span moves to that word, and the remaining words get
//!   the empty string. Renderers join word texts, so empty words vanish without
//!   leaving a gap. For example `["anar", " log"]` + `Anarlog` becomes
//!   `["Anarlog", ""]`.
//!
//! # Conservatism
//!
//! * Terms with fewer than 5 letters/digits only match exactly (ignoring case,
//!   spaces and punctuation); terms under 3 are ignored.
//! * Common English words are never rewritten into a different word, and a
//!   term that is itself a common word is ignored.
//! * Inflections (`Parakeet` vs `parakeets`) are left alone.
//! * Non-final (pending) words are never touched.

use std::collections::HashSet;

use crate::{FinalizedWord, WordState};

const MAX_NGRAM: usize = 3;
/// Words further apart than this are different utterances, not one phrase.
const MAX_GAP_MS: i64 = 2_000;
const MIN_FUZZY_KEY_CHARS: usize = 5;
const MIN_TERM_KEY_CHARS: usize = 3;
/// Normalized edit distance limits by term length.
const MAX_NORMALIZED_DISTANCE_SHORT: f32 = 0.2; // 5..=7 chars: one edit
const MAX_NORMALIZED_DISTANCE_LONG: f32 = 0.25; // 8+ chars: two edits at 8

/// One applied correction, for logging and tests.
#[derive(Debug, Clone, PartialEq)]
pub struct VocabularyCorrection {
    /// Index of the first corrected word in the input slice.
    pub start: usize,
    /// Number of input words the correction spans.
    pub len: usize,
    pub term: String,
    pub distance: usize,
}

/// Corrects `words` toward `terms`. See the module docs for the contract.
pub fn correct_words(words: &[FinalizedWord], terms: &[String]) -> Vec<FinalizedWord> {
    correct_words_with_report(words, terms).0
}

pub fn correct_words_with_report(
    words: &[FinalizedWord],
    terms: &[String],
) -> (Vec<FinalizedWord>, Vec<VocabularyCorrection>) {
    let terms = prepare_terms(terms);
    if words.is_empty() || terms.is_empty() {
        return (words.to_vec(), vec![]);
    }

    let parts: Vec<WordParts> = words.iter().map(|w| WordParts::split(&w.text)).collect();
    let mut candidates = collect_candidates(words, &parts, &terms);

    // Best (lowest normalized distance, then longest span) first, then keep
    // the non-overlapping ones.
    candidates.sort_by(|a, b| {
        a.normalized
            .total_cmp(&b.normalized)
            .then(b.len.cmp(&a.len))
            .then(a.start.cmp(&b.start))
    });
    let mut taken = vec![false; words.len()];
    let mut chosen: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        if taken[candidate.start..candidate.start + candidate.len]
            .iter()
            .any(|t| *t)
        {
            continue;
        }
        taken[candidate.start..candidate.start + candidate.len].fill(true);
        chosen.push(candidate);
    }
    chosen.sort_by_key(|c| c.start);

    let mut out: Vec<FinalizedWord> = words.to_vec();
    let mut report = Vec::new();
    for candidate in chosen {
        let term = &terms[candidate.term];
        if apply(&mut out, &parts, &candidate, term) {
            report.push(VocabularyCorrection {
                start: candidate.start,
                len: candidate.len,
                term: term.display.clone(),
                distance: candidate.distance,
            });
        }
    }
    (out, report)
}

#[derive(Debug)]
struct Term {
    display: String,
    tokens: Vec<String>,
    key: Vec<char>,
}

#[derive(Debug)]
struct Candidate {
    start: usize,
    len: usize,
    term: usize,
    distance: usize,
    normalized: f32,
}

struct WordParts<'a> {
    prefix: &'a str,
    core: &'a str,
    suffix: &'a str,
}

impl<'a> WordParts<'a> {
    /// Splits `text` into leading non-alphanumerics (spaces, quotes), the
    /// alphanumeric core (inner punctuation such as `'` or `-` stays in it)
    /// and trailing non-alphanumerics.
    fn split(text: &'a str) -> Self {
        let start = text.char_indices().find(|(_, c)| c.is_alphanumeric());
        let Some((start, _)) = start else {
            return Self {
                prefix: text,
                core: "",
                suffix: "",
            };
        };
        let end = text
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_alphanumeric())
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(text.len());
        Self {
            prefix: &text[..start],
            core: &text[start..end],
            suffix: &text[end..],
        }
    }

    fn key(&self) -> Vec<char> {
        key_chars(self.core)
    }
}

fn key_chars(text: &str) -> Vec<char> {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn prepare_terms(terms: &[String]) -> Vec<Term> {
    let mut seen = HashSet::new();
    terms
        .iter()
        .filter_map(|raw| {
            let display = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            let key = key_chars(&display);
            if key.len() < MIN_TERM_KEY_CHARS || !seen.insert(key.clone()) {
                return None;
            }
            let key_string: String = key.iter().collect();
            if is_common_word(&key_string) {
                return None;
            }
            Some(Term {
                tokens: display.split(' ').map(str::to_string).collect(),
                display,
                key,
            })
        })
        .collect()
}

fn max_distance(term_len: usize) -> usize {
    if term_len < MIN_FUZZY_KEY_CHARS {
        return 0;
    }
    let ratio = if term_len >= 8 {
        MAX_NORMALIZED_DISTANCE_LONG
    } else {
        MAX_NORMALIZED_DISTANCE_SHORT
    };
    (term_len as f32 * ratio).floor() as usize
}

fn collect_candidates(
    words: &[FinalizedWord],
    parts: &[WordParts<'_>],
    terms: &[Term],
) -> Vec<Candidate> {
    let keys: Vec<Vec<char>> = parts.iter().map(WordParts::key).collect();
    let mut candidates = Vec::new();

    for start in 0..words.len() {
        if keys[start].is_empty() || words[start].state != WordState::Final {
            continue;
        }
        let mut joined: Vec<char> = Vec::new();
        for len in 1..=MAX_NGRAM {
            let end = start + len;
            if end > words.len() {
                break;
            }
            let last = end - 1;
            if keys[last].is_empty() || words[last].state != WordState::Final {
                break;
            }
            if len > 1 && !contiguous(&words[last - 1], &words[last]) {
                break;
            }
            joined.extend(keys[last].iter());

            for (term_index, term) in terms.iter().enumerate() {
                let allowed = max_distance(term.key.len());
                if joined.len().abs_diff(term.key.len()) > allowed {
                    continue;
                }
                let distance = edit_distance(&joined, &term.key);
                if distance > allowed {
                    continue;
                }
                if is_noop(&words[start..end], &parts[start..end], term, distance) {
                    continue;
                }
                if distance > 0 && !fuzzy_allowed(&joined, &term.key, len) {
                    continue;
                }
                if distance == 0 && len == 1 && is_common_word(&joined.iter().collect::<String>()) {
                    continue;
                }
                candidates.push(Candidate {
                    start,
                    len,
                    term: term_index,
                    distance,
                    normalized: distance as f32 / joined.len().max(term.key.len()) as f32,
                });
            }
        }
    }
    candidates
}

fn contiguous(prev: &FinalizedWord, next: &FinalizedWord) -> bool {
    prev.channel == next.channel
        && prev.speaker_index == next.speaker_index
        && next.start_ms - prev.end_ms <= MAX_GAP_MS
}

/// Rejects fuzzy matches that are really inflections or common words.
fn fuzzy_allowed(candidate: &[char], term: &[char], span_len: usize) -> bool {
    if candidate.len() < MIN_FUZZY_KEY_CHARS {
        return false;
    }
    // "parakeets" vs "parakeet": one is the other plus a suffix.
    if candidate.starts_with(term) || term.starts_with(candidate) {
        return false;
    }
    if span_len == 1 && is_common_word(&candidate.iter().collect::<String>()) {
        return false;
    }
    true
}

/// True when applying the term would not change the text.
fn is_noop(words: &[FinalizedWord], parts: &[WordParts<'_>], term: &Term, distance: usize) -> bool {
    if distance != 0 || words.len() != term.tokens.len() {
        return false;
    }
    parts
        .iter()
        .zip(&term.tokens)
        .all(|(part, token)| part.core == token)
}

fn apply(
    out: &mut [FinalizedWord],
    parts: &[WordParts<'_>],
    candidate: &Candidate,
    term: &Term,
) -> bool {
    let range = candidate.start..candidate.start + candidate.len;
    if candidate.len == term.tokens.len() {
        for (offset, token) in term.tokens.iter().enumerate() {
            let index = candidate.start + offset;
            let p = &parts[index];
            out[index].text = format!("{}{}{}", p.prefix, token, p.suffix);
        }
    } else {
        let first = &parts[candidate.start];
        let last = &parts[range.end - 1];
        out[candidate.start].text = format!("{}{}{}", first.prefix, term.display, last.suffix);
        for index in candidate.start + 1..range.end {
            out[index].text = String::new();
        }
    }
    true
}

fn edit_distance(a: &[char], b: &[char]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// Frequent English words that must never be rewritten into a term (and that
/// are not accepted as terms). Lowercase, letters only.
fn is_common_word(key: &str) -> bool {
    const COMMON: &[&str] = &[
        "about", "above", "after", "again", "against", "all", "also", "always", "among", "and",
        "another", "any", "are", "around", "back", "because", "been", "before", "being", "below",
        "between", "both", "but", "can", "come", "could", "did", "does", "doing", "done", "down",
        "during", "each", "even", "every", "few", "first", "for", "from", "get", "give", "going",
        "good", "got", "great", "had", "has", "have", "having", "her", "here", "him", "his", "how",
        "into", "its", "just", "know", "last", "let", "like", "little", "long", "look", "made",
        "make", "many", "may", "might", "more", "most", "much", "must", "need", "never", "new",
        "next", "not", "now", "off", "old", "once", "only", "other", "our", "out", "over", "own",
        "part", "people", "please", "put", "really", "right", "said", "same", "say", "see",
        "should", "since", "some", "still", "such", "take", "tell", "than", "that", "the", "their",
        "them", "then", "there", "these", "they", "thing", "things", "think", "this", "those",
        "though", "through", "time", "today", "too", "two", "under", "until", "use", "very",
        "want", "was", "way", "well", "went", "were", "what", "when", "where", "which", "while",
        "who", "why", "will", "with", "without", "work", "would", "yeah", "year", "yes", "you",
        "your", "three", "four", "five", "sure", "okay", "maybe", "week", "day", "number", "point",
        "team", "call", "meeting", "start", "stop", "plan", "issue",
    ];
    COMMON.contains(&key)
}

#[cfg(test)]
mod tests;
