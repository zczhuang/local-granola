use std::sync::Arc;
use std::time::{Duration, Instant};

use anlg_audio_retention::{AudioRetentionPolicy, StoredRetentionSettings, resolve_policy};
use sqlx::SqlitePool;
use tauri::Manager;
use tauri_plugin_fs_sync::FsSyncPluginExt;
use tauri_specta::Event;

use crate::{BatchSessionRegistry, ListenerPluginExt};

const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
const VOICEPRINT_CLEANUP_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

const AUDIO_RETENTION_SETTING_ID: &str = "audio_retention";
const SAVE_RECORDINGS_SETTING_ID: &str = "save_recordings";
const LEGACY_SETTINGS_ID: &str = "legacy_settings_document";
const LEGACY_MAIN_VALUES_ID: &str = "legacy_main_values_document";

/// Used when no retention setting exists: delete right after processing in the
/// local-only build, keep forever otherwise.
const DEFAULT_POLICY: AudioRetentionPolicy = if anlg_local_only::LOCAL_ONLY {
    AudioRetentionPolicy::None
} else {
    AudioRetentionPolicy::Forever
};

async fn load_policy(pool: &SqlitePool) -> Result<AudioRetentionPolicy, sqlx::Error> {
    let get = |id| anlg_db_app::get_app_setting_json(pool, id);
    Ok(resolve_policy(
        &StoredRetentionSettings {
            audio_retention: get(AUDIO_RETENTION_SETTING_ID).await?,
            save_recordings: get(SAVE_RECORDINGS_SETTING_ID).await?,
            legacy_settings: get(LEGACY_SETTINGS_ID).await?,
            legacy_main_values: get(LEGACY_MAIN_VALUES_ID).await?,
        },
        DEFAULT_POLICY,
    ))
}

fn retention_expired(
    candidate: &anlg_db_app::SessionAudioRetentionCandidate,
    policy: AudioRetentionPolicy,
    now_ms: i64,
) -> bool {
    anlg_audio_retention::retention_expired(
        candidate.created_at_ms,
        policy,
        now_ms,
        candidate.has_words,
        candidate.transcript_complete,
    )
}

async fn session_audio_is_expired(pool: &SqlitePool, session_id: &str) -> Result<bool, String> {
    let policy = load_policy(pool).await.map_err(|e| e.to_string())?;
    let candidate = anlg_db_app::get_session_audio_retention_candidate(pool, session_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(candidate.is_some_and(|candidate| {
        retention_expired(&candidate, policy, chrono::Utc::now().timestamp_millis())
    }))
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SessionAudioRetentionPhase {
    Deleting,
    Deleted,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, specta::Type, tauri_specta::Event)]
pub struct SessionAudioRetentionEvent {
    pub session_id: String,
    pub phase: SessionAudioRetentionPhase,
}

fn db_pool<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Option<SqlitePool> {
    app.try_state::<tauri_plugin_db::ManagedState>()
        .map(|state| state.pool().clone())
}

// BTreeMap rather than HashMap: `BTreeMap::new` is const, a static initializer must be.
static AUDIO_HOLDS: std::sync::Mutex<std::collections::BTreeMap<String, u32>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Keeps a session's audio on disk (both retention sweeps and processed-audio
/// deletion skip it) until a matching `release_session_audio_hold`. Counted, so
/// overlapping holders cannot release each other's hold. Used by the
/// post-meeting re-pass, which must read the audio after live processing ends.
pub fn hold_session_audio(session_id: &str) {
    if let Ok(mut holds) = AUDIO_HOLDS.lock() {
        *holds.entry(session_id.to_string()).or_insert(0) += 1;
    }
}

pub fn release_session_audio_hold(session_id: &str) {
    if let Ok(mut holds) = AUDIO_HOLDS.lock()
        && let Some(count) = holds.get_mut(session_id)
    {
        if *count <= 1 {
            holds.remove(session_id);
        } else {
            *count -= 1;
        }
    }
}

/// Conservative: a poisoned lock counts as held.
fn session_audio_is_held(session_id: &str) -> bool {
    AUDIO_HOLDS
        .lock()
        .map_or(true, |holds| holds.contains_key(session_id))
}

fn batch_session_belongs_to(batch_session_id: &str, session_id: &str) -> bool {
    batch_session_id
        .strip_prefix(session_id)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
}

/// Conservative: any uncertainty about capture or batch state keeps the audio.
async fn session_audio_is_idle<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: &str,
) -> bool {
    if session_audio_is_held(session_id) {
        return false;
    }
    let Ok(snapshot) = app.listener().get_capture_snapshot().await else {
        return false;
    };
    if snapshot.active_session_id.as_deref() == Some(session_id)
        || snapshot
            .finalizing_session_ids
            .iter()
            .any(|id| id == session_id)
    {
        return false;
    }

    let registry = app.state::<Arc<BatchSessionRegistry>>();
    registry.sessions.lock().is_ok_and(|sessions| {
        !sessions
            .keys()
            .any(|id| batch_session_belongs_to(id, session_id))
    })
}

#[derive(Clone, Copy)]
enum Eligibility {
    Expired,
    Processed,
    LogicallyDeleted,
}

