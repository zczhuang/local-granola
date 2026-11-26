// Single source of truth for this plugin's Tauri commands. build.rs includes
// this file to drive permission generation, and the lib tests assert the
// specta bindings and generated permission files stay in sync with it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const COMMANDS: &[&str] = &[
    "export",
    "get_google_drive_sync_status",
    "save_google_drive_credentials",
    "connect_google_drive",
    "disconnect_google_drive",
    "update_google_drive_sync_config",
    "sync_notes_to_google_drive",
    "get_zoom_sync_status",
    "save_zoom_credentials",
    "connect_zoom",
    "disconnect_zoom",
    "update_zoom_sync_config",
    "sync_zoom_meetings",
];
