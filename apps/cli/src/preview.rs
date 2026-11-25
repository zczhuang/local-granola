// Without the `ollama` feature nothing calls into this module.
#![cfg_attr(not(feature = "ollama"), allow(dead_code))]

//! Unsaved enhanced-notes preview for the MCP server.
//!
//! The preview runs the same engine the desktop app saves with, the
//! `enhanced-notes` crate, against a local Ollama model, and returns the
//! document it produced without writing anything. The anchors, the validation of
//! every citation against the real transcript turns and the source links are the
//! engine's, so the preview shows what the app would save. The backend sits
//! behind [`PreviewBackend`] so tests can swap the generator.

use std::future::Future;
use std::pin::Pin;

use anlg_agent_access as access;
#[cfg(feature = "ollama")]
use anlg_enhanced_notes as engine;
use serde_json::Value;
#[cfg(feature = "ollama")]
use serde_json::json;

/// Default base URL of the local Ollama daemon.
pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";
/// Default notes model (see the fork's COMMON brief).
pub const DEFAULT_MODEL: &str = "gemma4:26b-mlx";

#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error(
        "refusing to contact {0}: the notes preview only talks to a loopback Ollama (localhost, 127.0.0.0/8 or ::1)"
    )]
    NotLoopback(String),
    #[error("{0}")]
    Invalid(String),
    #[error("local model request failed: {0}")]
    Backend(String),
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a backend returns: the enhanced document the app would save, plus metadata about the run.
pub struct PreviewRun {
    /// TipTap document with the user's lines and the AI lines, as `enhance` renders it.
    pub doc: Value,
    pub generation: Value,
    pub warnings: Vec<String>,
}

/// A generator of unsaved enhanced notes.
pub trait PreviewBackend: Send + Sync {
    fn preview<'a>(
        &'a self,
        inputs: &'a access::EnhanceInputs,
        model: &'a str,
    ) -> BoxFuture<'a, Result<PreviewRun, PreviewError>>;
}

/// Turns an `OLLAMA_HOST` value into a base URL. Ollama itself accepts `0.0.0.0`, `[::]` and bare
/// `host`/`host:port` there (the common way to bind it), but those are bind addresses, not
/// something to connect to: the unspecified addresses become 127.0.0.1, a missing scheme becomes
/// `http://`, a missing port becomes 11434. Any other host is passed through unchanged, so a remote
/// host is still refused by `normalize_loopback_url`.
pub fn ollama_host_to_url(raw: &str) -> String {
    let raw = raw.trim();
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let Ok(mut url) = url::Url::parse(&with_scheme) else {
        return raw.to_string();
    };
    let unspecified = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_unspecified(),
        Some(url::Host::Ipv6(ip)) => ip.is_unspecified(),
        _ => false,
    };
    if unspecified {
        let _ = url.set_host(Some("127.0.0.1"));
    }
    if url.port().is_none() && matches!(url.scheme(), "http") {
        let _ = url.set_port(Some(11434));
    }
    url.to_string().trim_end_matches('/').to_string()
}

/// Accepts `host:port`, `http://host:port` and `http://host:port/` and refuses
/// anything that is not loopback.
pub fn normalize_loopback_url(raw: &str) -> Result<url::Url, PreviewError> {
    let raw = raw.trim();
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let url = url::Url::parse(&with_scheme)
        .map_err(|_| PreviewError::Invalid(format!("invalid Ollama URL '{raw}'")))?;
    if !matches!(url.scheme(), "http" | "https") || !anlg_local_only::is_local_url(&url) {
        return Err(PreviewError::NotLoopback(raw.to_string()));
    }
    Ok(url)
}

#[cfg(feature = "ollama")]
/// The `enhanced-notes` engine over a loopback Ollama.
pub struct EnhancedNotesBackend {
    base: url::Url,
}

#[cfg(feature = "ollama")]
impl EnhancedNotesBackend {
    pub fn new(base_url: &str) -> Result<Self, PreviewError> {
        Ok(Self {
            base: normalize_loopback_url(base_url)?,
        })
    }
}

#[cfg(feature = "ollama")]
impl PreviewBackend for EnhancedNotesBackend {
    fn preview<'a>(
        &'a self,
        inputs: &'a access::EnhanceInputs,
        model: &'a str,
    ) -> BoxFuture<'a, Result<PreviewRun, PreviewError>> {
        Box::pin(async move { self.run(inputs, model).await })
    }
}

#[cfg(feature = "ollama")]
/// The engine's input for a meeting. Speaker labels are the names the app shows, keyed by
/// `channel:speaker_index` (or `channel` for words without a speaker index).
fn engine_input(inputs: &access::EnhanceInputs) -> engine::EnhanceInput {
    let mut labels = std::collections::BTreeMap::new();
    let transcripts = inputs
        .index
        .engine_transcripts()
        .into_iter()
        .map(|(transcript_id, words)| {
            let words = words
                .into_iter()
                .map(|word| {
                    let channel = word.channel as i32;
                    let speaker_index = word.speaker_index.map(|index| index as i32);
                    if let Some(label) = &word.label {
                        let key = match speaker_index {
                            Some(index) => format!("{channel}:{index}"),
                            None => channel.to_string(),
                        };
                        labels.entry(key).or_insert_with(|| label.clone());
                    }
                    engine::Word {
                        id: word.id,
                        text: word.text,
                        start_ms: word.start_ms,
                        end_ms: word.end_ms,
                        channel,
                        speaker_index,
                    }
                })
                .collect();
            engine::TranscriptInput {
                transcript_id,
                words,
            }
        })
        .collect();
    engine::EnhanceInput {
        raw_doc: inputs.raw_doc.clone(),
        transcripts,
        speaker_labels: engine::SpeakerLabels(labels),
        participants: inputs.participants.clone(),
        prev_doc: None,
        prev_metadata: None,
        regenerate: false,
        options: engine::EnhanceOptions::default(),
    }
}

