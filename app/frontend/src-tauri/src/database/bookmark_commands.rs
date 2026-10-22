use std::path::Path;
use std::sync::Mutex;

use chrono::Utc;
use log::{error, info};
use tauri::{AppHandle, Manager, Runtime};
use uuid::Uuid;

use super::repositories::transcript::{Bookmark, BookmarksRepository};
use crate::state::AppState;

/// Bookmarks created during a live recording can't be persisted immediately:
/// the `meetings` row only exists once the frontend saves the meeting after
/// `recording-stopped` (see `api_save_transcript`). Until then bookmarks are
/// queued here and flushed by `finalize_pending_bookmarks` once a real
/// `meeting_id` is known.
static PENDING_BOOKMARKS: Mutex<Vec<Bookmark>> = Mutex::new(Vec::new());

/// Recording-relative elapsed seconds for the active recording, or `None`
/// when nothing is recording. Reuses `get_recording_state` so we don't need
/// to touch the private `RECORDING_MANAGER` static.
async fn current_recording_offset() -> Option<f64> {
    let state = crate::audio::recording_commands::get_recording_state().await;
    if !state["is_recording"].as_bool().unwrap_or(false) {
        return None;
    }
    state["active_duration"]
        .as_f64()
        .or_else(|| state["recording_duration"].as_f64())
}

/// Adds a bookmark for a meeting, capturing the current recording position.
///
/// - With `meeting_id`: the bookmark is persisted to SQLite immediately.
/// - Without `meeting_id` while recording: it is queued in memory and can be
///   flushed later via `finalize_pending_bookmarks` once the meeting is saved.
/// - `offset_seconds` may be passed explicitly (e.g. re-saving a pending
///   bookmark); otherwise it is captured from the running recording.
#[tauri::command]
pub async fn add_bookmark<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: Option<String>,
    label: Option<String>,
    offset_seconds: Option<f64>,
    state: tauri::State<'_, AppState>,
) -> Result<Bookmark, String> {
    info!(
        "add_bookmark called (meeting_id: {:?}, label: {:?})",
        meeting_id, label
    );

    let offset = offset_seconds.or(current_recording_offset().await);

    if let Some(meeting_id) = meeting_id {
        let pool = state.db_manager.pool();
        BookmarksRepository::add_bookmark(pool, &meeting_id, label.as_deref(), offset, None)
            .await
            .map_err(|e| {
                error!("Failed to add bookmark for meeting {}: {}", meeting_id, e);
                format!("Failed to add bookmark: {}", e)
            })
    } else {
        if offset.is_none() {
            return Err(
                "No recording in progress and no meeting_id provided".to_string()
            );
        }
        let bookmark = Bookmark {
            id: format!("pending-{}", Uuid::new_v4()),
            meeting_id: String::new(),
            label,
            offset_seconds: offset,
            image_path: None,
            created_at: Utc::now().to_rfc3339(),
        };
        PENDING_BOOKMARKS
            .lock()
            .map_err(|e| e.to_string())?
            .push(bookmark.clone());
        Ok(bookmark)
    }
}

/// Lists persisted bookmarks for a meeting.
#[tauri::command]
pub async fn list_bookmarks<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Bookmark>, String> {
    let pool = state.db_manager.pool();
    BookmarksRepository::list_bookmarks(pool, &meeting_id)
        .await
        .map_err(|e| {
            error!("Failed to list bookmarks for meeting {}: {}", meeting_id, e);
            format!("Failed to list bookmarks: {}", e)
        })
}

/// Lists in-memory bookmarks captured during the current recording that have
/// not been attached to a persisted meeting yet.
#[tauri::command]
pub async fn list_pending_bookmarks<R: Runtime>(
    _app: AppHandle<R>,
) -> Result<Vec<Bookmark>, String> {
    Ok(PENDING_BOOKMARKS
        .lock()
        .map_err(|e| e.to_string())?
        .clone())
}

/// Moves all pending (live-recording) bookmarks into SQLite under the given
/// meeting. Call once after the meeting has been saved and its id is known.
/// Returns the persisted bookmarks.
#[tauri::command]
pub async fn finalize_pending_bookmarks<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Bookmark>, String> {
    let pending: Vec<Bookmark> = PENDING_BOOKMARKS
        .lock()
        .map_err(|e| e.to_string())?
        .drain(..)
        .collect();

    info!(
        "finalize_pending_bookmarks: attaching {} pending bookmark(s) to meeting {}",
        pending.len(),
        meeting_id
    );

    let pool = state.db_manager.pool();
    let mut saved = Vec::with_capacity(pending.len());
    for b in pending {
        let bookmark = BookmarksRepository::add_bookmark(
            pool,
            &meeting_id,
            b.label.as_deref(),
            b.offset_seconds,
            b.image_path.as_deref(),
        )
        .await
        .map_err(|e| format!("Failed to persist pending bookmark: {}", e))?;
        saved.push(bookmark);
    }
    Ok(saved)
}

