use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

const ZOOM_AUTH_URL: &str = "https://zoom.us/oauth/authorize";
const ZOOM_TOKEN_URL: &str = "https://zoom.us/oauth/token";
const ZOOM_API_BASE: &str = "https://api.zoom.us/v2";
const ZOOM_REDIRECT_PORT: u16 = 43821;
pub const ZOOM_REDIRECT_URI: &str = "http://127.0.0.1:43821/zoom/callback";
const ZOOM_CALENDAR_ID: &str = "zoom-local-calendar";
const OAUTH_TIMEOUT: Duration = Duration::from_secs(300);
const TOKEN_REFRESH_SKEW_SECS: i64 = 120;
const SESSION_MATCH_WINDOW_MINS: i64 = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ZoomSyncConfig {
    pub auto_sync: bool,
    pub sync_calendar_events: bool,
    pub enrich_sessions: bool,
}

impl Default for ZoomSyncConfig {
    fn default() -> Self {
        Self {
            auto_sync: true,
            sync_calendar_events: true,
            enrich_sessions: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ZoomMeetingPreview {
    pub meeting_id: String,
    pub topic: String,
    pub start_time: Option<String>,
    pub join_url: Option<String>,
    pub participant_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ZoomSyncStatus {
    pub configured: bool,
    pub client_id_hint: Option<String>,
    pub redirect_uri: String,
    pub connected: bool,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub synced_meetings_count: u32,
    pub enriched_sessions_count: u32,
    pub last_synced_at_ms: Option<f64>,
    pub last_error: Option<String>,
    pub config: ZoomSyncConfig,
    pub upcoming_meetings: Vec<ZoomMeetingPreview>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ZoomSyncReport {
    pub meetings_synced: u32,
    pub events_upserted: u32,
    pub sessions_enriched: u32,
    pub participants_added: u32,
    pub synced_at_ms: f64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredZoomAccount {
    email: String,
    display_name: Option<String>,
    access_token: String,
    refresh_token: Option<String>,
    expires_at_epoch_secs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoredZoomSync {
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    account: Option<StoredZoomAccount>,
    #[serde(default)]
    config: ZoomSyncConfig,
    #[serde(default)]
    synced_meetings_count: u32,
    #[serde(default)]
    enriched_sessions_count: u32,
    #[serde(default)]
    last_synced_at_ms: Option<i64>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    upcoming_meetings: Vec<ZoomMeetingPreview>,
}

#[derive(Debug, Deserialize)]
struct ZoomTokenResponse {
    access_token: String,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ZoomUserMeResponse {
    email: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ZoomMeetingsListResponse {
    #[serde(default)]
    meetings: Vec<ZoomMeetingItem>,
}

#[derive(Debug, Clone, Deserialize)]
struct ZoomMeetingItem {
    #[serde(default)]
    id: Option<serde_json::Value>,
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    duration: Option<i64>,
    #[serde(default)]
    join_url: Option<String>,
    #[serde(default)]
    agenda: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ZoomMeetingDetailResponse {
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    join_url: Option<String>,
    #[serde(default)]
    agenda: Option<String>,
    #[serde(default)]
    settings: Option<ZoomMeetingSettings>,
}

#[derive(Debug, Deserialize)]
struct ZoomMeetingSettings {
    #[serde(default)]
    meeting_invitees: Vec<ZoomInvitee>,
}

#[derive(Debug, Deserialize)]
struct ZoomInvitee {
    #[serde(default)]
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ZoomRegistrantsResponse {
    #[serde(default)]
    registrants: Vec<ZoomRegistrant>,
}

#[derive(Debug, Deserialize)]
struct ZoomRegistrant {
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ZoomPastParticipantsResponse {
    #[serde(default)]
    participants: Vec<ZoomPastParticipant>,
}

#[derive(Debug, Deserialize)]
struct ZoomPastParticipant {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    user_email: Option<String>,
}

#[derive(Debug, Clone)]
struct ZoomParticipant {
    name: String,
    email: Option<String>,
}

#[derive(Debug, Clone)]
struct EnrichedZoomMeeting {
    meeting_id: String,
    uuid: Option<String>,
    topic: String,
    start_time: Option<String>,
    duration_mins: i64,
    join_url: Option<String>,
    agenda: Option<String>,
    participants: Vec<ZoomParticipant>,
}

fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn store_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("zoom-oauth.json")
}

fn load_store(path: &Path) -> Result<StoredZoomSync, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| format!("failed to read Zoom sync store: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StoredZoomSync::default()),
        Err(e) => Err(format!("failed to read Zoom sync store: {e}")),
    }
}

fn save_store(path: &Path, store: &StoredZoomSync) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create Zoom sync directory: {e}"))?;
    }
    let serialized = serde_json::to_string_pretty(store)
        .map_err(|e| format!("failed to serialize Zoom sync store: {e}"))?;
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, serialized)
        .map_err(|e| format!("failed to write Zoom sync store: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp_path, path)
        .map_err(|e| format!("failed to commit Zoom sync store: {e}"))?;
    Ok(())
}

fn mask_client_id(client_id: &str) -> String {
    let trimmed = client_id.trim();
    if trimmed.len() <= 12 {
        return trimmed.to_string();
    }
    format!("{}…{}", &trimmed[..6], &trimmed[trimmed.len() - 4..])
}

fn effective_credentials(store: &StoredZoomSync) -> Option<(String, Option<String>)> {
    if let Some(id) = store
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let secret = store
            .client_secret
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);
        return Some((id.to_string(), secret));
    }

    let env_id = std::env::var("OATMEAL_ZOOM_CLIENT_ID")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;
    let env_secret = std::env::var("OATMEAL_ZOOM_CLIENT_SECRET")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Some((env_id, env_secret))
}

fn build_status(store: &StoredZoomSync) -> ZoomSyncStatus {
    let creds = effective_credentials(store);
    ZoomSyncStatus {
        configured: creds.is_some(),
        client_id_hint: creds.map(|(id, _)| mask_client_id(&id)),
        redirect_uri: ZOOM_REDIRECT_URI.to_string(),
        connected: store.account.is_some(),
        email: store.account.as_ref().map(|a| a.email.clone()),
        display_name: store
            .account
            .as_ref()
            .and_then(|a| a.display_name.clone()),
        synced_meetings_count: store.synced_meetings_count,
        enriched_sessions_count: store.enriched_sessions_count,
        last_synced_at_ms: store.last_synced_at_ms.map(|ms| ms as f64),
        last_error: store.last_error.clone(),
        config: store.config.clone(),
        upcoming_meetings: store.upcoming_meetings.clone(),
    }
}

pub fn get_status(app_data_dir: &Path) -> Result<ZoomSyncStatus, String> {
    let store = load_store(&store_path(app_data_dir))?;
    Ok(build_status(&store))
}

pub fn save_credentials(
    app_data_dir: &Path,
    client_id: &str,
    client_secret: Option<&str>,
) -> Result<ZoomSyncStatus, String> {
    let trimmed_id = client_id.trim();
    if trimmed_id.is_empty() {
        return Err("Zoom OAuth Client ID is required.".to_string());
    }
    let path = store_path(app_data_dir);
    let mut store = load_store(&path)?;
    store.client_id = Some(trimmed_id.to_string());
    store.client_secret = client_secret
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    store.last_error = None;
    save_store(&path, &store)?;
    Ok(build_status(&store))
}

pub fn update_config(
    app_data_dir: &Path,
    config: ZoomSyncConfig,
) -> Result<ZoomSyncStatus, String> {
    let path = store_path(app_data_dir);
    let mut store = load_store(&path)?;
    store.config = config;
    save_store(&path, &store)?;
    Ok(build_status(&store))
}

pub fn disconnect(app_data_dir: &Path) -> Result<ZoomSyncStatus, String> {
    let path = store_path(app_data_dir);
    let mut store = load_store(&path)?;
    store.account = None;
    store.last_error = None;
    store.upcoming_meetings.clear();
    store.synced_meetings_count = 0;
    save_store(&path, &store)?;
    Ok(build_status(&store))
}

fn generate_pkce() -> (String, String) {
    let raw = format!(
        "{}{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let verifier = URL_SAFE_NO_PAD.encode(raw.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

async fn wait_for_oauth_callback(
    listener: TcpListener,
    expected_state: &str,
) -> Result<String, String> {
    let deadline = tokio::time::Instant::now() + OAUTH_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("Zoom sign-in timed out after 5 minutes.".to_string());
        }
        let (mut socket, _) = tokio::time::timeout(remaining, listener.accept())
            .await
            .map_err(|_| "Zoom sign-in timed out after 5 minutes.".to_string())?
            .map_err(|e| format!("failed to accept Zoom OAuth callback: {e}"))?;

        let mut buf = [0u8; 8192];
        let n = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut buf))
            .await
            .map_err(|_| "timed out reading Zoom OAuth callback".to_string())?
            .map_err(|e| format!("failed reading Zoom OAuth callback: {e}"))?;
        let req = String::from_utf8_lossy(&buf[..n]);
        let request_target = req
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");
        if !request_target.starts_with("/zoom/callback") {
            let _ = socket
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
            continue;
        }
        let full_url = format!("http://127.0.0.1:{ZOOM_REDIRECT_PORT}{request_target}");
        let parsed = Url::parse(&full_url)
            .map_err(|e| format!("invalid Zoom OAuth callback URL: {e}"))?;
        let mut code = None;
        let mut state = None;
        let mut oauth_error = None;
        for (k, v) in parsed.query_pairs() {
            match k.as_ref() {
                "code" => code = Some(v.into_owned()),
                "state" => state = Some(v.into_owned()),
                "error" => oauth_error = Some(v.into_owned()),
                _ => {}
            }
        }
        if let Some(err) = oauth_error {
            let body = format!(
                "<!doctype html><html><body style=\"font-family:-apple-system,sans-serif;padding:40px;text-align:center\"><h2>Zoom sign-in cancelled</h2><p>{err}</p><p>You can close this tab and return to Oatmeal.</p></body></html>"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            return Err(format!("Zoom OAuth error: {err}"));
        }
        if state.as_deref() != Some(expected_state) {
            let _ = socket
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 13\r\nConnection: close\r\n\r\nInvalid state")
                .await;
            return Err("Zoom OAuth state mismatch.".to_string());
        }
        let Some(code) = code else {
            return Err("Zoom OAuth callback missing authorization code.".to_string());
        };
        let body = "<!doctype html><html><body style=\"font-family:-apple-system,sans-serif;padding:40px;text-align:center\"><h2>Zoom connected to Oatmeal</h2><p>You can close this tab and return to Oatmeal.</p></body></html>";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = socket.write_all(resp.as_bytes()).await;
        return Ok(code);
    }
}

fn zoom_basic_auth(client_id: &str, client_secret: &str) -> String {
    let creds = format!("{client_id}:{client_secret}");
    format!("Basic {}", BASE64_STANDARD.encode(creds.as_bytes()))
}

pub async fn connect(app_data_dir: &Path) -> Result<ZoomSyncStatus, String> {
    let path = store_path(app_data_dir);
    let mut store = load_store(&path)?;
    let (client_id, client_secret) = effective_credentials(&store).ok_or_else(|| {
        "Zoom OAuth Client ID and Client Secret are not configured yet.".to_string()
    })?;

    let listener = TcpListener::bind(("127.0.0.1", ZOOM_REDIRECT_PORT))
        .await
        .map_err(|e| {
            format!(
                "Could not listen on port {ZOOM_REDIRECT_PORT} for Zoom OAuth callback: {e}"
            )
        })?;

    let (verifier, challenge) = generate_pkce();
    let state = uuid::Uuid::new_v4().simple().to_string();

    let mut auth_url = Url::parse(ZOOM_AUTH_URL).map_err(|e| e.to_string())?;
    auth_url
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", ZOOM_REDIRECT_URI)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);

    open::that(auth_url.as_str())
        .map_err(|e| format!("failed to open browser for Zoom sign-in: {e}"))?;

    let code = wait_for_oauth_callback(listener, &state).await?;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", ZOOM_REDIRECT_URI),
        ("code_verifier", verifier.as_str()),
    ];
    let mut req = http.post(ZOOM_TOKEN_URL);
    if let Some(secret) = client_secret.as_deref().filter(|s| !s.is_empty()) {
        req = req.header(
            reqwest::header::AUTHORIZATION,
            zoom_basic_auth(&client_id, secret),
        );
    } else {
        form.push(("client_id", client_id.as_str()));
    }

    let token_resp = req
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("failed to exchange Zoom OAuth code: {e}"))?;
    let status = token_resp.status();
    let token_text = token_resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("Zoom token exchange failed ({status}): {token_text}"));
    }
    let token: ZoomTokenResponse = serde_json::from_str(&token_text)
        .map_err(|e| format!("invalid Zoom token response: {e}"))?;

    let (email, display_name) = fetch_zoom_user_me(&http, ZOOM_API_BASE, &token.access_token)
        .await
        .unwrap_or_else(|_| ("Zoom account".to_string(), None));

    store.account = Some(StoredZoomAccount {
        email,
        display_name,
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at_epoch_secs: now_epoch_secs() + token.expires_in.unwrap_or(3600),
    });
    store.last_error = None;
    save_store(&path, &store)?;
    Ok(build_status(&store))
}

async fn fetch_zoom_user_me(
    http: &reqwest::Client,
    api_base: &str,
    access_token: &str,
) -> Result<(String, Option<String>), String> {
    let url = format!("{}/users/me", api_base.trim_end_matches('/'));
    let resp = http
        .get(&url)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("failed to fetch Zoom profile: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Zoom /users/me failed: {}", resp.status()));
    }
    let info: ZoomUserMeResponse = resp.json().await.map_err(|e| e.to_string())?;
    let email = info
        .email
        .filter(|e| !e.trim().is_empty())
        .unwrap_or_else(|| "Zoom account".to_string());
    let display_name = info
        .display_name
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            let full = [info.first_name, info.last_name]
                .into_iter()
                .flatten()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            (!full.is_empty()).then_some(full)
        });
    Ok((email, display_name))
}

