//! Prompt segments: transcript words grouped into speaker turns with short
//! ids (`s1`..`sN`). Each segment maps to a [`SourceRef`] expressed in word ids
//! (stable across speaker re-labeling, unlike rendered segment ids).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::util::{is_cjk, sha_hex};

/// A finalized transcript word as stored in `transcripts.words_json`.
/// Extra stored fields (state, ...) are ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub id: String,
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    #[serde(default)]
    pub channel: i32,
    #[serde(default)]
    pub speaker_index: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptInput {
    pub transcript_id: String,
    pub words: Vec<Word>,
}

/// Speaker display names. Keys are `"<channel>:<speaker_index>"` (most
/// specific) or `"<channel>"`. Unlisted speakers fall back to `You` (mic
/// channel) or `Speaker N`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpeakerLabels(pub BTreeMap<String, String>);

impl SpeakerLabels {
    fn lookup(&self, channel: i32, speaker: Option<i32>) -> Option<&String> {
        speaker
            .and_then(|s| self.0.get(&format!("{channel}:{s}")))
            .or_else(|| self.0.get(&channel.to_string()))
    }
}

/// Compact pointer into a transcript, by word id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceRef {
    pub transcript_id: String,
    pub start_word_id: String,
    pub end_word_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PromptSegment {
    pub sid: String,
    pub speaker: String,
    pub start_ms: i64,
    pub text: String,
    pub source: SourceRef,
}

impl PromptSegment {
    pub fn prompt_line(&self) -> String {
        format!(
            "{} [{} {}]: {}",
            self.sid,
            self.speaker,
            format_mmss(self.start_ms),
            self.text
        )
    }
}

pub fn format_mmss(ms: i64) -> String {
    let total = (ms.max(0) / 1000) as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Gap of silence that starts a new segment even for the same speaker.
const MAX_GAP_MS: i64 = 2500;
/// Soft size cap per segment (CJK characters count double).
const MAX_SEGMENT_WEIGHT: usize = 420;

fn weight(text: &str) -> usize {
    text.chars().map(|c| if is_cjk(c) { 2 } else { 1 }).sum()
}

fn push_word_text(buf: &mut String, word: &str) {
    let trimmed = word.trim();
    if trimmed.is_empty() {
        return;
    }
    if !buf.is_empty() {
        let prev = buf.chars().next_back().unwrap_or(' ');
        let next = trimmed.chars().next().unwrap_or(' ');
        let starts_with_space = word.starts_with(char::is_whitespace);
        let cjk_boundary = is_cjk(prev) || is_cjk(next);
        let no_space_punct = matches!(next, ',' | '.' | '!' | '?' | ';' | ':' | '%' | ')' | '\'');
        if (starts_with_space || !cjk_boundary) && !no_space_punct && !prev.is_whitespace() {
            buf.push(' ');
        }
    }
    buf.push_str(trimmed);
}

/// Group words into speaker turns. Transcripts are processed in the order
/// given; segment ids are global across transcripts.
pub fn build_segments(
    transcripts: &[TranscriptInput],
    labels: &SpeakerLabels,
) -> Vec<PromptSegment> {
    let mut out: Vec<PromptSegment> = Vec::new();
    let mut unknown: BTreeMap<(i32, Option<i32>), usize> = BTreeMap::new();

    for transcript in transcripts {
        let mut words: Vec<&Word> = transcript.words.iter().collect();
        words.sort_by_key(|w| (w.start_ms, w.end_ms));

        struct Open<'a> {
            key: (i32, Option<i32>),
            first: &'a Word,
            last: &'a Word,
            text: String,
        }
        let mut open: Option<Open> = None;
        let flush = |open: Option<Open>,
                     out: &mut Vec<PromptSegment>,
                     unknown: &mut BTreeMap<(i32, Option<i32>), usize>| {
            let Some(o) = open else { return };
            if o.text.trim().is_empty() {
                return;
            }
            let speaker = labels.lookup(o.key.0, o.key.1).cloned().unwrap_or_else(|| {
                if o.key.0 == 0 && o.key.1.is_none() {
                    return "You".to_owned();
                }
                let next = unknown.len() + 1;
                let n = *unknown.entry(o.key).or_insert(next);
                format!("Speaker {n}")
            });
            out.push(PromptSegment {
                sid: format!("s{}", out.len() + 1),
                speaker,
                start_ms: o.first.start_ms,
                text: o.text,
                source: SourceRef {
                    transcript_id: transcript.transcript_id.clone(),
                    start_word_id: o.first.id.clone(),
                    end_word_id: o.last.id.clone(),
                    start_ms: o.first.start_ms,
                    end_ms: o.last.end_ms,
                },
            });
        };

        for word in words {
            if word.text.trim().is_empty() {
                continue;
            }
            let key = (word.channel, word.speaker_index);
            let split = match &open {
                None => true,
                Some(o) => {
                    o.key != key
                        || word.start_ms - o.last.end_ms > MAX_GAP_MS
                        || (weight(&o.text) >= MAX_SEGMENT_WEIGHT
                            && o.text.ends_with(['.', '?', '!', '。', '？', '！']))
                        || weight(&o.text) >= MAX_SEGMENT_WEIGHT * 2
                }
            };
            if split {
                flush(open.take(), &mut out, &mut unknown);
                open = Some(Open {
                    key,
                    first: word,
                    last: word,
                    text: String::new(),
                });
            }
            let o = open.as_mut().expect("open segment");
            push_word_text(&mut o.text, &word.text);
            o.last = word;
        }
        flush(open.take(), &mut out, &mut unknown);
    }
    out
}

