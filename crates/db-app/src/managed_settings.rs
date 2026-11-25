use serde::Deserialize;
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::BTreeMap;
use std::sync::OnceLock;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedSettings {
    schema_version: u32,
    settings: BTreeMap<String, Value>,
}

fn invalid() -> sqlx::Error {
    sqlx::Error::Protocol("invalid managed settings file".into())
}

fn validate(bytes: &[u8]) -> Result<ManagedSettings, sqlx::Error> {
    if bytes.len() > 64 * 1024 {
        return Err(invalid());
    }
    let doc: ManagedSettings = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if doc.schema_version != 1 {
        return Err(invalid());
    }
    validate_settings(&doc.settings)?;
    Ok(doc)
}

fn validate_settings(settings: &BTreeMap<String, Value>) -> Result<(), sqlx::Error> {
    for (key, value) in settings {
        let valid = match key.as_str() {
            "telemetry_consent"
            | "crash_reporting_consent"
            | "cloud_sync_enabled"
            | "automatic_updates" => value == &Value::Bool(false),
            "intelligence_disabled" => value == &Value::Bool(true),
            "auto_start_scheduled_meetings" | "save_recordings" => value.is_boolean(),
            "current_llm_provider" | "current_llm_model" => value.as_str() == Some(""),
            "audio_retention" => value.as_str() == Some("forever"),
            "current_stt_provider" => value.as_str() == Some("custom"),
            "current_stt_model" => value
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.len() <= 200),
            "stt:custom" => value.as_object().is_some_and(|v| {
                v.len() == 3
                    && v.get("type").and_then(Value::as_str) == Some("stt")
                    && v.get("api_key").and_then(Value::as_str) == Some("")
                    && v.get("base_url").and_then(Value::as_str).is_some_and(|s| {
                        s.len() <= 2048
                            && url::Url::parse(s).is_ok_and(|u| {
                                u.scheme() == "https"
                                    && u.host_str().is_some()
                                    && u.username().is_empty()
                                    && u.password().is_none()
                                    && u.query().is_none()
                                    && u.fragment().is_none()
                            })
                    })
            }),
            _ => false,
        };
        if !valid {
            return Err(invalid());
        }
    }
    Ok(())
}

static BUILTIN_SETTINGS: OnceLock<BTreeMap<String, Value>> = OnceLock::new();

/// Registers an in-code settings document (for example the local-only build's
/// forced settings). It is merged over `managed_settings.json` by
/// `apply_managed_settings`, wins on conflicts, and is subject to the same
/// validation and the same lock triggers. Call once, before the database
/// schema is prepared; a second call with a different document is an error.
pub fn set_builtin_managed_settings(settings: BTreeMap<String, Value>) -> Result<(), sqlx::Error> {
    validate_settings(&settings)?;
    match BUILTIN_SETTINGS.set(settings.clone()) {
        Ok(()) => Ok(()),
        Err(_) if BUILTIN_SETTINGS.get() == Some(&settings) => Ok(()),
        Err(_) => Err(invalid()),
    }
}

