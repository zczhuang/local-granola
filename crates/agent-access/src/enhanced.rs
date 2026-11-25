//! Read-only access to enhanced notes, full-text search and transcript
//! segments. Everything here reads the local SQLite database only; nothing
//! writes and nothing touches the network.
//!
//! Enhanced notes are TipTap documents in which the user's own typed lines are
//! preserved verbatim and every AI-written list item carries
//! `attrs: {aiId, src}` where `src` lists source refs
//! `{transcript_id, start_word_id, end_word_id, start_ms, end_ms}` (the compact
//! keys `t/s/e/a/b` are accepted too).

use std::collections::{HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;

use crate::{Error, Pagination, Result, pagination, stored_folder_path};

pub const DEFAULT_SEARCH_LIMIT: u32 = 10;
pub const MAX_SEARCH_LIMIT: u32 = 50;
/// Upper bound on sessions the verifying pass of `search_meetings` inspects.
pub const MAX_SEARCH_SCAN: usize = 1000;
const SEARCH_CHUNK: i64 = 100;
const MAX_SEARCH_TERMS: usize = 8;
const MAX_SNIPPETS_PER_HIT: usize = 6;
const SNIPPET_RADIUS: usize = 70;
const MAX_SOURCE_TEXT_CHARS: usize = 1200;
pub const DEFAULT_SEGMENT_WORDS: u32 = 1000;
pub const MAX_SEGMENT_WORDS: u32 = 3000;
/// Silence that starts a new speaker turn even for the same speaker.
const TURN_GAP_MS: i64 = 2500;
/// Soft cap on the characters of one speaker turn.
const TURN_MAX_CHARS: usize = 600;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SearchMeetingsInput {
    #[schemars(
        description = "Words to find. Every word must appear somewhere in the meeting (title, typed notes, enhanced notes, summaries or transcript). Wrap a phrase in double quotes to match it exactly."
    )]
    pub query: String,
    #[schemars(description = "Only search meetings in this folder path and its subfolders")]
    #[serde(default)]
    pub folder_path: Option<String>,
    #[schemars(description = "Maximum meetings; defaults to 10 and is capped at 50")]
    #[schemars(range(min = 1, max = 50))]
    pub limit: Option<u32>,
    #[schemars(description = "Number of matching meetings to skip; defaults to 0")]
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SearchSnippet {
    /// `title`, `typed_notes`, `enhanced_notes`, `summary`, `transcript` or the
    /// document kind for any other document.
    pub source: String,
    pub text: String,
    /// `mm:ss` of the matching transcript word (transcript snippets only).
    pub time: Option<String>,
    pub start_ms: Option<i64>,
    pub transcript_id: Option<String>,
    pub document_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SearchHit {
    pub meeting_id: String,
    pub title: String,
    /// Meeting start, or its creation time when it has no start.
    pub date: String,
    pub folder_path: Option<String>,
    pub snippets: Vec<SearchSnippet>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SearchPage {
    pub query: String,
    pub hits: Vec<SearchHit>,
    pub pagination: Pagination,
    /// True when the bounded scan stopped before inspecting every candidate;
    /// narrow the query or folder to see older matches.
    pub scan_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetEnhancedNotesInput {
    #[schemars(description = "Anarlog meeting id")]
    pub meeting_id: String,
    #[schemars(
        description = "Specific enhanced document id; defaults to the most recently updated one"
    )]
    #[serde(default)]
    pub document_id: Option<String>,
}

/// A cited stretch of transcript, resolved to speakers, times and words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ResolvedSource {
    pub transcript_id: String,
    pub start_word_id: String,
    pub end_word_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    /// `mm:ss` (or `h:mm:ss`).
    pub start: String,
    pub end: String,
    pub speakers: Vec<String>,
    /// The cited transcript words, truncated to about 1200 characters.
    pub text: String,
    /// False when the cited words no longer exist (transcript edited or deleted).
    pub resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct AiBullet {
    pub ai_id: String,
    pub text: String,
    pub sources: Vec<ResolvedSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct NoteLine {
    /// The user's own typed text, never rewritten.
    pub text: String,
    /// `paragraph`, `heading` or `list_item`.
    pub kind: String,
    /// Nesting depth of the user's list item (0 for top level).
    pub depth: u32,
    pub ai_bullets: Vec<AiBullet>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct EnhancedActionItem {
    pub ai_id: String,
    pub text: String,
    /// Null when the saved document does not record an owner.
    pub owner: Option<String>,
    /// Null when the saved document does not record a due date.
    pub due: Option<String>,
    pub done: bool,
    pub sources: Vec<ResolvedSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct EnhancedNotes {
    pub meeting_id: String,
    pub title: String,
    /// `saved` for a stored document, `preview` for an unsaved model run.
    pub origin: String,
    pub document_id: Option<String>,
    pub updated_at: String,
    /// Raw generation metadata stored with the document (model, digests, ...).
    pub generation: Value,
    /// The user's typed lines in order, each with its AI bullets.
    pub lines: Vec<NoteLine>,
    /// AI bullets under the generated Overview (meetings without typed notes).
    pub overview: Vec<AiBullet>,
    pub decisions: Vec<AiBullet>,
    pub action_items: Vec<EnhancedActionItem>,
    pub open_questions: Vec<AiBullet>,
    /// Number of cited sources whose transcript words could not be found.
    pub unresolved_sources: usize,
    /// Markdown rendering with `[^n]` citations; each footnote starts with `[mm:ss]`.
    pub markdown: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetTranscriptSegmentInput {
    #[schemars(description = "Anarlog meeting id")]
    pub meeting_id: String,
    #[schemars(
        description = "Transcript id from a source ref; optional for time ranges (all transcripts are searched)"
    )]
    #[serde(default)]
    pub transcript_id: Option<String>,
    #[schemars(description = "First word id of a source ref (use with end_word_id)")]
    #[serde(default)]
    pub start_word_id: Option<String>,
    #[schemars(description = "Last word id of a source ref (use with start_word_id)")]
    #[serde(default)]
    pub end_word_id: Option<String>,
    #[schemars(
        description = "Range start in milliseconds, as stored on transcript words (use with end_ms)"
    )]
    #[serde(default)]
    pub start_ms: Option<i64>,
    #[schemars(description = "Range end in milliseconds (use with start_ms)")]
    #[serde(default)]
    pub end_ms: Option<i64>,
    #[schemars(description = "Maximum words returned; defaults to 1000 and is capped at 3000")]
    #[schemars(range(min = 1, max = 3000))]
    pub max_words: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SegmentTurn {
    pub transcript_id: String,
    pub speaker: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub start: String,
    pub end: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct TranscriptSegment {
    pub meeting_id: String,
    pub segments: Vec<SegmentTurn>,
    /// `[mm:ss] Speaker: text` lines, one per speaker turn.
    pub text: String,
    pub word_count: usize,
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// Transcript index
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct IWord {
    id: String,
    text: String,
    start_ms: i64,
    end_ms: i64,
    channel: i64,
    speaker_index: Option<i64>,
}

#[derive(Debug, Clone)]
struct ITranscript {
    id: String,
    words: Vec<IWord>,
}

/// One speaker turn of a transcript, addressable by word ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// Prompt id: `s1`, `s2`, ...
    pub sid: String,
    pub transcript_id: String,
    pub speaker: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub start_word_id: String,
    pub end_word_id: String,
    pub text: String,
}

impl Turn {
    pub fn source_ref(&self) -> SourceRef {
        SourceRef {
            transcript_id: self.transcript_id.clone(),
            start_word_id: self.start_word_id.clone(),
            end_word_id: self.end_word_id.clone(),
            start_ms: self.start_ms,
            end_ms: self.end_ms,
        }
    }
}

/// A reference to a stretch of transcript words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
    pub transcript_id: String,
    pub start_word_id: String,
    pub end_word_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// A transcript word as the notes engine takes it, with the label the app would show for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineWord {
    pub id: String,
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub channel: i64,
    pub speaker_index: Option<i64>,
    /// Display name of the speaker; `None` for words without a rendered segment.
    pub label: Option<String>,
}

/// A meeting's transcript words with speaker labels, for resolving citations.
#[derive(Debug, Clone, Default)]
pub struct TranscriptIndex {
    transcripts: Vec<ITranscript>,
    labels: HashMap<String, String>,
}

impl TranscriptIndex {
    pub async fn load(pool: &SqlitePool, meeting_id: &str) -> Result<Self> {
        let export = crate::get_meeting_export(pool, meeting_id.to_string()).await?;
        let labels = crate::transcript_render::speaker_labels_by_word_id(&export);
        Ok(Self::from_export(&export, labels))
    }

    fn from_export(export: &crate::MeetingExport, labels: HashMap<String, String>) -> Self {
        let transcripts = export
            .transcripts
            .iter()
            .map(|transcript| {
                let mut words = transcript
                    .words
                    .iter()
                    .filter_map(|word| {
                        let text = word.get("text")?.as_str()?.to_string();
                        Some(IWord {
                            id: word
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            text,
                            start_ms: word.get("start_ms").and_then(Value::as_i64).unwrap_or(0),
                            end_ms: word.get("end_ms").and_then(Value::as_i64).unwrap_or(0),
                            channel: word.get("channel").and_then(Value::as_i64).unwrap_or(0),
                            speaker_index: word.get("speaker_index").and_then(Value::as_i64),
                        })
                    })
                    .collect::<Vec<_>>();
                words.sort_by_key(|word| (word.start_ms, word.end_ms));
                ITranscript {
                    id: transcript.id.clone(),
                    words,
                }
            })
            .collect();
        Self {
            transcripts,
            labels,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.transcripts.iter().all(|t| t.words.is_empty())
    }

    /// Words per transcript, in time order, for the enhanced-notes engine.
    pub fn engine_transcripts(&self) -> Vec<(String, Vec<EngineWord>)> {
        self.transcripts
            .iter()
            .map(|transcript| {
                let words = transcript
                    .words
                    .iter()
                    .map(|word| EngineWord {
                        id: word.id.clone(),
                        text: word.text.clone(),
                        start_ms: word.start_ms,
                        end_ms: word.end_ms,
                        channel: word.channel,
                        speaker_index: word.speaker_index,
                        label: self.labels.get(&word.id).cloned(),
                    })
                    .collect();
                (transcript.id.clone(), words)
            })
            .collect()
    }

    fn speaker(&self, word: &IWord) -> String {
        self.labels.get(&word.id).cloned().unwrap_or_else(|| {
            if word.channel == 0 {
                "You".to_string()
            } else {
                "Speaker".to_string()
            }
        })
    }

    /// Words addressed by a source ref: the word-id range when both ids are
    /// found in order, else the words overlapping the ref's time range.
    fn words_for<'a>(&'a self, source: &SourceRef) -> Vec<(&'a ITranscript, &'a IWord)> {
        let candidates: Vec<&ITranscript> = if source.transcript_id.is_empty() {
            self.transcripts.iter().collect()
        } else {
            self.transcripts
                .iter()
                .filter(|t| t.id == source.transcript_id)
                .collect()
        };
        for transcript in &candidates {
            let start = transcript
                .words
                .iter()
                .position(|w| !source.start_word_id.is_empty() && w.id == source.start_word_id);
            let end = transcript
                .words
                .iter()
                .position(|w| !source.end_word_id.is_empty() && w.id == source.end_word_id);
            if let (Some(start), Some(end)) = (start, end)
                && start <= end
            {
                return transcript.words[start..=end]
                    .iter()
                    .map(|word| (*transcript, word))
                    .collect();
            }
        }
        let mut out = Vec::new();
        if source.end_ms >= source.start_ms && (source.start_ms != 0 || source.end_ms != 0) {
            for transcript in candidates {
                for word in &transcript.words {
                    if word.end_ms >= source.start_ms && word.start_ms <= source.end_ms {
                        out.push((transcript, word));
                    }
                }
            }
        }
        out
    }

    /// Resolve a citation to speakers, times and the cited words.
    pub fn resolve(&self, source: &SourceRef) -> ResolvedSource {
        let words = self.words_for(source);
        let mut speakers: Vec<String> = Vec::new();
        for (_, word) in &words {
            let speaker = self.speaker(word);
            if !speakers.contains(&speaker) {
                speakers.push(speaker);
            }
        }
        let (start_ms, end_ms) = match (words.first(), words.last()) {
            (Some((_, first)), Some((_, last))) => (first.start_ms, last.end_ms),
            _ => (source.start_ms, source.end_ms),
        };
        let text = truncate_chars(
            &join_words(words.iter().map(|(_, w)| w.text.as_str())),
            MAX_SOURCE_TEXT_CHARS,
        );
        ResolvedSource {
            transcript_id: source.transcript_id.clone(),
            start_word_id: source.start_word_id.clone(),
            end_word_id: source.end_word_id.clone(),
            start_ms,
            end_ms,
            start: format_mmss(start_ms),
            end: format_mmss(end_ms),
            speakers,
            text,
            resolved: !words.is_empty(),
        }
    }

    /// Speaker turns of all transcripts in order, with ids `s1..sN`.
    pub fn turns(&self) -> Vec<Turn> {
        let mut turns = Vec::new();
        for transcript in &self.transcripts {
            group_turns(self, transcript, &mut turns);
        }
        for (index, turn) in turns.iter_mut().enumerate() {
            turn.sid = format!("s{}", index + 1);
        }
        turns
    }
}

fn group_turns(index: &TranscriptIndex, transcript: &ITranscript, out: &mut Vec<Turn>) {
    let words = &transcript.words;
    let flush = |open: Option<(String, usize, usize)>, out: &mut Vec<Turn>| {
        let Some((speaker, first, last)) = open else {
            return;
        };
        let slice = &words[first..=last];
        let text = join_words(slice.iter().map(|w| w.text.as_str()));
        if text.trim().is_empty() {
            return;
        }
        out.push(Turn {
            sid: String::new(),
            transcript_id: transcript.id.clone(),
            speaker,
            start_ms: slice[0].start_ms,
            end_ms: slice[slice.len() - 1].end_ms,
            start_word_id: slice[0].id.clone(),
            end_word_id: slice[slice.len() - 1].id.clone(),
            text,
        });
    };
    // (speaker, first word position, last word position)
    let mut open: Option<(String, usize, usize)> = None;
    let mut open_chars = 0usize;
    for (position, word) in words.iter().enumerate() {
        if word.text.trim().is_empty() {
            continue;
        }
        let speaker = index.speaker(word);
        let split = match &open {
            None => true,
            Some((current, _, last)) => {
                *current != speaker
                    || word.start_ms - words[*last].end_ms > TURN_GAP_MS
                    || (open_chars >= TURN_MAX_CHARS
                        && words[*last]
                            .text
                            .trim_end()
                            .ends_with(['.', '?', '!', '。', '？', '！']))
            }
        };
        if split {
            flush(open.take(), out);
            open = Some((speaker, position, position));
            open_chars = 0;
        } else if let Some((_, _, last)) = open.as_mut() {
            *last = position;
        }
        open_chars += word.text.trim().chars().count() + 1;
    }
    flush(open.take(), out);
}

// ---------------------------------------------------------------------------
// get_transcript_segment
// ---------------------------------------------------------------------------

pub async fn get_transcript_segment(
    pool: &SqlitePool,
    input: GetTranscriptSegmentInput,
) -> Result<TranscriptSegment> {
    let transcript_id = input.transcript_id.clone().unwrap_or_default();
    let source = match (
        input.start_word_id.as_deref().filter(|id| !id.is_empty()),
        input.end_word_id.as_deref().filter(|id| !id.is_empty()),
        input.start_ms,
        input.end_ms,
    ) {
        (Some(start), Some(end), ..) => SourceRef {
            transcript_id,
            start_word_id: start.to_string(),
            end_word_id: end.to_string(),
            start_ms: input.start_ms.unwrap_or(0),
            end_ms: input.end_ms.unwrap_or(0),
        },
        (None, None, Some(start), Some(end)) => {
            if end < start {
                return Err(Error::Invalid("end_ms must not be before start_ms".into()));
            }
            SourceRef {
                transcript_id,
                start_word_id: String::new(),
                end_word_id: String::new(),
                start_ms: start,
                end_ms: end,
            }
        }
        _ => {
            return Err(Error::Invalid(
                "provide both start_word_id and end_word_id, or both start_ms and end_ms".into(),
            ));
        }
    };
    let max_words = input
        .max_words
        .unwrap_or(DEFAULT_SEGMENT_WORDS)
        .clamp(1, MAX_SEGMENT_WORDS) as usize;

    exists_or_not_found(pool, &input.meeting_id).await?;
    let index = TranscriptIndex::load(pool, &input.meeting_id).await?;
    let found = index.words_for(&source);
    let truncated = found.len() > max_words;

    // Group the selected words into speaker turns.
    struct Open<'a> {
        transcript_id: String,
        speaker: String,
        words: Vec<&'a IWord>,
    }
    fn close(open: Option<Open<'_>>, segments: &mut Vec<SegmentTurn>) {
        let Some(open) = open else { return };
        let text = join_words(open.words.iter().map(|w| w.text.as_str()));
        if text.trim().is_empty() {
            return;
        }
        let start_ms = open.words[0].start_ms;
        let end_ms = open.words[open.words.len() - 1].end_ms;
        segments.push(SegmentTurn {
            transcript_id: open.transcript_id,
            speaker: open.speaker,
            start_ms,
            end_ms,
            start: format_mmss(start_ms),
            end: format_mmss(end_ms),
            text,
        });
    }
    let mut segments: Vec<SegmentTurn> = Vec::new();
    let mut current: Option<Open<'_>> = None;
    let mut word_count = 0usize;
    for (transcript, word) in found.into_iter().take(max_words) {
        word_count += 1;
        let speaker = index.speaker(word);
        let same = current.as_ref().is_some_and(|open| {
            open.transcript_id == transcript.id
                && open.speaker == speaker
                && open
                    .words
                    .last()
                    .is_some_and(|last| word.start_ms - last.end_ms <= TURN_GAP_MS)
        });
        if !same {
            close(current.take(), &mut segments);
            current = Some(Open {
                transcript_id: transcript.id.clone(),
                speaker,
                words: Vec::new(),
            });
        }
        if let Some(open) = current.as_mut() {
            open.words.push(word);
        }
    }
    close(current.take(), &mut segments);

    let text = segments
        .iter()
        .map(|turn| format!("[{}] {}: {}", turn.start, turn.speaker, turn.text))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(TranscriptSegment {
        meeting_id: input.meeting_id,
        segments,
        text,
        word_count,
        truncated,
    })
}

async fn exists_or_not_found(pool: &SqlitePool, meeting_id: &str) -> Result<()> {
    let exists = anlg_db_app::get_session(pool, meeting_id)
        .await
        .map_err(|source| Error::Database {
            action: "load meeting",
            source,
        })?
        .is_some();
    if exists {
        Ok(())
    } else {
        Err(Error::NotFound(format!("meeting '{meeting_id}'")))
    }
}

// ---------------------------------------------------------------------------
// Enhanced notes document parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct RawAi {
    ai_id: String,
    text: String,
    src: Vec<SourceRef>,
    checked: bool,
    owner: Option<String>,
    due: Option<String>,
}

