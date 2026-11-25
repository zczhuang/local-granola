use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
const GOOGLE_DRIVE_API_BASE: &str = "https://www.googleapis.com";
const GOOGLE_DRIVE_SCOPES: &str =
    "https://www.googleapis.com/auth/drive.file https://www.googleapis.com/auth/userinfo.email";
const OAUTH_TIMEOUT: Duration = Duration::from_secs(300);
const TOKEN_REFRESH_SKEW_SECS: i64 = 120;
const DRIVE_FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const GOOGLE_DOC_MIME: &str = "application/vnd.google-apps.document";
const MARKDOWN_MIME: &str = "text/markdown";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoogleDriveSyncFormat {
    #[default]
    GoogleDoc,
    Markdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GoogleDriveSyncConfig {
    pub folder_name: String,
    pub format: GoogleDriveSyncFormat,
    pub include_transcript: bool,
    pub auto_sync: bool,
}

impl Default for GoogleDriveSyncConfig {
    fn default() -> Self {
        Self {
            folder_name: "Oatmeal Notes".to_string(),
            format: GoogleDriveSyncFormat::GoogleDoc,
            include_transcript: true,
            auto_sync: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GoogleDriveSyncStatus {
    pub configured: bool,
    pub credentials_source: String,
    pub client_id_hint: Option<String>,
    pub connected: bool,
    pub email: Option<String>,
    pub root_folder_id: Option<String>,
    pub root_folder_url: Option<String>,
    pub synced_count: u32,
    pub last_synced_at_ms: Option<f64>,
    pub last_error: Option<String>,
    pub config: GoogleDriveSyncConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GoogleDriveSyncReport {
    pub uploaded: u32,
    pub updated: u32,
    pub skipped: u32,
    pub failed: u32,
    pub total_synced: u32,
    pub folder_url: Option<String>,
    pub synced_at_ms: f64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SyncedFileRecord {
    drive_file_id: String,
    web_view_link: Option<String>,
    content_hash: String,
    format: GoogleDriveSyncFormat,
    file_name: String,
    parent_folder_id: String,
    synced_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredDriveAccount {
    email: String,
    access_token: String,
    refresh_token: Option<String>,
    expires_at_epoch_secs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoredGoogleDriveSync {
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    account: Option<StoredDriveAccount>,
    #[serde(default)]
    config: GoogleDriveSyncConfig,
    #[serde(default)]
    root_folder_id: Option<String>,
    #[serde(default)]
    root_folder_url: Option<String>,
    #[serde(default)]
    last_synced_at_ms: Option<i64>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    synced_files: HashMap<String, SyncedFileRecord>,
}

#[derive(Debug, Deserialize)]
struct CalendarOAuthCredentialsFallback {
    client_id: Option<String>,
    client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleDesktopClientFile {
    installed: Option<GoogleClientSection>,
    web: Option<GoogleClientSection>,
    client_id: Option<String>,
    client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleClientSection {
    client_id: String,
    client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UserInfoResponse {
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DriveFileEntry {
    id: String,
    #[serde(rename = "webViewLink")]
    web_view_link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DriveFilesListResponse {
    files: Vec<DriveFileEntry>,
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
    app_data_dir.join("google-drive-sync.json")
}

fn calendar_oauth_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("google-calendar-oauth.json")
}

fn load_store(path: &Path) -> Result<StoredGoogleDriveSync, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| format!("failed to read Google Drive sync store: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StoredGoogleDriveSync::default()),
        Err(e) => Err(format!("failed to read Google Drive sync store: {e}")),
    }
}

fn save_store(path: &Path, store: &StoredGoogleDriveSync) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create Google Drive sync directory: {e}"))?;
    }
    let serialized = serde_json::to_string_pretty(store)
        .map_err(|e| format!("failed to serialize Google Drive sync store: {e}"))?;
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, serialized)
        .map_err(|e| format!("failed to write Google Drive sync store: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp_path, path)
        .map_err(|e| format!("failed to commit Google Drive sync store: {e}"))?;
    Ok(())
}

fn mask_client_id(client_id: &str) -> String {
    let trimmed = client_id.trim();
    if trimmed.len() <= 16 {
        return trimmed.to_string();
    }
    format!("{}…{}", &trimmed[..10], &trimmed[trimmed.len() - 6..])
}

fn effective_credentials(
    app_data_dir: &Path,
    store: &StoredGoogleDriveSync,
) -> Option<(String, Option<String>, &'static str)> {
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
        return Some((id.to_string(), secret, "drive"));
    }

    if let Ok(raw) = std::fs::read_to_string(calendar_oauth_path(app_data_dir))
        && let Ok(cal) = serde_json::from_str::<CalendarOAuthCredentialsFallback>(&raw)
        && let Some(id) = cal
            .client_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        let secret = cal
            .client_secret
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);
        return Some((id.to_string(), secret, "calendar"));
    }

    let env_id = std::env::var("OATMEAL_GOOGLE_CLIENT_ID")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;
    let env_secret = std::env::var("OATMEAL_GOOGLE_CLIENT_SECRET")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Some((env_id, env_secret, "env"))
}

pub fn parse_oauth_credentials(
    client_id_or_json: &str,
    client_secret: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let trimmed = client_id_or_json.trim();
    if trimmed.is_empty() {
        return Err("Google OAuth Client ID or JSON is required.".to_string());
    }
    if trimmed.starts_with('{') {
        let parsed: GoogleDesktopClientFile = serde_json::from_str(trimmed).map_err(|e| {
            format!("Invalid Google OAuth JSON. Paste the Desktop client_secret_*.json file: {e}")
        })?;
        if let Some(section) = parsed.installed.or(parsed.web) {
            let id = section.client_id.trim().to_string();
            if id.is_empty() {
                return Err("OAuth JSON is missing client_id.".to_string());
            }
            let secret = section
                .client_secret
                .or_else(|| client_secret.map(ToString::to_string))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            return Ok((id, secret));
        }
        if let Some(id) = parsed
            .client_id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            let secret = parsed
                .client_secret
                .or_else(|| client_secret.map(ToString::to_string))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            return Ok((id, secret));
        }
        return Err("OAuth JSON does not contain an `installed` or `client_id` field.".to_string());
    }

    let secret = client_secret
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    Ok((trimmed.to_string(), secret))
}

fn build_status(app_data_dir: &Path, store: &StoredGoogleDriveSync) -> GoogleDriveSyncStatus {
    let creds = effective_credentials(app_data_dir, store);
    GoogleDriveSyncStatus {
        configured: creds.is_some(),
        credentials_source: creds
            .as_ref()
            .map(|(_, _, src)| (*src).to_string())
            .unwrap_or_else(|| "none".to_string()),
        client_id_hint: creds.map(|(id, _, _)| mask_client_id(&id)),
        connected: store.account.is_some(),
        email: store.account.as_ref().map(|a| a.email.clone()),
        root_folder_id: store.root_folder_id.clone(),
        root_folder_url: store.root_folder_url.clone(),
        synced_count: store.synced_files.len() as u32,
        last_synced_at_ms: store.last_synced_at_ms.map(|ms| ms as f64),
        last_error: store.last_error.clone(),
        config: store.config.clone(),
    }
}

pub fn get_status(app_data_dir: &Path) -> Result<GoogleDriveSyncStatus, String> {
    let store = load_store(&store_path(app_data_dir))?;
    Ok(build_status(app_data_dir, &store))
}

pub fn save_credentials(
    app_data_dir: &Path,
    client_id_or_json: &str,
    client_secret: Option<&str>,
) -> Result<GoogleDriveSyncStatus, String> {
    let (client_id, secret) = parse_oauth_credentials(client_id_or_json, client_secret)?;
    let path = store_path(app_data_dir);
    let mut store = load_store(&path)?;
    store.client_id = Some(client_id);
    store.client_secret = secret;
    save_store(&path, &store)?;
    Ok(build_status(app_data_dir, &store))
}

pub fn update_config(
    app_data_dir: &Path,
    config: GoogleDriveSyncConfig,
) -> Result<GoogleDriveSyncStatus, String> {
    let folder_name = config.folder_name.trim().to_string();
    let folder_name = if folder_name.is_empty() {
        "Oatmeal Notes".to_string()
    } else {
        folder_name
    };
    let path = store_path(app_data_dir);
    let mut store = load_store(&path)?;
    let folder_changed = store.config.folder_name != folder_name;
    store.config = GoogleDriveSyncConfig {
        folder_name,
        format: config.format,
        include_transcript: config.include_transcript,
        auto_sync: config.auto_sync,
    };
    if folder_changed {
        store.root_folder_id = None;
        store.root_folder_url = None;
        store.synced_files.clear();
    }
    save_store(&path, &store)?;
    Ok(build_status(app_data_dir, &store))
}

pub fn disconnect(app_data_dir: &Path) -> Result<GoogleDriveSyncStatus, String> {
    let path = store_path(app_data_dir);
    let mut store = load_store(&path)?;
    store.account = None;
    store.root_folder_id = None;
    store.root_folder_url = None;
    store.last_error = None;
    store.synced_files.clear();
    save_store(&path, &store)?;
    Ok(build_status(app_data_dir, &store))
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
            return Err("Google Drive sign-in timed out after 5 minutes.".to_string());
        }
        let (mut socket, _) = tokio::time::timeout(remaining, listener.accept())
            .await
            .map_err(|_| "Google Drive sign-in timed out after 5 minutes.".to_string())?
            .map_err(|e| format!("failed to accept OAuth callback: {e}"))?;

        let mut buf = [0u8; 8192];
        let n = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut buf))
            .await
            .map_err(|_| "timed out reading OAuth callback".to_string())?
            .map_err(|e| format!("failed reading OAuth callback: {e}"))?;
        let req = String::from_utf8_lossy(&buf[..n]);
        let request_target = req
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");
        if !request_target.starts_with("/callback") {
            let _ = socket
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
            continue;
        }
        let full_url = format!("http://127.0.0.1{request_target}");
        let parsed = Url::parse(&full_url)
            .map_err(|e| format!("invalid OAuth callback URL: {e}"))?;
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
                "<!doctype html><html><body style=\"font-family:-apple-system,sans-serif;padding:40px;text-align:center\"><h2>Google Drive sign-in cancelled</h2><p>{err}</p><p>You can close this tab and return to Oatmeal.</p></body></html>"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            return Err(format!("Google Drive OAuth error: {err}"));
        }
        if state.as_deref() != Some(expected_state) {
            let _ = socket
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 13\r\nConnection: close\r\n\r\nInvalid state")
                .await;
            return Err("Google Drive OAuth state mismatch.".to_string());
        }
        let Some(code) = code else {
            return Err("Google Drive OAuth callback missing authorization code.".to_string());
        };
        let body = "<!doctype html><html><body style=\"font-family:-apple-system,sans-serif;padding:40px;text-align:center\"><h2>Google Drive connected to Oatmeal</h2><p>You can close this tab and return to Oatmeal.</p></body></html>";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = socket.write_all(resp.as_bytes()).await;
        return Ok(code);
    }
}

pub async fn connect(app_data_dir: &Path) -> Result<GoogleDriveSyncStatus, String> {
    let path = store_path(app_data_dir);
    let mut store = load_store(&path)?;
    let (client_id, client_secret, _) = effective_credentials(app_data_dir, &store).ok_or_else(|| {
        "Google OAuth credentials are not configured yet. Paste your Google Cloud Desktop OAuth JSON first.".to_string()
    })?;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("failed to bind OAuth callback listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("failed to get OAuth callback port: {e}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let (verifier, challenge) = generate_pkce();
    let state = uuid::Uuid::new_v4().simple().to_string();

    let mut auth_url = Url::parse(GOOGLE_AUTH_URL).map_err(|e| e.to_string())?;
    auth_url
        .query_pairs_mut()
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", GOOGLE_DRIVE_SCOPES)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);

    open::that(auth_url.as_str())
        .map_err(|e| format!("failed to open browser for Google Drive sign-in: {e}"))?;

    let code = wait_for_oauth_callback(listener, &state).await?;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let mut form = vec![
        ("code", code.as_str()),
        ("client_id", client_id.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("grant_type", "authorization_code"),
        ("code_verifier", verifier.as_str()),
    ];
    let secret_holder = client_secret.clone().unwrap_or_default();
    if !secret_holder.is_empty() {
        form.push(("client_secret", secret_holder.as_str()));
    }

    let token_resp = http
        .post(GOOGLE_TOKEN_URL)
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("failed to exchange Google OAuth code: {e}"))?;
    let status = token_resp.status();
    let token_text = token_resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "Google token exchange failed ({status}): {token_text}"
        ));
    }
    let token: TokenResponse = serde_json::from_str(&token_text)
        .map_err(|e| format!("invalid Google token response: {e}"))?;

    let email = fetch_google_email(&http, GOOGLE_USERINFO_URL, &token.access_token)
        .await
        .unwrap_or_else(|_| "Google Drive account".to_string());

    store.account = Some(StoredDriveAccount {
        email,
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at_epoch_secs: now_epoch_secs() + token.expires_in.unwrap_or(3600),
    });
    store.last_error = None;
    save_store(&path, &store)?;
    Ok(build_status(app_data_dir, &store))
}

