use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn capture_command_completed(command: &str, outcome: &str, duration: Duration) {
    // LOCAL_ONLY builds never spawn the analytics `curl`, whatever the environment says.
    if anlg_local_only::LOCAL_ONLY || !telemetry_enabled() {
        return;
    }

    let Some(posthog_key) = posthog_key() else {
        return;
    };
    let Some(distinct_id) = distinct_id() else {
        return;
    };

    let host = std::env::var("POSTHOG_HOST")
        .unwrap_or_else(|_| "https://us.i.posthog.com".to_string())
        .trim_end_matches('/')
        .to_string();
    let payload = serde_json::json!({
        "api_key": posthog_key,
        "event": "cli_command_completed",
        "properties": {
            "distinct_id": distinct_id,
            "$session_id": new_id(),
            "surface": "cli",
            "analytics_schema_version": 1,
            "app_version": anarlog_cli::VERSION,
            "command": command,
            "outcome": outcome,
            "duration_ms": duration.as_millis(),
        },
    });

    let Ok(mut child) = Command::new("curl")
        .args([
            "--silent",
            "--fail",
            "--max-time",
            "0.75",
            "--request",
            "POST",
            "--header",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
            &format!("{host}/capture/"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = serde_json::to_writer(&mut stdin, &payload);
        let _ = stdin.flush();
    }
    let _ = child.wait();
}

pub fn telemetry_enabled() -> bool {
    telemetry_enabled_with(
        anlg_local_only::LOCAL_ONLY,
        std::env::var("ANARLOG_ANALYTICS").ok().as_deref(),
    )
}

/// Analytics and error reporting are opt-in and are forced off when the build
/// is local-only, even if `ANARLOG_ANALYTICS` is set.
fn telemetry_enabled_with(local_only: bool, opt_in: Option<&str>) -> bool {
    !local_only
        && opt_in.is_some_and(|value| {
            matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes")
        })
}

fn posthog_key() -> Option<String> {
    std::env::var("POSTHOG_API_KEY")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            option_env!("POSTHOG_API_KEY")
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

fn distinct_id() -> Option<String> {
    let path = analytics_id_path()?;

    if let Ok(value) = fs::read_to_string(&path) {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_owned());
        }
    }

    let value = new_id();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, &value);
    Some(value)
}

fn analytics_id_path() -> Option<PathBuf> {
    dirs::config_dir().map(|path| path.join("anarlog").join("analytics-id"))
}

fn new_id() -> String {
    let mut hasher = DefaultHasher::new();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    format!("cli-{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_only_forces_telemetry_off_even_when_opted_in() {
        for value in ["1", "true", "YES"] {
            assert!(telemetry_enabled_with(false, Some(value)));
            assert!(!telemetry_enabled_with(true, Some(value)));
        }
        assert!(!telemetry_enabled_with(false, None));
        assert!(!telemetry_enabled_with(false, Some("0")));
    }

    #[test]
    fn this_build_is_local_only_and_never_reports() {
        assert!(std::hint::black_box(anlg_local_only::LOCAL_ONLY));
        assert!(!telemetry_enabled());
        // Returns before touching curl or the network.
        capture_command_completed("mcp", "succeeded", Duration::from_millis(1));
    }
}
