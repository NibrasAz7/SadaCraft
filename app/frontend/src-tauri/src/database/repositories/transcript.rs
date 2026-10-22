use crate::api::{TranscriptSearchResult, TranscriptSegment};
use chrono::Utc;
use serde::Serialize;
use sqlx::{Connection, Error as SqlxError, SqlitePool};
use tracing::{error, info};
use uuid::Uuid;

/// Per-speaker statistics for a meeting, derived from transcript rows.
#[derive(Debug, Clone, Serialize)]
pub struct SpeakerStat {
    /// Speaker label stored on the transcript row ("You", "Remote", "Remote-N" or a custom name)
    pub speaker: String,
    /// Number of transcript segments attributed to this speaker
    pub line_count: i64,
    /// Approximate talk time in seconds, derived from segment timestamps
    pub talk_time_seconds: f64,
}

pub struct TranscriptsRepository;

impl TranscriptsRepository {
    /// Saves a new meeting and its associated transcript segments.
    /// This function uses a transaction to ensure that either both the meeting
    /// and all its transcripts are saved, or none of them are.
    pub async fn save_transcript(
        pool: &SqlitePool,
        meeting_title: &str,
        transcripts: &[TranscriptSegment],
        folder_path: Option<String>,
    ) -> Result<String, SqlxError> {
        let meeting_id = format!("meeting-{}", Uuid::new_v4());

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let now = Utc::now();

        // 1. Create the new meeting
        let result = sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at, folder_path) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&meeting_id)
        .bind(meeting_title)
        .bind(now)
        .bind(now)
        .bind(&folder_path)
        .execute(&mut *transaction)
        .await;

        if let Err(e) = result {
            error!("Failed to create meeting '{}': {}", meeting_title, e);
            transaction.rollback().await?;
            return Err(e);
        }

        info!("Successfully created meeting with id: {}", meeting_id);

        // 2. Save each transcript segment with audio timing fields
        for segment in transcripts {
            let transcript_id = format!("transcript-{}", Uuid::new_v4());
            let result = sqlx::query(
                "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&transcript_id)
            .bind(&meeting_id)
            .bind(&segment.text)
            .bind(&segment.timestamp)
            .bind(segment.audio_start_time)
            .bind(segment.audio_end_time)
            .bind(segment.duration)
            .bind(&segment.speaker)
            .execute(&mut *transaction)
            .await;

            if let Err(e) = result {
                error!(
                    "Failed to save transcript segment for meeting {}: {}",
                    meeting_id, e
                );
                transaction.rollback().await?;
                return Err(e);
            }
        }

        info!(
            "Successfully saved {} transcript segments for meeting {}",
            transcripts.len(),
            meeting_id
        );

        // Commit the transaction
        transaction.commit().await?;

        Ok(meeting_id)
    }

    /// Searches for a query string within the transcripts.
    /// It returns a list of matching transcripts with context.
    pub async fn search_transcripts(
        pool: &SqlitePool,
        query: &str,
    ) -> Result<Vec<TranscriptSearchResult>, SqlxError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let search_query = format!("%{}%", query.to_lowercase());

        let rows = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT m.id, m.title, t.transcript, t.timestamp
             FROM meetings m
             JOIN transcripts t ON m.id = t.meeting_id
             WHERE LOWER(t.transcript) LIKE ?",
        )
        .bind(&search_query)
        .fetch_all(pool)
        .await?;

        let results = rows
            .into_iter()
            .map(|(id, title, transcript, timestamp)| {
                let match_context = Self::get_match_context(&transcript, query);
                TranscriptSearchResult {
                    id,
                    title,
                    match_context,
                    timestamp,
                }
            })
            .collect();

        Ok(results)
    }

    /// Returns per-speaker line counts and talk time for a meeting.
    /// Talk time is derived from each segment's `duration`, falling back to
    /// `audio_end_time - audio_start_time` when duration is missing.
    /// Rows without a speaker label (pre-diarization data) are skipped.
    pub async fn get_speaker_stats(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Vec<SpeakerStat>, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let rows = sqlx::query_as::<_, (String, i64, f64)>(
            "SELECT speaker, COUNT(*) AS line_count,
                    COALESCE(SUM(COALESCE(duration, audio_end_time - audio_start_time, 0)), 0) AS talk_time_seconds
             FROM transcripts
             WHERE meeting_id = ? AND speaker IS NOT NULL AND speaker != ''
             GROUP BY speaker
             ORDER BY talk_time_seconds DESC",
        )
        .bind(meeting_id)
        .fetch_all(pool)
        .await?;

        let stats = rows
            .into_iter()
            .map(|(speaker, line_count, talk_time_seconds)| SpeakerStat {
                speaker,
                line_count,
                talk_time_seconds,
            })
            .collect();

        Ok(stats)
    }

    /// Renames a speaker label across all transcript rows of a meeting.
    /// Returns the number of rows updated.
    pub async fn rename_speaker(
        pool: &SqlitePool,
        meeting_id: &str,
        old_label: &str,
        new_label: &str,
    ) -> Result<u64, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let result = sqlx::query(
            "UPDATE transcripts SET speaker = ? WHERE meeting_id = ? AND speaker = ?",
        )
        .bind(new_label)
        .bind(meeting_id)
        .bind(old_label)
        .execute(pool)
        .await?;

        info!(
            "Renamed speaker '{}' to '{}' on {} transcript rows for meeting {}",
            old_label,
            new_label,
            result.rows_affected(),
            meeting_id
        );

        Ok(result.rows_affected())
    }

    /// Helper function to extract a snippet of text around the first match of a query.
    fn get_match_context(transcript: &str, query: &str) -> String {
        let transcript_lower = transcript.to_lowercase();
        let query_lower = query.to_lowercase();

        match transcript_lower.find(&query_lower) {
            Some(match_index) => {
                let start_index = match_index.saturating_sub(100);
                let end_index = (match_index + query.len() + 100).min(transcript.len());

                let mut context = String::new();
                if start_index > 0 {
                    context.push_str("...");
                }
                context.push_str(&transcript[start_index..end_index]);
                if end_index < transcript.len() {
                    context.push_str("...");
                }
                context
            }
            None => transcript.chars().take(200).collect(), // Fallback to the start of the transcript
        }
    }
}

