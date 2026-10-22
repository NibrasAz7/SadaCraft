-- Add bookmarks table for in-meeting timestamp markers and slide captures.
-- offset_seconds is recording-relative (matches transcripts.audio_start_time);
-- NULL for bookmarks created after the meeting ended (no live clock to capture).
-- image_path optionally points at a captured slide frame (PNG under app-data/slides/).
CREATE TABLE IF NOT EXISTS bookmarks (
    id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL,
    label TEXT,
    offset_seconds REAL,
    image_path TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);

-- Create index for faster lookups
CREATE INDEX IF NOT EXISTS idx_bookmarks_meeting_id ON bookmarks(meeting_id);