async fn ensure_fresh_access_token(
    http: &reqwest::Client,
    token_url: &str,
    store: &mut StoredZoomSync,
) -> Result<String, String> {
    let Some(account) = store.account.as_ref() else {
        return Err("Zoom is not connected.".to_string());
    };
    if account.expires_at_epoch_secs > now_epoch_secs() + TOKEN_REFRESH_SKEW_SECS {
        return Ok(account.access_token.clone());
    }

    let refresh_token = account.refresh_token.clone().ok_or_else(|| {
        "Zoom session expired. Please reconnect Zoom in Settings → Sync.".to_string()
    })?;
    let (client_id, client_secret) = effective_credentials(store)
        .ok_or_else(|| "Zoom OAuth client credentials are missing.".to_string())?;

    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.as_str()),
    ];
    let mut req = http.post(token_url);
    if let Some(secret) = client_secret.as_deref().filter(|s| !s.is_empty()) {
        req = req.header(
            reqwest::header::AUTHORIZATION,
            zoom_basic_auth(&client_id, secret),
        );
    } else {
        form.push(("client_id", client_id.as_str()));
    }

    let resp = req
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("failed to refresh Zoom token: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        if body.contains("invalid_grant") || body.contains("Invalid Token") {
            return Err("Zoom access expired or was revoked. Please reconnect Zoom.".to_string());
        }
        return Err(format!("Zoom token refresh failed ({status}): {body}"));
    }

    let refreshed: ZoomTokenResponse =
        serde_json::from_str(&body).map_err(|e| format!("invalid Zoom refresh response: {e}"))?;
    if let Some(acc) = store.account.as_mut() {
        acc.access_token = refreshed.access_token.clone();
        if let Some(rt) = refreshed.refresh_token {
            acc.refresh_token = Some(rt);
        }
        acc.expires_at_epoch_secs = now_epoch_secs() + refreshed.expires_in.unwrap_or(3600);
    }
    Ok(refreshed.access_token)
}

