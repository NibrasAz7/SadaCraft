# SadaCraft transcript format — and FilmCraft interop

This doc pins down two things:

1. **SadaCraft's own transcript schema** — what a recorded meeting's transcript actually
   looks like, in every place it surfaces (DB, `transcripts.json`, live events, MCP).
2. **The FilmCraft `transcript.set` contract** — the exact JSON FilmCraft's engine
   accepts, so a SadaCraft transcript can be converted and handed to FilmCraft without
   the format errors users keep hitting.

Everything below is verified against source: SadaCraft's `app/frontend/src-tauri/` and
FilmCraft's `crates/project/src/transcript.rs`, `crates/engine/src/transcript.rs` and
`docs/transcripts.md`. Where a feature is planned rather than shipped, it says so.

---

## 1. SadaCraft's transcript model

SadaCraft transcribes **segments**, not words. A segment is one VAD-bounded utterance —
typically a phrase or sentence — with recording-relative bounds. (The whisper engine
runs with `no_timestamps(true)` and token timestamps are kept only internally; no
per-word times are emitted today. Word-level output is planned — see §4.)

### `TranscriptSegment` — the canonical segment

`app/frontend/src-tauri/src/api/api.rs` (`MeetingTranscript` is the same shape read back):

| Field | Type | Meaning |
|---|---|---|
| `id` | string | `"transcript-<uuid>"`, unique per segment |
| `text` | string | The utterance text (already repetition-cleaned) |
| `timestamp` | string | RFC 3339 wall-clock time the segment was produced |
| `audio_start_time` | f64, optional | **Seconds from recording start** |
| `audio_end_time` | f64, optional | Seconds from recording start |
| `duration` | f64, optional | `audio_end_time − audio_start_time`, seconds |
| `speaker` | string, optional | `"You"` (mic), `"Remote"` (system audio), `"Remote-N"` (ONNX clustering, `SADACRAFT_DIARIZATION=1`), or a user-renamed label |

All times are **seconds, floating point, recording-relative** — the same clock the
bookmarks table uses (`offset_seconds`). Wall-clock `timestamp` is provenance, not sync.

### Live event — `TranscriptUpdate`

Emitted on the Tauri event `transcript-update` while recording
(`audio/transcription/worker.rs`):

```json
{
  "text": "…",
  "timestamp": "2026-11-22T14:30:05Z",
  "source": "whisper",
  "sequence_id": 12,
  "is_partial": false,
  "confidence": 0.83,
  "audio_start_time": 125.3,
  "audio_end_time": 128.6,
  "duration": 3.3,
  "speaker": "You"
}
```

`is_partial: true` marks in-flight previews; persisted rows are final segments.

### On disk — `transcripts.json` + `metadata.json`

Every meeting gets a folder under `sadacraft-recordings/` containing `audio.mp4`
(when auto-save is on), `metadata.json` and `transcripts.json`:

```json
{
  "version": "1.0",
  "segments": [ { "id": "…", "text": "…", "timestamp": "…",
                  "audio_start_time": 125.3, "audio_end_time": 128.6,
                  "duration": 3.3, "speaker": "You" } ],
  "last_updated": "2026-11-22T14:31:22Z",
  "total_segments": 42
}
```

> ⚠️ **Two writers exist.** The live-recording path (`recording_saver.rs`) serializes
> full `TranscriptSegment`s — `speaker` included when set. The import/re-transcription
> path (`audio/common.rs::write_transcripts_json`) writes a subset — `id`, `text`,
> `timestamp`, `audio_*`, `duration`, `sequence_id` — and **drops `speaker`**. Readers
> must treat `speaker` as optional. (Known inconsistency; unifying is a candidate fix.)

`metadata.json` (written by `recording_saver.rs`): `version`, `meeting_id`,
`meeting_name`, `created_at`, `completed_at`, `duration_seconds`, `devices`
(`microphone`, `system_audio`), `audio_file`, `transcript_file`, `sample_rate` (48000),
`status` (`"recording"` → `"completed"`).

