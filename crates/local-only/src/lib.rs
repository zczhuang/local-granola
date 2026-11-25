//! Single switch for the personal local-only build.
//!
//! `LOCAL_ONLY` is on unless the crate is compiled with `ANARLOG_LOCAL_ONLY=0`
//! (or `false`). When on, hosted AI, cloud sync, telemetry, crash reporting,
//! login gating and auto-updates are turned off by the callers.

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde_json::Value;

const fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const fn local_only_from(value: Option<&str>) -> bool {
    match value {
        Some(v) => !(bytes_eq(v.as_bytes(), b"0") || bytes_eq(v.as_bytes(), b"false")),
        None => true,
    }
}

/// Compile-time flag. Default ON.
pub const LOCAL_ONLY: bool = local_only_from(option_env!("ANARLOG_LOCAL_ONLY"));

/// True only for loopback hosts: `localhost`, `127.0.0.0/8` and `::1`.
pub fn is_local_url(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => IpAddr::V4(ip).is_loopback(),
        Some(url::Host::Ipv6(ip)) => IpAddr::V6(ip).is_loopback(),
        None => false,
    }
}

/// Managed-settings values that switch telemetry, crash reporting, cloud sync
/// and automatic updates off (same keys as `managed_settings.json`).
pub fn forced_settings() -> BTreeMap<String, Value> {
    [
        "telemetry_consent",
        "crash_reporting_consent",
        "cloud_sync_enabled",
        "automatic_updates",
    ]
    .into_iter()
    .map(|key| (key.to_string(), Value::Bool(false)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(s: &str) -> bool {
        is_local_url(&url::Url::parse(s).unwrap())
    }

    #[test]
    fn flag_parsing_defaults_on() {
        assert!(local_only_from(None));
        assert!(local_only_from(Some("1")));
        assert!(local_only_from(Some("")));
        assert!(!local_only_from(Some("0")));
        assert!(!local_only_from(Some("false")));
    }

    #[test]
    fn loopback_hosts_are_local() {
        assert!(local("http://localhost:11434/v1"));
        assert!(local("http://LOCALHOST/"));
        assert!(local("http://127.0.0.1:11434/v1"));
        assert!(local("http://127.5.6.7/"));
        assert!(local("http://[::1]:8080/"));
    }

    #[test]
    fn remote_hosts_are_not_local() {
        assert!(!local("https://api.openai.com/v1"));
        assert!(!local("http://localhost.evil.com/"));
        assert!(!local("http://192.168.1.2/"));
        assert!(!local("http://0.0.0.0/"));
        assert!(!local("http://[::ffff:8.8.8.8]/"));
        assert!(!local("http://127.0.0.1.evil.com/"));
    }

    #[test]
    fn forced_settings_turn_everything_off() {
        let s = forced_settings();
        assert_eq!(s.len(), 4);
        assert!(s.values().all(|v| *v == Value::Bool(false)));
        for k in [
            "telemetry_consent",
            "crash_reporting_consent",
            "cloud_sync_enabled",
            "automatic_updates",
        ] {
            assert!(s.contains_key(k));
        }
    }
}