fn merged_with_builtin(
    mut file: BTreeMap<String, Value>,
    builtin: Option<&BTreeMap<String, Value>>,
) -> BTreeMap<String, Value> {
    if let Some(builtin) = builtin {
        file.extend(builtin.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    file
}

/// Read policy from the directory containing the actual opened database, on
/// desktop and mobile alike. Malformed or unreadable policy fails startup.
pub async fn apply_managed_settings(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let files: Vec<(i64, String, String)> = sqlx::query_as("PRAGMA database_list")
        .fetch_all(pool)
        .await?;
    let file = files
        .iter()
        .find(|(_, name, _)| name == "main")
        .map(|(_, _, file)| file.as_str())
        .unwrap_or("");
    if file.is_empty() {
        return Ok(());
    }
    let path = std::path::Path::new(file)
        .parent()
        .ok_or_else(invalid)?
        .join("managed_settings.json");
    let settings = match std::fs::read(path) {
        Ok(bytes) => validate(&bytes)?.settings,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(sqlx::Error::Io(e)),
    };
    apply_settings_doc(pool, merged_with_builtin(settings, BUILTIN_SETTINGS.get())).await
}

/// Writes an in-code settings document into `app_settings` and locks the keys
/// through `managed_app_settings`, replacing any previous managed set. This is
/// the shared tail of `apply_managed_settings`.
pub async fn apply_settings_doc(
    pool: &SqlitePool,
    settings: BTreeMap<String, Value>,
) -> Result<(), sqlx::Error> {
    validate_settings(&settings)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("DELETE FROM managed_app_settings")
        .execute(&mut *tx)
        .await?;
    for (id, value) in settings {
        let json = serde_json::to_string(&value).map_err(|_| invalid())?;
        sqlx::query("INSERT INTO app_settings(id,value_json) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET value_json=excluded.value_json,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')")
            .bind(&id).bind(&json).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO managed_app_settings(id,value_json) VALUES(?,?)")
            .bind(id)
            .bind(json)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secrets_and_unsafe_values_are_rejected() {
        assert!(validate(br#"{"schema_version":1,"settings":{"intelligence_disabled":true,"telemetry_consent":false}}"#).is_ok());
        for text in [
            r#"{"schema_version":1,"settings":{"telemetry_consent":true}}"#,
            r#"{"schema_version":1,"settings":{"api_key":"secret"}}"#,
            r#"{"schema_version":2,"settings":{}}"#,
        ] {
            assert!(validate(text.as_bytes()).is_err());
        }
    }
    #[test]
    fn builtin_settings_override_the_file_and_are_validated() {
        let file = BTreeMap::from([
            ("telemetry_consent".to_string(), Value::Bool(false)),
            ("intelligence_disabled".to_string(), Value::Bool(true)),
        ]);
        let builtin = BTreeMap::from([("automatic_updates".to_string(), Value::Bool(false))]);
        let merged = merged_with_builtin(file.clone(), Some(&builtin));
        assert_eq!(merged.len(), 3);
        assert_eq!(merged_with_builtin(file, None).len(), 2);
        let bad = BTreeMap::from([("telemetry_consent".to_string(), Value::Bool(true))]);
        assert!(validate_settings(&bad).is_err());
        assert!(set_builtin_managed_settings(bad).is_err());
    }

    #[tokio::test]
    async fn settings_doc_applies_and_locks_without_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        let db = anlg_db_core::Db::connect_local_plain(&path).await.unwrap();
        crate::prepare_schema(&db).await.unwrap();
        let doc = BTreeMap::from([
            ("telemetry_consent".to_string(), Value::Bool(false)),
            ("cloud_sync_enabled".to_string(), Value::Bool(false)),
        ]);
        apply_settings_doc(db.pool(), doc).await.unwrap();
        let value: String =
            sqlx::query_scalar("SELECT value_json FROM app_settings WHERE id='cloud_sync_enabled'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(value, "false");
        assert!(
            sqlx::query("UPDATE app_settings SET value_json='true' WHERE id='cloud_sync_enabled'")
                .execute(db.pool())
                .await
                .is_err()
        );
        let rejected = BTreeMap::from([("telemetry_consent".to_string(), Value::Bool(true))]);
        assert!(apply_settings_doc(db.pool(), rejected).await.is_err());
    }

    #[tokio::test]
    async fn startup_policy_locks_settings_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        std::fs::write(dir.path().join("managed_settings.json"),br#"{"schema_version":1,"settings":{"intelligence_disabled":true,"telemetry_consent":false,"cloud_sync_enabled":false}}"#).unwrap();
        let db = anlg_db_core::Db::connect_local_plain(&path).await.unwrap();
        crate::prepare_schema(&db).await.unwrap();
        assert!(
            sqlx::query("UPDATE app_settings SET value_json='true' WHERE id='telemetry_consent'")
                .execute(db.pool())
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM app_settings WHERE id='intelligence_disabled'")
                .execute(db.pool())
                .await
                .is_err()
        );
        assert!(
            sqlx::query("UPDATE app_settings SET id='unmanaged' WHERE id='intelligence_disabled'")
                .execute(db.pool())
                .await
                .is_err()
        );
        sqlx::query("UPDATE app_settings SET value_json='true' WHERE id='intelligence_disabled'")
            .execute(db.pool())
            .await
            .unwrap();
        db.pool().close().await;
        let db = anlg_db_core::Db::connect_local_plain(&path).await.unwrap();
        crate::prepare_schema(&db).await.unwrap();
        let disabled: String = sqlx::query_scalar(
            "SELECT value_json FROM app_settings WHERE id='intelligence_disabled'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(disabled, "true");
        std::fs::write(
            dir.path().join("managed_settings.json"),
            br#"{"schema_version":1,"settings":{"intelligence_disabled":true}}"#,
        )
        .unwrap();
        apply_managed_settings(db.pool()).await.unwrap();
        sqlx::query("UPDATE app_settings SET value_json='true' WHERE id='telemetry_consent'")
            .execute(db.pool())
            .await
            .unwrap();
        assert!(
            sqlx::query("DELETE FROM app_settings WHERE id='intelligence_disabled'")
                .execute(db.pool())
                .await
                .is_err()
        );
        std::fs::write(dir.path().join("managed_settings.json"), b"malformed").unwrap();
        assert!(apply_managed_settings(db.pool()).await.is_err());
        assert!(
            sqlx::query("DELETE FROM app_settings WHERE id='intelligence_disabled'")
                .execute(db.pool())
                .await
                .is_err()
        );
    }
}
