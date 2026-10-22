//! "Ask your meeting" — Q&A over a meeting's transcript using the configured
//! summary LLM provider.
//!
//! Each request loads the meeting's transcript segments and stored summary from
//! SQLite, builds a single-shot prompt, and answers through the same
//! `generate_summary` plumbing used for summary generation (Ollama, Claude,
//! Groq, OpenRouter, OpenAI, BuiltInAI, CustomOpenAI). Conversation state lives
//! client-side; prior Q/A turns are passed back in and inlined into the prompt.

use crate::database::repositories::{
    meeting::MeetingsRepository, setting::SettingsRepository,
    summary::SummaryProcessesRepository,
};
use crate::state::AppState;
use crate::summary::llm_client::{generate_summary, LLMProvider};
use crate::summary::processor::clean_llm_markdown_detailed;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};
use tracing::info;

/// Rough character cap on the transcript excerpt so chat prompts stay within
/// reach of small local models.
const MAX_TRANSCRIPT_CHARS: usize = 40_000;
/// Only the most recent Q/A turns are inlined into each prompt.
const MAX_HISTORY_TURNS: usize = 8;

const ASK_SYSTEM_PROMPT: &str = "You are answering questions about a meeting. \
Answer using only the provided transcript (and the summary, when present). \
If the answer is not in the meeting content, say so plainly instead of guessing. \
Keep answers concise.";

/// One prior Q/A exchange, sent back by the client for multi-turn context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingChatTurn {
    pub question: String,
    pub answer: String,
}

/// Keeps the first ~2/3 and last ~1/3 of an over-long transcript so both the
/// meeting's opening context and its closing decisions remain in the prompt.
fn truncate_middle(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head_chars = max_chars * 2 / 3;
    let tail_chars = max_chars - head_chars;
    let head: String = text.chars().take(head_chars).collect();
    let tail: String = text
        .chars()
        .rev()
        .take(tail_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{}\n[... transcript truncated ...]\n{}", head, tail)
}

/// Answers a question about a meeting using the configured summary LLM.
///
/// `history` carries prior Q/A turns (newest last); only the latest
/// `MAX_HISTORY_TURNS` are inlined into the prompt.
#[tauri::command]
pub async fn ask_meeting<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    question: String,
    history: Option<Vec<MeetingChatTurn>>,
) -> Result<String, String> {
    let question = question.trim().to_string();
    if question.is_empty() {
        return Err("Question cannot be empty".to_string());
    }
    info!(
        "ask_meeting called for meeting_id: {}, question len: {}",
        meeting_id,
        question.len()
    );
    let pool = state.db_manager.pool();

    // Load transcript segments in recording order.
    let (segments, _total) = MeetingsRepository::get_meeting_transcripts_paginated(
        pool,
        &meeting_id,
        i64::MAX,
        0,
    )
    .await
    .map_err(|e| format!("Failed to load transcripts for meeting {}: {}", meeting_id, e))?;

    if segments.is_empty() {
        return Err("This meeting has no transcript to ask about.".to_string());
    }

    let mut transcript = String::new();
    for segment in &segments {
        if let Some(speaker) = segment
            .speaker
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            transcript.push_str(speaker);
            transcript.push_str(": ");
        }
        transcript.push_str(segment.transcript.trim());
        transcript.push('\n');
    }
    let transcript = truncate_middle(&transcript, MAX_TRANSCRIPT_CHARS);

    // Load the stored summary markdown, when one exists.
    let summary_markdown = SummaryProcessesRepository::get_summary_data(pool, &meeting_id)
        .await
        .ok()
        .flatten()
        .and_then(|process| process.result)
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| {
            value
                .get("markdown")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        });

    // Resolve the configured summary provider, mirroring service.rs.
    let config = SettingsRepository::get_model_config(pool)
        .await
        .map_err(|e| format!("Failed to load model config: {}", e))?
        .ok_or_else(|| {
            "No summary model configured. Choose a provider in Settings first.".to_string()
        })?;

    let provider = LLMProvider::from_str(&config.provider)?;
    let mut model_name = config.model.clone();

    let mut api_key =
        if matches!(
            provider,
            LLMProvider::Ollama | LLMProvider::BuiltInAI | LLMProvider::CustomOpenAI
        ) {
            // These providers don't use the standard API key columns.
            String::new()
        } else {
            match SettingsRepository::get_api_key(pool, &config.provider).await {
                Ok(Some(key)) if !key.is_empty() => key,
                Ok(_) => {
                    return Err(format!("API key not found for {}", config.provider));
                }
                Err(e) => {
                    return Err(format!(
                        "Failed to retrieve API key for {}: {}",
                        config.provider, e
                    ));
                }
            }
        };

    // Custom OpenAI-compatible endpoint supplies its own endpoint/key/model.
    let mut custom_openai_endpoint = None;
    let mut max_tokens = None;
    let mut temperature = None;
    let mut top_p = None;
    if provider == LLMProvider::CustomOpenAI {
        let custom = SettingsRepository::get_custom_openai_config(pool)
            .await
            .map_err(|e| format!("Failed to retrieve custom OpenAI config: {}", e))?
            .ok_or_else(|| {
                "Custom endpoint selected but not configured".to_string()
            })?;
        custom_openai_endpoint = Some(custom.endpoint);
        api_key = custom.api_key.unwrap_or_default();
        model_name = custom.model;
        max_tokens = custom.max_tokens.map(|t| t as u32);
        temperature = custom.temperature;
        top_p = custom.top_p;
    }

    // Build the single-shot prompt: transcript (+ summary) context, prior
    // turns, then the question.
    let mut user_prompt = format!("Transcript:\n{}\n", transcript);
    if let Some(markdown) = summary_markdown
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        user_prompt.push_str(&format!("\nSummary:\n{}\n", markdown));
    }
    let history = history.unwrap_or_default();
    let recent = &history[history.len().saturating_sub(MAX_HISTORY_TURNS)..];
    if !recent.is_empty() {
        user_prompt.push_str("\nPrevious questions and answers:\n");
        for turn in recent {
            user_prompt.push_str(&format!("Q: {}\nA: {}\n\n", turn.question, turn.answer));
        }
    }
    user_prompt.push_str(&format!("\nQ: {}", question));

    let client = reqwest::Client::new();
    let app_data_dir = app.path().app_data_dir().ok();
    let completion = generate_summary(
        &client,
        &provider,
        &model_name,
        &api_key,
        ASK_SYSTEM_PROMPT,
        &user_prompt,
        config.ollama_endpoint.as_deref(),
        custom_openai_endpoint.as_deref(),
        max_tokens,
        temperature,
        top_p,
        app_data_dir.as_ref(),
        None,
    )
    .await?;

    let answer = clean_llm_markdown_detailed(&completion.content).markdown;
    if answer.trim().is_empty() {
        return Err("The model returned an empty answer".to_string());
    }
    info!("ask_meeting answered for meeting_id: {}", meeting_id);
    Ok(answer)
}
