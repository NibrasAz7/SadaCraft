//! Automatic meeting detection — a lightweight, opt-in background monitor.
//!
//! When enabled (persisted `meeting_autodetect` flag in `store.json`, or env
//! `SADACRAFT_MEETING_AUTODETECT=1`), a tauri async task polls two signals and
//! emits a single `meeting-detected` event per meeting episode:
//!
//! 1. **Process heuristic** (Windows-first): known meeting-app executables
//!    (Zoom, Teams, WebEx) running for >30s while SadaCraft is not recording.
//!    Browser-embedded meetings (Meet/Teams-in-Chrome tabs) cannot be seen this
//!    way and are out of scope. Slack huddles are intentionally not covered:
//!    slack.exe is resident whether or not a huddle is active.
//!    "Actively using audio" is approximated cheaply: process age >30s AND our
//!    own capture not already recording. True per-process audio-session checks
//!    (IAudioSessionManager2) are deliberately not implemented.
//!
//! 2. **Audio-activity wake** (Windows only): a WASAPI loopback capture on the
//!    default render endpoint measures RMS; sustained speech-level activity
//!    (RMS >= `SADACRAFT_DETECT_RMS`, default 0.02) for longer than
//!    `SADACRAFT_DETECT_DWELL_S` seconds (default 20) counts as a signal. This
//!    keeps a shared-mode capture session open, so it only runs while the
//!    setting is on and is dropped again when the setting turns off.
//!
//! Debounce: `meeting-detected` fires once per episode; the detector re-arms
//! only after all signals have been absent for >= 2 minutes.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_store::StoreExt;
use tracing::{debug, info, warn};

const STORE_FILE: &str = "store.json";
const STORE_KEY: &str = "meeting_autodetect";

/// How often the detector ticks (process scan + RMS drain).
const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Signals must be absent for this long before the detector re-arms.
const REARM_ABSENCE: Duration = Duration::from_secs(120);
/// A meeting-app process must be at least this old to count — filters out
/// launcher/daemon respawns and pre-meeting app startups.
const MIN_PROCESS_AGE_S: u64 = 30;
/// Ticks to wait before retrying a failed/dead loopback monitor (~1 minute).
const AUDIO_RETRY_TICKS: u32 = 6;

/// Audio-signal tunables (Windows loopback path only).
#[cfg(target_os = "windows")]
const ENV_RMS_THRESHOLD: &str = "SADACRAFT_DETECT_RMS";
#[cfg(target_os = "windows")]
const ENV_DWELL_S: &str = "SADACRAFT_DETECT_DWELL_S";
#[cfg(target_os = "windows")]
const DEFAULT_RMS_THRESHOLD: f32 = 0.02;
#[cfg(target_os = "windows")]
const DEFAULT_DWELL_S: u64 = 20;

/// Process-name heuristic, normalized exe basename (lowercase, `.exe` stripped)
/// -> detail label reported in the event.
const MEETING_PROCESSES: &[(&str, &str)] = &[
    ("zoom", "zoom.exe"),
    ("zoom.us", "zoom.us"), // macOS bundle binary
    ("teams", "Teams.exe"), // classic Teams
    ("ms-teams", "ms-teams.exe"), // new Teams for Windows
    ("webex", "webex.exe"),
    ("ciscocollabhost", "CiscoCollabHost.exe"), // Webex App
    ("webexmta", "webexmta.exe"),               // Webex Meetings
    ("atmgr", "atmgr.exe"),                     // Webex meeting manager
];

/// Payload of the `meeting-detected` event consumed by the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct MeetingDetectedPayload {
    /// "process" | "audio"
    pub source: String,
    /// Rough 0..1 heuristic confidence.
    pub confidence: f32,
    /// Human-readable detail, e.g. the executable name or the RMS threshold hit.
    pub detail: String,
}

fn persisted_enabled(app_data_dir: &Path) -> bool {
    std::fs::read_to_string(app_data_dir.join(STORE_FILE))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get(STORE_KEY)?.as_bool())
        .unwrap_or(false)
}

/// True when env `SADACRAFT_MEETING_AUTODETECT` is set OR the persisted
/// settings toggle is on.
pub fn is_enabled(app_data_dir: Option<&PathBuf>) -> bool {
    if std::env::var("SADACRAFT_MEETING_AUTODETECT").is_ok_and(|v| v == "1" || v == "true") {
        return true;
    }
    app_data_dir.is_some_and(|dir| persisted_enabled(dir))
}