### In the DB — `transcripts` table

`meeting_minutes.sqlite` under `%APPDATA%\com.sadacraft.app\`:

```sql
transcripts(id TEXT PK, meeting_id TEXT FK→meetings.id, transcript TEXT,
            timestamp TEXT,                      -- RFC 3339 wall clock
            audio_start_time REAL, audio_end_time REAL, duration REAL,  -- seconds
            speaker TEXT)                        -- nullable
```

### Programmatic access (for sibling apps / agents)

| Surface | What you get |
|---|---|
| `sadacraft-mcp` (stdio JSON-RPC, read-only) | `list_meetings`, `get_meeting`, `get_transcript` (segments sorted by `audio_start_time`), `get_summary`, `search_transcripts` |
| `sadacraft-cli` | scripted reads over the same DB |
| Meeting-end webhook | POST fired when a recording finishes |
| Tauri commands/events | the in-app surface (`transcript-update`, `save_transcript`, `get_meeting_transcripts`, …) |

---

## 2. FilmCraft's `transcript.set` contract

FilmCraft's release builds ship **without** the `whisper` feature, so
`transcript.generate` fails with *"speech-to-text is not available in this build"*
and the supported path is importing a transcript via `transcript.set`
(`crates/engine/src/transcript.rs`). This is exactly the wall Discord users hit —
the command is strict about shape, and the errors below all trace to the schema.

### Envelope

```json
{ "item": 4, "transcript": { …Transcript… } }
```

- `item` — u64 **media item id** in the open project (from `project_inspect` /
  `media_import`). Subclip ids resolve to their parent media. A wrong id type or a
  missing item → `` `item` is required `` / `no such media item`.
- `transcript` — the object below. Missing → `` `transcript` is required ``.

### `Transcript`

```json
{
  "language": "en",
  "source": "sadacraft",
  "speakers": [ { "name": "You" }, { "name": "Remote" } ],
  "words": [
    { "text": "Hello,", "start": 0,          "end": 127008000000, "speaker": 0, "confidence": 0.92 },
    { "text": "everyone", "start": 127008000000, "end": 320076000000, "speaker": 0 }
  ]
}
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `language` | string | – | ISO 639-1 code of the spoken language (`"en"`, `"ar"`, `"it"`) |
| `source` | string | – | What produced it; empty becomes `"imported"`. Use `"sadacraft"` (or the whisper model id) |
| `speakers` | `[{name}]` | – | Label list; `word.speaker` indexes it. Missing indices are auto-filled `"Speaker N"` |
| `words` | `[Word]` | – | **Word-level**, time-ordered, non-overlapping |

### `Word`

| Field | Type | Required | Notes |
|---|---|---|---|
| `text` | string | ✅ | One word **with its attached punctuation**, no surrounding spaces (`"Hello,"`) |
| `start` | i64 | ✅ | Start time in **ticks** (see below), media time of the word's first sample |
| `end` | i64 | ✅ | End in ticks, **exclusive** — just after the word |
| `speaker` | u32 | – | 0-based index into `speakers`; omit for unattributed |
| `confidence` | f32 | – | 0.0–1.0, default 1.0 (clamped) |

### ⚠️ Ticks — the #1 format error

FilmCraft times are **`Tick`s: 254 016 000 000 ticks per second**
(`filmcraft_time::TICKS_PER_SECOND`), serialized as bare integers — *not* seconds,
milliseconds or timecode strings.

```
tick = round(seconds × 254_016_000_000)
1 s      → 254016000000
125.3 s  → 31836288000000
```

Sending `"start": 125.3` (a float) is a deserialize error; sending small numbers
("milliseconds") produces a transcript compressed into the first microseconds —
it *imports* but looks broken. Both failure modes are in the wild.

### Times are media time, not sequence time

`start`/`end` are on the **clip's own media timeline** — `0` = first sample of the
media file, regardless of where the clip sits in the sequence. That is what keeps the
transcript valid when the clip is trimmed, moved or reused.

