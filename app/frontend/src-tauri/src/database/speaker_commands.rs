use log::{error, info};
use tauri::{AppHandle, Runtime};

use super::repositories::transcript::{SpeakerStat, TranscriptsRepository};
use crate::state::AppState;

/// Maximum allowed length for a speaker label.
const MAX_SPEAKER_LABEL_LEN: usize = 64;

/// Returns per-speaker line counts and talk time for a meeting.
///
/// Stats are aggregated in SQLite so they cover the full meeting even when
/// the frontend only has a paginated slice of transcripts loaded.
#[tauri::command]
pub async fn api_get_meeting_speakers<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<SpeakerStat>, String> {
    info!("api_get_meeting_speakers called for meeting_id: {}", meeting_id);

    let pool = state.db_manager.pool();

    TranscriptsRepository::get_speaker_stats(pool, &meeting_id)
        .await
        .map_err(|e| {
            error!(
                "Failed to load speaker stats for meeting {}: {}",
                meeting_id, e
            );
            format!("Failed to load speakers: {}", e)
        })
}

/// Renames a speaker label across all transcript rows of a meeting.
///
/// The rename is persisted to SQLite and applies retroactively to existing
/// rows. Returns the number of transcript rows updated.
#[tauri::command]
pub async fn rename_speaker<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    old_label: String,
    new_label: String,
    state: tauri::State<'_, AppState>,
) -> Result<u64, String> {
    let new_label = new_label.trim().to_string();

    info!(
        "rename_speaker called for meeting_id: {} ('{}' -> '{}')",
        meeting_id, old_label, new_label
    );

    if meeting_id.trim().is_empty() || old_label.is_empty() {
        return Err("meeting_id and old_label cannot be empty".to_string());
    }
    if new_label.is_empty() {
        return Err("New speaker name cannot be empty".to_string());
    }
    if new_label.chars().count() > MAX_SPEAKER_LABEL_LEN {
        return Err(format!(
            "Speaker name cannot exceed {} characters",
            MAX_SPEAKER_LABEL_LEN
        ));
    }
    if new_label == old_label {
        return Ok(0);
    }

    let pool = state.db_manager.pool();

    TranscriptsRepository::rename_speaker(pool, &meeting_id, &old_label, &new_label)
        .await
        .map_err(|e| {
            error!(
                "Failed to rename speaker '{}' for meeting {}: {}",
                old_label, meeting_id, e
            );
            format!("Failed to rename speaker: {}", e)
        })
}