/// First meeting-app process older than `MIN_PROCESS_AGE_S`, as a detail label.
fn meeting_process_hit(sys: &sysinfo::System) -> Option<&'static str> {
    sys.processes().values().find_map(|p| {
        let name = p.name().to_string_lossy().to_lowercase();
        let base = name.strip_suffix(".exe").unwrap_or(&name);
        MEETING_PROCESSES
            .iter()
            .find(|(key, _)| *key == base)
            .and_then(|(_, label)| (p.run_time() >= MIN_PROCESS_AGE_S).then_some(*label))
    })
}

#[cfg(target_os = "windows")]
fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(default)
}

#[cfg(target_os = "windows")]
fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

// ============================================================================
// Audio-activity wake — WASAPI loopback RMS monitor (Windows only)
// ============================================================================
//
// cpal::Stream is !Send, so the stream is created, played and dropped on a
// dedicated std::thread. The cpal callback only accumulates sum-of-squares and
// a sample count into atomics; the async detector loop drains them once per
// tick, so no blocking happens on the audio thread.

#[cfg(target_os = "windows")]
struct AudioMonitor {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    sum_sq_bits: std::sync::Arc<std::sync::atomic::AtomicU64>,
    sample_count: std::sync::Arc<std::sync::atomic::AtomicU64>,
    errored: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(target_os = "windows")]