async fn fetch_google_email(
    http: &reqwest::Client,
    userinfo_url: &str,
    access_token: &str,
) -> Result<String, String> {
    let resp = http
        .get(userinfo_url)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("failed to fetch Google account email: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("userinfo failed: {}", resp.status()));
    }
    let info: UserInfoResponse = resp.json().await.map_err(|e| e.to_string())?;
    info.email
        .filter(|e| !e.trim().is_empty())
        .ok_or_else(|| "Google account email missing".to_string())
}

async fn ensure_fresh_access_token(
    http: &reqwest::Client,
    token_url: &str,
    app_data_dir: &Path,
    store: &mut StoredGoogleDriveSync,
) -> Result<String, String> {
    let Some(account) = store.account.as_ref() else {
        return Err("Google Drive is not connected.".to_string());
    };
    if account.expires_at_epoch_secs > now_epoch_secs() + TOKEN_REFRESH_SKEW_SECS {
        return Ok(account.access_token.clone());
    }

    let refresh_token = account.refresh_token.clone().ok_or_else(|| {
        "Google Drive session expired. Please reconnect Google Drive in Settings → Sync."
            .to_string()
    })?;
    let (client_id, client_secret, _) = effective_credentials(app_data_dir, store)
        .ok_or_else(|| "Google OAuth client credentials are missing.".to_string())?;

    let mut form = vec![
        ("client_id", client_id.as_str()),
        ("refresh_token", refresh_token.as_str()),
        ("grant_type", "refresh_token"),
    ];
    let secret_holder = client_secret.clone().unwrap_or_default();
    if !secret_holder.is_empty() {
        form.push(("client_secret", secret_holder.as_str()));
    }

    let resp = http
        .post(token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("failed to refresh Google Drive token: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        if body.contains("invalid_grant") {
            return Err(
                "Google Drive access expired or was revoked. Please reconnect Google Drive."
                    .to_string(),
            );
        }
        return Err(format!("Google token refresh failed ({status}): {body}"));
    }

    let refreshed: TokenResponse =
        serde_json::from_str(&body).map_err(|e| format!("invalid refresh response: {e}"))?;
    if let Some(acc) = store.account.as_mut() {
        acc.access_token = refreshed.access_token.clone();
        if let Some(rt) = refreshed.refresh_token {
            acc.refresh_token = Some(rt);
        }
        acc.expires_at_epoch_secs = now_epoch_secs() + refreshed.expires_in.unwrap_or(3600);
    }
    Ok(refreshed.access_token)
}

