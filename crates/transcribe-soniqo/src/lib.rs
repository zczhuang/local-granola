use std::path::{Path, PathBuf};
use std::time::Instant;

mod download_error;
mod error;
#[cfg(test)]
mod hub_anonymous;
mod model;
mod platform;
mod responses;
#[cfg(test)]
mod streaming_offline;
mod types;
mod vad_window;
mod windowed_live;

pub use error::{Error, Result};
pub use model::{
    LOCAL_BASE_URL, SoniqoModel, is_local_base_url, is_loopback_http_base_url,
    local_model_from_request,
};
pub use responses::{
    batch_response_from_channels, batch_response_from_text, stream_response_from_text,
};
pub use types::{
    DiarizationSegment, FileTranscript, FileTranscriptChunk, LivePartial, ModelDownloadState,
    TranscriptSource,
};
pub use vad_window::{VadWindowConfig, VadWindower, Window};

fn ensure_supported_platform(model: SoniqoModel) -> Result<()> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Err(Error::UnsupportedPlatform);
    }

    if model.requires_macos_15() {
        return Err(Error::RequiresMacOs15(model));
    }

    Ok(())
}

pub fn model_cache_dir(model: SoniqoModel) -> Result<PathBuf> {
    ensure_supported_platform(model)?;
    platform::model_cache_dir(model)
}

pub fn model_download_state(model: SoniqoModel) -> Result<ModelDownloadState> {
    ensure_supported_platform(model)?;
    let mut state = platform::model_download_state(model)?;
    state.error = state
        .error
        .as_deref()
        .map(download_error::user_facing_download_error);
    Ok(state)
}

pub fn start_model_download(model: SoniqoModel) -> Result<()> {
    ensure_supported_platform(model)?;
    platform::start_model_download(model)
}

pub fn reset_model(model: SoniqoModel) -> Result<()> {
    ensure_supported_platform(model)?;
    platform::reset_model(model)
}

pub fn is_model_downloaded(model: SoniqoModel) -> Result<bool> {
    Ok(model_download_state(model)?.status == "ready")
}

pub fn is_model_downloading(model: SoniqoModel) -> Result<bool> {
    Ok(model_download_state(model)?.status == "downloading")
}

pub fn delete_model(model: SoniqoModel) -> Result<()> {
    reset_model(model)?;

    // `ParakeetV3Live` shares its files with `ParakeetBatch`; deleting it only
    // releases the loaded model so the batch model's files stay put.
    if model.is_windowed_live() {
        return Ok(());
    }

    let cache_dir = model_cache_dir(model)?;
    if cache_dir.exists() {
        std::fs::remove_dir_all(&cache_dir).map_err(Error::Delete)?;
    }
    if model == SoniqoModel::ParakeetBatch {
        let cache_dir = platform::diarization_cache_dir()?;
        if cache_dir.exists() {
            std::fs::remove_dir_all(&cache_dir).map_err(Error::Delete)?;
        }
    }

    Ok(())
}

pub fn transcribe_file(
    model: SoniqoModel,
    path: impl AsRef<Path>,
    language: Option<&str>,
) -> Result<FileTranscript> {
    ensure_supported_platform(model)?;

    let path = path.as_ref();
    let language = language.unwrap_or_default();
    let language_label = if language.is_empty() {
        "auto"
    } else {
        language
    };
    let file_extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let started_at = Instant::now();

    tracing::info!(
        anarlog.stt.provider.name = "soniqo",
        anarlog.stt.model = %model,
        anarlog.stt.language = %language_label,
        file.extension = %file_extension,
        "soniqo_native_file_transcription_start"
    );

    let result = platform::transcribe_file(model, path, language);
    let elapsed_ms = started_at.elapsed().as_millis() as u64;

    match &result {
        Ok(transcript) => {
            tracing::info!(
                anarlog.stt.provider.name = "soniqo",
                anarlog.stt.model = %model,
                elapsed_ms,
                transcript.duration_seconds = transcript.duration_seconds,
                transcript.text_chars = transcript.text.chars().count(),
                "soniqo_native_file_transcription_completed"
            );
        }
        Err(error) => {
            tracing::error!(
                anarlog.stt.provider.name = "soniqo",
                anarlog.stt.model = %model,
                elapsed_ms,
                error = %error,
                "soniqo_native_file_transcription_failed"
            );
        }
    }

    result
}

