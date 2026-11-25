//! Direct local Google Calendar OAuth 2.0 (PKCE + loopback) and Calendar API v3 client.
//!
//! Runs entirely between this machine and Google's OAuth / Calendar endpoints with no hosted
//! proxy server. Tokens are stored locally in the app's data directory with 0600 permissions.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anlg_calendar_interface::EventFilter;
use anlg_google_calendar::{
    CalendarListEntry as GoogleCalendar, Event as GoogleEvent, ListCalendarsResponse,
    ListEventsResponse,
};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::error::Error;

pub const LOCAL_GOOGLE_CONNECTION_PREFIX: &str = "local-google:";

const GOOGLE_AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_CALENDAR_API_BASE: &str = "https://www.googleapis.com";
const GOOGLE_CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const OAUTH_CALLBACK_TIMEOUT: Duration = Duration::from_secs(180);
const TOKEN_REFRESH_SKEW_SECS: i64 = 60;

pub fn is_local_google_connection(connection_id: &str) -> bool {
    connection_id.starts_with(LOCAL_GOOGLE_CONNECTION_PREFIX)
}

pub fn store_path_for_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("google-calendar-oauth.json")
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct LocalGoogleAccountSummary {
    pub connection_id: String,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct LocalGoogleStatus {
    pub configured: bool,
    pub client_id_preview: Option<String>,
    pub accounts: Vec<LocalGoogleAccountSummary>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct LocalGoogleStoreFile {
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    accounts: Vec<StoredGoogleAccount>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredGoogleAccount {
    connection_id: String,
    email: String,
    access_token: String,
    refresh_token: String,
    expires_at_epoch_secs: i64,
}

#[derive(Debug, serde::Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default = "default_expires_in")]
    expires_in: i64,
}

fn default_expires_in() -> i64 {
    3600
}

#[derive(Debug, serde::Deserialize)]
struct GoogleOAuthErrorResponse {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// Parses either a raw Google Cloud OAuth Desktop JSON file (`{"installed": {...}}` or
/// `{"web": {...}}`) or an explicit `client_id` + optional `client_secret`.
pub fn parse_oauth_credentials(
    client_id_or_json: &str,
    client_secret: Option<&str>,
) -> Result<(String, Option<String>), Error> {
    let trimmed = client_id_or_json.trim();
    if trimmed.is_empty() {
        return Err(Error::Api(
            "Google OAuth Client ID (or downloaded credentials JSON) is required.".into(),
        ));
    }

    if trimmed.starts_with('{') {
        let value: serde_json::Value = serde_json::from_str(trimmed)
            .map_err(|e| Error::Api(format!("Invalid Google OAuth JSON: {e}")))?;
        let container = value
            .get("installed")
            .or_else(|| value.get("web"))
            .unwrap_or(&value);
        let parsed_id = container
            .get("client_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::Api("JSON is missing \"client_id\".".into()))?
            .to_string();
        let parsed_secret = container
            .get("client_secret")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                client_secret
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            });
        return Ok((parsed_id, parsed_secret));
    }

    let secret = client_secret
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok((trimmed.to_string(), secret))
}

fn env_fallback_credentials() -> (Option<String>, Option<String>) {
    let client_id = std::env::var("OATMEAL_GOOGLE_CLIENT_ID")
        .or_else(|_| std::env::var("GOOGLE_CALENDAR_CLIENT_ID"))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let client_secret = std::env::var("OATMEAL_GOOGLE_CLIENT_SECRET")
        .or_else(|_| std::env::var("GOOGLE_CALENDAR_CLIENT_SECRET"))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    (client_id, client_secret)
}

