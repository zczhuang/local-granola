//! LOCAL-GRANOLA: live-path entry point for `stt-routing`.
//!
//! Parakeet gives fast live text but cannot do Mandarin or mixed-language meetings. The desktop
//! app calls [`decide_stt_route`] once ~30 s of speech have been transcribed (see
//! `apps/desktop/src/stt/engine-routing.ts`). Evidence is the languages the user declared for the
//! session plus the scripts seen in the live transcript so far; the actual decision (thresholds,
//! mixed-language rule) is `stt_routing::Router`'s. Switching the live engine mid-meeting is too
//! invasive, so a Whisper verdict only forces the post-meeting Whisper re-pass.

use anlg_stt_routing::{LangProbs, Route, Router, RoutingPreference};

/// Minimum letters of transcript text before its scripts count as evidence.
const MIN_SCRIPT_LETTERS: usize = 20;

#[derive(Debug, Clone, serde::Deserialize, specta::Type)]
pub struct SttRouteInput {
    /// `auto | parakeet | whisper`; anything else is `auto`.
    pub preference: Option<String>,
    /// Languages the user declared for the session (ISO codes, `zh-CN` style accepted).
    pub declared_languages: Vec<String>,
    /// Live transcript text so far.
    pub sample_text: String,
    /// Speech transcribed so far, in milliseconds.
    pub speech_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "engine", rename_all = "snake_case")]
pub enum SttRouteDecision {
    /// Not enough evidence yet.
    Pending,
    Parakeet,
    /// `lang` pins Whisper to one language; `None` lets it detect per chunk (mixed meetings).
    Whisper { lang: Option<String> },
}

impl From<Route> for SttRouteDecision {
    fn from(route: Route) -> Self {
        match route {
            Route::Parakeet => Self::Parakeet,
            Route::Whisper { lang } => Self::Whisper { lang },
        }
    }
}

pub fn decide(input: &SttRouteInput) -> SttRouteDecision {
    let preference = RoutingPreference::parse_or_default(input.preference.as_deref());
    let mut router = Router::new(preference);
    let probs = evidence(&input.declared_languages, &input.sample_text);
    let decision = router.observe(input.speech_ms, &probs);
    match decision {
        anlg_stt_routing::Decision::Pending => SttRouteDecision::Pending,
        _ => router
            .current()
            .cloned()
            .map_or(SttRouteDecision::Pending, Into::into),
    }
}

/// Declared languages carry a total weight of 1; non-Latin scripts in the transcript add up to
/// another 1, split by their share of the letters seen. Latin text adds nothing (it cannot tell
/// the Parakeet languages apart, and the declared languages already say which one to expect).
fn evidence(declared: &[String], text: &str) -> LangProbs {
    let mut pairs: Vec<(String, f32)> = Vec::new();
    if !declared.is_empty() {
        let weight = 1.0 / declared.len() as f32;
        pairs.extend(declared.iter().map(|code| (code.clone(), weight)));
    }

    let (mut han, mut kana, mut hangul, mut letters) = (0usize, 0usize, 0usize, 0usize);
    for ch in text.chars().filter(|c| c.is_alphabetic()) {
        letters += 1;
        match ch as u32 {
            0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF => han += 1,
            0x3040..=0x30FF => kana += 1,
            0xAC00..=0xD7AF | 0x1100..=0x11FF => hangul += 1,
            _ => {}
        }
    }
    if letters >= MIN_SCRIPT_LETTERS {
        let share = |n: usize| n as f32 / letters as f32;
        pairs.push(("zh".to_string(), share(han)));
        // Kana text is Japanese even when it contains kanji.
        pairs.push(("ja".to_string(), share(kana)));
        pairs.push(("ko".to_string(), share(hangul)));
    }
    LangProbs::new(pairs)
}

/// Asks the router which engine the session's speech needs. Pure; no audio or model access.
#[tauri::command]
#[specta::specta]
pub fn decide_stt_route(input: SttRouteInput) -> Result<SttRouteDecision, String> {
    Ok(decide(&input))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(
        preference: Option<&str>,
        languages: &[&str],
        text: &str,
        speech_ms: u64,
    ) -> SttRouteInput {
        SttRouteInput {
            preference: preference.map(str::to_string),
            declared_languages: languages.iter().map(|l| l.to_string()).collect(),
            sample_text: text.to_string(),
            speech_ms,
        }
    }

    #[test]
    fn waits_for_enough_speech() {
        assert_eq!(
            decide(&input(Some("auto"), &["zh"], "", 5_000)),
            SttRouteDecision::Pending
        );
    }

    #[test]
    fn english_stays_on_parakeet() {
        assert_eq!(
            decide(&input(None, &["en"], "hello there everyone", 30_000)),
            SttRouteDecision::Parakeet
        );
        assert_eq!(
            decide(&input(None, &[], "", 30_000)),
            SttRouteDecision::Pending
        );
    }

    #[test]
    fn declared_mandarin_routes_to_whisper() {
        assert_eq!(
            decide(&input(None, &["zh-CN"], "", 30_000)),
            SttRouteDecision::Whisper {
                lang: Some("zh".to_string())
            }
        );
    }

    #[test]
    fn declared_english_and_mandarin_is_mixed() {
        assert_eq!(
            decide(&input(None, &["en", "zh"], "", 30_000)),
            SttRouteDecision::Whisper { lang: None }
        );
    }

    #[test]
    fn chinese_script_in_the_transcript_overrides_an_english_declaration() {
        let text = "我们今天讨论一下下个季度的产品路线图和发布计划还有预算";
        let decision = decide(&input(None, &["en"], text, 30_000));
        assert!(
            matches!(decision, SttRouteDecision::Whisper { .. }),
            "{decision:?}"
        );
    }

    #[test]
    fn short_scripts_and_latin_text_are_not_evidence() {
        assert_eq!(
            decide(&input(None, &["en"], "你好 hello", 30_000)),
            SttRouteDecision::Parakeet
        );
    }

    #[test]
    fn explicit_preference_is_honoured_without_waiting() {
        assert_eq!(
            decide(&input(Some("whisper"), &["en"], "", 0)),
            SttRouteDecision::Whisper { lang: None }
        );
        assert_eq!(
            decide(&input(Some("parakeet"), &["zh"], "", 0)),
            SttRouteDecision::Parakeet
        );
    }
}
