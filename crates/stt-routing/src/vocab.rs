//! Whisper initial-prompt construction from the user's custom vocabulary.
//!
//! Whisper treats the initial prompt as preceding transcript text, so a short
//! glossary nudges spelling of names and jargon. Whisper keeps only the last
//! ~224 prompt tokens, so callers should put this *after* any rolling context
//! and keep it short.

/// Longest single term that is still useful as a spelling hint.
const MAX_TERM_CHARS: usize = 60;

/// Default cap for [`build_whisper_prompt`] (roughly 100 tokens of English).
pub const DEFAULT_PROMPT_MAX_CHARS: usize = 400;

/// Builds `"Term A, Term B, Term C."` from `terms`.
///
/// * trims, drops empty terms and terms longer than 60 characters,
/// * dedupes case-insensitively (first spelling wins, order preserved),
/// * never cuts a term in half: stops before the first term that would push
///   the string past `max_chars`,
/// * returns an empty string when nothing qualifies.
pub fn build_whisper_prompt<S: AsRef<str>>(terms: &[S], max_chars: usize) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut out = String::new();
    let mut char_count = 0usize;

    for term in terms {
        let term = term
            .as_ref()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let len = term.chars().count();
        if len == 0 || len > MAX_TERM_CHARS {
            continue;
        }
        if !seen.insert(term.to_lowercase()) {
            continue;
        }
        // ", " separator plus the closing "." counts toward the cap.
        let added = len + if out.is_empty() { 1 } else { 2 };
        if char_count + added > max_chars {
            break;
        }
        if !out.is_empty() {
            out.push_str(", ");
            char_count += 2;
        }
        out.push_str(&term);
        char_count += len;
    }

    if out.is_empty() {
        return out;
    }
    out.push('.');
    out
}

/// Joins the rolling transcript context and the vocabulary prompt. Vocabulary
/// goes last so Whisper's tail-keeping truncation never drops it.
pub fn join_prompts(rolling_context: &str, vocabulary_prompt: &str) -> String {
    [rolling_context.trim(), vocabulary_prompt.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_glossary_sentence() {
        let terms = ["Anarlog", "Cedarwind", "QuickBooks"];
        assert_eq!(
            build_whisper_prompt(&terms, DEFAULT_PROMPT_MAX_CHARS),
            "Anarlog, Cedarwind, QuickBooks."
        );
    }

    #[test]
    fn dedupes_case_insensitively_and_normalizes_whitespace() {
        let terms = [
            "  Mailchimp ",
            "mailchimp",
            "MAILCHIMP",
            "",
            "   ",
            "Go  To   Market",
        ];
        assert_eq!(
            build_whisper_prompt(&terms, 400),
            "Mailchimp, Go To Market."
        );
    }

    #[test]
    fn caps_length_without_splitting_terms() {
        let terms = ["alpha", "bravo", "charlie", "delta"];
        // "alpha, bravo." is 13 chars; adding ", charlie" would exceed 15.
        let prompt = build_whisper_prompt(&terms, 15);
        assert_eq!(prompt, "alpha, bravo.");
        assert!(prompt.chars().count() <= 15);
    }

    #[test]
    fn skips_overlong_terms_and_handles_empty_input() {
        let long = "x".repeat(61);
        assert_eq!(build_whisper_prompt(&[long.as_str(), "ok"], 400), "ok.");
        assert_eq!(build_whisper_prompt::<&str>(&[], 400), "");
        assert_eq!(build_whisper_prompt(&["anything"], 0), "");
    }

    #[test]
    fn counts_characters_not_bytes() {
        let terms = ["周杰倫", "台積電"];
        assert_eq!(build_whisper_prompt(&terms, 400), "周杰倫, 台積電.");
    }

    #[test]
    fn vocabulary_goes_last_in_joined_prompt() {
        assert_eq!(join_prompts(" prior text ", "A, B."), "prior text\nA, B.");
        assert_eq!(join_prompts("", "A."), "A.");
        assert_eq!(join_prompts("prior", ""), "prior");
        assert_eq!(join_prompts("", ""), "");
    }
}