fn load_store(store_path: &Path) -> Result<LocalGoogleStoreFile, Error> {
    if !store_path.exists() {
        return Ok(LocalGoogleStoreFile::default());
    }
    let bytes = std::fs::read(store_path)
        .map_err(|e| Error::Api(format!("Failed to read local Google Calendar store: {e}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| Error::Api(format!("Failed to parse local Google Calendar store: {e}")))
}

fn save_store(store_path: &Path, store: &LocalGoogleStoreFile) -> Result<(), Error> {
    if let Some(parent) = store_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Api(format!("Failed to create app data directory: {e}")))?;
    }
    let json = serde_json::to_vec_pretty(store)
        .map_err(|e| Error::Api(format!("Failed to serialize local Google Calendar store: {e}")))?;
    let tmp_path = store_path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &json)
        .map_err(|e| Error::Api(format!("Failed to write local Google Calendar store: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp_path, store_path)
        .map_err(|e| Error::Api(format!("Failed to save local Google Calendar store: {e}")))?;
    Ok(())
}

fn effective_credentials(store: &LocalGoogleStoreFile) -> (Option<String>, Option<String>) {
    let (env_id, env_secret) = env_fallback_credentials();
    let client_id = store
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or(env_id);
    let client_secret = store
        .client_secret
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or(env_secret);
    (client_id, client_secret)
}

fn preview_client_id(client_id: &str) -> String {
    let chars: Vec<char> = client_id.chars().collect();
    if chars.len() <= 24 {
        return client_id.to_string();
    }
    let head: String = chars.iter().take(12).collect();
    let tail: String = chars.iter().skip(chars.len() - 10).collect();
    format!("{head}…{tail}")
}

pub fn get_status(store_path: &Path) -> Result<LocalGoogleStatus, Error> {
    let store = load_store(store_path)?;
    let (client_id, _) = effective_credentials(&store);
    Ok(LocalGoogleStatus {
        configured: client_id.is_some(),
        client_id_preview: client_id.as_deref().map(preview_client_id),
        accounts: store
            .accounts
            .into_iter()
            .map(|account| LocalGoogleAccountSummary {
                connection_id: account.connection_id,
                email: account.email,
            })
            .collect(),
    })
}

pub fn list_local_connection_ids(store_path: &Path) -> Result<Vec<String>, Error> {
    let store = load_store(store_path)?;
    Ok(store
        .accounts
        .into_iter()
        .map(|account| account.connection_id)
        .collect())
}

pub fn save_credentials(
    store_path: &Path,
    client_id_or_json: &str,
    client_secret: Option<&str>,
) -> Result<LocalGoogleStatus, Error> {
    let (client_id, parsed_secret) = parse_oauth_credentials(client_id_or_json, client_secret)?;
    let mut store = load_store(store_path)?;
    store.client_id = Some(client_id);
    if parsed_secret.is_some() || client_secret.is_some() {
        store.client_secret = parsed_secret;
    }
    save_store(store_path, &store)?;
    get_status(store_path)
}

pub fn disconnect_account(store_path: &Path, connection_id: &str) -> Result<(), Error> {
    let mut store = load_store(store_path)?;
    store
        .accounts
        .retain(|account| account.connection_id != connection_id);
    save_store(store_path, &store)
}

fn random_urlsafe_token() -> String {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn build_auth_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
) -> Result<String, Error> {
    let mut url = url::Url::parse(GOOGLE_AUTH_ENDPOINT)
        .map_err(|e| Error::Api(format!("Failed to build Google auth URL: {e}")))?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", GOOGLE_CALENDAR_SCOPE)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("state", state)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url.to_string())
}

fn make_http_client() -> Result<reqwest::Client, Error> {
    Ok(reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()?)
}

fn parse_oauth_callback_request(
    raw_request: &str,
    expected_state: &str,
) -> Option<Result<String, Error>> {
    let request_line = raw_request.lines().next()?;
    let target = request_line.split_whitespace().nth(1)?;
    let parsed = url::Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    if parsed.path() != "/callback" {
        return None;
    }

    let mut code: Option<String> = None;
    let mut state: Option<String> = None;
    let mut oauth_error: Option<String> = None;
    let mut oauth_error_desc: Option<String> = None;

    for (k, v) in parsed.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => oauth_error = Some(v.into_owned()),
            "error_description" => oauth_error_desc = Some(v.into_owned()),
            _ => {}
        }
    }

    if let Some(err) = oauth_error {
        let detail = oauth_error_desc
            .map(|d| format!("{err}: {d}"))
            .unwrap_or(err);
        return Some(Err(Error::Api(format!(
            "Google Calendar sign-in was cancelled or denied ({detail})."
        ))));
    }

    if state.as_deref() != Some(expected_state) {
        return Some(Err(Error::Api(
            "Google Calendar OAuth state mismatch. Please try connecting again.".into(),
        )));
    }

    match code.filter(|c| !c.is_empty()) {
        Some(c) => Some(Ok(c)),
        None => Some(Err(Error::Api(
            "Google Calendar OAuth callback did not include an authorization code.".into(),
        ))),
    }
}

async fn await_oauth_callback(
    listener: TcpListener,
    expected_state: &str,
) -> Result<String, Error> {
    let deadline = tokio::time::Instant::now() + OAUTH_CALLBACK_TIMEOUT;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(Error::Api(
                "Timed out waiting for Google Calendar sign-in in your browser.".into(),
            ));
        }

        let (mut socket, _) = tokio::time::timeout(deadline - now, listener.accept())
            .await
            .map_err(|_| {
                Error::Api("Timed out waiting for Google Calendar sign-in in your browser.".into())
            })?
            .map_err(|e| Error::Api(format!("OAuth callback listener failed: {e}")))?;

        let mut buf = vec![0u8; 8192];
        let n = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buf))
            .await
            .ok()
            .and_then(|res| res.ok())
            .unwrap_or(0);
        let raw = String::from_utf8_lossy(&buf[..n]);

        match parse_oauth_callback_request(&raw, expected_state) {
            None => {
                // Ignore browser favicon/preflight requests and keep listening.
                let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = socket.write_all(resp.as_bytes()).await;
            }
            Some(Ok(code)) => {
                let body = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Oatmeal</title><style>body{font-family:-apple-system,BlinkMacSystemFont,sans-serif;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0;background:#fafaf9;color:#1c1917}div{max-width:420px;padding:32px;border-radius:16px;background:#fff;box-shadow:0 4px 24px rgba(0,0,0,.06);text-align:center}h1{font-size:20px;margin:0 0 8px}p{font-size:14px;color:#57534e;margin:0}</style></head><body><div><h1>Google Calendar connected</h1><p>You can close this tab and return to Oatmeal.</p></div></body></html>";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                return Ok(code);
            }
            Some(Err(err)) => {
                let body = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Oatmeal</title><style>body{font-family:-apple-system,BlinkMacSystemFont,sans-serif;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0;background:#fafaf9;color:#1c1917}div{max-width:420px;padding:32px;border-radius:16px;background:#fff;box-shadow:0 4px 24px rgba(0,0,0,.06);text-align:center}h1{font-size:20px;margin:0 0 8px}p{font-size:14px;color:#57534e;margin:0}</style></head><body><div><h1>Google Calendar sign-in did not finish</h1><p>You can close this tab and try again in Oatmeal.</p></div></body></html>";
                let resp = format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                return Err(err);
            }
        }
    }
}

