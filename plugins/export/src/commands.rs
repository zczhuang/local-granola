use std::path::PathBuf;

use tauri::Manager;

use crate::ExportPluginExt;
use crate::google_drive::{
    self, GoogleDriveSyncConfig, GoogleDriveSyncReport, GoogleDriveSyncStatus,
};
use crate::zoom::{self, ZoomSyncConfig, ZoomSyncReport, ZoomSyncStatus};

fn app_data_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data directory: {e}"))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn export<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: PathBuf,
    input: crate::ExportInput,
) -> Result<(), String> {
    app.export()
        .export_pdf(&path, input)
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn get_google_drive_sync_status<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<GoogleDriveSyncStatus, String> {
    let dir = app_data_dir(&app)?;
    google_drive::get_status(&dir)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn save_google_drive_credentials<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    client_id_or_json: String,
    client_secret: Option<String>,
) -> Result<GoogleDriveSyncStatus, String> {
    let dir = app_data_dir(&app)?;
    google_drive::save_credentials(&dir, &client_id_or_json, client_secret.as_deref())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn connect_google_drive<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<GoogleDriveSyncStatus, String> {
    let dir = app_data_dir(&app)?;
    google_drive::connect(&dir).await
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn disconnect_google_drive<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<GoogleDriveSyncStatus, String> {
    let dir = app_data_dir(&app)?;
    google_drive::disconnect(&dir)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn update_google_drive_sync_config<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    config: GoogleDriveSyncConfig,
) -> Result<GoogleDriveSyncStatus, String> {
    let dir = app_data_dir(&app)?;
    google_drive::update_config(&dir, config)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn sync_notes_to_google_drive<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: Option<String>,
) -> Result<GoogleDriveSyncReport, String> {
    let dir = app_data_dir(&app)?;
    let db = app.state::<tauri_plugin_db::ManagedState>();
    google_drive::sync_notes(db.pool(), &dir, session_id).await
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn get_zoom_sync_status<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<ZoomSyncStatus, String> {
    let dir = app_data_dir(&app)?;
    zoom::get_status(&dir)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn save_zoom_credentials<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    client_id: String,
    client_secret: Option<String>,
) -> Result<ZoomSyncStatus, String> {
    let dir = app_data_dir(&app)?;
    zoom::save_credentials(&dir, &client_id, client_secret.as_deref())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn connect_zoom<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<ZoomSyncStatus, String> {
    let dir = app_data_dir(&app)?;
    zoom::connect(&dir).await
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn disconnect_zoom<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<ZoomSyncStatus, String> {
    let dir = app_data_dir(&app)?;
    zoom::disconnect(&dir)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn update_zoom_sync_config<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    config: ZoomSyncConfig,
) -> Result<ZoomSyncStatus, String> {
    let dir = app_data_dir(&app)?;
    zoom::update_config(&dir, config)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn sync_zoom_meetings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: Option<String>,
) -> Result<ZoomSyncReport, String> {
    let dir = app_data_dir(&app)?;
    let db = app.state::<tauri_plugin_db::ManagedState>();
    zoom::sync_meetings(db.pool(), &dir, session_id).await
}