### Server-side normalization (what `transcript.set` fixes for you)

On ingest, FilmCraft runs `Transcript::normalize()` then `check()`:

- words are **sorted** by `(start, end)` — input order doesn't matter;
- empty/whitespace words are **dropped**;
- `end < start` is clamped up to `start`;
- **overlaps are clipped**: a word's `end` is capped at the next word's `start`;
- `confidence` is clamped to 0–1;
- speaker indices beyond `speakers.len()` auto-create `"Speaker N"` labels.

So a sloppy import usually *succeeds but is silently mangled* — the real "format
errors" are the deserialize/validation failures:

| Error | Cause |
|---|---|
| `` `item` is required `` / `no such media item` | `item` missing, not a u64, or no such media item in the project |
| `` `transcript` is required `` | envelope lacks `transcript` |
| `invalid type: …` (serde) | wrong JSON shape — `"words": 3`, float `start`, `"speakers": ["You"]` (needs `{"name":…}`), etc. |
| `word N ends before it starts` / `overlaps the word before it` | invariant violation after normalize (rare — normalize repairs first) |

### How to reach it

| Path | Call |
|---|---|
| Control channel (`filmcraft --control 9876`) | `{"method":"engine.execute","params":{"command":"transcript.set","params":{…}}}` |
| MCP (`filmcraft-cli mcp`) | `command_run {"id":"transcript.set","params":{…}}` |
| CLI | `filmcraft-cli --project p.fcproj exec transcript.set '{…}'` |
| UI | Text panel ▸ Transcript ▸ Import Transcript |

Once set, `transcript.createCaptions` lays the words out on a caption track —
the path from imported transcript to subtitles.

---

## 3. SadaCraft → FilmCraft mapping

| SadaCraft | FilmCraft `transcript.set` |
|---|---|
| `audio_start_time` / `audio_end_time` (f64 s) | `word.start` / `word.end`: `round(s × 254016000000)`, end exclusive |
| `speaker` string (`"You"`, `"Remote-2"`, renamed) | `speakers[i].name` + `word.speaker = i` (first-seen order) |
| — | `language`: from recording settings / detection (`"en"`, `"ar"`, `"it"`) |
| — | `source`: `"sadacraft"` (or the whisper model id) |
| segment `text` | split into `words[]` (see below) |

**Granularity bridge (needed until word-level ships):** SadaCraft segments are
utterance-level. To satisfy FilmCraft's word-level schema, split each segment's `text`
on whitespace and distribute the words **evenly** across
`[audio_start_time, audio_end_time)` — order-preserving, non-overlapping, so it passes
`check()` unmodified. Times between words are then approximate (even spacing), which is
acceptable for captioning and text-based editing but not for precise word karaoke;
that needs real word timestamps (§4).

**Import direction** (FilmCraft → SadaCraft): the inverse mapping is trivial —
FilmCraft words already carry times; group words into utterances on pause gaps
(≥ ~0.5 s) and write `TranscriptSegment`s. A FilmCraft transcript can therefore seed a
SadaCraft meeting archive too.

---

## 4. Status

| Capability | State |
|---|---|
| Segment-level transcripts (schema §1) | ✅ shipped — recording, import, re-transcription paths |
| Speaker labels on segments | ✅ shipped (`You`/`Remote`; `Remote-N` behind `SADACRAFT_DIARIZATION=1`) |
| `transcripts.json` writer emitting `speaker` | ⚠️ partial — live-recording path only (see §1) |
| MCP/CLI/webhook transcript access | ✅ shipped |
| Word-level timestamps | 🔜 planned — unlocks exact FilmCraft `words` without even-spacing |
| SadaCraft → `transcript.set` export command | 🔜 planned — mapping in §3 |
| SRT/VTT export | 🔜 planned |
| Editable transcript / read-aloud | 🔜 planned |
| TTS (Piper-class, local) | 🔜 planned — not in the codebase today |