async fn exchange_authorization_code(
    http: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<GoogleTokenResponse, Error> {
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("code_verifier", code_verifier),
    ];
    if let Some(secret) = client_secret.filter(|s| !s.is_empty()) {
        form.push(("client_secret", secret));
    }

    let response = http.post(token_endpoint).form(&form).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if let Ok(err_body) = serde_json::from_str::<GoogleOAuthErrorResponse>(&text) {
            let detail = err_body
                .error_description
                .or(err_body.error)
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(Error::Api(format!(
                "Google token exchange failed: {detail}"
            )));
        }
        return Err(Error::Api(format!(
            "Google token exchange failed ({status}): {text}"
        )));
    }

    Ok(response.json().await?)
}

async fn refresh_access_token(
    http: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    refresh_token: &str,
) -> Result<GoogleTokenResponse, Error> {
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    if let Some(secret) = client_secret.filter(|s| !s.is_empty()) {
        form.push(("client_secret", secret));
    }

    let response = http.post(token_endpoint).form(&form).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if let Ok(err_body) = serde_json::from_str::<GoogleOAuthErrorResponse>(&text) {
            if err_body.error.as_deref() == Some("invalid_grant") {
                return Err(Error::ReconnectRequired);
            }
            let detail = err_body
                .error_description
                .or(err_body.error)
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(Error::Api(format!("Google token refresh failed: {detail}")));
        }
        return Err(Error::Api(format!(
            "Google token refresh failed ({status}): {text}"
        )));
    }

    Ok(response.json().await?)
}

