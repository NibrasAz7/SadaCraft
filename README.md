<!-- markdownlint-disable MD033 MD041 -->
<p align="center">
  <img src="assets/app-icon/sadacraft-256.png" width="128" height="128" alt="SadaCraft icon: a microphone emitting echo arcs on a deep indigo tile">
</p>

<h1 align="center">SadaCraft</h1>

<p align="center">
  <b>A meeting companion that listens locally; open source, privacy-first.</b><br>
  Live transcription, AI notes and a virtual camera for<br>
  <b>English · العربية · Italiano</b> meetings — Sada (صدى) means <i>echo</i>:<br>
  your meeting echoes back as transcripts, notes and a smarter camera.<br>
  Windows first
</p>

<p align="center">
  <img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-2b264f?style=flat-square">
  <img alt="Written in Rust + Tauri" src="https://img.shields.io/badge/written%20in-Rust%20%C2%B7%20Tauri-8a6bd6?style=flat-square&logo=rust&logoColor=white">
  <img alt="Runs on Windows" src="https://img.shields.io/badge/runs%20on-Windows-2b264f?style=flat-square">
  <img alt="Local-first, no cloud" src="https://img.shields.io/badge/local--first-no%20cloud-8a6bd6?style=flat-square">
  <img alt="Status: alpha" src="https://img.shields.io/badge/status-alpha-orange?style=flat-square">
  <br>
  <a href="https://remade.wiki/projects/sadacraft/"><img alt="Listed on Remade" src="https://remade.wiki/badges/listed-on-remade.svg"></a>
</p>

<p align="center">
  <a href="#quick-start">Quick start</a> ·
  <a href="#highlights">Highlights</a> ·
  <a href="#whats-here">What's here</a> ·
  <a href="#status">Status</a> ·
  <a href="#the-key-design-decision">Design</a> ·
  <a href="#license">License</a>
</p>

---

## Highlights

<table>
<tr>
<td width="33%" valign="top">

### Local transcription
whisper.cpp via `whisper-rs` with **Vulkan GPU acceleration** (verified on RTX 5070; CUDA path staged) — EN/AR/IT validated, `large-v3-turbo` recommended for Arabic.

</td>
<td width="33%" valign="top">

### Notes that improve
Local llama-helper sidecar or API providers; a **Propose–Probe–Compose** self-improvement loop learns per-template lessons from your edits.

</td>
<td width="33%" valign="top">

### A camera that is yours
**SadaCam** virtual camera streams real webcam frames over shared memory with name-card overlays — the media layer you own, not a platform hook.

</td>
</tr>
</table>

## What's here

| Path           | What it is                                                                                                                          |
| -------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `BLUEPRINT.md` | The product design — audio-first transcription, virtual camera, spotlight overlay, build-on-open-source plan                          |
| `app/`         | The Tauri 2 desktop app (Rust backend + Next.js frontend), derived from Meetily (MIT)                                                  |
| `app/sadacam/` | `MFCreateVirtualCamera` FrameServer host + `sadacam-cli` producer over a `Global\SadaCamFrame` shared-memory slot (BGRX/NV12)           |
| `tools/`       | Build/check scripts (Vulkan + CUDA toolchain helpers)                                                                                  |

Research reports (`research/`) that informed the design — behavioral analysis of caption-interception and user-mode virtual-camera approaches — are **not published**. No proprietary code, binaries, or assets from the original products are used or needed.

## Quick start

```console
cd app/frontend
pnpm install
pnpm run tauri:dev          # full desktop app (dev mode)
pnpm run tauri:dev:vulkan   # GPU-accelerated whisper build
```

Requires Windows 10/11, [pnpm](https://pnpm.io), a Rust toolchain, and Visual Studio Build Tools with the C++ workload. Whisper models download on first use.

## Status

Working alpha, Windows-focused:

- **Transcription** — local whisper.cpp, Vulkan GPU verified, CUDA 13.4+ staged
- **Speaker attribution** — mic → `You`, system audio → `Remote`; optional ONNX speaker clustering (`SADACRAFT_DIARIZATION=1`) labels remote speakers `Remote-N`
- **Summaries** — local llama-helper sidecar (bundled GGUF) or API providers; Propose–Probe–Compose lessons loop (`summary_lessons.json`)
- **Exports** — Markdown, DOCX, and print-styled HTML→PDF (browser path chosen for correct Arabic shaping)
- **SadaCam** — "SadaCraft Camera" virtual camera streaming webcam via shared memory (synthetic-bars fallback); producer reconnect + NV12 negotiation

Not yet: ONNX background segmentation, teleprompter/subtitle overlays, per-client format routing beyond MF negotiation.

## The key design decision

Caption scraping needs fragile per-platform hooks; owning the media layer doesn't. SadaCraft owns the audio (and video) layer itself, so platform hooks become optional enrichment — real speaker names — never a dependency.

## License

`MIT OR Apache-2.0` for SadaCraft-original code (`LICENSE-MIT`, `LICENSE-APACHE`). The transcription base under `app/` derives from [Meetily](https://github.com/Zackriya-Solutions/meetily) and retains its MIT license (`app/LICENSE.md`, © Zackriya Solutions). Non-code assets are original or appropriately licensed.
