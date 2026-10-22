# SadaCraft — Design Blueprint

*Sada (صدى, "echo"): your meeting echoes back as transcript, notes, subtitles, and a smarter camera. Personal tool — EN/AR/IT. Synthesizes `research/REPORT-tactiq.md` (WebRTC caption interception), `research/REPORT-vcam.md` (native camera pipeline + virtual-cam driver), and the open-source base survey.*

## Status (as built)

**Working now:**
- **Meetily base** (`app/`, MIT) running end-to-end on Windows/MSVC: mic + WASAPI loopback → Parakeet/Whisper transcription → local Qwen summary (verified live). Fixed along the way: MSVC-target override, NMake CMake wrapper (`msvc-env.cmd`), `llama-helper` sidecar built, **whisper-rs 0.13→0.16** (bindgen 0.72) to survive clang-23 opaque-struct bug.
- **Rebranded** → SadaCraft: crate `sadacraft`, identifier `com.sadacraft.app`, UI strings, tray, `sadacraft-recordings/`; appdata migrated `com.meetily.ai`→`com.sadacraft.app` (models + DB preserved). Remaining: icon/logo art.
- **SadaCam M1** (`app/sadacam/`) — **user-mode virtual camera, verified end-to-end**: Rust COM server (`DllGetClassObject`→`IClassFactory`→`IMFActivate`→`IMFMediaSourceEx`/`IMFMediaStream2`), `MFCreateVirtualCamera` + `Start`, synthetic RGB32 640×480@30 pull-model frames. Enumerates as "SadaCraft Camera"; ffmpeg captured 3 s of live frames. `sadacam-cli install|install-user|start|uninstall`.
- **Speaker labels** (`You`/`Remote`) wired end-to-end: per-stream VAD (mic vs system keep provenance — also fixes overlapped speech being dropped), `TranscriptUpdate.speaker` → `transcripts.speaker` column → UI chips. Deterministic stream attribution, not ML diarization — Remote-N clustering is open.
- **Exports** — MD/DOCX/PDF via `src-tauri/src/export.rs` + `TranscriptButtonGroup` dropdown. PDF = print-styled HTML → browser Save-as-PDF (deliberate: Rust PDF crates don't shape Arabic/RTL correctly).
- **Whisper GPU acceleration** — **Vulkan backend verified** (`--features vulkan`, `app/frontend/dev-vulkan.cmd`): ggml-vulkan compiled + ran on the discrete RTX 5070 (DXGI autodetect → `gpu_device`, env override `SADACRAFT_WHISPER_GPU_DEVICE`), ~0.4 s steady-state for a 10 s IT clip vs ~11 s iGPU (first-run = one-time SPIR-V JIT). `large-v3-turbo` validated for AR (~1.2 s warm) — correct orthography vs small's phonetic mashups. **CUDA path**: nvcc 13.0 crashes on MSVC 14.51 (verified unfixable by flags; CUDA 13.0's own `include\crt\` was also missing — repaired via NVIDIA's `cuda-crt-dev_win-64` conda pkg). **Fix identified**: CUDA **13.2+** supports VS 2026 — `winget upgrade Nvidia.CUDA` → then `tools\cuda-crt\cuda-check-v134.cmd` (staged). Install declined at UAC this session.

**Verified contract gotchas (from M1):** FS validates `MF_DEVICESTREAM_*` attrs on the stream descriptor (category/id/frameserver_shared/framesource_types) → `E_INVALIDARG` if absent; FrameServer runs as **LocalService** → HKLM registration (admin) required, HKCU invisible (`ERROR_PATH_NOT_FOUND`); LocalService can't read user-profile paths → DLL staged at `C:\ProgramData\SadaCraft\sadacam.dll` (`ACCESS_DENIED` otherwise). Samples are **pull-model**: produced synchronously inside `IMFMediaStream::RequestSample`.

## The one decision that shapes everything

**SadaCraft is audio-first, not DOM-first.** Tactiq needs per-platform page hooks because it borrows the platform's captions. SadaCraft captures audio itself (mic + WASAPI loopback) and transcribes locally → works on *every* meeting app (Meet, Teams, Zoom, WebEx, Discord, phone-bridge, whatever), in EN/AR/IT, with zero platform fragility. The Tactiq-style RTC hook becomes an *optional enhancer* (real speaker names from Meet's `collections` channel), not a dependency.

> **Field evidence (user report):** Tactiq lately fails to join meetings and nags for sign-in — exactly the two failure modes the static analysis predicts: (1) auth is delegated to `app.tactiq.io`→Firebase, so a broken handshake/token stalls everything; (2) capture depends on dynamic `world:"MAIN"` injection + Meet's RTC channel labels surviving Google's updates. Both are *cloud/platform couplings SadaCraft eliminates by design* — local audio capture can't "fail to join" and has no account to lose.

## Module map

```
┌────────────────────────── SadaCraft shell (Tauri) ──────────────────────────┐
│                                                                             │
│  AUDIO                VIDEO                     TRANSCRIPT + AI             │
│  ┌───────────────┐   ┌───────────────────────┐   ┌────────────────────────┐ │
│  │ mic (cpal)    │   │ webcam → segmentation │   │ faster-whisper          │ │
│  │ WASAPI loopback│  │ (ONNX/MediaPipe,      │   │ (EN/AR/IT, word        │ │
│  │  → mix/downmix│   │  256px model, GPU)    │   │  timestamps, VAD)      │ │
│  │ → ring buffers│   │ → composite (bg/blur/ │   │ → diarization          │ │
│  │ → [meetily]   │   │  overlay/subtitles)   │   │  (pyannote/Sortformer) │ │
│  └──────┬────────┘   │ → sadacam MF virtual  │   │ → Ollama: summarize/   │ │
│         │            │   cam source (pull)   │   │   actions/translate    │ │
│         │            └───────────────────────┘   └────────────────────────┘ │
│         ▼                                        └────────────────────────┘ │
│  spotlight overlay (always-on-top transparent layer —                       │
│  cursor ring + fading click ripple, user colors/effects, hotkey)            │
│  + optional: Meet/Teams RTC-caption hook (Tactiq-style) for speaker names   │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Build-on-top bases (don't reimplement)

| Piece | Base | License | Note |
|---|---|---|---|
| Meeting transcription engine | **Meetily** (`zackriya-solutions/meetily`) | MIT | **Embedded** at `app/` — running. Whisper/Parakeet, VAD, Ollama summaries, 99+ langs incl. EN/AR/IT + auto-detect |
| Virtual camera | **`MFCreateVirtualCamera`** (Win11 official API) — `app/sadacam/` own impl | Windows API | User-mode COM media source, pull-model — no kernel driver, no signing, no DShow filter. MS `wc-camera` sample at `research/oss/` as contract reference |
| Spotlight overlay | **FocusToolWin** | MIT | C# WPF: fading laser trail, cursor ring, click pulse, dim-spotlight, magnifier — your spec implemented; port/extend |
| Annotation | **ppInk** | MIT | pen/arrow/spot modes, multi-monitor, hotkeys, REST API |
| Mic cleanup | **RNNoise** (`werman/noise-suppression-for-voice`) | GPL-3.0 | VST or embed rnnoise directly (BSD) |
| Reference impl (video pipeline) | **NV Broadcast** | open | cam→ONNX→composite→loopback + Whisper notes — Linux; port the pattern to Windows |
| REA-only references | ZoomIt, Epic Pen, Krisp, SplitCam, VCam, Tactiq | proprietary | mechanisms learned, nothing copied |

## Feature plan

**Phase 1 — the echo (audio → transcript → notes)** — *base landed via Meetily*
1. ✅ Mic + WASAPI loopback capture, per-source mixing (source separation = "me vs everyone else" for free)
2. ✅ Whisper (`whisper-rs 0.16`) + Parakeet engines — `language=auto`, EN/AR/IT native + `auto-translate` (AR output is RTL — UI needs `dir="auto"` blocks)
3. ✅ Diarization — per-stream VAD gives "You / Remote" labels; **Remote-N clustering implemented** (`speaker_cluster.rs`: wespeaker ResNet34 ONNX via bundled `ort`, Kaldi-fbank + online cosine clustering, `SADACRAFT_DIARIZATION=1` gate; smoke-verified 0.92 same/0.33 cross-speaker). Limits: whole-segment granularity, <0.5s stays "Remote"; optional RTC-hook upgrade maps Meet deviceIds → real names
4. Local summary engine (llama-helper + Qwen GGUF bundled) + Ollama/Claude/Groq/OpenRouter connectors: summary, action items, decisions, per-language translation
5. Local archive (SQLite) + "chat with past meetings"

**Phase 2 — the camera**
- **M1 ✅ done**: COM media-source DLL + `MFCreateVirtualCamera` + synthetic RGB32 frames (`app/sadacam/`)
- **M2 ✅ done**: frame handoff via `Global\SadaCamFrame` shared-memory slot — **consumer creates** (FS host/LocalService can create `Global\` objects; interactive users can't), producer opens `GR|GW`, seqlock single-frame latest-wins. `RequestSample` seqlock-reads → falls back to synthetic when no producer. `sadacam-cli feed [--bmp]` verified: BMP frames flowed FS→ffmpeg (`read_seq` tracks consumption). VCam lesson applies: demand-gate holds — slot appears only when FS pulls. Open: per-app format table (`VCamVidSetup.xml`-style: NV12 for Zoom-style, ARGB32 for OBS-style)
- **M3a ✅ done**: real webcam capture → virtual camera (`webcam.rs`: MF enum + `IMFSourceReader`, RGB32/NV12/YUY2→BGRX, skips self device), name-card overlay (`overlay.rs`, 5×7 bitmap font) — `feed --webcam "HP" --name-card "..."` verified through ffmpeg. Robustness landed alongside: producer reconnect on consumer token change (`maybe_reconnect`, throttled 1s probe) + NV12 second media type (BT.601 BGRX→NV12; Zoom-style clients negotiate, RGB32 stays default). **Open**: ONNX selfie-segmentation → bg blur (ort bundled in src-tauri; needs mediapipe selfie-seg model), teleprompter/subtitle overlays

**Phase 2.5 — the AI layer (new, from feature discussion)**
- Speaker diarization UI (who said what)
- Custom summary templates — editor + per-meeting-type presets
- Custom AI-model connector (OpenAI-compatible endpoint field)
- ✅ Auto-detect meetings (process heuristic + WASAPI loopback RMS wake → suggest/auto-start; `src/detect/`)
- ✅ Advanced exports: PDF, DOCX, Markdown — one Tauri command each (PDF via print-styled HTML → browser print, for correct Arabic shaping)
- ✅ Integrations: local MCP server (stdin/JSON-RPC), webhook on meeting-end, `sadacraft-cli` for scripting
- ✅ "Ask your meeting" chat (`summary/chat.rs` + `MeetingChat`)
- ✅ **Self-improving AI** (`summary/self_improve.rs`, concept from google-harness2, not embedded): Propose–Probe–Compose per summary — LLM proposes one rule → final report regenerated with it (probe) → judge picks A/B → winner's markdown kept, rule persisted to `summary_lessons.json` keyed by template (auto-injects, free). Toggle in Settings→Summary + lessons viewer/editor (per-rule delete, clear-all); env `SADACRAFT_SUMMARY_SELF_IMPROVE` also honored. Verified live against real Qwen3.5-4B sidecar. See "Harness²" below.

**Phase 3 — the pointer** — ✅ `app/sadasight` (standalone crate, verified)
9. ✅ Spotlight overlay: transparent always-on-top layer on the *real* screen (ZoomIt/FocusToolWin pattern) — ring, click ripple, laser trail, dim-outside spotlight, annotation pen
10. ✅ Configurable colors/opacity/radius behind toggle hotkeys + persistent config; ✅ keystroke viz (`keys` effect — GDI-rasterized glyph atlas, fading chips anchored to primary-monitor bottom, `Ctrl+Alt+K` / `--effect keys`, modifier combos as `CTRL+X`)

**Stretch (VCam-inspired):** ✅ auto-frame + relighting (unit-tested), ✅ talk-time clock, ✅ bookmarks + slide-screenshot capture, ✅ RNNoise chain (already wired in pipeline). ❌ virtual mic (needs driver — out of scope), ❌ eye-contact correction (needs 3D gaze model — deferred).

## Hard-won lessons folded in from REA

From Tactiq: caption data rides **RTC data channels** (`captions/captions_v2`, `meet_messages`, `collections` device-map) — if we build the optional Meet hook, that's the intercept point (`chrome.scripting` `world:"MAIN"` + `CustomEvent` bridge), NOT DOM scraping. Their channel self-healing (reopen ≤5×, dedupe by `messageId`/`messageVersion`, 500 ms batched flush) is worth copying. Their `languageMismatch` UX pattern informs our auto-detect display.

From VCam: the **whole media pipeline runs in a Node-enabled worker** off the renderer thread; frames move as `VideoFrame.copyTo` CPU buffers; output is demand-gated by client enumeration (runtime-proven: zero frame writes without a consumer); inference happens at low res (180–360px) regardless of output res; pause = circular-buffer replay. Their kernel-driver path (AVStream + `IOCTL_XSplitVideoSendData` + in-driver RGB→YUV converters + per-process format quirks; live-decoded contract: `c05c`/`c004` 16B descriptors → `c001` 1.38 MB 720p-NV12 payload @ ~27 fps, `c080` 210B client heartbeat @ 5 Hz) is *how it's done in production* — we take the concept, not the driver. Windows' official `MFCreateVirtualCamera` user-mode path delivers the same end result without a driver.

From Harness² (`research/oss/google-harness2`, Apache-2.0, arXiv 2610.14224 — *concepts borrowed, code not embedded*): an agent **harness** is everything around the model — system prompt, guardrails, memory, tool guidance, subagents, skills, scripts, plugins. Their **Propose–Probe–Compose** loop refines that harness per task without ground truth (propose edits → probe candidates on the same input → compose the better harness from behavioral contrast); a persistent **improvement harness** accumulates lessons across tasks. SadaCraft adaptation: self-optimizing summary templates (probe variants regenerate on the same transcript, composer picks by contrast — runs on bundled Qwen), a lessons store keyed by meeting type (standup/interview/lecture), and the 8-component taxonomy as the architecture for the meeting agent layer. The framework itself (Python, benchmark adapters, external agent-CLI substrates) is not a dependency — wrong shape for Tauri/Rust.

## Remaining backlog (tracked tasks)

**Phase 2.5 — AI layer — ✅ implemented (cargo check + tsc clean)**
- ✅ Auto-detect meetings: `src/detect/` — process heuristic (Meet/Teams/Zoom) + WASAPI loopback RMS wake, episode debounce, Settings toggle
- ✅ Integrations: local MCP server (stdin/JSON-RPC over meeting DB), meeting-end webhook wired into summary service, `sadacraft-cli` binary
- ✅ "Ask your meeting" chat: `summary/chat.rs` — transcript + stored summary + ≤8-turn history, ~40k-char cap; `MeetingChat` mounted below SummaryPanel
- ✅ Custom summary template editor: `custom_templates.rs` + `TemplateEditor` — CRUD, section reorder, built-in override/reset; per-meeting-type presets
- ✅ Custom OpenAI-compatible endpoint: backend existed; `CustomEndpointSettings` card added in Settings→Summary
- ✅ Speaker panel: `SpeakerPanel` in TranscriptPanel — per-speaker line counts + talk time, persistent rename `Remote-N` → real name (`speaker_commands.rs`, SQL-aggregated)

**SadaCam — ✅ implemented (27/27 tests)**
- ✅ ONNX selfie-segmentation → blur/replace: `segment.rs` via bundled `ort`, mediapipe model downloaded, real inference verified
- ✅ Teleprompter + subtitle overlays (joins name-card)
- ✅ Per-client format routing table (config-driven, extends NV12 negotiation)
- ✅ Auto-frame + relighting (implemented + unit-tested; eye-contact documented as not feasible without a 3D gaze model — deferred)

**Phase 3 — the pointer — ✅ implemented**
- ✅ `app/sadasight` standalone crate (709KB): layered transparent always-on-top overlay — cursor ring, click ripple, laser trail, dim-outside spotlight, annotation pen, keystroke viz (chips + modifier combos, `Ctrl+Alt+K`), hotkeys, persistent config; live smoke-tested incl. `--effect` IPC round-trip.

**Stretch**
- ✅ Talk-time clock: `TalkTimeClock` per-speaker share bar in TranscriptPanel (live + saved meetings)
- ✅ In-meeting bookmarks + slide capture: `bookmark_commands.rs` + `MeetingBookmarks` — pending queue while unsaved → `finalize_pending_bookmarks` on meeting id; GDI BitBlt primary-monitor PNG under appdata `slides/`; SQLite migration `20261020000000_add_bookmarks`
- ✅ RNNoise mic cleanup: already in pipeline (`nnnoiseless`/`NoiseSuppressionProcessor`, `audio/audio_processing.rs`)
- ❌ Virtual mic output — not built (no virtual audio device; would need a driver — out of scope by design)
- ❌ Eye-contact correction — deferred (needs 3D gaze estimation)

**Ops (needs elevation/human — not code tasks)**
- Elevated deploy: `net stop FrameServer` → copy `sadacam.dll` to `C:\ProgramData\SadaCraft\` → `net start FrameServer` (rebuild first: `cargo build -p sadacam`; `C:\st-cam` cache was cleaned)
- CUDA 13.4.1: `winget upgrade Nvidia.CUDA` → `tools\cuda-crt\cuda-check-v134.cmd`
- Live verification: real meeting → `You`/`Remote-N` chips; self-improve → `summary_lessons.json`; real-client NV12 capture; real FrameServer recycle; bookmarks during a live recording; segmentation + teleprompter on a real camera feed

**Repo/polish gaps**
- ✅ Icon/logo art generated (real iconset, wired into `tauri.conf.json`)
- ✅ CI workflow at repo root: `cargo check` all 4 workspace members + `tsc` + Vulkan-SDK job
- ✅ RTL rendering pass: `dir="auto"` on transcript/summary inputs and views
- ✅ Model download UX: Ollama pull progress + cancel
- ✅ First-run onboarding: mic/system permission walkthrough + summary-model picker step
- ✅ Dead legacy `app/.github/` (Meetily workflows/templates, non-functional at nested path) removed
- ⚠️ Release build itself not run (`tauri build` NSIS installer needs a full release compile); code signing not configured; updater pubkey in `tauri.conf.json` is still Meetily's — needs a SadaCraft keypair before enabling the update channel

## Architecture choice

**Tauri** over Electron for the shell (smaller, Rust core owns audio capture — cpal/wasapi + ring buffers in native code, where they belong). The video compositor can still be a WebView canvas. Meetily is already Tauri — embedding stays coherent.

## What we are NOT building

No cloud sync, no accounts, no licensing/Stripe, no telemetry (Tactiq ships Datadog RUM session-replay — SadaCraft ships none), no kernel driver, no per-platform DOM scraping as the primary path. Everything local; the only network calls are optional Ollama (localhost) and model downloads.