async fn ensure_fresh_access_token(
    store_path: &Path,
    connection_id: &str,
    http: &reqwest::Client,
    token_endpoint: &str,
) -> Result<String, Error> {
    let mut store = load_store(store_path)?;
    let (client_id, client_secret) = effective_credentials(&store);
    let now = Utc::now().timestamp();

    let Some(idx) = store
        .accounts
        .iter()
        .position(|account| account.connection_id == connection_id)
    else {
        return Err(Error::ReconnectRequired);
    };

    if !store.accounts[idx].access_token.is_empty()
        && store.accounts[idx].expires_at_epoch_secs > now + TOKEN_REFRESH_SKEW_SECS
    {
        return Ok(store.accounts[idx].access_token.clone());
    }

    let Some(client_id) = client_id else {
        return Err(Error::ReconnectRequired);
    };
    let refresh_token = store.accounts[idx].refresh_token.clone();
    if refresh_token.is_empty() {
        return Err(Error::ReconnectRequired);
    }

    let refreshed = refresh_access_token(
        http,
        token_endpoint,
        &client_id,
        client_secret.as_deref(),
        &refresh_token,
    )
    .await?;

    store.accounts[idx].access_token = refreshed.access_token.clone();
    if let Some(new_refresh) = refreshed.refresh_token.filter(|t| !t.is_empty()) {
        store.accounts[idx].refresh_token = new_refresh;
    }
    store.accounts[idx].expires_at_epoch_secs = now + refreshed.expires_in.max(60);
    save_store(store_path, &store)?;

    Ok(refreshed.access_token)
}

async fn fetch_calendars_direct(
    http: &reqwest::Client,
    api_base: &str,
    access_token: &str,
) -> Result<Vec<GoogleCalendar>, Error> {
    let mut calendars = Vec::new();
    let mut page_token: Option<String> = None;
    let mut seen = HashSet::new();

    loop {
        let mut url = url::Url::parse(&format!(
            "{}/calendar/v3/users/me/calendarList",
            api_base.trim_end_matches('/')
        ))
        .map_err(|e| Error::Api(format!("Invalid Google Calendar API URL: {e}")))?;
        if let Some(ref token) = page_token {
            url.query_pairs_mut().append_pair("pageToken", token);
        }

        let response = http.get(url).bearer_auth(access_token).send().await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::ReconnectRequired);
        }
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Api(format!(
                "Google calendarList failed ({status}): {text}"
            )));
        }

        let parsed: ListCalendarsResponse = response.json().await?;
        calendars.extend(parsed.items);

        let Some(next) = parsed.next_page_token.filter(|t| !t.is_empty()) else {
            return Ok(calendars);
        };
        if !seen.insert(next.clone()) {
            return Err(Error::Api(
                "google calendarList.list returned a repeated page token".into(),
            ));
        }
        page_token = Some(next);
    }
}

async fn fetch_events_direct(
    http: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    filter: EventFilter,
) -> Result<Vec<GoogleEvent>, Error> {
    let encoded_calendar_id = urlencoding::encode(&filter.calendar_tracking_id);
    let base_url = format!(
        "{}/calendar/v3/calendars/{encoded_calendar_id}/events",
        api_base.trim_end_matches('/')
    );

    let mut events = Vec::new();
    let mut page_token: Option<String> = None;
    let mut seen = HashSet::new();

    loop {
        let mut url = url::Url::parse(&base_url)
            .map_err(|e| Error::Api(format!("Invalid Google Calendar events URL: {e}")))?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("timeMin", &filter.from.to_rfc3339());
            pairs.append_pair("timeMax", &filter.to.to_rfc3339());
            pairs.append_pair("singleEvents", "true");
            pairs.append_pair("orderBy", "startTime");
            if let Some(ref token) = page_token {
                pairs.append_pair("pageToken", token);
            }
        }

        let response = http.get(url).bearer_auth(access_token).send().await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::ReconnectRequired);
        }
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Api(format!(
                "Google events.list failed ({status}): {text}"
            )));
        }

        let parsed: ListEventsResponse = response.json().await?;
        events.extend(parsed.items);

        let Some(next) = parsed.next_page_token.filter(|t| !t.is_empty()) else {
            return Ok(events);
        };
        if !seen.insert(next.clone()) {
            return Err(Error::Api(
                "google events.list returned a repeated page token".into(),
            ));
        }
        page_token = Some(next);
    }
}

