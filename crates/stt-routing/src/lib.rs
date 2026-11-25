//! Pure speech-to-text engine routing.
//!
//! Decides, from Whisper language-detection probabilities, whether a meeting
//! should be transcribed with Parakeet (fast, Neural Engine, European
//! languages) or Whisper (Mandarin, mixed-language and everything Parakeet
//! cannot do). No audio and no model code lives here: callers feed in the
//! per-window probabilities and speech duration, and act on the [`Decision`].
//!
//! The router is deliberately sticky. It waits for [`RouterConfig::min_speech_ms`]
//! of speech (or very high confidence earlier), and afterwards only switches
//! after several consecutive contradicting windows.

pub mod vocab;

use std::{collections::BTreeMap, str::FromStr};

use anlg_language::PARAKEET_TDT_V3_LANGUAGE_CODES;
use serde::{Deserialize, Serialize};

/// User override, stored in settings as `auto | parakeet | whisper`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutingPreference {
    #[default]
    Auto,
    Parakeet,
    Whisper,
}

impl FromStr for RoutingPreference {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "parakeet" => Ok(Self::Parakeet),
            "whisper" => Ok(Self::Whisper),
            _ => Err(()),
        }
    }
}

impl RoutingPreference {
    /// Unknown or missing values fall back to `Auto`.
    pub fn parse_or_default(value: Option<&str>) -> Self {
        value.and_then(|v| v.parse().ok()).unwrap_or_default()
    }
}

/// Which engine should transcribe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "engine", rename_all = "snake_case")]
pub enum Route {
    Parakeet,
    /// `lang: Some(code)` pins Whisper to one language (for example `zh`),
    /// `None` lets Whisper detect per chunk (mixed-language meetings).
    Whisper {
        lang: Option<String>,
    },
}

impl Route {
    pub fn is_whisper(&self) -> bool {
        matches!(self, Self::Whisper { .. })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouterConfig {
    /// Speech needed before a decision is forced.
    pub min_speech_ms: u64,
    /// Speech needed before an early, high-confidence decision is allowed.
    pub early_min_speech_ms: u64,
    /// Aggregated top-language probability that allows an early decision.
    pub early_confidence: f32,
    /// Chinese share at or above which (together with `mixed_other_share`
    /// of other languages) the meeting counts as mixed.
    pub mixed_zh_share: f32,
    /// Minimum probability mass outside Chinese for a mixed route.
    pub mixed_other_share: f32,
    /// Probability mass on languages Parakeet cannot do (non-Chinese) that
    /// forces Whisper auto-detect even when a Parakeet language leads.
    pub unsupported_mass: f32,
    /// A window must have at least this much speech to vote on a switch.
    pub window_min_speech_ms: u64,
    /// A window must have at least this top probability to vote on a switch.
    pub window_min_confidence: f32,
    /// Consecutive contradicting windows needed to switch away from a route.
    pub switch_after_windows: u32,
    /// Same, but when the current route is mixed (Whisper auto-detect).
    pub leave_mixed_after_windows: u32,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            min_speech_ms: 30_000,
            early_min_speech_ms: 10_000,
            early_confidence: 0.95,
            mixed_zh_share: 0.2,
            mixed_other_share: 0.2,
            unsupported_mass: 0.3,
            window_min_speech_ms: 5_000,
            window_min_confidence: 0.8,
            switch_after_windows: 3,
            leave_mixed_after_windows: 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Not enough evidence yet; keep the default engine running.
    Pending,
    /// First decision for this meeting.
    Decided(Route),
    /// Decision changed after enough contradicting windows.
    Switched(Route),
    /// Decision stands.
    Unchanged,
}

/// Normalized language distribution keyed by base ISO 639 code.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LangProbs {
    probs: BTreeMap<String, f32>,
}

impl LangProbs {
    /// Builds a distribution from `(code, probability)` pairs. Codes such as
    /// `zh-CN` collapse to `zh`; `yue` (Cantonese) counts as Chinese. Invalid
    /// values are dropped and the rest renormalized to sum to 1.
    pub fn new<I, S>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (S, f32)>,
        S: AsRef<str>,
    {
        let mut probs: BTreeMap<String, f32> = BTreeMap::new();
        for (code, p) in pairs {
            if !p.is_finite() || p <= 0.0 {
                continue;
            }
            *probs.entry(normalize_code(code.as_ref())).or_default() += p;
        }
        let total: f32 = probs.values().sum();
        if total > 0.0 {
            for v in probs.values_mut() {
                *v /= total;
            }
        }
        Self { probs }
    }