fn meeting_id_to_string(value: &Option<serde_json::Value>) -> Option<String> {
    match value.as_ref()? {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

/// Zoom requires double URL-encoding for UUIDs starting with `/` or containing `//`.
pub fn encode_zoom_uuid(uuid: &str) -> String {
    let encoded_once = urlencoding::encode(uuid);
    if uuid.starts_with('/') || uuid.contains("//") {
        urlencoding::encode(&encoded_once).into_owned()
    } else {
        encoded_once.into_owned()
    }
}

fn name_from_email(email: &str) -> String {
    let local = email.split('@').next().unwrap_or(email);
    let cleaned = local
        .split(['.', '_', '-', '+'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() {
        email.to_string()
    } else {
        cleaned
    }
}

fn push_unique_participant(
    list: &mut Vec<ZoomParticipant>,
    seen_keys: &mut HashSet<String>,
    self_email: &str,
    raw_name: Option<&str>,
    raw_email: Option<&str>,
) {
    let email = raw_email
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.contains('@'))
        .map(str::to_lowercase);
    if let Some(ref e) = email
        && !self_email.is_empty()
        && e == self_email
    {
        return;
    }
    let name = raw_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .or_else(|| email.as_deref().map(name_from_email));
    let Some(name) = name else {
        return;
    };
    let key = email
        .clone()
        .unwrap_or_else(|| format!("name:{}", name.to_lowercase()));
    if seen_keys.insert(key) {
        list.push(ZoomParticipant { name, email });
    }
}

async fn fetch_zoom_meetings(
    http: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    self_email: &str,
) -> Result<Vec<EnrichedZoomMeeting>, String> {
    let base = api_base.trim_end_matches('/');
    let mut raw_meetings: Vec<(ZoomMeetingItem, bool)> = Vec::new();

    for (query_type, is_past) in [("upcoming_meetings", false), ("previous_meetings", true)] {
        let url = format!("{base}/users/me/meetings?type={query_type}&page_size=30");
        let resp = http
            .get(&url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| format!("failed to list Zoom {query_type}: {e}"))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            if query_type == "upcoming_meetings" {
                return Err(format!("Zoom meetings API failed ({status}): {body}"));
            }
            continue;
        }
        if let Ok(parsed) = serde_json::from_str::<ZoomMeetingsListResponse>(&body) {
            for item in parsed.meetings {
                raw_meetings.push((item, is_past));
            }
        }
    }

    let mut seen_meeting_keys = HashSet::new();
    let mut enriched = Vec::new();
    let self_email_lower = self_email.trim().to_lowercase();

    for (item, is_past) in raw_meetings {
        let Some(meeting_id) = meeting_id_to_string(&item.id) else {
            continue;
        };
        let dedup_key = format!(
            "{}:{}",
            meeting_id,
            item.start_time.as_deref().unwrap_or_default()
        );
        if !seen_meeting_keys.insert(dedup_key) {
            continue;
        }

        let mut topic = item
            .topic
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Zoom Meeting")
            .to_string();
        let mut join_url = item.join_url.clone();
        let mut agenda = item.agenda.clone();
        let mut participants = Vec::new();
        let mut seen_participants = HashSet::new();

        // 1. Meeting details (invitees + agenda + join_url)
        let detail_url = format!("{base}/meetings/{}", urlencoding::encode(&meeting_id));
        if let Ok(resp) = http.get(&detail_url).bearer_auth(access_token).send().await
            && resp.status().is_success()
            && let Ok(detail) = resp.json::<ZoomMeetingDetailResponse>().await
        {
            if let Some(t) = detail.topic.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                topic = t.to_string();
            }
            if join_url.is_none() {
                join_url = detail.join_url;
            }
            if agenda.is_none() {
                agenda = detail.agenda;
            }
            if let Some(settings) = detail.settings {
                for invitee in settings.meeting_invitees {
                    push_unique_participant(
                        &mut participants,
                        &mut seen_participants,
                        &self_email_lower,
                        None,
                        invitee.email.as_deref(),
                    );
                }
            }
        }

        // 2. Registrants (best-effort; skipped cleanly if registration isn't enabled or scope missing)
        let reg_url = format!(
            "{base}/meetings/{}/registrants?page_size=100",
            urlencoding::encode(&meeting_id)
        );
        if let Ok(resp) = http.get(&reg_url).bearer_auth(access_token).send().await
            && resp.status().is_success()
            && let Ok(regs) = resp.json::<ZoomRegistrantsResponse>().await
        {
            for reg in regs.registrants {
                let full_name = [reg.first_name, reg.last_name]
                    .into_iter()
                    .flatten()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                push_unique_participant(
                    &mut participants,
                    &mut seen_participants,
                    &self_email_lower,
                    (!full_name.is_empty()).then_some(full_name.as_str()),
                    reg.email.as_deref(),
                );
            }
        }

        // 3. Past meeting participants (best-effort; works on licensed plans with report/past_meetings scope)
        if is_past
            && let Some(uuid) = item.uuid.as_deref().filter(|s| !s.trim().is_empty())
        {
            let past_url = format!(
                "{base}/past_meetings/{}/participants?page_size=100",
                encode_zoom_uuid(uuid)
            );
            if let Ok(resp) = http.get(&past_url).bearer_auth(access_token).send().await
                && resp.status().is_success()
                && let Ok(past) = resp.json::<ZoomPastParticipantsResponse>().await
            {
                for p in past.participants {
                    push_unique_participant(
                        &mut participants,
                        &mut seen_participants,
                        &self_email_lower,
                        p.name.as_deref(),
                        p.user_email.as_deref(),
                    );
                }
            }
        }

        enriched.push(EnrichedZoomMeeting {
            meeting_id,
            uuid: item.uuid,
            topic,
            start_time: item.start_time,
            duration_mins: item.duration.unwrap_or(60).max(15),
            join_url,
            agenda,
            participants,
        });
    }

    Ok(enriched)
}

pub fn extract_zoom_meeting_id_from_text(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    for marker in ["zoom.us/j/", "zoom.us/w/", "zoom.us/s/"] {
        if let Some(pos) = lower.find(marker) {
            let after = &text[pos + marker.len()..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.len() >= 9 {
                return Some(digits);
            }
        }
    }
    None
}

fn is_generic_session_title(title: &str) -> bool {
    matches!(
        title.trim().to_ascii_lowercase().as_str(),
        "" | "untitled"
            | "new note"
            | "meeting"
            | "call"
            | "zoom"
            | "zoom meeting"
            | "zoom workplace"
    )
}

async fn upsert_zoom_calendar_events(
    pool: &SqlitePool,
    meetings: &[EnrichedZoomMeeting],
) -> Result<u32, String> {
    let now_iso = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO calendars (
            id, tracking_id_calendar, name, enabled, provider, source, color, connection_id, created_at, updated_at, deleted_at
        ) VALUES (?1, 'zoom-local', 'Zoom Meetings', 1, 'zoom', 'zoom', '#2D8CFF', 'local-zoom', ?2, ?2, NULL)
        ON CONFLICT(id) DO UPDATE SET
            name = 'Zoom Meetings',
            deleted_at = NULL,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(ZOOM_CALENDAR_ID)
    .bind(&now_iso)
    .execute(pool)
    .await
    .map_err(|e| format!("failed to upsert Zoom calendar: {e}"))?;

    let mut upserted = 0u32;
    for meeting in meetings {
        let Some(start_str) = meeting.start_time.as_deref() else {
            continue;
        };
        let Ok(start_dt) = DateTime::parse_from_rfc3339(start_str) else {
            continue;
        };
        let end_dt = start_dt + ChronoDuration::minutes(meeting.duration_mins);
        let event_id = format!(
            "zoom-event-{}-{}",
            meeting.meeting_id,
            start_dt.timestamp()
        );
        let participants_json = serde_json::to_string(
            &meeting
                .participants
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "name": p.name,
                        "email": p.email,
                        "is_current_user": false,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_string());

        sqlx::query(
            r#"
            INSERT INTO events (
                id, tracking_id_event, calendar_id, title, started_at, ended_at,
                location, meeting_link, description, note, recurrence_series_id,
                has_recurrence_rules, is_all_day, provider, participants_json,
                created_at, updated_at, deleted_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                'Zoom', ?7, ?8, '', '',
                0, 0, 'zoom', ?9,
                ?10, ?10, NULL
            )
            ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                started_at = excluded.started_at,
                ended_at = excluded.ended_at,
                meeting_link = excluded.meeting_link,
                description = excluded.description,
                participants_json = excluded.participants_json,
                deleted_at = NULL,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&event_id)
        .bind(&meeting.meeting_id)
        .bind(ZOOM_CALENDAR_ID)
        .bind(&meeting.topic)
        .bind(start_dt.to_rfc3339())
        .bind(end_dt.to_rfc3339())
        .bind(meeting.join_url.as_deref().unwrap_or_default())
        .bind(meeting.agenda.as_deref().unwrap_or_default())
        .bind(&participants_json)
        .bind(&now_iso)
        .execute(pool)
        .await
        .map_err(|e| format!("failed to upsert Zoom event {}: {e}", meeting.meeting_id))?;

        upserted += 1;
    }

    Ok(upserted)
}

async fn enrich_sessions_from_zoom(
    pool: &SqlitePool,
    meetings: &[EnrichedZoomMeeting],
    session_id_filter: Option<&str>,
) -> Result<(u32, u32), String> {
    if meetings.is_empty() {
        return Ok((0, 0));
    }

    let rows = if let Some(sid) = session_id_filter {
        sqlx::query(
            "SELECT id, workspace_id, owner_user_id, title, created_at, event_json FROM sessions WHERE id = ?1 AND deleted_at IS NULL",
        )
        .bind(sid)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("failed to load session for Zoom enrichment: {e}"))?
    } else {
        let cutoff = (Utc::now() - ChronoDuration::days(14)).to_rfc3339();
        sqlx::query(
            "SELECT id, workspace_id, owner_user_id, title, created_at, event_json FROM sessions WHERE deleted_at IS NULL AND created_at >= ?1 ORDER BY created_at DESC LIMIT 50",
        )
        .bind(&cutoff)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("failed to load recent sessions for Zoom enrichment: {e}"))?
    };

    let by_meeting_id: HashMap<&str, &EnrichedZoomMeeting> = meetings
        .iter()
        .map(|m| (m.meeting_id.as_str(), m))
        .collect();

    let mut sessions_enriched = 0u32;
    let mut participants_added = 0u32;
    let now_iso = Utc::now().to_rfc3339();

    for row in rows {
        let session_id: String = row.try_get("id").unwrap_or_default();
        let workspace_id: String = row.try_get("workspace_id").unwrap_or_default();
        let owner_user_id: String = row.try_get("owner_user_id").unwrap_or_default();
        let title: String = row.try_get("title").unwrap_or_default();
        let created_at: String = row.try_get("created_at").unwrap_or_default();
        let event_json: String = row.try_get("event_json").unwrap_or_default();

        // Match by explicit Zoom meeting ID first, then by start-time proximity (±20 min)
        let matched = extract_zoom_meeting_id_from_text(&event_json)
            .and_then(|id| by_meeting_id.get(id.as_str()).copied())
            .or_else(|| {
                let session_dt = DateTime::parse_from_rfc3339(&created_at).ok()?;
                meetings
                    .iter()
                    .filter_map(|m| {
                        let start_dt =
                            DateTime::parse_from_rfc3339(m.start_time.as_deref()?).ok()?;
                        let diff_mins = (session_dt.timestamp() - start_dt.timestamp()).abs() / 60;
                        (diff_mins <= SESSION_MATCH_WINDOW_MINS).then_some((diff_mins, m))
                    })
                    .min_by_key(|(diff, _)| *diff)
                    .map(|(_, m)| m)
            });

        let Some(meeting) = matched else {
            continue;
        };

        let mut session_touched = false;

        if is_generic_session_title(&title) && !is_generic_session_title(&meeting.topic) {
            sqlx::query("UPDATE sessions SET title = ?1, updated_at = ?2 WHERE id = ?3")
                .bind(&meeting.topic)
                .bind(&now_iso)
                .bind(&session_id)
                .execute(pool)
                .await
                .map_err(|e| format!("failed to update session title from Zoom: {e}"))?;
            session_touched = true;
        }

        for participant in &meeting.participants {
            let human_id = upsert_zoom_human(
                pool,
                &workspace_id,
                &owner_user_id,
                &participant.name,
                participant.email.as_deref(),
                &now_iso,
            )
            .await?;

            let existing_link: Option<String> = sqlx::query_scalar(
                "SELECT id FROM session_participants WHERE session_id = ?1 AND human_id = ?2 AND deleted_at IS NULL LIMIT 1",
            )
            .bind(&session_id)
            .bind(&human_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("failed to check session participant link: {e}"))?;

            if existing_link.is_none() {
                let link_id = uuid::Uuid::new_v4().to_string();
                sqlx::query(
                    r#"
                    INSERT INTO session_participants (
                        id, workspace_id, owner_user_id, session_id, human_id,
                        display_name, email, role, source, metadata_json, created_at, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '', 'zoom_api', '{}', ?8, ?8)
                    "#,
                )
                .bind(&link_id)
                .bind(&workspace_id)
                .bind(&owner_user_id)
                .bind(&session_id)
                .bind(&human_id)
                .bind(&participant.name)
                .bind(participant.email.as_deref().unwrap_or_default())
                .bind(&now_iso)
                .execute(pool)
                .await
                .map_err(|e| format!("failed to insert Zoom session participant: {e}"))?;
                participants_added += 1;
                session_touched = true;
            }
        }

        if session_touched {
            sessions_enriched += 1;
        }
    }

    Ok((sessions_enriched, participants_added))
}

