// Tauri commands for the summary self-improvement feature
// Exposes the settings toggle and the persisted per-template lessons store

use std::collections::HashMap;

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_store::StoreExt;
use tracing::info;

use super::self_improve::{self, Lesson};

const STORE_FILE: &str = "store.json";
const STORE_KEY: &str = "summary_self_improve";

/// Whether the self-improvement pass runs after each summary.
/// Effective state: env `SADACRAFT_SUMMARY_SELF_IMPROVE` OR the persisted flag.
#[tauri::command]
pub async fn api_get_summary_self_improve_enabled<R: Runtime>(
    app: AppHandle<R>,
) -> Result<bool, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {}", e))?;
    Ok(self_improve::is_enabled(Some(&app_data_dir)))
}

/// Persist the self-improvement toggle (`summary_self_improve`) into store.json.
#[tauri::command]
pub async fn api_set_summary_self_improve_enabled<R: Runtime>(
    app: AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    info!("api_set_summary_self_improve_enabled: {}", enabled);
    let store = app
        .store(STORE_FILE)
        .map_err(|e| format!("Failed to access store: {}", e))?;
    store.set(STORE_KEY, serde_json::json!(enabled));
    store
        .save()
        .map_err(|e| format!("Failed to save store: {}", e))?;
    Ok(())
}

/// All persisted lessons, keyed by template id.
#[tauri::command]
pub async fn api_list_summary_lessons<R: Runtime>(
    app: AppHandle<R>,
) -> Result<HashMap<String, Vec<Lesson>>, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {}", e))?;
    Ok(self_improve::list_lessons(&app_data_dir))
}

/// Delete a single learned rule from a template's lessons.
#[tauri::command]
pub async fn api_delete_summary_lesson<R: Runtime>(
    app: AppHandle<R>,
    template_id: String,
    rule: String,
) -> Result<bool, String> {
    info!(
        "api_delete_summary_lesson: template_id={}, rule={:?}",
        template_id, rule
    );
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {}", e))?;
    Ok(self_improve::delete_lesson(
        &app_data_dir,
        &template_id,
        &rule,
    ))
}

/// Clear every learned lesson for a template.
#[tauri::command]
pub async fn api_clear_summary_lessons<R: Runtime>(
    app: AppHandle<R>,
    template_id: String,
) -> Result<bool, String> {
    info!("api_clear_summary_lessons: template_id={}", template_id);
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {}", e))?;
    Ok(self_improve::clear_lessons(&app_data_dir, &template_id))
}
