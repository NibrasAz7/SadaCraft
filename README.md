# SadaCraft — Meeting Companion

Personal meeting companion: camera effects + local transcription + AI notes + screen spotlight, for **English / Arabic / Italian** meetings. Sada (صدى) = "echo" — your meeting echoes back as transcripts, notes, subtitles, and a smarter camera.

## What's here

- **`BLUEPRINT.md`** — the product design: audio-first transcription, virtual camera, spotlight overlay, build-on-open-source plan
- **`app/`** — the Tauri 2 desktop app (Rust backend + Next.js frontend), derived from Meetily (MIT)
- **`app/sadacam/`** — the virtual camera: `MFCreateVirtualCamera`-based FrameServer host + `sadacam-cli` producer over a `Global\SadaCamFrame` shared-memory slot (BGRX/NV12), with webcam capture (Media Foundation) and name-card overlays
- **`tools/`** — build/check scripts (Vulkan + CUDA toolchain helpers)

Research reports (`research/`) that informed the design — behavioral analysis of Tactiq (WebRTC data-channel caption interception) and XSplit VCam (user-mode media pipeline + per-client format negotiation) — are **not published**. No proprietary code, binaries, or assets from the original products are used or needed.

## Status

Working alpha, Windows-focused:

- **Transcription** — local whisper.cpp via `whisper-rs`, **Vulkan GPU acceleration** verified on RTX 5070 (CUDA 13.4+ path staged); EN/AR/IT validated, `large-v3-turbo` recommended for Arabic
- **Speaker attribution** — mic → `You`, system audio → `Remote`; optional ONNX speaker clustering (`wespeaker` embedding, `SADACRAFT_DIARIZATION=1`) labels remote speakers `Remote-N`
- **Summaries** — local llama-helper sidecar (bundled GGUF) or API providers; **Propose–Probe–Compose self-improvement** loop learns per-template lessons (`summary_lessons.json`), toggle in Settings
- **Exports** — Markdown, DOCX, and print-styled HTML→PDF (browser path chosen for correct Arabic shaping)
- **SadaCam** — virtual camera "SadaCraft Camera" streaming real webcam frames via shared memory (synthetic-bars fallback); producer reconnect + NV12 negotiation implemented

Not yet: ONNX background segmentation, teleprompter/subtitle overlays, per-client format routing beyond MF negotiation.

## Key design decision

Tactiq's caption scraping needs fragile per-platform hooks; VCam owns its media layer. SadaCraft follows VCam's instinct — own the audio (and video) layer yourself — so platform hooks become optional enrichment (real speaker names), not a dependency.

## License

`MIT OR Apache-2.0` for SadaCraft-original code (`LICENSE-MIT`, `LICENSE-APACHE`). The transcription base under `app/` derives from Meetily and retains its MIT license (`app/LICENSE.md`, © Zackriya Solutions).