fn escape_drive_query_literal(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

async fn ensure_drive_folder(
    http: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    name: &str,
    parent_id: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let escaped = escape_drive_query_literal(name);
    let mut query = format!(
        "mimeType = '{DRIVE_FOLDER_MIME}' and name = '{escaped}' and trashed = false"
    );
    if let Some(parent) = parent_id {
        query.push_str(&format!(" and '{}' in parents", escape_drive_query_literal(parent)));
    } else {
        query.push_str(" and 'root' in parents");
    }

    let list_url = format!(
        "{}/drive/v3/files?q={}&fields=files(id,name,webViewLink)&pageSize=1",
        api_base.trim_end_matches('/'),
        urlencoding::encode(&query)
    );
    let list_resp = http
        .get(&list_url)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("failed to query Google Drive folder '{name}': {e}"))?;
    let list_status = list_resp.status();
    let list_body = list_resp.text().await.unwrap_or_default();
    if !list_status.is_success() {
        return Err(format_drive_api_error(
            "query Google Drive folder",
            list_status,
            &list_body,
        ));
    }
    let parsed: DriveFilesListResponse = serde_json::from_str(&list_body)
        .map_err(|e| format!("invalid Google Drive folder list response: {e}"))?;
    if let Some(existing) = parsed.files.into_iter().next() {
        let link = existing
            .web_view_link
            .or_else(|| Some(format!("https://drive.google.com/drive/folders/{}", existing.id)));
        return Ok((existing.id, link));
    }

    let mut metadata = serde_json::json!({
        "name": name,
        "mimeType": DRIVE_FOLDER_MIME,
    });
    if let Some(parent) = parent_id {
        metadata["parents"] = serde_json::json!([parent]);
    }
    let create_url = format!(
        "{}/drive/v3/files?fields=id,name,webViewLink",
        api_base.trim_end_matches('/')
    );
    let create_resp = http
        .post(&create_url)
        .bearer_auth(access_token)
        .json(&metadata)
        .send()
        .await
        .map_err(|e| format!("failed to create Google Drive folder '{name}': {e}"))?;
    let create_status = create_resp.status();
    let create_body = create_resp.text().await.unwrap_or_default();
    if !create_status.is_success() {
        return Err(format_drive_api_error(
            "create Google Drive folder",
            create_status,
            &create_body,
        ));
    }
    let created: DriveFileEntry = serde_json::from_str(&create_body)
        .map_err(|e| format!("invalid Google Drive folder creation response: {e}"))?;
    let link = created
        .web_view_link
        .or_else(|| Some(format!("https://drive.google.com/drive/folders/{}", created.id)));
    Ok((created.id, link))
}

