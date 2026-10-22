//! `sadacraft-cli` — read-only scripting access to the meetings SQLite DB
//! at `%APPDATA%\com.sadacraft.app\meeting_minutes.sqlite`.
//!
//!   sadacraft-cli meetings list
//!   sadacraft-cli meetings transcript <id>
//!   sadacraft-cli meetings summary <id>
//!   sadacraft-cli meetings export <id> --format md|docx|pdf [--out <path>]

mod common;

use anyhow::{bail, Context, Result};
use app_lib::database::repositories::{
    meeting::MeetingsRepository, summary::SummaryProcessesRepository,
};
use app_lib::export;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "sadacraft-cli",
    version,
    about = "SadaCraft meetings CLI (read-only)"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Inspect stored meetings
    Meetings {
        #[command(subcommand)]
        action: MeetingsAction,
    },
}

#[derive(Subcommand)]
enum MeetingsAction {
    /// List all meetings, newest first
    List,
    /// Print a meeting's transcript
    Transcript {
        /// Meeting id (see `meetings list`)
        id: String,
    },
    /// Print a meeting's generated summary (markdown when available)
    Summary {
        /// Meeting id (see `meetings list`)
        id: String,
    },
    /// Export a meeting to a file (markdown, docx, or print-ready HTML)
    Export {
        /// Meeting id (see `meetings list`)
        id: String,
        /// Output format: md, docx, or pdf (pdf writes a print-ready HTML file)
        #[arg(long)]
        format: ExportFormat,
        /// Output file path (defaults to <meeting title>.<ext> in the current directory)
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ExportFormat {
    Md,
    Docx,
    /// Writes print-optimized HTML; open it in a browser and "Save as PDF"
    /// (same pipeline the app uses — Rust PDF crates lack RTL text shaping).
    Pdf,
}

/// Seconds → `HH:MM:SS` / `MM:SS` (mirrors `export::fmt_seconds`).
fn fmt_seconds(t: f64) -> String {
    let total = t.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

fn sanitize_filename(name: &str) -> String {
    let out: String = name
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            _ => c,
        })
        .collect();
    let out = out.trim().to_string();
    if out.is_empty() {
        "meeting".to_string()
    } else {
        out
    }
}

async fn get_meeting_or_err(
    pool: &sqlx::SqlitePool,
    id: &str,
) -> Result<app_lib::api::MeetingDetails> {
    match MeetingsRepository::get_meeting(pool, id).await {
        Ok(Some(meeting)) => Ok(meeting),
        Ok(None) | Err(sqlx::Error::RowNotFound) => bail!("meeting not found: {id}"),
        Err(e) => Err(e).context("failed to load meeting"),
    }
}

async fn list_meetings(pool: &sqlx::SqlitePool) -> Result<()> {
    let meetings = MeetingsRepository::get_meetings(pool)
        .await
        .context("failed to list meetings")?;
    println!("{:<44}  {:<25}  {}", "ID", "CREATED", "TITLE");
    for m in meetings {
        println!("{:<44}  {:<25}  {}", m.id, m.created_at.0.to_rfc3339(), m.title);
    }
    Ok(())
}

async fn print_transcript(pool: &sqlx::SqlitePool, id: &str) -> Result<()> {
    let mut meeting = get_meeting_or_err(pool, id).await?;
    meeting.transcripts.sort_by(|a, b| {
        a.audio_start_time
            .partial_cmp(&b.audio_start_time)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.timestamp.cmp(&b.timestamp))
    });
    for t in &meeting.transcripts {
        let time = t
            .audio_start_time
            .map(fmt_seconds)
            .unwrap_or_else(|| t.timestamp.clone());
        let speaker = t
            .speaker
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!(" {s}:"))
            .unwrap_or_default();
        println!("[{time}]{speaker} {}", t.text.replace('\n', " "));
    }
    Ok(())
}

async fn print_summary(pool: &sqlx::SqlitePool, id: &str) -> Result<()> {
    let process = SummaryProcessesRepository::get_summary_data(pool, id)
        .await
        .context("failed to load summary")?
        .ok_or_else(|| anyhow::anyhow!("no summary process for meeting: {id}"))?;

    eprintln!("status: {}", process.status);
    if let Some(error) = &process.error {
        eprintln!("error: {error}");
    }
    match process
        .result
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
    {
        Some(data) => {
            if let Some(markdown) = data.get("markdown").and_then(|v| v.as_str()) {
                println!("{markdown}");
            } else {
                println!("{}", serde_json::to_string_pretty(&data)?);
            }
        }
        None => println!("(no summary result stored)"),
    }
    Ok(())
}

async fn export_meeting(
    pool: &sqlx::SqlitePool,
    id: &str,
    format: ExportFormat,
    out: Option<PathBuf>,
) -> Result<()> {
    let meeting = get_meeting_or_err(pool, id).await?;
    let summary = SummaryProcessesRepository::get_summary_data(pool, id)
        .await
        .ok()
        .flatten()
        .and_then(|p| p.result);

    let ext = match format {
        ExportFormat::Md => "md",
        ExportFormat::Docx => "docx",
        ExportFormat::Pdf => "html",
    };
    let path = out.unwrap_or_else(|| {
        PathBuf::from(format!("{}.{}", sanitize_filename(&meeting.title), ext))
    });

    match format {
        ExportFormat::Md => std::fs::write(
            &path,
            export::render_meeting_markdown(&meeting, &summary),
        ),
        ExportFormat::Docx => {
            export::write_meeting_docx(&meeting, &summary, &path).map_err(anyhow::Error::msg)?;
            Ok(())
        }
        ExportFormat::Pdf => std::fs::write(
            &path,
            export::render_meeting_html(&meeting, &summary),
        ),
    }
    .with_context(|| format!("failed to write {}", path.display()))?;

    if matches!(format, ExportFormat::Pdf) {
        eprintln!(
            "note: 'pdf' writes print-ready HTML — open {} in a browser and Print → Save as PDF",
            path.display()
        );
    }
    println!("{}", path.display());
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let Commands::Meetings { action } = cli.command;
    let pool = common::connect_read_only().await?;
    match action {
        MeetingsAction::List => list_meetings(&pool).await,
        MeetingsAction::Transcript { id } => print_transcript(&pool, &id).await,
        MeetingsAction::Summary { id } => print_summary(&pool, &id).await,
        MeetingsAction::Export { id, format, out } => {
            export_meeting(&pool, &id, format, out).await
        }
    }
}
