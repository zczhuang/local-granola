//! Pure audio-retention policy logic (no Tauri / database dependencies).

use serde_json::Value;

pub const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// Upper bound for `days:N` so the millisecond math can never overflow.
pub const MAX_DAYS: u32 = 3650;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioRetentionPolicy {
    None,
    OneDay,
    ThreeDays,
    OneWeek,
    OneMonth,
    /// `days:N` with `1 <= N <= MAX_DAYS`.
    Days(u32),
    Forever,
}

impl AudioRetentionPolicy {
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Bool(false) => Some(Self::None),
            Value::Bool(true) => Some(Self::Forever),
            Value::String(value) => Self::parse(value),
            _ => None,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "oneDay" => Some(Self::OneDay),
            "threeDays" => Some(Self::ThreeDays),
            "oneWeek" => Some(Self::OneWeek),
            "oneMonth" => Some(Self::OneMonth),
            "forever" => Some(Self::Forever),
            other => {
                let digits = other.strip_prefix("days:")?;
                if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let n: u32 = digits.parse().ok()?;
                (1..=MAX_DAYS).contains(&n).then_some(Self::Days(n))
            }
        }
    }

    pub fn duration_ms(self) -> Option<i64> {
        match self {
            Self::None => Some(0),
            Self::OneDay => Some(DAY_MS),
            Self::ThreeDays => Some(3 * DAY_MS),
            Self::OneWeek => Some(7 * DAY_MS),
            Self::OneMonth => Some(30 * DAY_MS),
            Self::Days(n) => Some(i64::from(n) * DAY_MS),
            Self::Forever => None,
        }
    }
}

#[derive(Debug, Default)]
pub struct StoredRetentionSettings {
    pub audio_retention: Option<String>,
    pub save_recordings: Option<String>,
    pub legacy_settings: Option<String>,
    pub legacy_main_values: Option<String>,
}

/// Mirrors the desktop settings resolution: direct row, then legacy documents,
/// then `save_recordings = false`, then `default`.
pub fn resolve_policy(
    stored: &StoredRetentionSettings,
    default: AudioRetentionPolicy,
) -> AudioRetentionPolicy {
    let parse = |json: &Option<String>| {
        json.as_deref()
            .and_then(|json| serde_json::from_str::<Value>(json).ok())
    };

    if let Some(policy) = parse(&stored.audio_retention)
        .as_ref()
        .and_then(AudioRetentionPolicy::from_value)
    {
        return policy;
    }

    let legacy_settings = parse(&stored.legacy_settings).unwrap_or(Value::Null);
    let legacy_main_values = parse(&stored.legacy_main_values).unwrap_or(Value::Null);
    let legacy = [
        legacy_settings.pointer("/general/audio_retention"),
        legacy_settings.pointer("/general/saveAudioAfterMeeting"),
        legacy_settings.pointer("/general/save_recordings"),
        legacy_main_values.get("audio_retention"),
        legacy_main_values.get("save_recordings"),
    ];
    if let Some(policy) = legacy
        .into_iter()
        .flatten()
        .find_map(AudioRetentionPolicy::from_value)
    {
        return policy;
    }

    if parse(&stored.save_recordings) == Some(Value::Bool(false)) {
        return AudioRetentionPolicy::None;
    }
    default
}

pub fn is_expired(created_at_ms: Option<i64>, policy: AudioRetentionPolicy, now_ms: i64) -> bool {
    match (policy, policy.duration_ms(), created_at_ms) {
        (AudioRetentionPolicy::None, _, _) => true,
        (_, Some(duration), Some(created_at)) => now_ms >= created_at.saturating_add(duration),
        _ => false,
    }
}