#[derive(Debug, Clone)]
struct RawLine {
    text: String,
    kind: &'static str,
    depth: u32,
    ai: Vec<RawAi>,
}

#[derive(Debug, Default)]
struct ParsedDoc {
    lines: Vec<RawLine>,
    overview: Vec<RawAi>,
    decisions: Vec<RawAi>,
    actions: Vec<RawAi>,
    questions: Vec<RawAi>,
}

impl ParsedDoc {
    fn has_ai(&self) -> bool {
        !(self.overview.is_empty()
            && self.decisions.is_empty()
            && self.actions.is_empty()
            && self.questions.is_empty()
            && self.lines.iter().all(|l| l.ai.is_empty()))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Overview,
    Decisions,
    Actions,
    Questions,
}

fn group_for_heading(text: &str) -> Option<Group> {
    match text.trim().to_lowercase().as_str() {
        "overview" => Some(Group::Overview),
        "decisions" => Some(Group::Decisions),
        "action items" => Some(Group::Actions),
        "open questions" => Some(Group::Questions),
        _ => None,
    }
}

fn node_type(node: &Value) -> &str {
    node.get("type").and_then(Value::as_str).unwrap_or("")
}

fn children(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn is_list(node: &Value) -> bool {
    matches!(node_type(node), "bulletList" | "orderedList" | "taskList")
}

fn is_item(node: &Value) -> bool {
    matches!(node_type(node), "listItem" | "taskItem")
}

fn ai_id(node: &Value) -> Option<&str> {
    node.get("attrs")?
        .get("aiId")?
        .as_str()
        .filter(|id| !id.is_empty())
}

fn all_ai(list: &Value) -> bool {
    let items = children(list);
    !items.is_empty()
        && items
            .iter()
            .all(|item| is_item(item) && ai_id(item).is_some())
}

fn push_inline_text(node: &Value, out: &mut String) {
    if let Some(text) = node.get("text").and_then(Value::as_str) {
        out.push_str(text);
        return;
    }
    match node_type(node) {
        "hardBreak" => {
            out.push(' ');
            return;
        }
        kind if kind.starts_with("mention-") => {
            let attrs = node.get("attrs");
            for key in ["label", "name", "id"] {
                if let Some(value) = attrs.and_then(|a| a.get(key)).and_then(Value::as_str) {
                    out.push_str(value);
                    return;
                }
            }
            return;
        }
        _ => {}
    }
    for child in children(node) {
        push_inline_text(child, out);
        if matches!(node_type(child), "paragraph" | "heading") {
            out.push(' ');
        }
    }
}

fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn plain_text(node: &Value) -> String {
    let mut out = String::new();
    push_inline_text(node, &mut out);
    clean(&out)
}

/// Text of a list item without its nested lists.
fn item_text(item: &Value) -> String {
    let mut parts = Vec::new();
    for child in children(item) {
        if !is_list(child) {
            let text = plain_text(child);
            if !text.is_empty() {
                parts.push(text);
            }
        }
    }
    parts.join(" ")
}

fn parse_src(value: &Value) -> Vec<SourceRef> {
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|entry| {
            let text = |long: &str, short: &str| {
                entry
                    .get(long)
                    .or_else(|| entry.get(short))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            let number = |long: &str, short: &str| {
                entry
                    .get(long)
                    .or_else(|| entry.get(short))
                    .and_then(Value::as_i64)
            };
            Some(SourceRef {
                transcript_id: text("transcript_id", "t")?,
                start_word_id: text("start_word_id", "s")?,
                end_word_id: text("end_word_id", "e")?,
                start_ms: number("start_ms", "a").unwrap_or(0),
                end_ms: number("end_ms", "b").unwrap_or(0),
            })
        })
        .collect()
}

fn raw_ai(item: &Value) -> RawAi {
    let attrs = item.get("attrs");
    let attr_text = |key: &str| {
        attrs
            .and_then(|a| a.get(key))
            .and_then(Value::as_str)
            .map(clean)
            .filter(|v| !v.is_empty())
    };
    RawAi {
        ai_id: ai_id(item).unwrap_or_default().to_string(),
        text: item_text(item),
        src: attrs
            .and_then(|a| a.get("src"))
            .map(parse_src)
            .unwrap_or_default(),
        checked: attrs
            .and_then(|a| a.get("checked"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        owner: attr_text("owner"),
        due: attr_text("due"),
    }
}

fn parse_doc(doc: &Value) -> ParsedDoc {
    let mut parsed = ParsedDoc::default();
    walk_blocks(children(doc), 0, &mut parsed);
    parsed
}

fn push_group(parsed: &mut ParsedDoc, group: Group, list: &Value) {
    for item in children(list) {
        if !is_item(item) {
            continue;
        }
        let ai = raw_ai(item);
        match group {
            Group::Overview => parsed.overview.push(ai),
            Group::Decisions => parsed.decisions.push(ai),
            Group::Actions => parsed.actions.push(ai),
            Group::Questions => parsed.questions.push(ai),
        }
    }
}

fn attach_ai(parsed: &mut ParsedDoc, target: Option<usize>, list: &Value) {
    for item in children(list) {
        let ai = raw_ai(item);
        match target.or(parsed.lines.len().checked_sub(1)) {
            Some(index) => parsed.lines[index].ai.push(ai),
            None => parsed.overview.push(ai),
        }
    }
}

fn walk_blocks(nodes: &[Value], depth: u32, parsed: &mut ParsedDoc) {
    let mut i = 0;
    while i < nodes.len() {
        let node = &nodes[i];
        match node_type(node) {
            "heading" => {
                let text = plain_text(node);
                if let (Some(group), Some(next)) = (group_for_heading(&text), nodes.get(i + 1))
                    && is_list(next)
                    && all_ai(next)
                {
                    push_group(parsed, group, next);
                    i += 2;
                    continue;
                }
                if !text.is_empty() {
                    parsed.lines.push(RawLine {
                        text,
                        kind: "heading",
                        depth,
                        ai: Vec::new(),
                    });
                }
            }
            "paragraph" => {
                let text = plain_text(node);
                if !text.is_empty() {
                    parsed.lines.push(RawLine {
                        text,
                        kind: "paragraph",
                        depth,
                        ai: Vec::new(),
                    });
                }
            }
            _ if is_list(node) => {
                if all_ai(node) {
                    attach_ai(parsed, None, node);
                } else {
                    walk_list(children(node), depth, parsed);
                }
            }
            "blockquote" => walk_blocks(children(node), depth, parsed),
            _ => {
                let text = plain_text(node);
                if !text.is_empty() {
                    parsed.lines.push(RawLine {
                        text,
                        kind: "paragraph",
                        depth,
                        ai: Vec::new(),
                    });
                }
            }
        }
        i += 1;
    }
}

fn walk_list(items: &[Value], depth: u32, parsed: &mut ParsedDoc) {
    for item in items {
        if !is_item(item) {
            continue;
        }
        if ai_id(item).is_some() {
            let ai = raw_ai(item);
            match parsed.lines.len().checked_sub(1) {
                Some(index) => parsed.lines[index].ai.push(ai),
                None => parsed.overview.push(ai),
            }
            continue;
        }
        parsed.lines.push(RawLine {
            text: item_text(item),
            kind: "list_item",
            depth,
            ai: Vec::new(),
        });
        let own = parsed.lines.len() - 1;
        for child in children(item) {
            if is_list(child) {
                if all_ai(child) {
                    attach_ai(parsed, Some(own), child);
                } else {
                    walk_list(children(child), depth + 1, parsed);
                }
            }
        }
    }
}

/// The user's own typed lines (AI lines excluded), in document order.
pub fn typed_lines(body: &str, body_format: &str) -> Vec<String> {
    if body_format == "prosemirror_json"
        && let Ok(doc) = serde_json::from_str::<Value>(body)
    {
        return parse_doc(&doc)
            .lines
            .into_iter()
            .map(|line| line.text)
            .filter(|text| !text.is_empty())
            .collect();
    }
    body.lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(['-', '*', '+', '#'])
                .trim()
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

// ---------------------------------------------------------------------------
// get_enhanced_notes
// ---------------------------------------------------------------------------

pub async fn get_enhanced_notes(
    pool: &SqlitePool,
    input: GetEnhancedNotesInput,
) -> Result<EnhancedNotes> {
    let session = anlg_db_app::get_session(pool, &input.meeting_id)
        .await
        .map_err(|source| Error::Database {
            action: "load meeting",
            source,
        })?
        .ok_or_else(|| Error::NotFound(format!("meeting '{}'", input.meeting_id)))?;
    let documents = anlg_db_app::list_session_documents(pool, &input.meeting_id)
        .await
        .map_err(|source| Error::Database {
            action: "load documents",
            source,
        })?;

    let mut best: Option<(String, ParsedDoc, String, Value)> = None;
    for document in documents {
        if document.body_format != "prosemirror_json" || !document.body.contains("\"aiId\"") {
            continue;
        }
        if let Some(wanted) = &input.document_id
            && &document.id != wanted
        {
            continue;
        }
        let Ok(doc) = serde_json::from_str::<Value>(&document.body) else {
            continue;
        };
        let parsed = parse_doc(&doc);
        if !parsed.has_ai() {
            continue;
        }
        let newer = best
            .as_ref()
            .is_none_or(|(_, _, updated, _)| document.updated_at >= *updated);
        if newer {
            let generation =
                serde_json::from_str(&document.generation_metadata_json).unwrap_or(Value::Null);
            best = Some((document.id, parsed, document.updated_at, generation));
        }
    }
    let Some((document_id, parsed, updated_at, generation)) = best else {
        return Err(Error::NotFound(format!(
            "enhanced notes for meeting '{}'",
            input.meeting_id
        )));
    };

    let index = TranscriptIndex::load(pool, &input.meeting_id).await?;
    Ok(build_enhanced_notes(
        &input.meeting_id,
        &session.title,
        "saved",
        Some(document_id),
        updated_at,
        generation,
        parsed,
        &index,
    ))
}

#[allow(clippy::too_many_arguments)]
fn build_enhanced_notes(
    meeting_id: &str,
    title: &str,
    origin: &str,
    document_id: Option<String>,
    updated_at: String,
    generation: Value,
    parsed: ParsedDoc,
    index: &TranscriptIndex,
) -> EnhancedNotes {
    let unresolved = std::cell::Cell::new(0usize);
    let resolve = |refs: &[SourceRef]| -> Vec<ResolvedSource> {
        refs.iter()
            .map(|source| {
                let resolved = index.resolve(source);
                if !resolved.resolved {
                    unresolved.set(unresolved.get() + 1);
                }
                resolved
            })
            .collect()
    };
    let bullet = |ai: &RawAi| AiBullet {
        ai_id: ai.ai_id.clone(),
        text: ai.text.clone(),
        sources: resolve(&ai.src),
    };

    let lines = parsed
        .lines
        .iter()
        .map(|line| NoteLine {
            text: line.text.clone(),
            kind: line.kind.to_string(),
            depth: line.depth,
            ai_bullets: line.ai.iter().map(&bullet).collect(),
        })
        .collect();
    let overview = parsed.overview.iter().map(&bullet).collect();
    let decisions = parsed.decisions.iter().map(&bullet).collect();
    let open_questions = parsed.questions.iter().map(&bullet).collect();
    let action_items = parsed
        .actions
        .iter()
        .map(|ai| {
            let (owner, due) = action_owner_due(ai, &generation);
            EnhancedActionItem {
                ai_id: ai.ai_id.clone(),
                text: ai.text.clone(),
                owner,
                due,
                done: ai.checked,
                sources: resolve(&ai.src),
            }
        })
        .collect();

    let mut notes = EnhancedNotes {
        meeting_id: meeting_id.to_string(),
        title: title.to_string(),
        origin: origin.to_string(),
        document_id,
        updated_at,
        generation,
        lines,
        overview,
        decisions,
        action_items,
        open_questions,
        unresolved_sources: unresolved.get(),
        markdown: String::new(),
    };
    notes.markdown = render_markdown(&notes);
    notes
}

/// `(owner, due)` from a trailing `(owner: X; due: Y)`, `(owner: X)` or `(due: Y)`.
fn owner_due_suffix(text: &str) -> (Option<String>, Option<String>) {
    let text = text.trim_end();
    let Some(inner) = text
        .strip_suffix(')')
        .and_then(|rest| rest.rsplit_once('(').map(|(_, inner)| inner))
    else {
        return (None, None);
    };
    let (mut owner, mut due) = (None, None);
    for part in inner.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("owner:") {
            owner = Some(clean(value)).filter(|v| !v.is_empty());
        } else if let Some(value) = part.strip_prefix("due:") {
            due = Some(clean(value)).filter(|v| !v.is_empty());
        } else {
            return (None, None);
        }
    }
    (owner, due)
}

/// Owner and due date: `owner`/`due` node attributes first, then an
/// `action_items` array in the generation metadata matched by `ai_id` or text.
fn action_owner_due(ai: &RawAi, generation: &Value) -> (Option<String>, Option<String>) {
    if ai.owner.is_some() || ai.due.is_some() {
        return (ai.owner.clone(), ai.due.clone());
    }
    // The enhanced-notes engine writes them into the text: "task (owner: Dana; due: Friday)".
    let (suffix_owner, suffix_due) = owner_due_suffix(&ai.text);
    if suffix_owner.is_some() || suffix_due.is_some() {
        return (suffix_owner, suffix_due);
    }
    let wanted = clean(&ai.text.to_lowercase());
    let from_meta = generation
        .get("action_items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find(|item| {
                let id = item
                    .get("ai_id")
                    .or_else(|| item.get("aiId"))
                    .and_then(Value::as_str);
                id == Some(ai.ai_id.as_str())
                    || item
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| clean(&text.to_lowercase()) == wanted)
            })
        });
    let get = |key: &str| {
        from_meta
            .and_then(|item| item.get(key))
            .and_then(Value::as_str)
            .map(clean)
            .filter(|v| !v.is_empty())
    };
    (get("owner"), get("due"))
}

