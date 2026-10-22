//! Meeting export — Markdown, DOCX, and PDF (via print-styled HTML).
//!
//! PDF is exported as a print-optimized HTML file opened in the default
//! browser so the OS print pipeline handles fonts and RTL text shaping
//! (Rust PDF crates do not shape Arabic correctly). The user picks
//! "Save as PDF" in the browser print dialog.

use crate::api::MeetingDetails;
use crate::database::repositories::meeting::MeetingsRepository;
use crate::database::repositories::summary::SummaryProcessesRepository;
use crate::state::AppState;
use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::DialogExt;
use tracing::info as log_info;

/// One rendered summary block: (type, content) where type is "bullet",
/// "numbered", or "text".
struct SummarySection {
    title: String,
    blocks: Vec<(String, String)>,
}

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

/// Parse `summary_processes.result` JSON into ordered sections.
/// Accepts either a `{...sections...}` map or a wrapper with a `summary` key.
fn parse_summary(result: &Option<String>) -> Vec<SummarySection> {
    let Some(raw) = result.as_deref() else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    let map = value
        .get("summary")
        .and_then(|v| v.as_object())
        .or_else(|| value.as_object());
    let Some(map) = map else { return Vec::new() };

    map.iter()
        .filter_map(|(_, section)| {
            let title = section.get("title")?.as_str()?.to_string();
            let blocks = section
                .get("blocks")
                .and_then(|b| b.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|b| {
                            Some((
                                b.get("type").and_then(|t| t.as_str()).unwrap_or("text").to_string(),
                                b.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string(),
                            ))
                        })
                        .filter(|(_, c)| !c.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            Some(SummarySection { title, blocks })
        })
        .filter(|s| !s.blocks.is_empty())
        .collect()
}

fn render_markdown(meeting: &MeetingDetails, sections: &[SummarySection]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", meeting.title));
    out.push_str(&format!("*Recorded: {}*\n\n", meeting.created_at));

    if !sections.is_empty() {
        out.push_str("## Summary\n\n");
        for section in sections {
            out.push_str(&format!("### {}\n\n", section.title));
            for (kind, content) in &section.blocks {
                match kind.as_str() {
                    "bullet" => out.push_str(&format!("- {}\n", content)),
                    "numbered" => out.push_str(&format!("1. {}\n", content)),
                    _ => out.push_str(&format!("{}\n", content)),
                }
            }
            out.push('\n');
        }
    }

    out.push_str("## Transcript\n\n");
    for t in &meeting.transcripts {
        let time = t
            .audio_start_time
            .map(fmt_seconds)
            .unwrap_or_else(|| t.timestamp.clone());
        let speaker = t.speaker.as_deref().unwrap_or("");
        let speaker = if speaker.is_empty() {
            String::new()
        } else {
            format!(" {}:", speaker)
        };
        let text = t.text.replace('\n', " ");
        out.push_str(&format!("- **[{}]{}** {}\n", time, speaker, text));
    }
    out
}

fn render_docx(
    meeting: &MeetingDetails,
    sections: &[SummarySection],
    path: &std::path::Path,
) -> Result<(), String> {
    use docx_rs::*;
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;

    let mut doc = Docx::new().add_paragraph(
        Paragraph::new().add_run(Run::new().add_text(&meeting.title).size(48).bold()),
    );
    doc = doc.add_paragraph(
        Paragraph::new()
            .add_run(Run::new().add_text(format!("Recorded: {}", meeting.created_at)).size(20)),
    );

    if !sections.is_empty() {
        doc = doc.add_paragraph(
            Paragraph::new().add_run(Run::new().add_text("Summary").size(32).bold()),
        );
        for section in sections {
            doc = doc.add_paragraph(
                Paragraph::new()
                    .add_run(Run::new().add_text(&section.title).size(24).bold()),
            );
            for (kind, content) in &section.blocks {
                let prefix = match kind.as_str() {
                    "bullet" => "• ",
                    _ => "",
                };
                doc = doc.add_paragraph(
                    Paragraph::new().add_run(Run::new().add_text(format!("{}{}", prefix, content))),
                );
            }
        }
    }

    doc = doc.add_paragraph(
        Paragraph::new().add_run(Run::new().add_text("Transcript").size(32).bold()),
    );
    for t in &meeting.transcripts {
        let time = t
            .audio_start_time
            .map(fmt_seconds)
            .unwrap_or_else(|| t.timestamp.clone());
        let mut head = format!("[{}]", time);
        if let Some(speaker) = t.speaker.as_deref().filter(|s| !s.is_empty()) {
            head.push_str(&format!(" {}:", speaker));
        }
        doc = doc.add_paragraph(
            Paragraph::new()
                .add_run(Run::new().add_text(head).bold().size(20))
                .add_run(Run::new().add_text(format!(" {}", t.text)).size(20)),
        );
    }

    doc.build()
        .pack(file)
        .map_err(|e| format!("Failed to write DOCX: {}", e))?;
    Ok(())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn render_html(meeting: &MeetingDetails, sections: &[SummarySection]) -> String {
    let mut body = String::new();
    body.push_str(&format!("<h1>{}</h1>", html_escape(&meeting.title)));
    body.push_str(&format!(
        "<p class=\"meta\">Recorded: {}</p>",
        html_escape(&meeting.created_at)
    ));

    if !sections.is_empty() {
        body.push_str("<h2>Summary</h2>");
        for section in sections {
            body.push_str(&format!("<h3>{}</h3>", html_escape(&section.title)));
            body.push_str("<ul>");
            for (kind, content) in &section.blocks {
                match kind.as_str() {
                    "bullet" | "numbered" => body.push_str(&format!(
                        "<li dir=\"auto\">{}</li>",
                        html_escape(content)
                    )),
                    _ => body.push_str(&format!(
                        "<p dir=\"auto\">{}</p>",
                        html_escape(content)
                    )),
                }
            }
            body.push_str("</ul>");
        }
    }

    body.push_str("<h2>Transcript</h2>");
    for t in &meeting.transcripts {
        let time = t
            .audio_start_time
            .map(fmt_seconds)
            .unwrap_or_else(|| t.timestamp.clone());
        let speaker = t
            .speaker
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!(" {}:", html_escape(s)))
            .unwrap_or_default();
        body.push_str(&format!(
            "<p class=\"seg\" dir=\"auto\"><span class=\"ts\">[{}]{}</span> {}</p>",
            html_escape(&time),
            speaker,
            html_escape(&t.text.replace('\n', " "))
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>{title}</title>
<style>
body {{ font-family: system-ui, "Segoe UI", Arial, sans-serif; max-width: 720px; margin: 40px auto; padding: 0 20px; color: #1a1a1a; line-height: 1.55; }}
h1 {{ font-size: 26px; margin-bottom: 4px; }}
.meta {{ color: #666; font-size: 13px; margin-top: 0; }}
h2 {{ font-size: 19px; border-bottom: 1px solid #ddd; padding-bottom: 4px; margin-top: 32px; }}
h3 {{ font-size: 15px; margin-bottom: 4px; }}
ul {{ margin-top: 4px; }}
.seg {{ margin: 2px 0; font-size: 14px; }}
.ts {{ font-weight: 600; color: #444; }}
@media print {{ body {{ margin: 0; }} }}
</style></head><body>{body}</body></html>"#,
        title = html_escape(&meeting.title),
        body = body
    )
}

/// Render a meeting to Markdown. Shared with the `sadacraft-cli` binary.
/// `summary_result` is the raw `summary_processes.result` JSON blob.
pub fn render_meeting_markdown(
    meeting: &MeetingDetails,
    summary_result: &Option<String>,
) -> String {
    render_markdown(meeting, &parse_summary(summary_result))
}

/// Render a meeting to a `.docx` file. Shared with the `sadacraft-cli` binary.
pub fn write_meeting_docx(
    meeting: &MeetingDetails,
    summary_result: &Option<String>,
    path: &std::path::Path,
) -> Result<(), String> {
    render_docx(meeting, &parse_summary(summary_result), path)
}

/// Render a meeting to print-optimized HTML (the app's "PDF" export path —
/// opened in a browser so the OS print pipeline produces the PDF). Shared
/// with the `sadacraft-cli` binary.
pub fn render_meeting_html(
    meeting: &MeetingDetails,
    summary_result: &Option<String>,
) -> String {
    render_html(meeting, &parse_summary(summary_result))
}

/// Export a meeting to Markdown (.md), Word (.docx), or PDF (via browser
/// print of a generated HTML document). Opens a native save dialog;
/// returns the written path, or `None` if the user cancelled.
#[tauri::command]
pub async fn export_meeting<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    format: String,
    state: tauri::State<'_, AppState>,
) -> Result<Option<String>, String> {
    log_info!(
        "export_meeting called for meeting_id: {}, format: {}",
        meeting_id,
        format
    );

    let ext = match format.as_str() {
        "md" => "md",
        "docx" => "docx",
        "pdf" => "html",
        other => return Err(format!("Unsupported export format: {}", other)),
    };

    let pool = state.db_manager.pool();
    let meeting = MeetingsRepository::get_meeting(pool, &meeting_id)
        .await
        .map_err(|e| format!("Failed to retrieve meeting: {}", e))?
        .ok_or_else(|| format!("Meeting not found: {}", meeting_id))?;

    let summary = SummaryProcessesRepository::get_summary_data(pool, &meeting_id)
        .await
        .ok()
        .flatten()
        .and_then(|p| p.result);
    let sections = parse_summary(&summary);

    let filter_name = match format.as_str() {
        "md" => "Markdown",
        "docx" => "Word Document",
        _ => "PDF (HTML for print)",
    };
    let default_name = format!("{}.{}", sanitize_filename(&meeting.title), ext);

    let app_clone = app.clone();
    let file_path = tokio::task::spawn_blocking(move || {
        app_clone
            .dialog()
            .file()
            .set_file_name(&default_name)
            .add_filter(filter_name, &[ext])
            .blocking_save_file()
    })
    .await
    .map_err(|e| format!("File dialog task failed: {}", e))?;

    let Some(file_path) = file_path else {
        log_info!("Export cancelled by user");
        return Ok(None);
    };
    let path = file_path
        .as_path()
        .ok_or_else(|| "Save dialog returned a non-filesystem path".to_string())?
        .to_path_buf();

    match format.as_str() {
        "md" => std::fs::write(&path, render_markdown(&meeting, &sections))
            .map_err(|e| format!("Failed to write file: {}", e))?,
        "docx" => render_docx(&meeting, &sections, &path)?,
        _ => {
            std::fs::write(&path, render_html(&meeting, &sections))
                .map_err(|e| format!("Failed to write file: {}", e))?;
            // Open in the default browser so the user can print → Save as PDF.
            #[cfg(target_os = "windows")]
            {
                let _ = std::process::Command::new("cmd")
                    .args(&["/C", "start", "", &path.to_string_lossy()])
                    .spawn();
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("open").arg(&path).spawn();
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
            }
        }
    }

    let path_str = path.to_string_lossy().to_string();
    log_info!("Exported meeting {} to {}", meeting_id, path_str);
    Ok(Some(path_str))
}