async fn delete_local_audio<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    pool: &SqlitePool,
    session_id: &str,
    eligibility: Eligibility,
) -> Result<bool, String> {
    let _guard = app.fs_sync().lock_session_audio(session_id).await;
    if !session_audio_is_idle(app, session_id).await {
        return Ok(false);
    }
    let eligible = match eligibility {
        Eligibility::Expired => session_audio_is_expired(pool, session_id).await?,
        Eligibility::Processed => anlg_db_app::session_audio_is_processed(pool, session_id)
            .await
            .map_err(|e| e.to_string())?,
        Eligibility::LogicallyDeleted => {
            anlg_db_app::session_audio_is_logically_deleted(pool, session_id)
                .await
                .map_err(|e| e.to_string())?
        }
    };
    if !eligible {
        return Ok(false);
    }

    let emit = |phase| {
        let _ = SessionAudioRetentionEvent {
            session_id: session_id.to_string(),
            phase,
        }
        .emit(app);
    };
    emit(SessionAudioRetentionPhase::Deleting);
    let deleted = app.fs_sync().delete_session_audio_locked(session_id)?;
    anlg_db_app::mark_session_audio_absent(pool, session_id)
        .await
        .map_err(|e| e.to_string())?;
    if deleted {
        emit(SessionAudioRetentionPhase::Deleted);
    }
    Ok(deleted)
}

async fn sweep<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    pool: &SqlitePool,
) -> Result<(), String> {
    let logically_deleted = anlg_db_app::list_logically_deleted_session_audio(pool)
        .await
        .map_err(|e| e.to_string())?;
    for session_id in logically_deleted {
        if let Err(error) =
            delete_local_audio(app, pool, &session_id, Eligibility::LogicallyDeleted).await
        {
            tracing::warn!(%session_id, %error, "deleted_session_audio_cleanup_failed");
        }
    }

    let policy = load_policy(pool).await.map_err(|e| e.to_string())?;
    if policy == AudioRetentionPolicy::Forever {
        return Ok(());
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let candidates = anlg_db_app::list_session_audio_retention_candidates(pool)
        .await
        .map_err(|e| e.to_string())?;
    for candidate in candidates {
        if !retention_expired(&candidate, policy, now_ms) {
            continue;
        }
        let session_id = candidate.session_id;
        if let Err(error) = delete_local_audio(app, pool, &session_id, Eligibility::Expired).await {
            tracing::warn!(%session_id, %error, "expired_session_audio_cleanup_failed");
        }
    }
    Ok(())
}

pub(crate) fn spawn<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_voiceprint_cleanup: Option<Instant> = None;
        loop {
            interval.tick().await;
            let Some(pool) = db_pool(&app) else {
                continue;
            };
            if let Err(error) = sweep(&app, &pool).await {
                tracing::warn!(%error, "audio_retention_sweep_failed");
            }
            if last_voiceprint_cleanup.is_none_or(|at| at.elapsed() >= VOICEPRINT_CLEANUP_INTERVAL)
            {
                last_voiceprint_cleanup = Some(Instant::now());
                if let Err(error) =
                    crate::voiceprint::cleanup_expired_voiceprint_candidates(&app, &pool).await
                {
                    tracing::warn!(%error, "voiceprint_candidate_cleanup_failed");
                }
            }
        }
    });
}

/// Frontend entry point for `hold_session_audio`: the post-meeting re-pass pins a session's audio
/// against the Rust retention sweep, which the TS-side hold cannot reach. Counted like the Rust API.
#[tauri::command]
#[specta::specta]
pub fn acquire_session_audio_hold(session_id: String) -> Result<(), String> {
    hold_session_audio(&session_id);
    Ok(())
}

/// Frontend entry point for `release_session_audio_hold`.
#[tauri::command]
#[specta::specta]
pub fn drop_session_audio_hold(session_id: String) -> Result<(), String> {
    release_session_audio_hold(&session_id);
    Ok(())
}

/// Deletes local audio right after processing when the retention policy is `none`.
#[tauri::command]
#[specta::specta]
pub async fn delete_processed_session_audio<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<bool, String> {
    let pool = db_pool(&app).ok_or_else(|| "database is not ready yet".to_string())?;
    if load_policy(&pool).await.map_err(|e| e.to_string())? != AudioRetentionPolicy::None {
        return Ok(false);
    }
    delete_local_audio(&app, &pool, &session_id, Eligibility::Processed).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_sessions_match_their_capture_session_only() {
        assert!(batch_session_belongs_to("s1", "s1"));
        assert!(batch_session_belongs_to("s1:recovery", "s1"));
        assert!(!batch_session_belongs_to("s10", "s1"));
    }

    #[test]
    fn audio_holds_are_counted() {
        hold_session_audio("hold-a");
        hold_session_audio("hold-a");
        release_session_audio_hold("hold-a");
        assert!(session_audio_is_held("hold-a"));
        release_session_audio_hold("hold-a");
        assert!(!session_audio_is_held("hold-a"));
        release_session_audio_hold("hold-a");
        assert!(!session_audio_is_held("hold-a"));
    }

    fn candidate(
        has_words: bool,
        transcript_complete: bool,
    ) -> anlg_db_app::SessionAudioRetentionCandidate {
        anlg_db_app::SessionAudioRetentionCandidate {
            session_id: "s".into(),
            created_at_ms: Some(0),
            has_words,
            transcript_complete,
        }
    }

    #[test]
    fn none_policy_deletes_completed_empty_transcripts_but_not_unfinished_ones() {
        let p = AudioRetentionPolicy::None;
        assert!(!retention_expired(&candidate(false, false), p, 1));
        assert!(retention_expired(&candidate(false, true), p, 1));
        assert!(retention_expired(&candidate(true, false), p, 1));
    }

    #[test]
    fn local_only_build_defaults_to_deleting_after_processing() {
        assert_eq!(
            DEFAULT_POLICY == AudioRetentionPolicy::None,
            anlg_local_only::LOCAL_ONLY
        );
    }
}
