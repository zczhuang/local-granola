use std::str::FromStr;

use crate::{Error, Result};

pub const LOCAL_BASE_URL: &str = "soniqo://local";

pub fn is_local_base_url(base_url: &str) -> bool {
    base_url.trim_end_matches('/') == LOCAL_BASE_URL
}

pub fn is_loopback_http_base_url(base_url: &str) -> bool {
    let Some(rest) = base_url
        .trim()
        .strip_prefix("http://")
        .or_else(|| base_url.trim().strip_prefix("https://"))
    else {
        return false;
    };

    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();

    let host = authority
        .strip_prefix('[')
        .and_then(|value| value.split(']').next())
        .unwrap_or_else(|| authority.split(':').next().unwrap_or_default());

    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|addr| addr.is_loopback())
}

pub fn local_model_from_request(base_url: &str, model: &str) -> Option<SoniqoModel> {
    let model = model.parse().ok()?;

    if is_local_base_url(base_url) || is_loopback_http_base_url(base_url) {
        Some(model)
    } else {
        None
    }
}

#[derive(
    Debug, Clone, Copy, serde::Serialize, serde::Deserialize, specta::Type, Eq, Hash, PartialEq,
)]
pub enum SoniqoModel {
    #[serde(rename = "soniqo-parakeet-streaming")]
    ParakeetStreaming,
    #[serde(rename = "soniqo-parakeet-batch")]
    ParakeetBatch,
    /// Parakeet-TDT-v3 for live use: the batch model run on VAD-cut windows,
    /// one windower per audio source so mic ("me") and system ("them") stay
    /// separate. Shares model files with `ParakeetBatch`.
    #[serde(rename = "soniqo-parakeet-v3-live")]
    ParakeetV3Live,
    #[serde(rename = "soniqo-omnilingual")]
    Omnilingual,
    #[serde(rename = "soniqo-qwen3-small")]
    Qwen3Small,
    #[serde(rename = "soniqo-qwen3-large")]
    Qwen3Large,
}

impl SoniqoModel {
    const ALL: &'static [Self] = &[
        Self::ParakeetStreaming,
        Self::ParakeetBatch,
        Self::ParakeetV3Live,
        Self::Omnilingual,
    ];

    const KNOWN: &'static [Self] = &[
        Self::ParakeetStreaming,
        Self::ParakeetBatch,
        Self::ParakeetV3Live,
        Self::Omnilingual,
        Self::Qwen3Small,
        Self::Qwen3Large,
    ];

    const SELECTABLE: &'static [Self] = &[
        Self::ParakeetStreaming,
        Self::ParakeetBatch,
        Self::ParakeetV3Live,
    ];

    pub const fn all() -> &'static [Self] {
        Self::ALL
    }

    pub const fn selectable() -> &'static [Self] {
        Self::SELECTABLE
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParakeetStreaming => "soniqo-parakeet-streaming",
            Self::ParakeetBatch => "soniqo-parakeet-batch",
            Self::ParakeetV3Live => "soniqo-parakeet-v3-live",
            Self::Omnilingual => "soniqo-omnilingual",
            Self::Qwen3Small => "soniqo-qwen3-small",
            Self::Qwen3Large => "soniqo-qwen3-large",
        }
    }

    pub const fn repo(self) -> &'static str {
        match self {
            Self::ParakeetStreaming => "aufklarer/Parakeet-EOU-120M-CoreML-INT8",
            Self::ParakeetBatch | Self::ParakeetV3Live => {
                "aufklarer/Parakeet-TDT-v3-CoreML-INT8-30s"
            }
            Self::Omnilingual => "aufklarer/Omnilingual-ASR-CTC-300M-CoreML-INT8-10s",
            Self::Qwen3Small => "aufklarer/Qwen3-ASR-0.6B-MLX-4bit",
            Self::Qwen3Large => "aufklarer/Qwen3-ASR-1.7B-MLX-8bit",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::ParakeetStreaming => "Parakeet Streaming",
            Self::ParakeetBatch => "Parakeet Batch",
            Self::ParakeetV3Live => "Parakeet v3 Live",
            Self::Omnilingual => "Omnilingual ASR",
            Self::Qwen3Small => "Qwen3 ASR 0.6B",
            Self::Qwen3Large => "Qwen3 ASR 1.7B",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::ParakeetStreaming => "Realtime transcription for 25 European languages.",
            Self::ParakeetBatch => {
                "Batch transcription with on-device speaker labels for 25 European languages."
            }
            Self::ParakeetV3Live => {
                "Realtime transcription with Parakeet v3 on voice-activity windows, mic and system audio kept separate."
            }
            Self::Omnilingual => "Multilingual batch transcription.",
            Self::Qwen3Small => "Multilingual batch transcription.",
            Self::Qwen3Large => "Multilingual batch transcription.",
        }
    }

    pub const fn size_bytes(self) -> u64 {
        match self {
            Self::ParakeetStreaming => 120 * 1024 * 1024,
            Self::ParakeetBatch | Self::ParakeetV3Live => 632 * 1024 * 1024,
            Self::Omnilingual => 300 * 1024 * 1024,
            Self::Qwen3Small => 600 * 1024 * 1024,
            Self::Qwen3Large => 1_700 * 1024 * 1024,
        }
    }

    pub const fn supports_live(self) -> bool {
        matches!(self, Self::ParakeetStreaming | Self::ParakeetV3Live)
    }

    /// True when live audio is cut into VAD windows and transcribed with the
    /// batch model instead of using the native streaming session.
    pub const fn is_windowed_live(self) -> bool {
        matches!(self, Self::ParakeetV3Live)
    }

    /// The model id the Swift bridge knows. `ParakeetV3Live` is a Rust-side
    /// mode of the batch model, so downloads, caches and transcription calls
    /// go to `ParakeetBatch`.
    pub const fn bridge_model(self) -> Self {
        match self {
            Self::ParakeetV3Live => Self::ParakeetBatch,
            model => model,
        }
    }

    pub const fn is_available_on_current_platform(self) -> bool {
        cfg!(all(target_os = "macos", target_arch = "aarch64")) && !self.requires_macos_15()
    }

    pub(crate) const fn requires_macos_15(self) -> bool {
        matches!(self, Self::Qwen3Small | Self::Qwen3Large)
    }

    pub const fn supports_live_on_current_platform(self) -> bool {
        self.supports_live() && self.is_available_on_current_platform()
    }

    pub const fn batch_model(self) -> Self {
        match self {
            Self::ParakeetStreaming | Self::ParakeetV3Live => Self::ParakeetBatch,
            model => model,
        }
    }

    pub fn supports_language(self, language: &anlg_language::Language) -> bool {
        match self {
            Self::ParakeetStreaming | Self::ParakeetBatch | Self::ParakeetV3Live => {
                anlg_language::is_parakeet_tdt_v3_language(language)
            }
            Self::Omnilingual | Self::Qwen3Small | Self::Qwen3Large => true,
        }
    }

    pub fn supports_languages(self, languages: &[anlg_language::Language]) -> bool {
        languages
            .iter()
            .all(|language| self.supports_language(language))
    }

    fn matches_identifier(self, value: &str) -> bool {
        value == self.as_str()
            || value == self.repo()
            || matches!(
                (self, value),
                (Self::ParakeetBatch, "aufklarer/Parakeet-TDT-v3-CoreML-INT8")
            )
    }
}

impl std::fmt::Display for SoniqoModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SoniqoModel {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::KNOWN
            .iter()
            .copied()
            .find(|model| model.matches_identifier(value))
            .ok_or_else(|| Error::UnsupportedModel(value.to_string()))
    }
}