pub async fn connect_account(store_path: &Path) -> Result<LocalGoogleAccountSummary, Error> {
    let store = load_store(store_path)?;
    let (client_id, client_secret) = effective_credentials(&store);
    let Some(client_id) = client_id else {
        return Err(Error::Api(
            "Save your Google Cloud Desktop OAuth Client ID first.".into(),
        ));
    };

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| Error::Api(format!("Could not bind local OAuth callback port: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| Error::Api(format!("Could not read local OAuth callback port: {e}")))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let state = random_urlsafe_token();
    let code_verifier = random_urlsafe_token();
    let code_challenge = pkce_challenge(&code_verifier);

    let auth_url = build_auth_url(&client_id, &redirect_uri, &state, &code_challenge)?;
    open::that(&auth_url)
        .map_err(|e| Error::Api(format!("Failed to open browser for Google sign-in: {e}")))?;

    let code = await_oauth_callback(listener, &state).await?;

    let http = make_http_client()?;
    let tokens = exchange_authorization_code(
        &http,
        GOOGLE_TOKEN_ENDPOINT,
        &client_id,
        client_secret.as_deref(),
        &code,
        &redirect_uri,
        &code_verifier,
    )
    .await?;

    let calendars = fetch_calendars_direct(&http, GOOGLE_CALENDAR_API_BASE, &tokens.access_token)
        .await?;
    let email = calendars
        .iter()
        .find(|cal| cal.primary == Some(true))
        .map(|cal| cal.id.clone())
        .or_else(|| calendars.first().map(|cal| cal.id.clone()))
        .unwrap_or_else(|| "google-account".to_string());

    let connection_id = format!("{LOCAL_GOOGLE_CONNECTION_PREFIX}{email}");
    let expires_at = Utc::now().timestamp() + tokens.expires_in.max(60);

    let mut store = load_store(store_path)?;
    if let Some(existing) = store
        .accounts
        .iter_mut()
        .find(|account| account.connection_id == connection_id)
    {
        existing.email = email.clone();
        existing.access_token = tokens.access_token;
        if let Some(refresh) = tokens.refresh_token.filter(|t| !t.is_empty()) {
            existing.refresh_token = refresh;
        }
        existing.expires_at_epoch_secs = expires_at;
    } else {
        let refresh_token = tokens.refresh_token.unwrap_or_default();
        store.accounts.push(StoredGoogleAccount {
            connection_id: connection_id.clone(),
            email: email.clone(),
            access_token: tokens.access_token,
            refresh_token,
            expires_at_epoch_secs: expires_at,
        });
    }
    save_store(store_path, &store)?;

    Ok(LocalGoogleAccountSummary {
        connection_id,
        email,
    })
}

pub async fn list_calendars(
    store_path: &Path,
    connection_id: &str,
) -> Result<Vec<GoogleCalendar>, Error> {
    let http = make_http_client()?;
    let token =
        ensure_fresh_access_token(store_path, connection_id, &http, GOOGLE_TOKEN_ENDPOINT).await?;
    fetch_calendars_direct(&http, GOOGLE_CALENDAR_API_BASE, &token).await
}

pub async fn list_events(
    store_path: &Path,
    connection_id: &str,
    filter: EventFilter,
) -> Result<Vec<GoogleEvent>, Error> {
    let http = make_http_client()?;
    let token =
        ensure_fresh_access_token(store_path, connection_id, &http, GOOGLE_TOKEN_ENDPOINT).await?;
    fetch_events_direct(&http, GOOGLE_CALENDAR_API_BASE, &token, filter).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path, query_param},
    };

    fn temp_store_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "oatmeal-local-google-test-{name}-{}.json",
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn parses_desktop_oauth_json_and_plain_credentials() {
        let desktop_json = r#"{
            "installed": {
                "client_id": "12345.apps.googleusercontent.com",
                "client_secret": "test-google-client-secret"
            }
        }"#;
        let (id, secret) = parse_oauth_credentials(desktop_json, None).unwrap();
        assert_eq!(id, "12345.apps.googleusercontent.com");
        assert_eq!(secret.as_deref(), Some("test-google-client-secret"));

        let (id2, secret2) =
            parse_oauth_credentials("999.apps.googleusercontent.com", Some("  my-sec  ")).unwrap();
        assert_eq!(id2, "999.apps.googleusercontent.com");
        assert_eq!(secret2.as_deref(), Some("my-sec"));

        assert!(parse_oauth_credentials("   ", None).is_err());
    }

    #[test]
    fn parses_oauth_callback_and_ignores_favicon() {
        assert!(parse_oauth_callback_request("GET /favicon.ico HTTP/1.1\r\n", "st1").is_none());

        let ok = parse_oauth_callback_request(
            "GET /callback?code=abc123&state=st1 HTTP/1.1\r\nHost: 127.0.0.1\r\n",
            "st1",
        )
        .unwrap()
        .unwrap();
        assert_eq!(ok, "abc123");

        let mismatch = parse_oauth_callback_request(
            "GET /callback?code=abc123&state=wrong HTTP/1.1\r\n",
            "st1",
        )
        .unwrap();
        assert!(mismatch.is_err());

        let denied = parse_oauth_callback_request(
            "GET /callback?error=access_denied&state=st1 HTTP/1.1\r\n",
            "st1",
        )
        .unwrap();
        assert!(denied.is_err());
    }

    #[tokio::test]
    async fn refreshes_expired_token_and_maps_invalid_grant_to_reconnect_required() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "fresh-access-token",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        let store_path = temp_store_path("refresh");
        save_store(
            &store_path,
            &LocalGoogleStoreFile {
                client_id: Some("cid.apps.googleusercontent.com".into()),
                client_secret: Some("csec".into()),
                accounts: vec![StoredGoogleAccount {
                    connection_id: "local-google:alex@example.com".into(),
                    email: "alex@example.com".into(),
                    access_token: "stale-token".into(),
                    refresh_token: "refresh-1".into(),
                    expires_at_epoch_secs: 0,
                }],
            },
        )
        .unwrap();

        let http = make_http_client().unwrap();
        let token = ensure_fresh_access_token(
            &store_path,
            "local-google:alex@example.com",
            &http,
            &format!("{}/token", server.uri()),
        )
        .await
        .unwrap();
        assert_eq!(token, "fresh-access-token");

        // Now verify invalid_grant maps cleanly to ReconnectRequired.
        let revoked_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "Token has been expired or revoked."
            })))
            .mount(&revoked_server)
            .await;

        let mut store = load_store(&store_path).unwrap();
        store.accounts[0].expires_at_epoch_secs = 0;
        save_store(&store_path, &store).unwrap();

        let err = ensure_fresh_access_token(
            &store_path,
            "local-google:alex@example.com",
            &http,
            &format!("{}/token", revoked_server.uri()),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::ReconnectRequired));

        let _ = std::fs::remove_file(&store_path);
    }

    #[tokio::test]
    async fn fetches_google_calendars_and_paginated_events_directly() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/calendar/v3/users/me/calendarList"))
            .and(header("authorization", "Bearer tok-123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "items": [{
                    "id": "alex@example.com",
                    "summary": "Primary",
                    "primary": true,
                    "accessRole": "owner"
                }]
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/calendar/v3/calendars/alex%40example.com/events"))
            .and(query_param("singleEvents", "true"))
            .and(query_param("orderBy", "startTime"))
            .and(header("authorization", "Bearer tok-123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "items": [{
                    "id": "evt-zoom",
                    "summary": "Weekly Zoom Sync",
                    "status": "confirmed",
                    "location": "https://us06web.zoom.us/j/98765432100?pwd=Ab_Cd-Ef123.1",
                    "start": {"dateTime": "2026-11-25T15:00:00-05:00"},
                    "end": {"dateTime": "2026-11-25T15:30:00-05:00"}
                }]
            })))
            .mount(&server)
            .await;

        let http = make_http_client().unwrap();
        let calendars = fetch_calendars_direct(&http, &server.uri(), "tok-123")
            .await
            .unwrap();
        assert_eq!(calendars.len(), 1);
        assert_eq!(calendars[0].id, "alex@example.com");

        let events = fetch_events_direct(
            &http,
            &server.uri(),
            "tok-123",
            EventFilter {
                calendar_tracking_id: "alex@example.com".into(),
                from: "2026-11-25T00:00:00Z".parse().unwrap(),
                to: "2026-11-26T00:00:00Z".parse().unwrap(),
            },
        )
        .await
        .unwrap();
        let converted = crate::convert::convert_google_events(events, "alex@example.com");
        assert_eq!(converted.len(), 1);
        assert_eq!(
            converted[0].meeting_link.as_deref(),
            Some("https://us06web.zoom.us/j/98765432100?pwd=Ab_Cd-Ef123.1")
        );
    }
}