/// Deletes a bookmark by id — from SQLite, or from the pending queue when it
/// hasn't been persisted yet.
#[tauri::command]
pub async fn delete_bookmark<R: Runtime>(
    _app: AppHandle<R>,
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    // Pending bookmarks (live recording, not yet persisted) have no DB row.
    {
        let mut pending = PENDING_BOOKMARKS.lock().map_err(|e| e.to_string())?;
        if let Some(pos) = pending.iter().position(|b| b.id == id) {
            pending.remove(pos);
            return Ok(true);
        }
    }

    let pool = state.db_manager.pool();
    BookmarksRepository::delete_bookmark(pool, &id)
        .await
        .map_err(|e| {
            error!("Failed to delete bookmark {}: {}", id, e);
            format!("Failed to delete bookmark: {}", e)
        })
}

/// Captures the primary monitor as a PNG and records a bookmark pointing at
/// the image — the "this slide was on screen" note. The bookmark behaves like
/// `add_bookmark` (persisted when `meeting_id` is known, queued while a
/// recording is still unsaved).
///
/// Implemented on Windows only (GDI BitBlt). On other platforms it returns an
/// error so the frontend can degrade to a plain bookmark-with-note.
#[tauri::command]
pub async fn capture_frame_note<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: Option<String>,
    label: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<Bookmark, String> {
    info!(
        "capture_frame_note called (meeting_id: {:?}, label: {:?})",
        meeting_id, label
    );

    let offset = current_recording_offset().await;

    // Persist frames under app-data/slides/<meeting_id|_pending>/<ts>.png.
    // Frames for unsaved recordings land in `_pending`; the stored path stays
    // valid after the bookmark is finalized (the directory name is cosmetic).
    let slides_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?
        .join("slides")
        .join(meeting_id.as_deref().unwrap_or("_pending"));

    std::fs::create_dir_all(&slides_dir)
        .map_err(|e| format!("Failed to create slides dir: {}", e))?;

    let image_path = slides_dir
        .join(format!("slide-{}.png", Utc::now().timestamp_millis()));
    let image_path_str = image_path.to_string_lossy().to_string();

    capture_primary_monitor_png(&image_path)?;

    if let Some(meeting_id) = meeting_id {
        let pool = state.db_manager.pool();
        BookmarksRepository::add_bookmark(
            pool,
            &meeting_id,
            label.as_deref(),
            offset,
            Some(&image_path_str),
        )
        .await
        .map_err(|e| {
            error!("Failed to add slide bookmark for meeting {}: {}", meeting_id, e);
            format!("Failed to add slide bookmark: {}", e)
        })
    } else {
        if offset.is_none() {
            return Err(
                "No recording in progress and no meeting_id provided".to_string()
            );
        }
        let bookmark = Bookmark {
            id: format!("pending-{}", Uuid::new_v4()),
            meeting_id: String::new(),
            label,
            offset_seconds: offset,
            image_path: Some(image_path_str),
            created_at: Utc::now().to_rfc3339(),
        };
        PENDING_BOOKMARKS
            .lock()
            .map_err(|e| e.to_string())?
            .push(bookmark.clone());
        Ok(bookmark)
    }
}

/// Screenshot of the primary monitor via GDI BitBlt → PNG (Windows only).
#[cfg(target_os = "windows")]
fn capture_primary_monitor_png(path: &Path) -> Result<(), String> {
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetDesktopWindow, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
    };

    unsafe {
        let hwnd = GetDesktopWindow();
        let width = GetSystemMetrics(SM_CXSCREEN);
        let height = GetSystemMetrics(SM_CYSCREEN);
        if width <= 0 || height <= 0 {
            return Err("Invalid screen dimensions".to_string());
        }

        let screen_dc = GetDC(hwnd);
        if screen_dc.is_invalid() {
            return Err("GetDC failed for desktop window".to_string());
        }

        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.is_invalid() {
            ReleaseDC(hwnd, screen_dc);
            return Err("CreateCompatibleDC failed".to_string());
        }

        let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
        if bitmap.is_invalid() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(hwnd, screen_dc);
            return Err("CreateCompatibleBitmap failed".to_string());
        }

        let old_obj = SelectObject(mem_dc, bitmap);
        let blt_result = BitBlt(mem_dc, 0, 0, width, height, screen_dc, 0, 0, SRCCOPY);

        // Read pixels out of the compatible bitmap (32bpp BGRA, top-down).
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // negative → top-down row order
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0 as u32,
            ..Default::default()
        };
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let lines = GetDIBits(
            mem_dc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        // Release GDI objects before doing any further (fallible) work.
        SelectObject(mem_dc, old_obj);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(hwnd, screen_dc);

        blt_result.map_err(|e| format!("BitBlt failed: {}", e))?;
        if lines == 0 {
            return Err("GetDIBits returned 0 scanlines".to_string());
        }

        // GDI emits BGRA; the image crate wants RGBA.
        for px in pixels.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        let img = image::RgbaImage::from_raw(width as u32, height as u32, pixels)
            .ok_or_else(|| "Failed to wrap screenshot pixels".to_string())?;
        img.save(path)
            .map_err(|e| format!("Failed to save screenshot PNG: {}", e))
    }
}

#[cfg(not(target_os = "windows"))]
fn capture_primary_monitor_png(_path: &Path) -> Result<(), String> {
    Err("capture_frame_note is only implemented on Windows (GDI BitBlt); \
         on this platform use a plain bookmark instead"
        .to_string())
}