/// A timestamped in-meeting marker, optionally pointing at a captured slide
/// frame (`image_path`). `offset_seconds` is recording-relative so a bookmark
/// can drive audio seek on the meeting detail view; it is NULL for bookmarks
/// created outside a live recording.
#[derive(Debug, Clone, Serialize)]
pub struct Bookmark {
    pub id: String,
    pub meeting_id: String,
    pub label: Option<String>,
    /// Seconds from recording start (same clock as `Transcript::audio_start_time`)
    pub offset_seconds: Option<f64>,
    /// Absolute path to a captured slide frame (PNG under app-data/slides/),
    /// if this bookmark was created via `capture_frame_note`
    pub image_path: Option<String>,
    pub created_at: String,
}

pub struct BookmarksRepository;

impl BookmarksRepository {
    /// Inserts a bookmark row and returns it.
    pub async fn add_bookmark(
        pool: &SqlitePool,
        meeting_id: &str,
        label: Option<&str>,
        offset_seconds: Option<f64>,
        image_path: Option<&str>,
    ) -> Result<Bookmark, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let id = format!("bookmark-{}", Uuid::new_v4());
        let created_at = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO bookmarks (id, meeting_id, label, offset_seconds, image_path, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(meeting_id)
        .bind(label)
        .bind(offset_seconds)
        .bind(image_path)
        .bind(&created_at)
        .execute(pool)
        .await?;

        Ok(Bookmark {
            id,
            meeting_id: meeting_id.to_string(),
            label: label.map(|l| l.to_string()),
            offset_seconds,
            image_path: image_path.map(|p| p.to_string()),
            created_at,
        })
    }

    /// Lists bookmarks for a meeting ordered by recording offset.
    pub async fn list_bookmarks(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Vec<Bookmark>, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let rows = sqlx::query_as::<_, (String, String, Option<String>, Option<f64>, Option<String>, String)>(
            "SELECT id, meeting_id, label, offset_seconds, image_path, created_at
             FROM bookmarks
             WHERE meeting_id = ?
             ORDER BY COALESCE(offset_seconds, 9e18) ASC, created_at ASC",
        )
        .bind(meeting_id)
        .fetch_all(pool)
        .await?;

        let bookmarks = rows
            .into_iter()
            .map(|(id, meeting_id, label, offset_seconds, image_path, created_at)| Bookmark {
                id,
                meeting_id,
                label,
                offset_seconds,
                image_path,
                created_at,
            })
            .collect();

        Ok(bookmarks)
    }

    /// Deletes a bookmark row. Returns true if a row was removed.
    pub async fn delete_bookmark(pool: &SqlitePool, id: &str) -> Result<bool, SqlxError> {
        if id.trim().is_empty() {
            return Err(SqlxError::Protocol("bookmark id cannot be empty".to_string()));
        }

        let result = sqlx::query("DELETE FROM bookmarks WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}
