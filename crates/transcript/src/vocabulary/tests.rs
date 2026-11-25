use super::*;

fn word(id: &str, text: &str, start_ms: i64, end_ms: i64) -> FinalizedWord {
    FinalizedWord {
        id: id.to_string(),
        text: text.to_string(),
        start_ms,
        end_ms,
        channel: 0,
        state: WordState::Final,
        speaker_index: None,
    }
}

fn words(texts: &[&str]) -> Vec<FinalizedWord> {
    texts
        .iter()
        .enumerate()
        .map(|(i, t)| word(&format!("w{i}"), t, i as i64 * 400, i as i64 * 400 + 300))
        .collect()
}

fn terms(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

fn texts(words: &[FinalizedWord]) -> Vec<&str> {
    words.iter().map(|w| w.text.as_str()).collect()
}

fn joined(words: &[FinalizedWord]) -> String {
    words.iter().map(|w| w.text.as_str()).collect()
}

fn assert_invariants(before: &[FinalizedWord], after: &[FinalizedWord]) {
    assert_eq!(before.len(), after.len());
    for (a, b) in before.iter().zip(after) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.start_ms, b.start_ms);
        assert_eq!(a.end_ms, b.end_ms);
        assert_eq!(a.channel, b.channel);
        assert_eq!(a.speaker_index, b.speaker_index);
        assert_eq!(a.state, b.state);
    }
}

#[test]
fn two_words_to_one_term_keeps_count_and_empties_the_tail() {
    let input = words(&[" we", " use", " anar", " log", " daily"]);
    let out = correct_words(&input, &terms(&["Anarlog"]));
    assert_invariants(&input, &out);
    assert_eq!(texts(&out), [" we", " use", " Anarlog", "", " daily"]);
    assert_eq!(joined(&out), " we use Anarlog daily");
}

#[test]
fn trailing_punctuation_moves_to_the_first_word_of_a_merged_span() {
    let input = words(&[" open", " anar", " log,", " please"]);
    let out = correct_words(&input, &terms(&["Anarlog"]));
    assert_eq!(texts(&out), [" open", " Anarlog,", "", " please"]);
    assert_eq!(joined(&out), " open Anarlog, please");
}

#[test]
fn single_word_fuzzy_match_preserves_prefix_and_suffix() {
    let input = words(&[" ask", " Cedarwind's", " team", " about", " Mailchimp."]);
    let out = correct_words(&input, &terms(&["Cedarwind", "MailChimp"]));
    // "Cedarwind's" is an exact core match only after the apostrophe split; it
    // is an inflection of the term, so it is left alone.
    assert_eq!(texts(&out)[1], " Cedarwind's");
    assert_eq!(texts(&out)[4], " MailChimp.");
}

#[test]
fn one_edit_misspelling_is_corrected() {
    let input = words(&[" the", " Cedarwynd", " proposal"]);
    let out = correct_words(&input, &terms(&["Cedarwind"]));
    assert_eq!(texts(&out), [" the", " Cedarwind", " proposal"]);
}

#[test]
fn split_term_maps_token_by_token_when_word_counts_match() {
    let input = words(&[" about", " quik", " books", " online"]);
    let out = correct_words(&input, &terms(&["Quick Books"]));
    assert_eq!(texts(&out), [" about", " Quick", " Books", " online"]);
}

#[test]
fn three_words_to_single_term() {
    let input = words(&[" sing", " a", " pore", " office"]);
    let out = correct_words(&input, &terms(&["Singapore"]));
    assert_invariants(&input, &out);
    assert_eq!(texts(&out), [" Singapore", "", "", " office"]);
}

#[test]
fn case_only_difference_is_fixed_for_distinctive_terms() {
    let input = words(&[" use", " quickbooks", " online"]);
    let out = correct_words(&input, &terms(&["QuickBooks"]));
    assert_eq!(texts(&out)[1], " QuickBooks");
}

#[test]
fn short_terms_only_match_exactly() {
    let input = words(&[" the", " aws", " bill", " and", " ab"]);
    let out = correct_words(&input, &terms(&["AWS", "AWD", "AB"]));
    assert_eq!(texts(&out)[1], " AWS");
    // A 3-letter term never fuzzy-matches ("aws" is not rewritten to "AWD").
    assert_eq!(texts(&out)[2], " bill");
    // Terms under three characters are ignored entirely.
    assert_eq!(texts(&out)[4], " ab");
}

#[test]
fn common_words_are_never_rewritten_or_accepted_as_terms() {
    let input = words(&[" we", " will", " have", " people", " there"]);
    let out = correct_words(&input, &terms(&["Will", "Peoples", "Thera"]));
    assert_eq!(texts(&out), texts(&input));
}

#[test]
fn inflections_are_left_alone() {
    let input = words(&[" two", " parakeets", " flew"]);
    let out = correct_words(&input, &terms(&["Parakeet"]));
    assert_eq!(texts(&out), texts(&input));
}

#[test]
fn does_not_join_words_across_utterances_speakers_or_channels() {
    let mut input = words(&[" anar", " log"]);
    input[1].start_ms = input[0].end_ms + 5_000;
    assert_eq!(
        texts(&correct_words(&input, &terms(&["Anarlog"]))),
        [" anar", " log"]
    );

    let mut input = words(&[" anar", " log"]);
    input[1].speaker_index = Some(1);
    assert_eq!(
        texts(&correct_words(&input, &terms(&["Anarlog"]))),
        [" anar", " log"]
    );

    let mut input = words(&[" anar", " log"]);
    input[1].channel = 1;
    assert_eq!(
        texts(&correct_words(&input, &terms(&["Anarlog"]))),
        [" anar", " log"]
    );
}

#[test]
fn pending_words_are_not_corrected() {
    let mut input = words(&[" anar", " log"]);
    input[0].state = WordState::Pending;
    let out = correct_words(&input, &terms(&["Anarlog"]));
    assert_eq!(texts(&out), [" anar", " log"]);
}

#[test]
fn exact_match_is_a_noop_and_not_reported() {
    let input = words(&[" use", " Anarlog", " today"]);
    let (out, report) = correct_words_with_report(&input, &terms(&["Anarlog"]));
    assert_eq!(texts(&out), texts(&input));
    assert!(report.is_empty());
}

#[test]
fn overlapping_candidates_prefer_the_longer_exact_match() {
    let input = words(&[" cedar", " wind", " labs"]);
    let out = correct_words(&input, &terms(&["Cedarwind", "Cedarwind Labs"]));
    assert_invariants(&input, &out);
    assert_eq!(texts(&out), [" Cedarwind Labs", "", ""]);
}

#[test]
fn empty_inputs_are_passthrough() {
    assert!(correct_words(&[], &terms(&["Anarlog"])).is_empty());
    let input = words(&[" hello"]);
    assert_eq!(texts(&correct_words(&input, &[])), [" hello"]);
}

#[test]
fn report_describes_each_correction() {
    let input = words(&[" use", " anar", " log"]);
    let (_, report) = correct_words_with_report(&input, &terms(&["Anarlog"]));
    assert_eq!(
        report,
        vec![VocabularyCorrection {
            start: 1,
            len: 2,
            term: "Anarlog".to_string(),
            distance: 0,
        }]
    );
}