// ---------------------------------------------------------------------------
// Preview support: inputs and result assembly
// ---------------------------------------------------------------------------

/// Everything a local model needs to enhance a meeting's notes.
#[derive(Debug, Clone)]
pub struct EnhanceInputs {
    pub meeting_id: String,
    pub title: String,
    pub participants: Vec<String>,
    /// The user's typed lines in order; empty when they typed nothing.
    pub typed_lines: Vec<String>,
    /// The user's note as a TipTap document, the engine's input. Built from `typed_lines` when
    /// the stored note is not a ProseMirror document.
    pub raw_doc: Value,
    pub index: TranscriptIndex,
}

/// A TipTap document with one paragraph per line.
pub fn doc_from_lines(lines: &[String]) -> Value {
    let content: Vec<Value> = lines
        .iter()
        .map(|line| {
            serde_json::json!({"type": "paragraph", "content": [{"type": "text", "text": line}]})
        })
        .collect();
    serde_json::json!({"type": "doc", "content": content})
}

pub async fn load_enhance_inputs(pool: &SqlitePool, meeting_id: &str) -> Result<EnhanceInputs> {
    let meeting = crate::get_meeting(
        pool,
        crate::GetMeetingInput {
            meeting_id: meeting_id.to_string(),
        },
    )
    .await?;
    let note = anlg_db_app::get_session_note(pool, meeting_id)
        .await
        .map_err(|source| Error::Database {
            action: "load note",
            source,
        })?;
    let typed = match &note {
        Some(note) => typed_lines(&note.body, &note.body_format),
        None => Vec::new(),
    };
    let stored_doc = note
        .as_ref()
        .filter(|note| note.body_format == "prosemirror_json")
        .and_then(|note| serde_json::from_str::<Value>(&note.body).ok())
        .filter(|doc| doc.get("type").and_then(Value::as_str) == Some("doc"));
    let raw_doc = stored_doc.unwrap_or_else(|| doc_from_lines(&typed));
    let index = TranscriptIndex::load(pool, meeting_id).await?;
    let participants = meeting
        .participants
        .iter()
        .map(|p| p.display_name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    Ok(EnhanceInputs {
        meeting_id: meeting.id,
        title: meeting.title,
        participants,
        typed_lines: typed,
        raw_doc,
        index,
    })
}

/// Assemble an unsaved [`EnhancedNotes`] (origin `preview`) from the document the enhanced-notes
/// engine produced. It is parsed exactly as a saved document is, so a preview shows what the app
/// would save, anchors and sources included.
pub fn assemble_preview_from_doc(
    inputs: &EnhanceInputs,
    doc: &Value,
    generation: Value,
) -> EnhancedNotes {
    build_enhanced_notes(
        &inputs.meeting_id,
        &inputs.title,
        "preview",
        None,
        String::new(),
        generation,
        parse_doc(doc),
        &inputs.index,
    )
}

// ---------------------------------------------------------------------------
// Markdown rendering
// ---------------------------------------------------------------------------

type FootnoteKey = (String, String, String, i64, i64);

#[derive(Default)]
struct Footnotes {
    order: Vec<ResolvedSource>,
    numbers: HashMap<FootnoteKey, usize>,
}

impl Footnotes {
    fn refs(&mut self, sources: &[ResolvedSource]) -> String {
        let mut out = String::new();
        let mut seen = HashSet::new();
        for source in sources {
            let key = (
                source.transcript_id.clone(),
                source.start_word_id.clone(),
                source.end_word_id.clone(),
                source.start_ms,
                source.end_ms,
            );
            let number = match self.numbers.get(&key) {
                Some(number) => *number,
                None => {
                    self.order.push(source.clone());
                    self.numbers.insert(key, self.order.len());
                    self.order.len()
                }
            };
            if seen.insert(number) {
                out.push_str(&format!("[^{number}]"));
            }
        }
        if out.is_empty() {
            out
        } else {
            format!(" {out}")
        }
    }
}

fn footnote_line(number: usize, source: &ResolvedSource) -> String {
    let range = if source.start == source.end {
        format!("[{}]", source.start)
    } else {
        format!("[{}–{}]", source.start, source.end)
    };
    if !source.resolved {
        return format!("[^{number}]: {range} (transcript words no longer available)");
    }
    let speakers = if source.speakers.is_empty() {
        String::new()
    } else {
        format!(" {}:", source.speakers.join(", "))
    };
    format!(
        "[^{number}]: {range}{speakers} \"{}\"",
        clean(&source.text).replace('"', "'")
    )
}

fn md_bullet(foot: &mut Footnotes, indent: usize, bullet: &AiBullet) -> String {
    format!(
        "{}- {}{}\n",
        "  ".repeat(indent),
        bullet.text,
        foot.refs(&bullet.sources)
    )
}

/// Markdown with the user's lines in order, AI bullets nested below them and
/// footnote-style citations whose definitions start with `[mm:ss]`.
pub fn render_markdown(notes: &EnhancedNotes) -> String {
    let mut out = String::new();
    let mut foot = Footnotes::default();
    if !notes.title.trim().is_empty() {
        out.push_str(&format!("# {}\n\n", notes.title.trim()));
    }
    if !notes.overview.is_empty() {
        out.push_str("## Overview\n\n");
        for bullet in &notes.overview {
            out.push_str(&md_bullet(&mut foot, 0, bullet));
        }
        out.push('\n');
    }
    for line in &notes.lines {
        match line.kind.as_str() {
            "heading" => {
                out.push_str(&format!("\n### {}\n\n", line.text));
                for bullet in &line.ai_bullets {
                    out.push_str(&md_bullet(&mut foot, 0, bullet));
                }
            }
            "list_item" => {
                let indent = line.depth as usize;
                out.push_str(&format!("{}- {}\n", "  ".repeat(indent), line.text));
                for bullet in &line.ai_bullets {
                    out.push_str(&md_bullet(&mut foot, indent + 1, bullet));
                }
            }
            _ => {
                out.push_str(&format!("\n{}\n", line.text));
                if !line.ai_bullets.is_empty() {
                    out.push('\n');
                }
                for bullet in &line.ai_bullets {
                    out.push_str(&md_bullet(&mut foot, 0, bullet));
                }
            }
        }
    }
    if !notes.decisions.is_empty() {
        out.push_str("\n## Decisions\n\n");
        for bullet in &notes.decisions {
            out.push_str(&md_bullet(&mut foot, 0, bullet));
        }
    }
    if !notes.action_items.is_empty() {
        out.push_str("\n## Action items\n\n");
        for item in &notes.action_items {
            let mut meta = Vec::new();
            if let Some(owner) = &item.owner {
                meta.push(format!("owner: {owner}"));
            }
            if let Some(due) = &item.due {
                meta.push(format!("due: {due}"));
            }
            let meta = if meta.is_empty() {
                String::new()
            } else {
                format!(" ({})", meta.join(", "))
            };
            out.push_str(&format!(
                "- [{}] {}{}{}\n",
                if item.done { "x" } else { " " },
                item.text,
                meta,
                foot.refs(&item.sources)
            ));
        }
    }
    if !notes.open_questions.is_empty() {
        out.push_str("\n## Open questions\n\n");
        for bullet in &notes.open_questions {
            out.push_str(&md_bullet(&mut foot, 0, bullet));
        }
    }
    if !foot.order.is_empty() {
        out.push('\n');
        for (i, source) in foot.order.iter().enumerate() {
            out.push_str(&footnote_line(i + 1, source));
            out.push('\n');
        }
    }
    out.trim().to_string() + "\n"
}

// ---------------------------------------------------------------------------
// search_meetings
// ---------------------------------------------------------------------------

fn parse_terms(query: &str) -> Vec<String> {
    fn push(current: &mut String, terms: &mut Vec<String>) {
        let term = clean(&current.to_lowercase());
        current.clear();
        if !term.is_empty() && !terms.contains(&term) && terms.len() < MAX_SEARCH_TERMS {
            terms.push(term);
        }
    }
    let mut terms: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in query.chars() {
        match c {
            '"' => {
                push(&mut current, &mut terms);
                quoted = !quoted;
            }
            c if c.is_whitespace() && !quoted => push(&mut current, &mut terms),
            c => current.push(c),
        }
    }
    push(&mut current, &mut terms);
    terms
}

fn like_pattern(term: &str) -> String {
    let mut out = String::from("%");
    for c in term.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

struct Haystack {
    chars: Vec<char>,
    folded: Vec<char>,
}

impl Haystack {
    fn new(text: &str) -> Self {
        let chars: Vec<char> = text
            .chars()
            .map(|c| if c.is_whitespace() { ' ' } else { c })
            .collect();
        let folded = chars.iter().copied().map(fold).collect();
        Self { chars, folded }
    }

    fn find(&self, term: &[char]) -> Option<usize> {
        if term.is_empty() || term.len() > self.folded.len() {
            return None;
        }
        self.folded.windows(term.len()).position(|w| w == term)
    }

    fn snippet(&self, at: usize, len: usize) -> String {
        let start = at.saturating_sub(SNIPPET_RADIUS);
        let end = (at + len + SNIPPET_RADIUS).min(self.chars.len());
        let body = clean(&self.chars[start..end].iter().collect::<String>());
        format!(
            "{}{}{}",
            if start > 0 { "…" } else { "" },
            body,
            if end < self.chars.len() { "…" } else { "" }
        )
    }
}

struct SearchSource {
    label: String,
    document_id: Option<String>,
    transcript_id: Option<String>,
    haystack: Haystack,
    /// Char index -> word start_ms (transcripts only).
    times: Option<Vec<i64>>,
}

fn search_sources(
    session_title: &str,
    documents: &[anlg_db_app::SessionDocumentRow],
    index: &TranscriptIndex,
) -> Vec<SearchSource> {
    let mut sources = vec![SearchSource {
        label: "title".into(),
        document_id: None,
        transcript_id: None,
        haystack: Haystack::new(session_title),
        times: None,
    }];
    for document in documents {
        let markdown = crate::body_to_markdown(&document.body, &document.body_format);
        let label = if document.body.contains("\"aiId\"") {
            "enhanced_notes".to_string()
        } else {
            match document.kind.as_str() {
                "note" => "typed_notes".to_string(),
                "summary" | "template_output" => "summary".to_string(),
                other => other.to_string(),
            }
        };
        let text = format!("{} {}", document.title, markdown);
        sources.push(SearchSource {
            label,
            document_id: Some(document.id.clone()),
            transcript_id: None,
            haystack: Haystack::new(&text),
            times: None,
        });
    }
    for transcript in &index.transcripts {
        let mut text = String::new();
        let mut times: Vec<i64> = Vec::new();
        let mut previous: Option<char> = None;
        for word in &transcript.words {
            let trimmed = word.text.trim();
            if trimmed.is_empty() {
                continue;
            }
            let first = trimmed.chars().next().unwrap_or(' ');
            if let Some(prev) = previous
                && !(is_cjk(prev) && is_cjk(first))
            {
                text.push(' ');
                times.push(word.start_ms);
            }
            for c in trimmed.chars() {
                text.push(if c.is_whitespace() { ' ' } else { c });
                times.push(word.start_ms);
            }
            previous = trimmed.chars().last();
        }
        sources.push(SearchSource {
            label: "transcript".into(),
            document_id: None,
            transcript_id: Some(transcript.id.clone()),
            haystack: Haystack::new(&text),
            times: Some(times),
        });
    }
    sources
}

/// Returns snippets when every term is found somewhere in the meeting.
fn verify_and_snippet(terms: &[Vec<char>], sources: &[SearchSource]) -> Option<Vec<SearchSnippet>> {
    let mut snippets: Vec<SearchSnippet> = Vec::new();
    for term in terms {
        let mut found = false;
        for source in sources {
            let Some(at) = source.haystack.find(term) else {
                continue;
            };
            found = true;
            if snippets.len() >= MAX_SNIPPETS_PER_HIT {
                continue;
            }
            let start_ms = source
                .times
                .as_ref()
                .and_then(|times| times.get(at).copied());
            let text = source.haystack.snippet(at, term.len());
            if snippets
                .iter()
                .any(|s| s.source == source.label && s.text == text)
            {
                continue;
            }
            snippets.push(SearchSnippet {
                source: source.label.clone(),
                text,
                time: start_ms.map(format_mmss),
                start_ms,
                transcript_id: source.transcript_id.clone(),
                document_id: source.document_id.clone(),
            });
        }
        if !found {
            return None;
        }
    }
    Some(snippets)
}

/// Bounded full-text search over the local database. SQL `LIKE` narrows the
/// candidates (ASCII case-insensitive); a Rust pass then verifies matches
/// Unicode-aware, drops false positives from JSON keys, and builds snippets.
pub async fn search_meetings(pool: &SqlitePool, input: SearchMeetingsInput) -> Result<SearchPage> {
    let terms = parse_terms(&input.query);
    if terms.is_empty() {
        return Err(Error::Invalid("query must contain a search term".into()));
    }
    let limit = input
        .limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT);
    let offset = input.offset.unwrap_or(0);
    let folder_path = input
        .folder_path
        .as_deref()
        .map(|path| {
            crate::normalize_folder_path(path)
                .ok_or_else(|| Error::Invalid(format!("invalid folder path '{path}'")))
        })
        .transpose()?
        .filter(|path| !path.is_empty());
    let term_chars: Vec<Vec<char>> = terms
        .iter()
        .map(|t| t.chars().map(fold).collect())
        .collect();
    let wanted = offset as usize + limit as usize + 1;

    let mut hits: Vec<SearchHit> = Vec::new();
    let mut scanned = 0usize;
    let mut exhausted = false;
    let mut chunk_offset = 0i64;
    while hits.len() < wanted && scanned < MAX_SEARCH_SCAN {
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT s.id, s.title, s.started_at, s.created_at, s.folder_path
             FROM sessions s WHERE s.deleted_at IS NULL",
        );
        if let Some(folder) = &folder_path {
            query.push(" AND (s.folder_path = ");
            query.push_bind(folder.clone());
            query.push(" OR (s.folder_path > ");
            query.push_bind(format!("{folder}/"));
            query.push(" AND s.folder_path < ");
            query.push_bind(format!("{folder}0"));
            query.push("))");
        }
        for term in &terms {
            let pattern = like_pattern(term);
            query.push(" AND (lower(s.title) LIKE ");
            query.push_bind(pattern.clone());
            query.push(
                " ESCAPE '\\' OR EXISTS (SELECT 1 FROM session_documents d WHERE d.session_id = s.id
                 AND d.deleted_at IS NULL AND (lower(d.body) LIKE ",
            );
            query.push_bind(pattern.clone());
            query.push(" ESCAPE '\\' OR lower(d.title) LIKE ");
            query.push_bind(pattern.clone());
            query.push(
                " ESCAPE '\\')) OR EXISTS (SELECT 1 FROM transcripts t WHERE t.session_id = s.id
                 AND t.deleted_at IS NULL AND lower(t.words_json) LIKE ",
            );
            query.push_bind(pattern);
            query.push(" ESCAPE '\\'))");
        }
        query.push(
            " ORDER BY COALESCE(NULLIF(s.started_at, ''), s.created_at) DESC, s.created_at DESC, s.id DESC LIMIT ",
        );
        query.push_bind(SEARCH_CHUNK);
        query.push(" OFFSET ");
        query.push_bind(chunk_offset);
        let rows = query
            .build_query_as::<(String, String, String, String, String)>()
            .fetch_all(pool)
            .await
            .map_err(|source| Error::Database {
                action: "search meetings",
                source,
            })?;
        chunk_offset += SEARCH_CHUNK;
        let fetched = rows.len();
        for (id, title, started_at, created_at, folder) in rows {
            scanned += 1;
            let documents = anlg_db_app::list_session_documents(pool, &id)
                .await
                .map_err(|source| Error::Database {
                    action: "load documents",
                    source,
                })?;
            let index = TranscriptIndex::load(pool, &id).await?;
            let sources = search_sources(&title, &documents, &index);
            if let Some(snippets) = verify_and_snippet(&term_chars, &sources) {
                hits.push(SearchHit {
                    meeting_id: id,
                    title,
                    date: if started_at.is_empty() {
                        created_at
                    } else {
                        started_at
                    },
                    folder_path: stored_folder_path(folder),
                    snippets,
                });
                if hits.len() >= wanted {
                    break;
                }
            }
        }
        if (fetched as i64) < SEARCH_CHUNK {
            exhausted = true;
            break;
        }
    }

    let has_more = hits.len() > offset as usize + limit as usize;
    let total = (exhausted && !has_more).then_some(hits.len());
    let page: Vec<SearchHit> = hits
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();
    let pagination = pagination(offset, limit, page.len(), total, has_more);
    Ok(SearchPage {
        query: input.query,
        hits: page,
        pagination,
        scan_truncated: !exhausted && scanned >= MAX_SEARCH_SCAN,
    })
}

// ---------------------------------------------------------------------------
// Small text helpers
// ---------------------------------------------------------------------------

pub fn format_mmss(ms: i64) -> String {
    let total = (ms.max(0) / 1000) as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF
        | 0x3000..=0x303F)
}

fn join_words<'a>(words: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for word in words {
        let trimmed = word.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(prev) = out.chars().next_back() {
            let next = trimmed.chars().next().unwrap_or(' ');
            let cjk_boundary = is_cjk(prev) && is_cjk(next);
            let no_space_before = matches!(next, ',' | '.' | '!' | '?' | ';' | ':' | '%' | ')');
            if !cjk_boundary && !no_space_before {
                out.push(' ');
            }
        }
        out.push_str(trimmed);
    }
    out
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests;
