// Per-app audio capture via WASAPI process loopback (Windows) plus the
// audio-session process listing that feeds the app picker.
//
// Process loopback (ActivateAudioInterfaceAsync with
// AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK on the virtual
// VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK endpoint) captures only the render
// audio of a chosen PID — including child processes — regardless of which
// output endpoint it plays through. Requires Windows 10 build 20348+
// (Windows 11 21H2+ / Server 2022); older systems get a clear error.
// It is per-process, not per-tab: a browser captures all of that app's audio.

use anyhow::Result;
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::super::pipeline::AudioCapture;

/// Minimum OS build for AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK
/// (documented requirement of the Application Loopback API).
#[cfg(target_os = "windows")]
const MIN_PROCESS_LOOPBACK_BUILD: u32 = 20348;

#[derive(Debug, Clone, Serialize)]
pub struct AudioProcessInfo {
    pub pid: u32,
    pub name: String,
    /// Reserved — window titles are not enumerated.
    pub window_title: Option<String>,
}

/// Process names (lowercase exe basename) used for the fallback picker list
/// when WASAPI session enumeration returns nothing.
const AUDIO_APP_NAMES: &[&str] = &[
    "zoom", "zoom.us", "teams", "ms-teams", "webex", "ciscocollabhost",
    "webexmta", "atmgr", "chrome", "msedge", "firefox", "opera", "brave",
    "arc", "discord", "slack", "spotify", "vlc", "mpc-hc64", "mpc-be64",
];

/// Best-effort exe name for a PID via sysinfo (works on every platform).
pub fn process_name_for_pid(pid: u32) -> Option<String> {
    let pid_t = sysinfo::Pid::from_u32(pid);
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(
        sysinfo::ProcessesToUpdate::Some(&[pid_t]),
        true,
    );
    sys.process(pid_t)
        .map(|p| p.name().to_string_lossy().to_string())
}

/// OS build number parsed from sysinfo's long version string
/// (e.g. "Windows 11 Pro 23H2 (22631)").
#[cfg(target_os = "windows")]
fn os_build_number() -> Option<u32> {
    let version = sysinfo::System::long_os_version()?;
    let open = version.rfind('(')?;
    let close = version.rfind(')')?;
    if close <= open {
        return None;
    }
    version[open + 1..close].trim().parse().ok()
}

