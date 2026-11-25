//! Granola-style enhanced notes, fully local.
//!
//! The user's own typed notes stay as the skeleton, verbatim and in order. A
//! local LLM adds AI bullets under each line, every one linked back to the
//! transcript words it came from, plus Decisions, Action items and Open
//! questions. Re-enhancing keeps the user's manual edits to AI lines.
//!
//! This crate is pure: all I/O goes through [`ExtractClient`] (an Ollama
//! implementation is behind the `ollama` feature).
//!
//! Transcript input uses the local [`Word`] type (the shape of
//! `transcripts.words_json`) rather than depending on the `transcript` crate,
//! which pulls in audio and template crates this engine does not need.

pub mod anchors;
pub mod chunk;
pub mod prompt;
pub mod reconcile;
pub mod render;
pub mod schema;
pub mod segments;
mod util;

#[cfg(feature = "ollama")]
pub mod ollama;

use std::future::Future;

use serde_json::Value;

pub use anchors::{Anchor, AnchorRef, extract_anchors};
pub use prompt::{ChineseScript, SYSTEM_PROMPT};
pub use reconcile::{Engine, Metadata, ReconcileStats};
pub use schema::{EnhanceOutput, output_json_schema};
pub use segments::{PromptSegment, SourceRef, SpeakerLabels, TranscriptInput, Word};

/// Bump when the document/metadata shape changes in a way that should re-trigger enhancement.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, thiserror::Error)]
pub enum EnhanceError {
    #[error("the session has no transcript text to enhance from")]
    NoTranscript,
    #[error("model request failed: {0}")]
    Client(String),
    #[error("model returned an unusable answer: {0}")]
    BadModelOutput(String),
    #[error("generated document is invalid: {0}")]
    InvalidDocument(String),
}

/// One structured-output call to a local model.
pub trait ExtractClient {
    fn model_name(&self) -> String;
    fn extract(
        &self,
        system: &str,
        user: &str,
        schema: &Value,
    ) -> impl Future<Output = Result<EnhanceOutput, EnhanceError>> + Send;
}

#[derive(Debug, Clone)]
pub struct EnhanceOptions {
    pub num_ctx: usize,
    pub chinese_script: ChineseScript,
}

impl Default for EnhanceOptions {
    fn default() -> Self {
        Self {
            num_ctx: 32_768,
            chinese_script: ChineseScript::Traditional,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnhanceInput {
    /// The user's raw typed note (TipTap JSON).
    pub raw_doc: Value,
    pub transcripts: Vec<TranscriptInput>,
    pub speaker_labels: SpeakerLabels,
    pub participants: Vec<String>,
    /// The currently saved enhanced document and its metadata, if any.
    pub prev_doc: Option<Value>,
    pub prev_metadata: Option<Metadata>,
    /// Discard the user's edits and deletions of AI lines.
    pub regenerate: bool,
    pub options: EnhanceOptions,
}

#[derive(Debug, Clone)]
pub struct EnhanceResult {
    pub doc: Value,
    pub metadata: Metadata,
    pub source_hash: String,
    pub stats: ReconcileStats,
    pub chunks: usize,
    pub report: schema::ValidationReport,
}

/// Hashes of the inputs: `(raw_hash, transcript_hash, source_hash)`.
/// `source_hash` changes when the typed notes (text) or the transcript change.
pub fn compute_hashes(
    raw_doc: &Value,
    transcripts: &[TranscriptInput],
) -> (String, String, String) {
    let raw_text = extract_anchors(raw_doc)
        .iter()
        .map(|a| a.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let raw_hash = util::sha_hex(&raw_text, 32);
    let transcript_hash = segments::transcripts_digest(transcripts);
    let source_hash = util::sha_hex(
        &format!(
            "v{}:{raw_hash}:{transcript_hash}",
            reconcile::METADATA_SCHEMA_VERSION
        ),
        32,
    );
    (raw_hash, transcript_hash, source_hash)
}

pub async fn enhance<C: ExtractClient>(
    input: EnhanceInput,
    client: &C,
) -> Result<EnhanceResult, EnhanceError> {
    let EnhanceInput {
        raw_doc,
        transcripts,
        speaker_labels,
        participants,
        prev_doc,
        prev_metadata,
        regenerate,
        options,
    } = input;

    let mut anchors = extract_anchors(&raw_doc);
    if anchors.is_empty() {
        anchors.push(anchors::overview_anchor());
    }
    let segments = segments::build_segments(&transcripts, &speaker_labels);
    if segments.is_empty() {
        return Err(EnhanceError::NoTranscript);
    }

    let schema = output_json_schema();
    let fixed_tokens = chunk::estimate_tokens(SYSTEM_PROMPT)
        + chunk::estimate_tokens(&schema.to_string())
        + chunk::estimate_tokens(&prompt::build_user_prompt(
            &anchors,
            &[],
            &participants,
            options.chinese_script,
            None,
        ));
    let ranges = chunk::plan_chunks(&segments, fixed_tokens, options.num_ctx);
    let total = ranges.len();

    let mut outputs = Vec::with_capacity(total);
    let mut report = schema::ValidationReport::default();
    for (i, range) in ranges.into_iter().enumerate() {
        let part = &segments[range];
        let user = prompt::build_user_prompt(
            &anchors,
            part,
            &participants,
            options.chinese_script,
            Some(prompt::ChunkInfo {
                index: i + 1,
                total,
            }),
        );
        let raw = client.extract(SYSTEM_PROMPT, &user, &schema).await?;
        let (valid, r) = schema::validate(raw, &anchors, part, &participants);
        report.unknown_anchors += r.unknown_anchors;
        report.dropped_bullets += r.dropped_bullets;
        report.unknown_sources += r.unknown_sources;
        outputs.push(valid);
    }
    let merged = chunk::merge_outputs(outputs, &anchors);

    let engine = Engine {
        name: "enhanced-notes".into(),
        version: ENGINE_VERSION.into(),
        model: client.model_name(),
        num_ctx: options.num_ctx,
        chunks: total,
    };
    let reconciled = reconcile::reconcile(
        prev_doc.as_ref(),
        prev_metadata.as_ref(),
        &anchors,
        &merged,
        &segments,
        regenerate,
        engine,
    );
    let (raw_hash, transcript_hash, source_hash) = compute_hashes(&raw_doc, &transcripts);
    let mut metadata = reconciled.metadata;
    metadata.raw_hash = raw_hash;
    metadata.transcript_hash = transcript_hash;

    let doc = render::render_doc(&raw_doc, &anchors, &reconciled.layout);
    let errors = anlg_tiptap::validate_tiptap_json(&doc);
    if !errors.is_empty() {
        return Err(EnhanceError::InvalidDocument(
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }

    Ok(EnhanceResult {
        doc,
        metadata,
        source_hash,
        stats: reconciled.stats,
        chunks: total,
        report,
    })
}