async fn upsert_zoom_human(
    pool: &SqlitePool,
    workspace_id: &str,
    owner_user_id: &str,
    name: &str,
    email: Option<&str>,
    now_iso: &str,
) -> Result<String, String> {
    if let Some(email) = email.filter(|s| !s.trim().is_empty()) {
        if let Some(existing_id) = sqlx::query_scalar::<_, String>(
            "SELECT id FROM humans WHERE deleted_at IS NULL AND LOWER(TRIM(email)) = LOWER(?1) LIMIT 1",
        )
        .bind(email)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("failed to look up human by email: {e}"))?
        {
            return Ok(existing_id);
        }
    }

    if let Some((existing_id, existing_email)) = sqlx::query_as::<_, (String, String)>(
        "SELECT id, email FROM humans WHERE deleted_at IS NULL AND LOWER(TRIM(name)) = LOWER(?1) LIMIT 1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("failed to look up human by name: {e}"))?
    {
        if existing_email.trim().is_empty()
            && let Some(email) = email.filter(|s| !s.trim().is_empty())
        {
            let _ = sqlx::query("UPDATE humans SET email = ?1, updated_at = ?2 WHERE id = ?3")
                .bind(email)
                .bind(now_iso)
                .bind(&existing_id)
                .execute(pool)
                .await;
        }
        return Ok(existing_id);
    }

    let human_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO humans (
            id, workspace_id, owner_user_id, organization_id, name, email,
            phone, job_title, linkedin_username, memo, pinned, metadata_json, created_at, updated_at
        ) VALUES (?1, ?2, ?3, '', ?4, ?5, '', '', '', '', 0, '{}', ?6, ?6)
        "#,
    )
    .bind(&human_id)
    .bind(workspace_id)
    .bind(owner_user_id)
    .bind(name)
    .bind(email.unwrap_or_default())
    .bind(now_iso)
    .execute(pool)
    .await
    .map_err(|e| format!("failed to create Zoom human contact: {e}"))?;
    Ok(human_id)
}