fn add_f64(atomic: &std::sync::atomic::AtomicU64, value: f64) {
    use std::sync::atomic::Ordering;
    // CAS loop instead of fetch_update/try_update: try_update is not in the
    // MSRV (rust-version = 1.77) and fetch_update is deprecated on newer
    // toolchains.
    let mut cur = atomic.load(Ordering::Relaxed);
    loop {
        let new = f64::to_bits(f64::from_bits(cur) + value);
        match atomic.compare_exchange_weak(cur, new, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

#[cfg(target_os = "windows")]
fn accumulate(
    sum_sq_bits: &std::sync::atomic::AtomicU64,
    sample_count: &std::sync::atomic::AtomicU64,
    data: &[f32],
) {
    use std::sync::atomic::Ordering;
    if data.is_empty() {
        return;
    }
    let sum: f64 = data.iter().map(|&s| (s as f64) * (s as f64)).sum();
    add_f64(sum_sq_bits, sum);
    sample_count.fetch_add(data.len() as u64, Ordering::Relaxed);
}

#[cfg(target_os = "windows")]
fn build_loopback_stream(
    sum_sq_bits: std::sync::Arc<std::sync::atomic::AtomicU64>,
    sample_count: std::sync::Arc<std::sync::atomic::AtomicU64>,
    errored: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<cpal::Stream> {
    use cpal::traits::{DeviceTrait, HostTrait};
    use std::sync::atomic::Ordering;

    // WASAPI loopback: an *input* stream built on an *output* device captures
    // the render mix (cpal sets AUDCLNT_STREAMFLAGS_LOOPBACK internally).
    let host = cpal::host_from_id(cpal::HostId::Wasapi)?;
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no default output device"))?;
    let config = device.default_output_config()?;
    let stream_config: cpal::StreamConfig = config.clone().into();

    let err_cb = move |e: cpal::StreamError| {
        warn!("meeting_detect: loopback stream error: {}", e);
        errored.store(true, Ordering::Relaxed);
    };

    match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let (ss, sc) = (sum_sq_bits, sample_count);
            Ok(device.build_input_stream(
                &stream_config,
                move |data: &[f32], _| accumulate(&ss, &sc, data),
                err_cb,
                None,
            )?)
        }
        cpal::SampleFormat::I16 => {
            let (ss, sc) = (sum_sq_bits, sample_count);
            Ok(device.build_input_stream(
                &stream_config,
                move |data: &[i16], _| {
                    let f32_data: Vec<f32> =
                        data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                    accumulate(&ss, &sc, &f32_data);
                },
                err_cb,
                None,
            )?)
        }
        cpal::SampleFormat::I32 => {
            let (ss, sc) = (sum_sq_bits, sample_count);
            Ok(device.build_input_stream(
                &stream_config,
                move |data: &[i32], _| {
                    let f32_data: Vec<f32> =
                        data.iter().map(|&s| s as f32 / i32::MAX as f32).collect();
                    accumulate(&ss, &sc, &f32_data);
                },
                err_cb,
                None,
            )?)
        }
        cpal::SampleFormat::U16 => {
            let (ss, sc) = (sum_sq_bits, sample_count);
            Ok(device.build_input_stream(
                &stream_config,
                move |data: &[u16], _| {
                    let f32_data: Vec<f32> = data
                        .iter()
                        .map(|&s| (s as f32 - u16::MAX as f32 / 2.0) / (u16::MAX as f32 / 2.0))
                        .collect();
                    accumulate(&ss, &sc, &f32_data);
                },
                err_cb,
                None,
            )?)
        }
        other => anyhow::bail!("unsupported loopback sample format: {:?}", other),
    }
}

#[cfg(target_os = "windows")]
impl AudioMonitor {
    /// Open the loopback stream on a dedicated thread. Returns None if the
    /// stream could not be built or started within the startup timeout.
    fn start() -> Option<Self> {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::Arc;

        let stop = Arc::new(AtomicBool::new(false));
        let sum_sq_bits = Arc::new(AtomicU64::new(0));
        let sample_count = Arc::new(AtomicU64::new(0));
        let errored = Arc::new(AtomicBool::new(false));

        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let thread = std::thread::spawn({
            let stop = stop.clone();
            let sum_sq_bits = sum_sq_bits.clone();
            let sample_count = sample_count.clone();
            let errored = errored.clone();
            move || {
                match build_loopback_stream(sum_sq_bits, sample_count, errored) {
                    Ok(stream) => {
                        use cpal::traits::StreamTrait;
                        match stream.play() {
                            Ok(()) => {
                                let _ = ready_tx.send(Ok(()));
                                // Keep the stream alive until told to stop.
                                while !stop.load(Ordering::Relaxed) {
                                    std::thread::sleep(Duration::from_millis(200));
                                }
                            }
                            Err(e) => {
                                let _ = ready_tx.send(Err(format!("play failed: {}", e)));
                            }
                        }
                        // Explicit pause before drop so the audio thread stops
                        // invoking callbacks before the stream is torn down.
                        let _ = stream.pause();
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                    }
                }
            }
        });

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Some(Self {
                stop,
                sum_sq_bits,
                sample_count,
                errored,
                thread: Some(thread),
            }),
            other => {
                if let Ok(Err(e)) = other.as_ref() {
                    debug!("meeting_detect: loopback monitor failed to start: {}", e);
                }
                // Ensure the worker exits even if it is somehow still waiting.
                stop.store(true, Ordering::Relaxed);
                let _ = thread.join();
                None
            }
        }
    }

    /// True once the stream reported an error — caller should drop and retry.
    fn dead(&self) -> bool {
        self.errored.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Drain the accumulated sum-of-squares and return the RMS of audio seen
    /// since the last call, or None if no samples arrived.
    fn drain_rms(&self) -> Option<f32> {
        use std::sync::atomic::Ordering;
        let n = self.sample_count.swap(0, Ordering::Relaxed);
        let bits = self.sum_sq_bits.swap(0, Ordering::Relaxed);
        if n == 0 {
            return None;
        }
        Some((f64::from_bits(bits) / n as f64).sqrt() as f32)
    }
}

#[cfg(target_os = "windows")]
impl Drop for AudioMonitor {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// ============================================================================
// Detector task
// ============================================================================

pub struct MeetingDetector;

impl MeetingDetector {
    /// Spawn the background monitor. Near-zero cost while the
    /// `meeting_autodetect` setting is off (one store.json read per 10s tick).
    pub fn spawn<R: Runtime>(app: AppHandle<R>) {
        tauri::async_runtime::spawn(async move {
            run(app).await;
        });
    }
}

async fn run<R: Runtime>(app: AppHandle<R>) {
    use sysinfo::ProcessesToUpdate;

    let mut sys = sysinfo::System::new();
    let mut episode_latched = false;
    let mut last_signal_seen: Option<Instant> = None;

    #[cfg(target_os = "windows")]
    let rms_threshold = env_f32(ENV_RMS_THRESHOLD, DEFAULT_RMS_THRESHOLD);
    #[cfg(target_os = "windows")]
    let dwell = Duration::from_secs(env_u64(ENV_DWELL_S, DEFAULT_DWELL_S));
    #[cfg(target_os = "windows")]
    let mut audio_monitor: Option<AudioMonitor> = None;
    #[cfg(target_os = "windows")]
    let mut audio_retry_in: u32 = 0;
    #[cfg(target_os = "windows")]
    let mut audio_above_since: Option<Instant> = None;

    info!(
        "meeting_detect: monitor started (poll={}s, re-arm={}s)",
        POLL_INTERVAL.as_secs(),
        REARM_ABSENCE.as_secs()
    );

    loop {
        tokio::time::sleep(POLL_INTERVAL).await;

        // Re-read the persisted toggle every tick so a settings change takes
        // effect without an app restart.
        let app_data_dir = app.path().app_data_dir().ok();
        if !is_enabled(app_data_dir.as_ref()) {
            if episode_latched || last_signal_seen.is_some() {
                debug!("meeting_detect: disabled — resetting episode state");
            }
            episode_latched = false;
            last_signal_seen = None;
            #[cfg(target_os = "windows")]
            {
                // Dropping the monitor releases the loopback capture session.
                audio_monitor = None;
                audio_above_since = None;
                audio_retry_in = 0;
            }
            continue;
        }

        #[cfg(target_os = "windows")]
        {
            if audio_monitor.as_ref().is_some_and(|m| m.dead()) {
                debug!("meeting_detect: loopback stream errored — restarting");
                audio_monitor = None;
            }
            if audio_monitor.is_none() {
                if audio_retry_in > 0 {
                    audio_retry_in -= 1;
                } else {
                    audio_monitor = AudioMonitor::start();
                    if audio_monitor.is_none() {
                        audio_retry_in = AUDIO_RETRY_TICKS;
                    }
                }
            }
        }

        // Drain the RMS accumulator every tick (even while recording) so the
        // window always reflects roughly the last POLL_INTERVAL of audio.
        #[cfg(target_os = "windows")]
        let rms = audio_monitor.as_ref().and_then(|m| m.drain_rms());

        if crate::audio::recording_commands::is_recording().await {
            // Own capture already running — suppress emission and keep the
            // episode latched so it does not re-fire when recording stops.
            episode_latched = true;
            last_signal_seen = Some(Instant::now());
            continue;
        }

        // Signal 1: meeting-app process heuristic.
        sys.refresh_processes(ProcessesToUpdate::All, true);
        let process_hit = meeting_process_hit(&sys);

        // Signal 2: sustained system-audio activity.
        #[cfg(target_os = "windows")]
        let audio_detail = match rms {
            Some(rms) if rms >= rms_threshold => {
                let since = audio_above_since.get_or_insert_with(Instant::now);
                (since.elapsed() >= dwell).then(|| {
                    format!(
                        "system audio RMS >= {:.3} sustained >= {}s",
                        rms_threshold,
                        dwell.as_secs()
                    )
                })
            }
            _ => {
                audio_above_since = None;
                None
            }
        };
        #[cfg(not(target_os = "windows"))]
        let audio_detail: Option<String> = None;

        let signal = process_hit
            .map(|exe| ("process", 0.9_f32, exe.to_string()))
            .or_else(|| audio_detail.map(|d| ("audio", 0.6_f32, d)));

        match signal {
            Some((source, confidence, detail)) => {
                last_signal_seen = Some(Instant::now());
                if !episode_latched {
                    episode_latched = true;
                    let payload = MeetingDetectedPayload {
                        source: source.to_string(),
                        confidence,
                        detail: detail.clone(),
                    };
                    info!(
                        "meeting_detect: emitting meeting-detected (source={}, detail={})",
                        source, detail
                    );
                    if let Err(e) = app.emit("meeting-detected", payload) {
                        warn!("meeting_detect: failed to emit meeting-detected: {}", e);
                    }
                }
            }
            None => {
                if let Some(seen) = last_signal_seen {
                    if seen.elapsed() >= REARM_ABSENCE {
                        episode_latched = false;
                        last_signal_seen = None;
                        debug!("meeting_detect: signals quiet — re-armed");
                    }
                }
            }
        }
    }
}

// ============================================================================
// Tauri commands — settings toggle
// ============================================================================

/// Whether automatic meeting detection is enabled.
/// Effective state: env `SADACRAFT_MEETING_AUTODETECT` OR the persisted flag.
#[tauri::command]
pub async fn api_get_meeting_autodetect_enabled<R: Runtime>(
    app: AppHandle<R>,
) -> Result<bool, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {}", e))?;
    Ok(is_enabled(Some(&app_data_dir)))
}

/// Persist the auto-detect toggle (`meeting_autodetect`) into store.json.
/// Takes effect on the next detector tick (<= 10s) — toggling off also
/// releases the loopback capture session.
#[tauri::command]
pub async fn api_set_meeting_autodetect_enabled<R: Runtime>(
    app: AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    info!("api_set_meeting_autodetect_enabled: {}", enabled);
    let store = app
        .store(STORE_FILE)
        .map_err(|e| format!("Failed to access store: {}", e))?;
    store.set(STORE_KEY, serde_json::json!(enabled));
    store
        .save()
        .map_err(|e| format!("Failed to save store: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_enabled_reads_store_key() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!persisted_enabled(dir.path()));
        std::fs::write(dir.path().join(STORE_FILE), r#"{"meeting_autodetect": true}"#).unwrap();
        assert!(persisted_enabled(dir.path()));
        std::fs::write(dir.path().join(STORE_FILE), r#"{"meeting_autodetect": false}"#).unwrap();
        assert!(!persisted_enabled(dir.path()));
    }
}