    pub fn is_empty(&self) -> bool {
        self.probs.is_empty()
    }

    pub fn get(&self, code: &str) -> f32 {
        self.probs
            .get(&normalize_code(code))
            .copied()
            .unwrap_or(0.0)
    }

    pub fn zh_share(&self) -> f32 {
        self.get("zh")
    }

    /// Most likely language and its probability. Ties break alphabetically.
    pub fn top(&self) -> Option<(&str, f32)> {
        self.probs
            .iter()
            .fold(None::<(&str, f32)>, |best, (code, p)| match best {
                Some((_, bp)) if bp >= *p => best,
                _ => Some((code.as_str(), *p)),
            })
    }

    /// Most likely language other than Chinese.
    pub fn top_non_zh(&self) -> Option<(&str, f32)> {
        self.probs
            .iter()
            .filter(|(code, _)| code.as_str() != "zh")
            .fold(None::<(&str, f32)>, |best, (code, p)| match best {
                Some((_, bp)) if bp >= *p => best,
                _ => Some((code.as_str(), *p)),
            })
    }

    fn unsupported_non_zh_mass(&self) -> f32 {
        self.probs
            .iter()
            .filter(|(code, _)| code.as_str() != "zh" && !is_parakeet_language(code))
            .map(|(_, p)| *p)
            .sum()
    }
}

fn normalize_code(code: &str) -> String {
    let lower = code.trim().to_ascii_lowercase();
    let base = lower.split(['-', '_']).next().unwrap_or_default();
    match base {
        // Whisper reports Cantonese separately; route it like Mandarin.
        "yue" | "cmn" | "zho" | "chi" => "zh".to_string(),
        other => other.to_string(),
    }
}

pub fn is_parakeet_language(code: &str) -> bool {
    PARAKEET_TDT_V3_LANGUAGE_CODES.contains(&normalize_code(code).as_str())
}

/// Maps one distribution to a route, without any hysteresis.
pub fn classify(probs: &LangProbs, config: &RouterConfig) -> Option<Route> {
    let (top, _) = probs.top()?;
    let zh = probs.zh_share();

    if zh >= config.mixed_zh_share && (1.0 - zh) >= config.mixed_other_share {
        return Some(Route::Whisper { lang: None });
    }
    if zh > 0.5 && top == "zh" {
        return Some(Route::Whisper {
            lang: Some("zh".to_string()),
        });
    }
    if probs.unsupported_non_zh_mass() >= config.unsupported_mass && is_parakeet_language(top) {
        // A Parakeet language leads but a lot of audio is in a language it
        // cannot transcribe.
        return Some(Route::Whisper { lang: None });
    }
    if is_parakeet_language(top) {
        return Some(Route::Parakeet);
    }
    Some(Route::Whisper {
        lang: Some(top.to_string()),
    })
}

#[derive(Debug, Clone)]
pub struct Router {
    config: RouterConfig,
    preference: RoutingPreference,
    /// Speech-duration-weighted sum of window distributions.
    weighted: BTreeMap<String, f64>,
    speech_ms: u64,
    current: Option<Route>,
    contradicting: u32,
}

impl Router {
    pub fn new(preference: RoutingPreference) -> Self {
        Self::with_config(preference, RouterConfig::default())
    }

    pub fn with_config(preference: RoutingPreference, config: RouterConfig) -> Self {
        let current = match preference {
            RoutingPreference::Auto => None,
            RoutingPreference::Parakeet => Some(Route::Parakeet),
            RoutingPreference::Whisper => Some(Route::Whisper { lang: None }),
        };
        Self {
            config,
            preference,
            weighted: BTreeMap::new(),
            speech_ms: 0,
            current,
            contradicting: 0,
        }
    }