/// Turn cited segment ids into compact source refs; runs of adjacent segments
/// of one transcript collapse into a single word-id range.
pub fn compact_sources(sids: &[String], segments: &[PromptSegment]) -> Vec<SourceRef> {
    let mut positions: Vec<usize> = sids
        .iter()
        .filter_map(|sid| segments.iter().position(|s| &s.sid == sid))
        .collect();
    positions.sort_unstable();
    positions.dedup();
    let mut out: Vec<SourceRef> = Vec::new();
    let mut prev: Option<usize> = None;
    for pos in positions {
        let src = &segments[pos].source;
        match (prev, out.last_mut()) {
            (Some(p), Some(last)) if p + 1 == pos && last.transcript_id == src.transcript_id => {
                last.end_word_id = src.end_word_id.clone();
                last.end_ms = src.end_ms;
            }
            _ => out.push(src.clone()),
        }
        prev = Some(pos);
    }
    out
}

/// Digest of transcript content (ids, text, timing) used for staleness checks.
pub fn transcripts_digest(transcripts: &[TranscriptInput]) -> String {
    let mut buf = String::new();
    for t in transcripts {
        buf.push_str(&t.transcript_id);
        buf.push('\n');
        for w in &t.words {
            buf.push_str(&format!(
                "{}|{}|{}|{}\n",
                w.id, w.text, w.start_ms, w.end_ms
            ));
        }
    }
    sha_hex(&buf, 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn w(
        id: &str,
        text: &str,
        start: i64,
        end: i64,
        channel: i32,
        speaker: Option<i32>,
    ) -> Word {
        Word {
            id: id.into(),
            text: text.into(),
            start_ms: start,
            end_ms: end,
            channel,
            speaker_index: speaker,
        }
    }

    #[test]
    fn groups_by_speaker_and_gap_with_word_id_sources() {
        let t = TranscriptInput {
            transcript_id: "t1".into(),
            words: vec![
                w("w1", " Hello", 0, 400, 0, None),
                w("w2", " there", 400, 800, 0, None),
                w("w3", " Hi", 900, 1200, 1, Some(0)),
                w("w4", " again", 9000, 9400, 1, Some(0)),
            ],
        };
        let mut labels = SpeakerLabels::default();
        labels.0.insert("1:0".into(), "Dana".into());
        let segs = build_segments(&[t], &labels);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].sid, "s1");
        assert_eq!(segs[0].speaker, "You");
        assert_eq!(segs[0].text, "Hello there");
        assert_eq!(segs[0].source.start_word_id, "w1");
        assert_eq!(segs[0].source.end_word_id, "w2");
        assert_eq!(segs[1].speaker, "Dana");
        assert_eq!(segs[2].start_ms, 9000);
        assert_eq!(segs[2].prompt_line(), "s3 [Dana 00:09]: again");
    }

    #[test]
    fn relabeling_does_not_change_sources() {
        let t = TranscriptInput {
            transcript_id: "t1".into(),
            words: vec![w("w1", "Hi", 0, 100, 1, Some(0))],
        };
        let a = build_segments(std::slice::from_ref(&t), &SpeakerLabels::default());
        let mut labels = SpeakerLabels::default();
        labels.0.insert("1".into(), "Sam".into());
        let b = build_segments(&[t], &labels);
        assert_ne!(a[0].speaker, b[0].speaker);
        assert_eq!(a[0].source, b[0].source);
    }

    #[test]
    fn joins_cjk_without_spaces() {
        let t = TranscriptInput {
            transcript_id: "t".into(),
            words: vec![
                w("a", "我們", 0, 100, 0, None),
                w("b", "決定", 100, 200, 0, None),
                w("c", " ship", 200, 300, 0, None),
            ],
        };
        let segs = build_segments(&[t], &SpeakerLabels::default());
        assert_eq!(segs[0].text, "我們決定 ship");
    }

    #[test]
    fn parses_stored_words_json_ignoring_extra_fields() {
        let json = r#"[{"id":"a","text":" hi","start_ms":0,"end_ms":10,"channel":1,"state":"final","speaker_index":2}]"#;
        let words: Vec<Word> = serde_json::from_str(json).unwrap();
        assert_eq!(words[0].speaker_index, Some(2));
        assert_eq!(words[0].channel, 1);
    }

    #[test]
    fn adjacent_sources_collapse_into_one_word_range() {
        let seg = |n: usize, tid: &str| PromptSegment {
            sid: format!("s{n}"),
            speaker: "A".into(),
            start_ms: 0,
            text: "x".into(),
            source: SourceRef {
                transcript_id: tid.into(),
                start_word_id: format!("w{n}a"),
                end_word_id: format!("w{n}b"),
                start_ms: n as i64,
                end_ms: n as i64 + 1,
            },
        };
        let segs = vec![seg(1, "t"), seg(2, "t"), seg(3, "u"), seg(4, "u")];
        let refs = compact_sources(&["s2".into(), "s1".into(), "s4".into(), "s9".into()], &segs);
        assert_eq!(refs.len(), 2);
        assert_eq!(
            (refs[0].start_word_id.as_str(), refs[0].end_word_id.as_str()),
            ("w1a", "w2b")
        );
        assert_eq!(refs[1].start_word_id, "w4a");
    }

    #[test]
    fn mmss_formats_hours() {
        assert_eq!(format_mmss(65_000), "01:05");
        assert_eq!(format_mmss(3_725_000), "1:02:05");
    }
}