#[cfg(feature = "ollama")]
fn run_warnings(report: &engine::schema::ValidationReport, chunks: usize) -> Vec<String> {
    let mut warnings = Vec::new();
    if report.dropped_bullets + report.unknown_anchors + report.unknown_sources > 0 {
        warnings.push(format!(
            "Dropped {} unsupported bullet(s) and {} unknown anchor(s) from the model output ({} unknown source id(s)).",
            report.dropped_bullets, report.unknown_anchors, report.unknown_sources
        ));
    }
    if chunks > 1 {
        warnings.push(format!(
            "The transcript was long, so it was processed in {chunks} chunks."
        ));
    }
    warnings
}

#[cfg(feature = "ollama")]
impl EnhancedNotesBackend {
    async fn run(
        &self,
        inputs: &access::EnhanceInputs,
        model: &str,
    ) -> Result<PreviewRun, PreviewError> {
        if inputs.index.is_empty() {
            return Err(PreviewError::Invalid(format!(
                "meeting '{}' has no transcript words to enhance notes from",
                inputs.meeting_id
            )));
        }
        let input = engine_input(inputs);
        let client =
            engine::ollama::OllamaClient::new(self.base.as_str(), model, input.options.num_ctx)
                .map_err(|error| PreviewError::Backend(error.to_string()))?;
        let result = engine::enhance(input, &client)
            .await
            .map_err(|error| match error {
                engine::EnhanceError::NoTranscript => PreviewError::Invalid(error.to_string()),
                other => PreviewError::Backend(other.to_string()),
            })?;
        let warnings = run_warnings(&result.report, result.chunks);
        Ok(PreviewRun {
            doc: result.doc,
            generation: json!({
                "backend": "enhanced-notes",
                "engine_version": engine::ENGINE_VERSION,
                "model": model,
                "base_url": self.base.as_str(),
                "chunks": result.chunks,
                "source_hash": result.source_hash,
                "dropped_bullets": result.report.dropped_bullets,
                "dropped_unknown_anchors": result.report.unknown_anchors,
                "dropped_unknown_sources": result.report.unknown_sources,
            }),
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_urls_are_accepted() {
        for ok in [
            "http://127.0.0.1:11434",
            "127.0.0.1:11434",
            "localhost:11434",
            "http://[::1]:11434/",
            "http://127.5.5.5",
        ] {
            assert!(normalize_loopback_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "https://api.openai.com",
            "http://192.168.1.20:11434",
            "http://0.0.0.0:11434",
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com",
            "10.0.0.1:11434",
            "file:///tmp/x",
        ] {
            assert!(normalize_loopback_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ollama_host_values_are_normalized_before_the_loopback_check() {
        for raw in [
            "0.0.0.0",
            "0.0.0.0:11434",
            "0.0.0.0:8080",
            "http://0.0.0.0:11434",
            "http://0.0.0.0",
            "[::]",
            "http://[::]:11434",
            "localhost",
            "127.0.0.1:9",
        ] {
            let url = ollama_host_to_url(raw);
            assert!(
                normalize_loopback_url(&url).is_ok(),
                "{raw} -> {url} should be loopback"
            );
        }
        assert_eq!(ollama_host_to_url("0.0.0.0"), "http://127.0.0.1:11434");
        assert_eq!(
            ollama_host_to_url("0.0.0.0:11434"),
            "http://127.0.0.1:11434"
        );
        assert_eq!(ollama_host_to_url("http://[::]:9"), "http://127.0.0.1:9");
        assert_eq!(ollama_host_to_url("[::]"), "http://127.0.0.1:11434");
        assert_eq!(ollama_host_to_url("localhost"), "http://localhost:11434");
    }

    #[test]
    fn ollama_host_normalization_keeps_remote_hosts_refused() {
        for remote in [
            "10.0.0.5",
            "192.168.1.20:11434",
            "http://ollama.example.com:11434",
            "https://api.openai.com",
            "0.0.0.1",
        ] {
            let url = ollama_host_to_url(remote);
            assert!(normalize_loopback_url(&url).is_err(), "{remote} -> {url}");
        }
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn the_backend_refuses_non_loopback_urls() {
        assert!(matches!(
            EnhancedNotesBackend::new("https://example.com"),
            Err(PreviewError::NotLoopback(_))
        ));
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn warnings_report_dropped_bullets_and_chunking() {
        let none = engine::schema::ValidationReport::default();
        assert!(run_warnings(&none, 1).is_empty());
        let report = engine::schema::ValidationReport {
            unknown_anchors: 1,
            dropped_bullets: 2,
            unknown_sources: 3,
        };
        let warnings = run_warnings(&report, 2);
        assert!(warnings[0].contains("Dropped 2 unsupported bullet(s) and 1 unknown anchor(s)"));
        assert!(warnings[1].contains("2 chunks"));
    }
}