pub async fn sync_meetings(
    pool: &SqlitePool,
    app_data_dir: &Path,
    session_id: Option<String>,
) -> Result<ZoomSyncReport, String> {
    sync_meetings_with_endpoints(pool, app_data_dir, session_id, ZOOM_TOKEN_URL, ZOOM_API_BASE)
        .await
}

async fn sync_meetings_with_endpoints(
    pool: &SqlitePool,
    app_data_dir: &Path,
    session_id: Option<String>,
    token_url: &str,
    api_base: &str,
) -> Result<ZoomSyncReport, String> {
    let path = store_path(app_data_dir);
    let mut store = load_store(&path)?;
    if store.account.is_none() {
        return Err("Zoom is not connected.".to_string());
    }

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let access_token = match ensure_fresh_access_token(&http, token_url, &mut store).await {
        Ok(tok) => tok,
        Err(err) => {
            store.last_error = Some(err.clone());
            let _ = save_store(&path, &store);
            return Err(err);
        }
    };
    let self_email = store
        .account
        .as_ref()
        .map(|a| a.email.clone())
        .unwrap_or_default();

    let meetings = match fetch_zoom_meetings(&http, api_base, &access_token, &self_email).await {
        Ok(list) => list,
        Err(err) => {
            store.last_error = Some(err.clone());
            let _ = save_store(&path, &store);
            return Err(err);
        }
    };

    let mut errors = Vec::new();
    let events_upserted = if store.config.sync_calendar_events {
        match upsert_zoom_calendar_events(pool, &meetings).await {
            Ok(n) => n,
            Err(e) => {
                errors.push(e);
                0
            }
        }
    } else {
        0
    };

    let (sessions_enriched, participants_added) = if store.config.enrich_sessions {
        match enrich_sessions_from_zoom(pool, &meetings, session_id.as_deref()).await {
            Ok(pair) => pair,
            Err(e) => {
                errors.push(e);
                (0, 0)
            }
        }
    } else {
        (0, 0)
    };

    let now_ms = now_epoch_ms();
    let now_rfc = Utc::now().to_rfc3339();
    let mut upcoming: Vec<ZoomMeetingPreview> = meetings
        .iter()
        .filter(|m| {
            m.start_time
                .as_deref()
                .is_some_and(|start| start >= now_rfc.as_str())
        })
        .map(|m| ZoomMeetingPreview {
            meeting_id: m.meeting_id.clone(),
            topic: m.topic.clone(),
            start_time: m.start_time.clone(),
            join_url: m.join_url.clone(),
            participant_count: m.participants.len() as u32,
        })
        .collect();
    upcoming.sort_by(|a, b| a.start_time.cmp(&b.start_time));
    upcoming.truncate(8);

    store.synced_meetings_count = meetings.len() as u32;
    store.enriched_sessions_count = store
        .enriched_sessions_count
        .saturating_add(sessions_enriched);
    store.last_synced_at_ms = Some(now_ms);
    store.last_error = errors.first().cloned();
    store.upcoming_meetings = upcoming;
    save_store(&path, &store)?;

    Ok(ZoomSyncReport {
        meetings_synced: meetings.len() as u32,
        events_upserted,
        sessions_enriched,
        participants_added,
        synced_at_ms: now_ms as f64,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;
    use tempfile::tempdir;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE calendars (
              id TEXT PRIMARY KEY NOT NULL,
              tracking_id_calendar TEXT NOT NULL DEFAULT '',
              name TEXT NOT NULL DEFAULT '',
              enabled INTEGER NOT NULL DEFAULT 0,
              provider TEXT NOT NULL DEFAULT '',
              source TEXT NOT NULL DEFAULT '',
              color TEXT NOT NULL DEFAULT '#888',
              connection_id TEXT NOT NULL DEFAULT '',
              created_at TEXT NOT NULL DEFAULT '',
              updated_at TEXT NOT NULL DEFAULT '',
              deleted_at TEXT
            );
            CREATE TABLE events (
              id TEXT PRIMARY KEY NOT NULL,
              tracking_id_event TEXT NOT NULL DEFAULT '',
              calendar_id TEXT NOT NULL DEFAULT '',
              title TEXT NOT NULL DEFAULT '',
              started_at TEXT NOT NULL DEFAULT '',
              ended_at TEXT NOT NULL DEFAULT '',
              location TEXT NOT NULL DEFAULT '',
              meeting_link TEXT NOT NULL DEFAULT '',
              description TEXT NOT NULL DEFAULT '',
              note TEXT NOT NULL DEFAULT '',
              recurrence_series_id TEXT NOT NULL DEFAULT '',
              has_recurrence_rules INTEGER NOT NULL DEFAULT 0,
              is_all_day INTEGER NOT NULL DEFAULT 0,
              provider TEXT NOT NULL DEFAULT '',
              participants_json TEXT,
              created_at TEXT NOT NULL DEFAULT '',
              updated_at TEXT NOT NULL DEFAULT '',
              deleted_at TEXT,
              attendance_json TEXT
            );
            CREATE TABLE sessions (
              id TEXT PRIMARY KEY NOT NULL,
              workspace_id TEXT NOT NULL DEFAULT '',
              owner_user_id TEXT NOT NULL DEFAULT '',
              title TEXT NOT NULL DEFAULT '',
              created_at TEXT NOT NULL DEFAULT '',
              updated_at TEXT NOT NULL DEFAULT '',
              event_json TEXT NOT NULL DEFAULT '',
              deleted_at TEXT
            );
            CREATE TABLE humans (
              id TEXT PRIMARY KEY NOT NULL,
              workspace_id TEXT NOT NULL DEFAULT '',
              owner_user_id TEXT NOT NULL DEFAULT '',
              organization_id TEXT NOT NULL DEFAULT '',
              name TEXT NOT NULL DEFAULT '',
              email TEXT NOT NULL DEFAULT '',
              phone TEXT NOT NULL DEFAULT '',
              job_title TEXT NOT NULL DEFAULT '',
              linkedin_username TEXT NOT NULL DEFAULT '',
              memo TEXT NOT NULL DEFAULT '',
              pinned INTEGER NOT NULL DEFAULT 0,
              pin_order INTEGER,
              metadata_json TEXT NOT NULL DEFAULT '{}',
              created_at TEXT NOT NULL DEFAULT '',
              updated_at TEXT NOT NULL DEFAULT '',
              deleted_at TEXT
            );
            CREATE TABLE session_participants (
              id TEXT PRIMARY KEY NOT NULL,
              workspace_id TEXT NOT NULL DEFAULT '',
              owner_user_id TEXT NOT NULL DEFAULT '',
              session_id TEXT NOT NULL DEFAULT '',
              human_id TEXT NOT NULL DEFAULT '',
              display_name TEXT NOT NULL DEFAULT '',
              email TEXT NOT NULL DEFAULT '',
              role TEXT NOT NULL DEFAULT '',
              source TEXT NOT NULL DEFAULT '',
              metadata_json TEXT NOT NULL DEFAULT '{}',
              created_at TEXT NOT NULL DEFAULT '',
              updated_at TEXT NOT NULL DEFAULT '',
              deleted_at TEXT
            );
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[test]
    fn encodes_zoom_uuid_and_extracts_meeting_ids() {
        assert_eq!(encode_zoom_uuid("abc123=="), "abc123%3D%3D");
        assert_eq!(encode_zoom_uuid("/abc//def=="), "%252Fabc%252F%252Fdef%253D%253D");
        assert_eq!(
            extract_zoom_meeting_id_from_text("https://us02web.zoom.us/j/98765432109?pwd=xyz"),
            Some("98765432109".to_string())
        );
    }

    #[tokio::test]
    async fn refreshes_token_syncs_zoom_events_and_enriches_session_participants() {
        let dir = tempdir().unwrap();
        let pool = setup_test_db().await;
        let server = MockServer::start().await;

        let now = Utc::now();
        let past_start = (now - ChronoDuration::minutes(5)).to_rfc3339();
        let future_start = (now + ChronoDuration::hours(2)).to_rfc3339();

        // Seed an untitled Oatmeal session that took place during the Zoom meeting
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, owner_user_id, title, created_at, updated_at, event_json) VALUES ('s1', 'w1', 'u1', '', ?1, ?1, '')",
        )
        .bind(now.to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();

        // Seed an expired Zoom OAuth token to verify automatic refresh
        save_credentials(dir.path(), "zoom-client-id", Some("zoom-client-secret")).unwrap();
        let mut store = load_store(&store_path(dir.path())).unwrap();
        store.account = Some(StoredZoomAccount {
            email: "stuart@example.com".to_string(),
            display_name: Some("Stuart Chuang".to_string()),
            access_token: "expired-token".to_string(),
            refresh_token: Some("refresh-123".to_string()),
            expires_at_epoch_secs: 10,
        });
        save_store(&store_path(dir.path()), &store).unwrap();

        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "fresh-zoom-token",
                "refresh_token": "refresh-456",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v2/users/me/meetings"))
            .and(query_param("type", "upcoming_meetings"))
            .and(header("authorization", "Bearer fresh-zoom-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "meetings": [{
                    "id": 99911122233u64,
                    "uuid": "uuid-upcoming",
                    "topic": "Product Roadmap Sync",
                    "start_time": future_start,
                    "duration": 45,
                    "join_url": "https://us02web.zoom.us/j/99911122233"
                }]
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v2/users/me/meetings"))
            .and(query_param("type", "previous_meetings"))
            .and(header("authorization", "Bearer fresh-zoom-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "meetings": [{
                    "id": 88877766655u64,
                    "uuid": "/past//uuid",
                    "topic": "Design Architecture Review",
                    "start_time": past_start,
                    "duration": 30,
                    "join_url": "https://us02web.zoom.us/j/88877766655"
                }]
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v2/meetings/99911122233"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "topic": "Product Roadmap Sync",
                "agenda": "Q1 milestones",
                "settings": {
                    "meeting_invitees": [
                        { "email": "ada.lovelace@example.com" },
                        { "email": "stuart@example.com" }
                    ]
                }
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v2/meetings/99911122233/registrants"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "registrants": [{
                    "first_name": "Grace",
                    "last_name": "Hopper",
                    "email": "grace@example.com"
                }]
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v2/meetings/88877766655"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "topic": "Design Architecture Review",
                "settings": { "meeting_invitees": [] }
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v2/meetings/88877766655/registrants"))
            .respond_with(ResponseTemplate::new(400))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v2/past_meetings/%252Fpast%252F%252Fuuid/participants"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "participants": [
                    { "name": "Linus Torvalds", "user_email": "linus@example.com" },
                    { "name": "Stuart Chuang", "user_email": "stuart@example.com" }
                ]
            })))
            .mount(&server)
            .await;

        let token_url = format!("{}/oauth/token", server.uri());
        let api_base = format!("{}/v2", server.uri());
        let report = sync_meetings_with_endpoints(&pool, dir.path(), None, &token_url, &api_base)
            .await
            .unwrap();

        assert_eq!(report.meetings_synced, 2);
        assert_eq!(report.events_upserted, 2);
        assert_eq!(report.sessions_enriched, 1);
        assert_eq!(report.participants_added, 1);

        let updated_title: String =
            sqlx::query_scalar("SELECT title FROM sessions WHERE id = 's1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(updated_title, "Design Architecture Review");

        let linked: Vec<(String, String)> = sqlx::query_as(
            "SELECT display_name, email FROM session_participants WHERE session_id = 's1'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            linked,
            vec![("Linus Torvalds".to_string(), "linus@example.com".to_string())]
        );

        let status = get_status(dir.path()).unwrap();
        assert_eq!(status.upcoming_meetings.len(), 1);
        assert_eq!(status.upcoming_meetings[0].topic, "Product Roadmap Sync");
        assert_eq!(status.upcoming_meetings[0].participant_count, 2);
    }
}