    pub fn preference(&self) -> RoutingPreference {
        self.preference
    }

    /// The route in force, `None` while an `Auto` router is still pending.
    pub fn current(&self) -> Option<&Route> {
        self.current.as_ref()
    }

    pub fn speech_ms(&self) -> u64 {
        self.speech_ms
    }

    /// Feeds one language-detection window. `window_speech_ms` is the amount
    /// of actual speech (not silence) the window contained.
    pub fn observe(&mut self, window_speech_ms: u64, probs: &LangProbs) -> Decision {
        if self.preference != RoutingPreference::Auto {
            return Decision::Unchanged;
        }
        if probs.is_empty() || window_speech_ms == 0 {
            return self.status_without_evidence();
        }

        self.speech_ms += window_speech_ms;
        for (code, p) in &probs.probs {
            *self.weighted.entry(code.clone()).or_default() +=
                f64::from(*p) * window_speech_ms as f64;
        }

        match self.current.clone() {
            None => self.try_initial_decision(),
            Some(current) => self.observe_after_decision(&current, window_speech_ms, probs),
        }
    }

    /// Forces a decision from whatever evidence exists (end of the first
    /// window of audio, or the user starting a batch pass). With no evidence
    /// an `Auto` router picks Parakeet.
    pub fn finish(&mut self) -> Route {
        if self.current.is_none() {
            let route = self.aggregate_route().unwrap_or(Route::Parakeet);
            self.current = Some(route);
        }
        self.current.clone().unwrap_or(Route::Parakeet)
    }

    fn status_without_evidence(&self) -> Decision {
        if self.current.is_some() {
            Decision::Unchanged
        } else {
            Decision::Pending
        }
    }

    fn aggregate(&self) -> LangProbs {
        LangProbs::new(self.weighted.iter().map(|(k, v)| (k.as_str(), *v as f32)))
    }

    fn aggregate_route(&self) -> Option<Route> {
        classify(&self.aggregate(), &self.config)
    }

    fn try_initial_decision(&mut self) -> Decision {
        let aggregate = self.aggregate();
        let enough = self.speech_ms >= self.config.min_speech_ms;
        let early = self.speech_ms >= self.config.early_min_speech_ms
            && aggregate
                .top()
                .is_some_and(|(_, p)| p >= self.config.early_confidence);
        if !(enough || early) {
            return Decision::Pending;
        }
        match classify(&aggregate, &self.config) {
            Some(route) => {
                self.current = Some(route.clone());
                self.contradicting = 0;
                Decision::Decided(route)
            }
            None => Decision::Pending,
        }
    }

    fn observe_after_decision(
        &mut self,
        current: &Route,
        window_speech_ms: u64,
        probs: &LangProbs,
    ) -> Decision {
        if window_speech_ms < self.config.window_min_speech_ms {
            return Decision::Unchanged;
        }
        let Some((_, confidence)) = probs.top() else {
            return Decision::Unchanged;
        };
        let Some(window_route) = classify(probs, &self.config) else {
            return Decision::Unchanged;
        };

        if &window_route == current {
            self.contradicting = 0;
            return Decision::Unchanged;
        }
        // Mixed windows are inherently low-confidence, so only single-language
        // windows must clear the confidence bar to count as a contradiction.
        let window_is_mixed = window_route == Route::Whisper { lang: None };
        if !window_is_mixed && confidence < self.config.window_min_confidence {
            return Decision::Unchanged;
        }

        self.contradicting += 1;
        let needed = if matches!(current, Route::Whisper { lang: None }) {
            self.config.leave_mixed_after_windows
        } else {
            self.config.switch_after_windows
        };
        if self.contradicting >= needed {
            self.contradicting = 0;
            self.current = Some(window_route.clone());
            // Restart evidence so the new route is judged on fresh audio.
            self.weighted.clear();
            self.speech_ms = 0;
            return Decision::Switched(window_route);
        }
        Decision::Unchanged
    }
}

#[cfg(test)]
mod tests;
