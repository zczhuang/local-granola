use tauri::Manager;

const MAX_CONCURRENT_FIRE_AND_FORGET_EVENTS: usize = 32;

mod commands;
mod error;
mod ext;
mod session;
mod store;

pub use error::{Error, Result};
pub use ext::*;
use session::*;
use store::*;

pub use anlg_analytics::*;

pub struct ManagedState {
    client: anlg_analytics::AnalyticsClient,
    fire_and_forget_slots: std::sync::Arc<tokio::sync::Semaphore>,
    groups: std::sync::Mutex<std::collections::HashMap<String, String>>,
    session: std::sync::Mutex<SessionTracker>,
    /// True in the local-only build or when no PostHog key was compiled in.
    forced_disabled: bool,
}

impl ManagedState {
    fn new(client: anlg_analytics::AnalyticsClient, forced_disabled: bool) -> Self {
        Self {
            client,
            forced_disabled,
            fire_and_forget_slots: std::sync::Arc::new(tokio::sync::Semaphore::new(
                MAX_CONCURRENT_FIRE_AND_FORGET_EVENTS,
            )),
            groups: std::sync::Mutex::new(std::collections::HashMap::new()),
            session: std::sync::Mutex::new(SessionTracker::new(std::time::SystemTime::now())),
        }
    }
}

/// The PostHog key to use, or `None` when analytics must stay off: always in the
/// local-only build, and whenever no well-formed key was compiled in.
fn effective_posthog_key(local_only: bool, key: Option<&'static str>) -> Option<&'static str> {
    if local_only {
        return None;
    }
    key.filter(|key| key.starts_with("phc_"))
}

const PLUGIN_NAME: &str = "analytics";

fn make_specta_builder<R: tauri::Runtime>() -> tauri_specta::Builder<R> {
    tauri_specta::Builder::<R>::new()
        .plugin_name(PLUGIN_NAME)
        .commands(tauri_specta::collect_commands![
            commands::event_fire_and_forget::<tauri::Wry>,
            commands::event::<tauri::Wry>,
            commands::set_properties::<tauri::Wry>,
            commands::set_disabled::<tauri::Wry>,
            commands::is_disabled::<tauri::Wry>,
            commands::identify::<tauri::Wry>,
            commands::clear_groups::<tauri::Wry>,
        ])
        .error_handling(tauri_specta::ErrorHandlingMode::Result)
}

pub fn init<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    let specta_builder = make_specta_builder();

    tauri::plugin::Builder::new(PLUGIN_NAME)
        .invoke_handler(specta_builder.invoke_handler())
        .setup(|app, _api| {
            let posthog_key =
                effective_posthog_key(anlg_local_only::LOCAL_ONLY, option_env!("POSTHOG_API_KEY"));

            let client = {
                let mut builder = anlg_analytics::AnalyticsClientBuilder::default();
                if let Some(key) = posthog_key {
                    builder = builder.with_posthog(key);
                }

                builder.build()
            };

            assert!(app.manage(ManagedState::new(client, posthog_key.is_none())));
            Ok(())
        })
        .build()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn analytics_is_off_when_local_only_or_keyless() {
        assert_eq!(effective_posthog_key(true, Some("phc_abc")), None);
        assert_eq!(effective_posthog_key(false, None), None);
        assert_eq!(effective_posthog_key(false, Some("nope")), None);
        assert_eq!(
            effective_posthog_key(false, Some("phc_abc")),
            Some("phc_abc")
        );
    }

    #[test]
    fn export_types() {
        const OUTPUT_FILE: &str = "./js/bindings.gen.ts";

        make_specta_builder::<tauri::Wry>()
            .export(
                specta_typescript::Typescript::default()
                    .formatter(specta_typescript::formatter::prettier)
                    .bigint(specta_typescript::BigIntExportBehavior::Number),
                OUTPUT_FILE,
            )
            .unwrap();

        let content = std::fs::read_to_string(OUTPUT_FILE).unwrap();
        std::fs::write(OUTPUT_FILE, format!("// @ts-nocheck\n{content}")).unwrap();
    }
}