/// Retention decision for one session. Under `None` audio is deleted once the transcript is
/// finished: it has words, or transcription completed with none (silence, failed recognition),
/// so those recordings are not orphaned. Never while the transcript is still unfinished.
/// Callers must already have excluded sessions that are processing or held.
pub fn retention_expired(
    created_at_ms: Option<i64>,
    policy: AudioRetentionPolicy,
    now_ms: i64,
    has_words: bool,
    transcript_complete: bool,
) -> bool {
    if policy == AudioRetentionPolicy::None && !has_words && !transcript_complete {
        return false;
    }
    is_expired(created_at_ms, policy, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use AudioRetentionPolicy as P;

    fn stored(
        audio_retention: Option<&str>,
        save_recordings: Option<&str>,
        legacy_settings: Option<&str>,
        legacy_main_values: Option<&str>,
    ) -> StoredRetentionSettings {
        StoredRetentionSettings {
            audio_retention: audio_retention.map(str::to_string),
            save_recordings: save_recordings.map(str::to_string),
            legacy_settings: legacy_settings.map(str::to_string),
            legacy_main_values: legacy_main_values.map(str::to_string),
        }
    }

    #[test]
    fn retention_policy_never_deletes_audio_the_user_chose_to_keep() {
        let cases = [
            (stored(None, None, None, None), P::Forever),
            (stored(Some(r#""unknown""#), None, None, None), P::Forever),
            (stored(Some("not json"), Some("false"), None, None), P::None),
            (
                stored(Some(r#""oneWeek""#), Some("false"), None, None),
                P::OneWeek,
            ),
            (stored(Some("true"), Some("false"), None, None), P::Forever),
            (stored(Some("false"), None, None, None), P::None),
            (
                stored(
                    None,
                    Some("false"),
                    Some(r#"{"general":{"saveAudioAfterMeeting":"threeDays"}}"#),
                    None,
                ),
                P::ThreeDays,
            ),
            (
                stored(None, None, None, Some(r#"{"save_recordings":false}"#)),
                P::None,
            ),
            (stored(Some(r#""days:14""#), None, None, None), P::Days(14)),
        ];
        for (settings, expected) in cases {
            assert_eq!(
                resolve_policy(&settings, P::Forever),
                expected,
                "{settings:?}"
            );
        }
    }

    #[test]
    fn unset_settings_use_the_supplied_default() {
        assert_eq!(
            resolve_policy(&stored(None, None, None, None), P::None),
            P::None
        );
        // An explicit choice beats the default.
        assert_eq!(
            resolve_policy(&stored(Some(r#""oneWeek""#), None, None, None), P::None),
            P::OneWeek
        );
    }

    #[test]
    fn days_values_parse_strictly() {
        assert_eq!(P::parse("days:1"), Some(P::Days(1)));
        assert_eq!(P::parse("days:3650"), Some(P::Days(3650)));
        for bad in [
            "days:0",
            "days:",
            "days:-1",
            "days:1.5",
            "days:3651",
            "days: 2",
            "Days:2",
            "days:99999999999",
        ] {
            assert_eq!(P::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn retention_window_starts_at_session_creation() {
        let created = Some(1_000);
        assert!(!is_expired(created, P::OneDay, 1_000 + DAY_MS - 1));
        assert!(is_expired(created, P::OneDay, 1_000 + DAY_MS));
        assert!(!is_expired(created, P::Days(5), 1_000 + 5 * DAY_MS - 1));
        assert!(is_expired(created, P::Days(5), 1_000 + 5 * DAY_MS));
        assert!(!is_expired(None, P::OneMonth, i64::MAX));
        assert!(!is_expired(created, P::Forever, i64::MAX));
        assert!(is_expired(None, P::None, 0));
    }

    #[test]
    fn none_policy_waits_for_a_finished_transcript() {
        // Unfinished transcript: keep.
        assert!(!retention_expired(Some(0), P::None, 1, false, false));
        // Words present, or completed with zero words: delete.
        assert!(retention_expired(Some(0), P::None, 1, true, false));
        assert!(retention_expired(None, P::None, 1, false, true));
    }

    #[test]
    fn timed_policies_ignore_transcript_state() {
        let day = P::OneWeek.duration_ms().unwrap();
        assert!(retention_expired(Some(0), P::OneWeek, day, false, false));
        assert!(!retention_expired(Some(0), P::OneWeek, day - 1, true, true));
        assert!(!retention_expired(
            Some(0),
            P::Forever,
            i64::MAX,
            true,
            true
        ));
    }
}