/// True when the OS is new enough for WASAPI process-loopback capture.
/// Unknown build → optimistic (the activation itself will fail cleanly).
pub fn process_loopback_supported() -> bool {
    #[cfg(target_os = "windows")]
    {
        match os_build_number() {
            Some(build) => build >= MIN_PROCESS_LOOPBACK_BUILD,
            None => true,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// Processes that plausibly produce audio right now.
///
/// Windows: enumerates live WASAPI audio sessions on the default render
/// endpoint (only apps actually making sound), falling back to a curated
/// process-name scan when no sessions are active. Other platforms: error.
pub fn list_audio_processes() -> Result<Vec<AudioProcessInfo>> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::list_audio_processes_windows()
    }
    #[cfg(not(target_os = "windows"))]
    {
        anyhow::bail!("Per-app audio capture is only supported on Windows")
    }
}

/// Activate a WASAPI process-loopback capture for `pid` and pump samples
/// into `capture` until `stop` is set. Runs synchronously — intended to be
/// called on a dedicated blocking thread. Returns once the capture has been
/// started (samples then flow on the COM worker thread) or on failure.
#[cfg(target_os = "windows")]
pub fn activate_process_loopback(
    pid: u32,
    capture: AudioCapture,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    windows_impl::activate_process_loopback(pid, capture, stop)
}

#[cfg(not(target_os = "windows"))]
pub fn activate_process_loopback(
    _pid: u32,
    _capture: AudioCapture,
    _stop: Arc<AtomicBool>,
) -> Result<()> {
    anyhow::bail!("Per-app audio capture is only supported on Windows")
}

// ============================================================================
// Tauri commands
// ============================================================================

/// List processes that can currently produce audio (for the app picker).
/// Tauri command name stays `list_audio_processes`; the inner fn was renamed
/// to avoid a value-namespace clash.
#[tauri::command(rename = "list_audio_processes")]
pub async fn list_audio_processes_command() -> Result<Vec<AudioProcessInfo>, String> {
    list_audio_processes().map_err(|e| e.to_string())
}

/// Whether per-app (process loopback) capture is supported on this system.
#[tauri::command]
pub async fn audio_app_capture_supported() -> bool {
    process_loopback_supported()
}

// ============================================================================
// Windows implementation
// ============================================================================

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;
    use std::collections::HashSet;
    use std::sync::mpsc;
    use windows::core::{implement, Interface};
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::{
        CLSCTX_ALL, CoCreateInstance, CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    /// 16-bit PCM stereo at the pipeline rate — AudioCapture downsamples to
    /// mono and resamples if needed, same as any other system stream.
    const CAPTURE_SAMPLE_RATE: u32 = 48000;
    const CAPTURE_CHANNELS: u16 = 2;
    const CAPTURE_BITS_PER_SAMPLE: u16 = 16;
    const CAPTURE_BLOCK_ALIGN: u16 = CAPTURE_CHANNELS * (CAPTURE_BITS_PER_SAMPLE / 8);
    const ACTIVATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

    /// Raw PROPVARIANT{vt=VT_BLOB, blob={cbSize,pBlobData}} — windows_core's
    /// PROPVARIANT is opaque, so we build the documented C layout ourselves.
    #[repr(C)]
    struct BlobPropVariant {
        vt: u16,
        reserved: [u16; 3],
        cb_size: u32,
        _pad: u32,
        p_blob_data: *mut u8,
    }

    const VT_BLOB: u16 = 65;

    enum ActivationStatus {
        Started,
        Failed(String),
    }

    #[implement(IActivateAudioInterfaceCompletionHandler)]
    struct LoopbackActivationHandler {
        status_tx: mpsc::Sender<ActivationStatus>,
        stop: Arc<AtomicBool>,
        capture: AudioCapture,
    }

    impl IActivateAudioInterfaceCompletionHandler_Impl for LoopbackActivationHandler {
        fn ActivateCompleted(
            &self,
            activateoperation: Option<&IActivateAudioInterfaceAsyncOperation>,
        ) -> windows_core::Result<()> {
            // Everything WASAPI happens here on the COM worker thread so no
            // COM interface ever crosses threads. The pump loop below runs
            // for the lifetime of the recording; the OS holds a reference to
            // this handler until ActivateCompleted returns. Success is
            // reported from inside activate_and_pump once the client starts.
            let operation = match activateoperation {
                Some(op) => op,
                None => {
                    let _ = self
                        .status_tx
                        .send(ActivationStatus::Failed("activation returned no operation".to_string()));
                    return Ok(());
                }
            };
            if let Err(e) = self.activate_and_pump(operation) {
                let _ = self.status_tx.send(ActivationStatus::Failed(e));
            }
            Ok(())
        }
    }

    impl LoopbackActivationHandler {
        fn activate_and_pump(
            &self,
            operation: &IActivateAudioInterfaceAsyncOperation,
        ) -> std::result::Result<(), String> {
            unsafe {
                let mut activate_hr = windows_core::HRESULT::default();
                let mut punk: Option<windows_core::IUnknown> = None;
                operation
                    .GetActivateResult(&mut activate_hr, &mut punk)
                    .map_err(|e| format!("GetActivateResult failed: {}", e))?;
                activate_hr
                    .ok()
                    .map_err(|e| format!("audio interface activation failed: {}", e))?;
                let punk = punk.ok_or_else(|| "activation returned no interface".to_string())?;
                let client: IAudioClient = punk
                    .cast()
                    .map_err(|e| format!("IAudioClient cast failed: {}", e))?;

                let format = WAVEFORMATEX {
                    wFormatTag: WAVE_FORMAT_PCM as u16,
                    nChannels: CAPTURE_CHANNELS,
                    nSamplesPerSec: CAPTURE_SAMPLE_RATE,
                    nAvgBytesPerSec: CAPTURE_SAMPLE_RATE * CAPTURE_BLOCK_ALIGN as u32,
                    nBlockAlign: CAPTURE_BLOCK_ALIGN,
                    wBitsPerSample: CAPTURE_BITS_PER_SAMPLE,
                    cbSize: 0,
                };

                client
                    .Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_LOOPBACK
                            | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                        0,
                        0,
                        &format,
                        None,
                    )
                    .map_err(|e| format!("IAudioClient::Initialize failed: {}", e))?;

                let capture_client: IAudioCaptureClient = client
                    .GetService()
                    .map_err(|e| format!("IAudioCaptureClient unavailable: {}", e))?;

                let event = CreateEventW(None, false, false, None)
                    .map_err(|e| format!("CreateEventW failed: {}", e))?;
                client
                    .SetEventHandle(event)
                    .map_err(|e| format!("SetEventHandle failed: {}", e))?;
                client.Start().map_err(|e| format!("IAudioClient::Start failed: {}", e))?;

                // Tell the caller capture is live, then pump until stopped.
                let _ = self.status_tx.send(ActivationStatus::Started);

                let result = self.pump_loop(&client, &capture_client, event);

                let _ = client.Stop();
                let _ = CloseHandle(event);
                result
            }
        }

        fn pump_loop(
            &self,
            client: &IAudioClient,
            capture_client: &IAudioCaptureClient,
            event: windows::Win32::Foundation::HANDLE,
        ) -> std::result::Result<(), String> {
            unsafe {
                loop {
                    if self.stop.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    let wait = WaitForSingleObject(event, 200);
                    if wait != WAIT_OBJECT_0 {
                        continue;
                    }
                    loop {
                        let packet_frames = capture_client
                            .GetNextPacketSize()
                            .map_err(|e| format!("GetNextPacketSize failed: {}", e))?;
                        if packet_frames == 0 {
                            break;
                        }
                        let mut data: *mut u8 = std::ptr::null_mut();
                        let mut frames: u32 = 0;
                        let mut flags: u32 = 0;
                        capture_client
                            .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                            .map_err(|e| format!("GetBuffer failed: {}", e))?;

                        let sample_count = frames as usize * CAPTURE_CHANNELS as usize;
                        let f32_data: Vec<f32> = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32
                            != 0
                            || data.is_null()
                        {
                            vec![0.0; sample_count]
                        } else {
                            let samples = std::slice::from_raw_parts(
                                data as *const i16,
                                sample_count,
                            );
                            samples
                                .iter()
                                .map(|&s| s as f32 / i16::MAX as f32)
                                .collect()
                        };

                        let release_result = capture_client.ReleaseBuffer(frames);
                        self.capture.process_audio_data(&f32_data);
                        release_result
                            .map_err(|e| format!("ReleaseBuffer failed: {}", e))?;
                    }
                    // Keep the client referenced so the borrow checker sees it
                    // used across the loop.
                    let _ = client;
                }
            }
        }
    }

    /// Enumerate WASAPI audio sessions on the default render endpoint.
    /// Returns (pid, display-ish name) pairs for ACTIVE sessions only.
    fn audio_session_pids() -> Result<Vec<(u32, Option<String>)>> {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            let fresh_init = hr == windows_core::HRESULT(0);
            let result = audio_session_pids_inner();
            if fresh_init {
                CoUninitialize();
            }
            result
        }
    }

    fn audio_session_pids_inner() -> Result<Vec<(u32, Option<String>)>> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| anyhow::anyhow!("CoCreateInstance(MMDeviceEnumerator) failed: {}", e))?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| anyhow::anyhow!("GetDefaultAudioEndpoint failed: {}", e))?;
            let session_manager: IAudioSessionManager2 = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| anyhow::anyhow!("IAudioSessionManager2 activation failed: {}", e))?;
            let sessions = session_manager
                .GetSessionEnumerator()
                .map_err(|e| anyhow::anyhow!("GetSessionEnumerator failed: {}", e))?;
            let count = sessions
                .GetCount()
                .map_err(|e| anyhow::anyhow!("session count failed: {}", e))?;

            let mut out = Vec::new();
            let mut seen = HashSet::new();
            for i in 0..count {
                let session = match sessions.GetSession(i) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                if session.GetState().ok() != Some(AudioSessionStateActive) {
                    continue;
                }
                let control: IAudioSessionControl2 = match session.cast() {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let pid = match control.GetProcessId() {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                if pid == 0 || !seen.insert(pid) {
                    continue;
                }
                let display = control
                    .GetDisplayName()
                    .ok()
                    .map(|pwstr| {
                        let s = pwstr.to_string().unwrap_or_default();
                        windows::Win32::System::Com::CoTaskMemFree(Some(pwstr.as_ptr() as *const _));
                        s
                    })
                    .filter(|s| !s.is_empty());
                out.push((pid, display));
            }
            Ok(out)
        }
    }

    /// Curated fallback: running processes whose exe basename matches a known
    /// audio-producing app (used when no WASAPI sessions are active).
    fn curated_audio_processes() -> Vec<AudioProcessInfo> {
        use sysinfo::ProcessesToUpdate;
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(ProcessesToUpdate::All, true);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for process in sys.processes().values() {
            let raw = process.name().to_string_lossy().to_lowercase();
            let base = raw.strip_suffix(".exe").unwrap_or(&raw);
            if !AUDIO_APP_NAMES.contains(&base) || !seen.insert(process.pid().as_u32()) {
                continue;
            }
            out.push(AudioProcessInfo {
                pid: process.pid().as_u32(),
                name: process.name().to_string_lossy().to_string(),
                window_title: None,
            });
        }
        out
    }

    pub(super) fn list_audio_processes_windows() -> Result<Vec<AudioProcessInfo>> {
        let sessions = audio_session_pids().unwrap_or_default();
        let mut out: Vec<AudioProcessInfo> = sessions
            .into_iter()
            .map(|(pid, display)| AudioProcessInfo {
                pid,
                name: process_name_for_pid(pid)
                    .or(display)
                    .unwrap_or_else(|| format!("PID {}", pid)),
                window_title: None,
            })
            .collect();
        if out.is_empty() {
            out = curated_audio_processes();
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(out)
    }

    pub(super) fn activate_process_loopback(
        pid: u32,
        capture: AudioCapture,
        stop: Arc<AtomicBool>,
    ) -> Result<()> {
        if !process_loopback_supported() {
            anyhow::bail!(
                "Per-app audio capture requires Windows 10 build {} or later",
                MIN_PROCESS_LOOPBACK_BUILD
            );
        }

        let (status_tx, status_rx) = mpsc::channel::<ActivationStatus>();
        let handler: IActivateAudioInterfaceCompletionHandler =
            LoopbackActivationHandler {
                status_tx,
                stop,
                capture,
            }
            .into();

        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            let fresh_init = hr == windows_core::HRESULT(0);

            let mut activation_params = BlobPropVariant {
                vt: VT_BLOB,
                reserved: [0; 3],
                cb_size: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                _pad: 0,
                p_blob_data: std::ptr::null_mut(),
            };
            let mut audio_params = AUDIOCLIENT_ACTIVATION_PARAMS {
                ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
                Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                    ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                        TargetProcessId: pid,
                        ProcessLoopbackMode:
                            PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                    },
                },
            };
            activation_params.p_blob_data = &mut audio_params as *mut _ as *mut u8;

            let activate_result = ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(
                    &activation_params as *const BlobPropVariant
                        as *const windows_core::PROPVARIANT,
                ),
                &handler,
            );

            // The completion handler must stay alive until ActivateCompleted
            // runs; `handler` is held until the status arrives below.
            let result = match activate_result {
                Ok(_operation) => match status_rx.recv_timeout(ACTIVATION_TIMEOUT) {
                    Ok(ActivationStatus::Started) => Ok(()),
                    Ok(ActivationStatus::Failed(e)) => Err(anyhow::anyhow!(e)),
                    Err(_) => Err(anyhow::anyhow!(
                        "Timed out waiting for audio interface activation"
                    )),
                },
                Err(e) => Err(anyhow::anyhow!(
                    "ActivateAudioInterfaceAsync failed: {} (per-app capture needs Windows 10 build {}+)",
                    e,
                    MIN_PROCESS_LOOPBACK_BUILD
                )),
            };

            if fresh_init {
                CoUninitialize();
            }
            result
        }
    }
}
