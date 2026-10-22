//! Meeting-end webhook — POSTs a JSON payload to a user-configured HTTPS URL
//! when a meeting's summary generation finishes.
//!
//! Config lives in `store.json` (the tauri-plugin-store file under
//! `app_data_dir`), mirroring `self_improve::persisted_enabled`:
//! - `summary_webhook_url`   — HTTPS endpoint; unset/empty disables the hook
//! - `summary_webhook_secret` — optional shared secret sent as `X-Webhook-Secret`
//!
//! Delivery is fire-and-forget: the POST runs in a spawned task with a short
//! timeout and failures are logged, never fatal.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{info, warn};

const STORE_FILE: &str = "store.json";
const STORE_URL_KEY: &str = "summary_webhook_url";
const STORE_SECRET_KEY: &str = "summary_webhook_secret";
const WEBHOOK_TIMEOUT: Duration = Duration::from_secs(5);

/// JSON body POSTed to the configured webhook endpoint.
#[derive(Debug, Serialize)]
pub struct SummaryWebhookPayload {
    pub meeting_id: String,
    pub meeting_name: Option<String>,
    /// Summary generation processing time in seconds.
    pub duration: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_url: Option<String>,
    /// Stored `summary_processes.result` value (contains `markdown`, ...).
    pub summary: serde_json::Value,
}

fn store_value(app_data_dir: &Path, key: &str) -> Option<String> {
    std::fs::read_to_string(app_data_dir.join(STORE_FILE))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get(key)?.as_str().map(str::to_string))
        .filter(|s| !s.trim().is_empty())
}

/// Configured `(url, secret)` pair, or `None` when the hook is disabled.
/// Returns `None` (and logs) when the configured URL is not HTTPS.
fn webhook_config(app_data_dir: &Path) -> Option<(String, Option<String>)> {
    let url = store_value(app_data_dir, STORE_URL_KEY)?;
    if !url.to_ascii_lowercase().starts_with("https://") {
        warn!(
            "summary webhook skipped: {} must be an https:// URL",
            STORE_URL_KEY
        );
        return None;
    }
    Some((url, store_value(app_data_dir, STORE_SECRET_KEY)))
}

/// Fire-and-forget: spawn the POST on the tokio runtime and return
/// immediately. Call once summary generation for a meeting has completed
/// (e.g. from `SummaryService::process_transcript_background`, after
/// `update_process_completed` returns `Ok(true)`).
///
/// `summary` is the `summary_processes.result` JSON value just persisted.
pub fn notify_summary_complete(
    app_data_dir: Option<&PathBuf>,
    meeting_id: &str,
    meeting_name: Option<String>,
    duration: f64,
    summary: serde_json::Value,
) {
    let Some(dir) = app_data_dir else { return };
    let Some((url, secret)) = webhook_config(dir) else {
        return;
    };

    let payload = SummaryWebhookPayload {
        meeting_id: meeting_id.to_string(),
        meeting_name,
        duration,
        transcript_url: None,
        summary,
    };

    tokio::spawn(async move {
        let send = async {
            let mut request = reqwest::Client::builder()
                .timeout(WEBHOOK_TIMEOUT)
                .build()
                .map_err(|e| e.to_string())?
                .post(&url)
                .json(&payload);
            if let Some(secret) = &secret {
                request = request.header("X-Webhook-Secret", secret);
            }
            request.send().await.map_err(|e| e.to_string())
        };

        match send.await {
            Ok(response) if response.status().is_success() => {
                info!(
                    "summary webhook delivered for meeting_id={} (status {})",
                    payload.meeting_id,
                    response.status()
                );
            }
            Ok(response) => warn!(
                "summary webhook for meeting_id={} returned status {}",
                payload.meeting_id,
                response.status()
            ),
            Err(e) => warn!(
                "summary webhook failed for meeting_id={}: {}",
                payload.meeting_id, e
            ),
        }
    });
}