fn format_drive_api_error(action: &str, status: reqwest::StatusCode, body: &str) -> String {
    if body.contains("accessNotConfigured") || body.contains("Google Drive API has not been used") {
        return "Google Drive API is not enabled in your Google Cloud project. Enable 'Google Drive API' in Google Cloud Console and retry.".to_string();
    }
    format!("Failed to {action} ({status}): {body}")
}

fn build_multipart_related(
    boundary: &str,
    metadata: &serde_json::Value,
    markdown: &str,
) -> Vec<u8> {
    let meta_json = serde_json::to_string(metadata).unwrap_or_else(|_| "{}".to_string());
    let mut body = Vec::with_capacity(meta_json.len() + markdown.len() + 256);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
    body.extend_from_slice(meta_json.as_bytes());
    body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
    body.extend_from_slice(format!("Content-Type: {MARKDOWN_MIME}; charset=UTF-8\r\n\r\n").as_bytes());
    body.extend_from_slice(markdown.as_bytes());
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

async fn upload_note_file(
    http: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    existing_file_id: Option<&str>,
    parent_folder_id: &str,
    file_name: &str,
    format: GoogleDriveSyncFormat,
    markdown: &str,
) -> Result<(DriveFileEntry, bool), String> {
    let target_mime = match format {
        GoogleDriveSyncFormat::GoogleDoc => GOOGLE_DOC_MIME,
        GoogleDriveSyncFormat::Markdown => MARKDOWN_MIME,
    };
    let boundary = format!("oatmeal_{}", uuid::Uuid::new_v4().simple());
    let content_type = format!("multipart/related; boundary={boundary}");

    if let Some(file_id) = existing_file_id {
        let metadata = serde_json::json!({
            "name": file_name,
        });
        let body = build_multipart_related(&boundary, &metadata, markdown);
        let url = format!(
            "{}/upload/drive/v3/files/{}?uploadType=multipart&fields=id,name,webViewLink",
            api_base.trim_end_matches('/'),
            urlencoding::encode(file_id)
        );
        let resp = http
            .patch(&url)
            .bearer_auth(access_token)
            .header(reqwest::header::CONTENT_TYPE, &content_type)
            .body(body)
            .send()
            .await
            .map_err(|e| format!("failed to update '{file_name}' in Google Drive: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.is_success() {
            let entry: DriveFileEntry = serde_json::from_str(&text)
                .map_err(|e| format!("invalid Google Drive update response: {e}"))?;
            return Ok((entry, true));
        }
        if status != reqwest::StatusCode::NOT_FOUND {
            return Err(format_drive_api_error("update note in Google Drive", status, &text));
        }
        // Existing file was deleted from Drive; recreate cleanly below.
    }

    let metadata = serde_json::json!({
        "name": file_name,
        "mimeType": target_mime,
        "parents": [parent_folder_id],
    });
    let body = build_multipart_related(&boundary, &metadata, markdown);
    let url = format!(
        "{}/upload/drive/v3/files?uploadType=multipart&fields=id,name,webViewLink",
        api_base.trim_end_matches('/')
    );
    let resp = http
        .post(&url)
        .bearer_auth(access_token)
        .header(reqwest::header::CONTENT_TYPE, &content_type)
        .body(body)
        .send()
        .await
        .map_err(|e| format!("failed to upload '{file_name}' to Google Drive: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format_drive_api_error("upload note to Google Drive", status, &text));
    }
    let entry: DriveFileEntry = serde_json::from_str(&text)
        .map_err(|e| format!("invalid Google Drive upload response: {e}"))?;
    Ok((entry, false))
}

fn sanitize_filename(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' => ' ',
            _ => c,
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        "Untitled meeting".to_string()
    } else if trimmed.chars().count() > 100 {
        trimmed.chars().take(100).collect()
    } else {
        trimmed.to_string()
    }
}

fn note_file_name(meeting: &anlg_agent_access::Meeting, format: GoogleDriveSyncFormat) -> String {
    let date_prefix = meeting
        .started_at
        .get(..10)
        .filter(|s| s.len() == 10 && s.chars().nth(4) == Some('-'))
        .or_else(|| {
            meeting
                .created_at
                .get(..10)
                .filter(|s| s.len() == 10 && s.chars().nth(4) == Some('-'))
        });
    let base = sanitize_filename(&meeting.title);
    let stem = match date_prefix {
        Some(date) => format!("{date} - {base}"),
        None => base,
    };
    match format {
        GoogleDriveSyncFormat::GoogleDoc => stem,
        GoogleDriveSyncFormat::Markdown => format!("{stem}.md"),
    }
}

fn is_empty_meeting_export(export: &anlg_agent_access::MeetingExport) -> bool {
    let has_note = export
        .meeting
        .note
        .as_ref()
        .is_some_and(|n| !n.markdown.trim().is_empty());
    let has_summary = export
        .meeting
        .summaries
        .iter()
        .any(|s| !s.markdown.trim().is_empty());
    let has_actions = !export.meeting.action_items.is_empty();
    let has_transcript = export.transcripts.iter().any(|t| !t.words.is_empty());
    !(has_note || has_summary || has_actions || has_transcript)
}

fn compute_sync_hash(
    format: GoogleDriveSyncFormat,
    parent_folder_id: &str,
    file_name: &str,
    markdown: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{format:?}\0{parent_folder_id}\0{file_name}\0"));
    hasher.update(markdown.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub async fn sync_notes(
    pool: &sqlx::SqlitePool,
    app_data_dir: &Path,
    session_id: Option<String>,
) -> Result<GoogleDriveSyncReport, String> {
    sync_notes_with_endpoints(
        pool,
        app_data_dir,
        session_id,
        GOOGLE_TOKEN_URL,
        GOOGLE_DRIVE_API_BASE,
    )
    .await
}

async fn collect_meeting_ids(
    pool: &sqlx::SqlitePool,
    session_id: Option<String>,
) -> Result<Vec<String>, String> {
    if let Some(id) = session_id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        return Ok(vec![id]);
    }
    let mut ids = Vec::new();
    let mut offset = 0u32;
    loop {
        let page = anlg_agent_access::list_meetings(
            pool,
            anlg_agent_access::ListMeetingsInput {
                limit: Some(200),
                offset: Some(offset),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| format!("failed to list local meetings: {e}"))?;
        ids.extend(page.meetings.into_iter().map(|m| m.id));
        match page.pagination.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    Ok(ids)
}

pub(crate) async fn sync_notes_with_endpoints(
    pool: &sqlx::SqlitePool,
    app_data_dir: &Path,
    session_id: Option<String>,
    token_url: &str,
    api_base: &str,
) -> Result<GoogleDriveSyncReport, String> {
    let path = store_path(app_data_dir);
    let mut store = load_store(&path)?;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    let access_token = match ensure_fresh_access_token(&http, token_url, app_data_dir, &mut store).await {
        Ok(token) => token,
        Err(err) => {
            store.last_error = Some(err.clone());
            let _ = save_store(&path, &store);
            return Err(err);
        }
    };

    let (root_folder_id, root_folder_url) = match store.root_folder_id.clone() {
        Some(existing_id) => (
            existing_id.clone(),
            store
                .root_folder_url
                .clone()
                .or_else(|| Some(format!("https://drive.google.com/drive/folders/{existing_id}"))),
        ),
        None => match ensure_drive_folder(
            &http,
            api_base,
            &access_token,
            &store.config.folder_name,
            None,
        )
        .await
        {
            Ok(folder) => {
                store.root_folder_id = Some(folder.0.clone());
                store.root_folder_url = folder.1.clone();
                folder
            }
            Err(err) => {
                store.last_error = Some(err.clone());
                let _ = save_store(&path, &store);
                return Err(err);
            }
        },
    };

    let meeting_ids = collect_meeting_ids(pool, session_id).await?;
    let mut subfolder_cache: HashMap<(String, String), String> = HashMap::new();
    let mut uploaded = 0u32;
    let mut updated = 0u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;
    let mut errors = Vec::new();
    let format = store.config.format;
    let include_transcript = store.config.include_transcript;

    for meeting_id in meeting_ids {
        let export = match anlg_agent_access::get_meeting_export(pool, meeting_id.clone()).await {
            Ok(exp) => exp,
            Err(e) => {
                failed += 1;
                errors.push(format!("{meeting_id}: {e}"));
                continue;
            }
        };

        if is_empty_meeting_export(&export) {
            skipped += 1;
            continue;
        }

        let mut parent_id = root_folder_id.clone();
        if let Some(folder_path) = export.meeting.folder_path.as_deref() {
            let mut folder_ok = true;
            for segment in folder_path.split('/').map(str::trim).filter(|s| !s.is_empty()) {
                let key = (parent_id.clone(), segment.to_string());
                if let Some(cached) = subfolder_cache.get(&key) {
                    parent_id = cached.clone();
                    continue;
                }
                match ensure_drive_folder(&http, api_base, &access_token, segment, Some(&parent_id))
                    .await
                {
                    Ok((id, _)) => {
                        subfolder_cache.insert(key, id.clone());
                        parent_id = id;
                    }
                    Err(err) => {
                        failed += 1;
                        errors.push(format!("{meeting_id}: {err}"));
                        folder_ok = false;
                        break;
                    }
                }
            }
            if !folder_ok {
                continue;
            }
        }

        let file_name = note_file_name(&export.meeting, format);
        let markdown = if include_transcript {
            export.to_markdown()
        } else {
            export.meeting.to_markdown()
        };
        let hash = compute_sync_hash(format, &parent_id, &file_name, &markdown);

        let existing = store.synced_files.get(&meeting_id);
        if let Some(record) = existing
            && record.content_hash == hash
            && record.format == format
            && record.parent_folder_id == parent_id
        {
            skipped += 1;
            continue;
        }

        // Only update in place if format and parent folder still match; otherwise upload fresh.
        let existing_file_id = existing
            .filter(|r| r.format == format && r.parent_folder_id == parent_id)
            .map(|r| r.drive_file_id.as_str());

        match upload_note_file(
            &http,
            api_base,
            &access_token,
            existing_file_id,
            &parent_id,
            &file_name,
            format,
            &markdown,
        )
        .await
        {
            Ok((entry, was_update)) => {
                if was_update {
                    updated += 1;
                } else {
                    uploaded += 1;
                }
                store.synced_files.insert(
                    meeting_id,
                    SyncedFileRecord {
                        drive_file_id: entry.id,
                        web_view_link: entry.web_view_link,
                        content_hash: hash,
                        format,
                        file_name,
                        parent_folder_id: parent_id,
                        synced_at_ms: now_epoch_ms(),
                    },
                );
            }
            Err(err) => {
                failed += 1;
                errors.push(format!("{file_name}: {err}"));
            }
        }
    }

    let synced_at_ms = now_epoch_ms();
    store.last_synced_at_ms = Some(synced_at_ms);
    store.last_error = errors.first().cloned();
    save_store(&path, &store)?;

    Ok(GoogleDriveSyncReport {
        uploaded,
        updated,
        skipped,
        failed,
        total_synced: store.synced_files.len() as u32,
        folder_url: root_folder_url,
        synced_at_ms: synced_at_ms as f64,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn reuses_calendar_oauth_credentials_as_fallback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            calendar_oauth_path(dir.path()),
            r#"{"client_id":"123456789012-abc.apps.googleusercontent.com","client_secret":"sec-123"}"#,
        )
        .unwrap();

        let status = get_status(dir.path()).unwrap();
        assert!(status.configured);
        assert_eq!(status.credentials_source, "calendar");
        assert!(status.client_id_hint.is_some());
    }

    #[test]
    fn parses_desktop_oauth_json_and_saves_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{"installed":{"client_id":"999999999999-xyz.apps.googleusercontent.com","client_secret":"test-secret"}}"#;
        let status = save_credentials(dir.path(), json, None).unwrap();
        assert!(status.configured);
        assert_eq!(status.credentials_source, "drive");
    }

    #[tokio::test]
    async fn syncs_new_skips_unchanged_and_updates_changed_notes() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("app.db");
        let db = tauri_plugin_db::open_app_db(Some(&db_path)).await.unwrap();
        let pool = db.pool().clone();

        sqlx::query(
            "INSERT INTO sessions (id, title, created_at, started_at, folder_path) VALUES ('s1', 'Weekly Sync', '2026-11-25T10:00:00Z', '2026-11-25T10:00:00Z', '')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO session_documents (id, session_id, kind, title, body_format, body) VALUES ('d1', 's1', 'summary', 'Summary', 'markdown', '- Ship Google Drive sync')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Seed connected account with expired token to exercise refresh too
        let initial_store = StoredGoogleDriveSync {
            client_id: Some("cid.apps.googleusercontent.com".into()),
            client_secret: Some("csec".into()),
            account: Some(StoredDriveAccount {
                email: "alex@example.com".into(),
                access_token: "old-token".into(),
                refresh_token: Some("rt-1".into()),
                expires_at_epoch_secs: 0,
            }),
            ..Default::default()
        };
        save_store(&store_path(dir.path()), &initial_store).unwrap();

        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "fresh-token",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        // Folder lookup returns empty, then folder creation returns folder-1
        Mock::given(method("GET"))
            .and(path("/drive/v3/files"))
            .and(header("authorization", "Bearer fresh-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "files": []
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/drive/v3/files"))
            .and(header("authorization", "Bearer fresh-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "folder-1",
                "name": "Oatmeal Notes",
                "webViewLink": "https://drive.google.com/drive/folders/folder-1"
            })))
            .mount(&server)
            .await;

        // Upload new file
        Mock::given(method("POST"))
            .and(path("/upload/drive/v3/files"))
            .and(query_param("uploadType", "multipart"))
            .and(header("authorization", "Bearer fresh-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "file-1",
                "name": "2026-11-25 - Weekly Sync",
                "webViewLink": "https://docs.google.com/document/d/file-1/edit"
            })))
            .mount(&server)
            .await;

        let token_url = format!("{}/token", server.uri());
        let report1 = sync_notes_with_endpoints(&pool, dir.path(), None, &token_url, &server.uri())
            .await
            .unwrap();
        assert_eq!(report1.uploaded, 1);
        assert_eq!(report1.updated, 0);
        assert_eq!(report1.skipped, 0);
        assert_eq!(report1.failed, 0);
        assert_eq!(
            report1.folder_url.as_deref(),
            Some("https://drive.google.com/drive/folders/folder-1")
        );

        // Second sync with unchanged content skips
        let report2 = sync_notes_with_endpoints(&pool, dir.path(), None, &token_url, &server.uri())
            .await
            .unwrap();
        assert_eq!(report2.uploaded, 0);
        assert_eq!(report2.updated, 0);
        assert_eq!(report2.skipped, 1);

        // Edit summary -> PATCH updates file-1 in place
        sqlx::query("UPDATE session_documents SET body = '- Shipped Google Drive sync!' WHERE id = 'd1'")
            .execute(&pool)
            .await
            .unwrap();

        Mock::given(method("PATCH"))
            .and(path("/upload/drive/v3/files/file-1"))
            .and(query_param("uploadType", "multipart"))
            .and(header("authorization", "Bearer fresh-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "file-1",
                "name": "2026-11-25 - Weekly Sync",
                "webViewLink": "https://docs.google.com/document/d/file-1/edit"
            })))
            .mount(&server)
            .await;

        let report3 = sync_notes_with_endpoints(&pool, dir.path(), None, &token_url, &server.uri())
            .await
            .unwrap();
        assert_eq!(report3.uploaded, 0);
        assert_eq!(report3.updated, 1);
        assert_eq!(report3.skipped, 0);
    }
}