pub fn transcribe_samples(
    model: SoniqoModel,
    samples: &[f32],
    language: Option<&str>,
) -> Result<FileTranscript> {
    ensure_supported_platform(model)?;
    platform::transcribe_samples(model.bridge_model(), samples, language.unwrap_or_default())
}

pub fn diarize_samples(
    model: SoniqoModel,
    samples: &[f32],
    exact_speakers: usize,
) -> Result<Vec<DiarizationSegment>> {
    ensure_supported_platform(model)?;
    if model.batch_model() != SoniqoModel::ParakeetBatch {
        return Err(Error::Bridge(format!(
            "{} does not support speaker diarization",
            model.display_name()
        )));
    }
    if exact_speakers < 2 {
        return Err(Error::Bridge(
            "speaker diarization requires at least two speakers".to_string(),
        ));
    }

    platform::diarize_samples(model.batch_model(), samples, exact_speakers)
}

enum LiveBackend {
    /// Native Soniqo streaming session (`ParakeetStreaming`).
    Native { session_token: String },
    /// Batch model on VAD windows (`ParakeetV3Live`).
    Windowed(Box<windowed_live::WindowedLive<PlatformTranscriber>>),
}

/// Runs the Swift bridge's file transcription on raw samples.
struct PlatformTranscriber {
    model: SoniqoModel,
}

impl windowed_live::SamplesTranscriber for PlatformTranscriber {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        // LOCAL-GRANOLA(mac-untested): needs the Swift `_soniqo_transcribe_samples` bridge.
        // Parakeet-v3 detects the language itself, so no hint is passed.
        platform::transcribe_samples(self.model.bridge_model(), samples, "")
            .map(|transcript| transcript.text)
    }
}

pub struct LiveTranscriptionSession {
    model: SoniqoModel,
    backend: LiveBackend,
    stopped: bool,
}

impl LiveTranscriptionSession {
    pub fn start(model: SoniqoModel) -> Result<Self> {
        ensure_supported_platform(model)?;

        if !model.supports_live() {
            return Err(Error::Bridge(format!(
                "{} does not support realtime transcription",
                model.display_name()
            )));
        }

        let backend = if model.is_windowed_live() {
            let mut transcriber = PlatformTranscriber { model };
            // Load (and compile) the model now so the first utterance does not
            // pay for it; surfaces download problems at session start like the
            // native streaming path does.
            windowed_live::SamplesTranscriber::transcribe(&mut transcriber, &[0.0; 16_000])
                .map_err(|error| {
                    tracing::error!(
                        anarlog.stt.provider.name = "soniqo",
                        anarlog.stt.model = %model,
                        error = %error,
                        "soniqo_windowed_live_start_failed"
                    );
                    error
                })?;
            LiveBackend::Windowed(Box::new(windowed_live::WindowedLive::new(transcriber)))
        } else {
            let session_token = platform::live_start(model).map_err(|error| {
                tracing::error!(
                    anarlog.stt.provider.name = "soniqo",
                    anarlog.stt.model = %model,
                    error = %error,
                    "soniqo_native_live_start_failed"
                );
                error
            })?;
            LiveBackend::Native { session_token }
        };

        Ok(Self {
            model,
            backend,
            stopped: false,
        })
    }

    pub fn append(
        &mut self,
        source: TranscriptSource,
        samples: &[f32],
    ) -> Result<Vec<LivePartial>> {
        match &mut self.backend {
            LiveBackend::Native { session_token } => {
                platform::live_append(session_token, source, samples)
            }
            LiveBackend::Windowed(live) => live.append(source, samples),
        }
    }

    pub fn finalize(&mut self, source: TranscriptSource) -> Result<Vec<LivePartial>> {
        match &mut self.backend {
            LiveBackend::Native { session_token } => platform::live_finalize(session_token, source),
            LiveBackend::Windowed(live) => live.finalize(source),
        }
    }

    pub fn model(&self) -> SoniqoModel {
        self.model
    }

    pub fn stop(mut self) -> Result<()> {
        self.stopped = true;
        match &self.backend {
            LiveBackend::Native { session_token } => platform::live_stop(session_token),
            LiveBackend::Windowed(_) => Ok(()),
        }
    }
}

impl Drop for LiveTranscriptionSession {
    fn drop(&mut self) {
        if !self.stopped
            && let LiveBackend::Native { session_token } = &self.backend
        {
            let _ = platform::live_stop(session_token);
        }
    }
}

#[cfg(test)]
mod tests;
