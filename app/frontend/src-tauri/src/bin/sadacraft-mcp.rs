//! `sadacraft-mcp` — a minimal MCP (Model Context Protocol) server speaking
//! newline-delimited JSON-RPC 2.0 over stdio. Exposes read-only tools over
//! the meetings SQLite DB at `%APPDATA%\com.sadacraft.app\meeting_minutes.sqlite`.
//!
//! Implemented protocol subset: `initialize`, `ping`, `tools/list`,
//! `tools/call`; `notifications/*` are ignored, stdin EOF exits.

mod common;

use anyhow::Result;
use app_lib::database::repositories::{
    meeting::MeetingsRepository, summary::SummaryProcessesRepository,
    transcript::TranscriptsRepository,
};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::io::{BufRead, Write};
use tokio::sync::OnceCell;

static POOL: OnceCell<SqlitePool> = OnceCell::const_new();

async fn pool() -> Result<&'static SqlitePool, String> {
    POOL.get_or_try_init(|| common::connect_read_only())
        .await
        .map_err(|e| e.to_string())
}

fn tool_schemas() -> Value {
    json!([
        {
            "name": "list_meetings",
            "description": "List all recorded meetings, newest first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "description": "Maximum number of meetings to return" }
                }
            }
        },
        {
            "name": "get_meeting",
            "description": "Get one meeting by id, including all transcript segments.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "meeting_id": { "type": "string" }
                },
                "required": ["meeting_id"]
            }
        },
        {
            "name": "get_transcript",
            "description": "Get the transcript segments of a meeting, ordered by recording time.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "meeting_id": { "type": "string" }
                },
                "required": ["meeting_id"]
            }
        },
        {
            "name": "get_summary",
            "description": "Get the generated summary for a meeting (status plus parsed result data).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "meeting_id": { "type": "string" }
                },
                "required": ["meeting_id"]
            }
        },
        {
            "name": "search_transcripts",
            "description": "Case-insensitive substring search across all meeting transcripts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "description": "Maximum number of matches to return" }
                },
                "required": ["query"]
            }
        }
    ])
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing required argument '{key}'"))
}

fn arg_limit(args: &Value) -> Option<usize> {
    args.get("limit")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
}

async fn call_tool(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "list_meetings" => {
            let mut meetings = MeetingsRepository::get_meetings(pool().await?)
                .await
                .map_err(|e| e.to_string())?;
            if let Some(limit) = arg_limit(args) {
                meetings.truncate(limit);
            }
            serde_json::to_value(&meetings).map_err(|e| e.to_string())
        }
        "get_meeting" => {
            let meeting_id = arg_str(args, "meeting_id")?;
            match MeetingsRepository::get_meeting(pool().await?, meeting_id).await {
                Ok(Some(meeting)) => {
                    serde_json::to_value(&meeting).map_err(|e| e.to_string())
                }
                Ok(None) | Err(sqlx::Error::RowNotFound) => {
                    Err(format!("meeting not found: {meeting_id}"))
                }
                Err(e) => Err(e.to_string()),
            }
        }
        "get_transcript" => {
            let meeting_id = arg_str(args, "meeting_id")?;
            match MeetingsRepository::get_meeting(pool().await?, meeting_id).await {
                Ok(Some(mut meeting)) => {
                    meeting.transcripts.sort_by(|a, b| {
                        a.audio_start_time
                            .partial_cmp(&b.audio_start_time)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| a.timestamp.cmp(&b.timestamp))
                    });
                    Ok(json!({
                        "meeting_id": meeting.id,
                        "title": meeting.title,
                        "transcripts": meeting.transcripts,
                    }))
                }
                Ok(None) | Err(sqlx::Error::RowNotFound) => {
                    Err(format!("meeting not found: {meeting_id}"))
                }
                Err(e) => Err(e.to_string()),
            }
        }
        "get_summary" => {
            let meeting_id = arg_str(args, "meeting_id")?;
            let process = SummaryProcessesRepository::get_summary_data(pool().await?, meeting_id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no summary process for meeting: {meeting_id}"))?;
            let data = process
                .result
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
            Ok(json!({
                "meeting_id": process.meeting_id,
                "status": process.status,
                "data": data,
                "start": process.start_time.map(|t| t.to_rfc3339()),
                "end": process.end_time.map(|t| t.to_rfc3339()),
                "error": process.error,
            }))
        }
        "search_transcripts" => {
            let query = arg_str(args, "query")?;
            let mut results = TranscriptsRepository::search_transcripts(pool().await?, query)
                .await
                .map_err(|e| e.to_string())?;
            if let Some(limit) = arg_limit(args) {
                results.truncate(limit);
            }
            serde_json::to_value(&results).map_err(|e| e.to_string())
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() }
    })
}

async fn handle_request(msg: &Value) -> Option<Value> {
    let id = match msg.get("id") {
        Some(id) => id.clone(),
        None => return None, // notification — never respond
    };
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return Some(rpc_error(id, -32600, "invalid request: missing method"));
    };

    match method {
        "initialize" => {
            let requested = msg
                .get("params")
                .and_then(|p| p.get("protocolVersion"))
                .and_then(Value::as_str);
            Some(rpc_result(
                id,
                json!({
                    "protocolVersion": requested.unwrap_or("2024-11-05"),
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "sadacraft-mcp",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }),
            ))
        }
        "ping" => Some(rpc_result(id, json!({}))),
        "tools/list" => Some(rpc_result(id, json!({ "tools": tool_schemas() }))),
        "tools/call" => {
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match call_tool(name, &args).await {
                Ok(value) => Some(rpc_result(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&value)
                                .unwrap_or_else(|_| value.to_string()),
                        }],
                        "isError": false,
                    }),
                )),
                Err(e) => Some(rpc_result(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": e }],
                        "isError": true,
                    }),
                )),
            }
        }
        other if other.starts_with("notifications/") => None,
        _ => Some(rpc_error(id, -32601, format!("method not found: {method}"))),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(line) {
            Ok(msg) => handle_request(&msg).await,
            Err(e) => Some(rpc_error(Value::Null, -32700, format!("parse error: {e}"))),
        };
        if let Some(response) = response {
            let mut out = stdout.lock();
            serde_json::to_writer(&mut out, &response)?;
            out.write_all(b"\n")?;
            out.flush()?;
        }
    }
    Ok(())
}
