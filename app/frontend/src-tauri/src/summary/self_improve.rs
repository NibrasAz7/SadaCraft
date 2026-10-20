//! Self-improving summary templates — a Propose–Probe–Compose loop
//! (concept borrowed from google-harness2, implemented natively).
//!
//! - **Propose**: the LLM inspects (source, produced summary) and proposes one
//!   concrete improvement rule ("always capture decision owners", ...).
//! - **Probe**: the final report is regenerated with the candidate rule
//!   injected — only the report pass reruns, not the chunked base summary.
//! - **Compose**: the LLM judges A (baseline) vs B (candidate); if B wins, its
//!   markdown is kept and the rule is persisted in a per-template lessons store.
//!
//! Enabled via env `SADACRAFT_SUMMARY_SELF_IMPROVE=1` or the persisted
//! `summary_self_improve` flag in `store.json` (Settings → Summary toggle).
//! Persisted lessons are injected into every later final-report prompt for
//! that template — this part is always active once lessons exist and costs
//! nothing.

use super::llm_client::{generate_summary, LLMProvider};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

const MAX_LESSON_CHARS: usize = 280;
const MAX_LESSONS_PER_TEMPLATE: usize = 16;
/// Source excerpt cap for propose/judge prompts — keeps local models in context.
const JUDGE_SOURCE_CHARS: usize = 6000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub rule: String,
    pub wins: u32,
}

type LessonsFile = HashMap<String, Vec<Lesson>>;

fn lessons_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("summary_lessons.json")
}

/// Persisted toggle lives in `store.json` (the tauri-plugin-store file, which
/// resolves under `app_data_dir`) under the `summary_self_improve` key.
const STORE_FILE: &str = "store.json";
const STORE_KEY: &str = "summary_self_improve";

fn persisted_enabled(app_data_dir: &Path) -> bool {
    std::fs::read_to_string(app_data_dir.join(STORE_FILE))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get(STORE_KEY)?.as_bool())
        .unwrap_or(false)
}

/// True when the env var is set OR the persisted settings toggle is on.
pub fn is_enabled(app_data_dir: Option<&PathBuf>) -> bool {
    if std::env::var("SADACRAFT_SUMMARY_SELF_IMPROVE").is_ok_and(|v| v == "1" || v == "true") {
        return true;
    }
    app_data_dir.is_some_and(|dir| persisted_enabled(dir))
}

fn load_all(app_data_dir: &Path) -> LessonsFile {
    std::fs::read_to_string(lessons_path(app_data_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_all(app_data_dir: &Path, lessons: &LessonsFile) {
    match serde_json::to_string_pretty(lessons) {
        Ok(json) => {
            if let Err(e) = std::fs::write(lessons_path(app_data_dir), json) {
                warn!("failed to persist summary lessons: {}", e);
            }
        }
        Err(e) => warn!("failed to serialize summary lessons: {}", e),
    }
}

/// Markdown block of learned rules for a template, or None if no lessons exist.
/// Always cheap — file read only, no LLM calls.
pub fn lessons_block(app_data_dir: Option<&PathBuf>, template_id: &str) -> Option<String> {
    let dir = app_data_dir?;
    let all = load_all(dir);
    let lessons = all.get(template_id)?;
    if lessons.is_empty() {
        return None;
    }
    let mut block = String::from("\n**LEARNED RULES** (accumulated from prior meetings — apply to every section):\n");
    for l in lessons {
        block.push_str(&format!("- {}\n", l.rule));
    }
    Some(block)
}

/// All persisted lessons, keyed by template id.
pub(crate) fn list_lessons(app_data_dir: &Path) -> LessonsFile {
    load_all(app_data_dir)
}

/// Remove one rule from a template. Returns true when something was deleted.
pub(crate) fn delete_lesson(app_data_dir: &Path, template_id: &str, rule: &str) -> bool {
    let mut all = load_all(app_data_dir);
    let removed = match all.get_mut(template_id) {
        Some(list) => {
            let before = list.len();
            list.retain(|l| l.rule != rule);
            list.len() < before
        }
        None => false,
    };
    if all.get(template_id).is_some_and(|l| l.is_empty()) {
        all.remove(template_id);
    }
    if removed {
        save_all(app_data_dir, &all);
    }
    removed
}

/// Remove every lesson for a template. Returns true when something was deleted.
pub(crate) fn clear_lessons(app_data_dir: &Path, template_id: &str) -> bool {
    let mut all = load_all(app_data_dir);
    if all.remove(template_id).is_some() {
        save_all(app_data_dir, &all);
        true
    } else {
        false
    }
}

fn append_lesson(app_data_dir: &Path, template_id: &str, rule: String) {
    let mut all = load_all(app_data_dir);
    let list = all.entry(template_id.to_string()).or_default();
    if list.iter().any(|l| l.rule == rule) {
        return;
    }
    list.push(Lesson { rule, wins: 1 });
    list.truncate(MAX_LESSONS_PER_TEMPLATE);
    save_all(app_data_dir, &all);
}

const PROPOSE_SYSTEM: &str = r#"You improve meeting-summary instructions. Compare the source text against the produced summary and propose ONE short, concrete rule that would have made the summary better (e.g. "always capture decision owners with deadlines", "preserve product names verbatim").

Rules:
- Exactly one imperative sentence, max 25 words.
- The rule must apply to FUTURE meetings, not just this one.
- Do NOT correct facts of this summary.
- If the summary is already excellent, reply exactly: NONE"#;

const JUDGE_SYSTEM: &str = r#"You compare two summaries (A and B) generated from the same meeting source. Pick the better one by faithfulness to the source, completeness of key points/decisions/action items, and structure.

Reply with exactly one line: "A" or "B", followed by " — " and a 5-10 word reason."#;

fn excerpt(text: &str, max_chars: usize) -> &str {
    let mut end = text.len().min(max_chars);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[allow(clippy::too_many_arguments)]
async fn llm(
    client: &Client,
    provider: &LLMProvider,
    model_name: &str,
    api_key: &str,
    system_prompt: &str,
    user_prompt: &str,
    ollama_endpoint: Option<&str>,
    custom_openai_endpoint: Option<&str>,
    app_data_dir: Option<&PathBuf>,
    cancellation_token: Option<&CancellationToken>,
) -> Result<String, String> {
    let completion = generate_summary(
        client, provider, model_name, api_key, system_prompt, user_prompt,
        ollama_endpoint, custom_openai_endpoint, Some(2048), Some(0.2), Some(0.9),
        app_data_dir, cancellation_token,
    )
    .await?;
    Ok(super::processor::clean_llm_markdown_detailed(&completion.content).markdown)
}

/// One Propose–Probe–Compose round.
///
/// `source_text` is the (already chunked/combined) text the final report was
/// generated from; `base_markdown` is the English report it produced.
/// `report_prompt_for` rebuilds the final-report system prompt given an extra
/// instructions block — lets the probe reuse the caller's exact prompt path.
///
/// Returns `(winning_markdown, learned_rule)` — markdown is the candidate when
/// it won, otherwise the unchanged baseline.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_pass(
    client: &Client,
    provider: &LLMProvider,
    model_name: &str,
    api_key: &str,
    source_text: &str,
    base_markdown: &str,
    template_id: &str,
    report_prompt_for: impl Fn(&str) -> String,
    report_user_prompt: &str,
    ollama_endpoint: Option<&str>,
    custom_openai_endpoint: Option<&str>,
    app_data_dir: Option<&PathBuf>,
    cancellation_token: Option<&CancellationToken>,
) -> Result<(String, Option<String>), String> {
    if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
        return Err("Summary generation was cancelled".to_string());
    }

    // PROPOSE
    let proposal = llm(
        client, provider, model_name, api_key, PROPOSE_SYSTEM,
        &format!(
            "<source>\n{}\n</source>\n\n<summary>\n{}\n</summary>",
            excerpt(source_text, JUDGE_SOURCE_CHARS),
            excerpt(base_markdown, JUDGE_SOURCE_CHARS)
        ),
        ollama_endpoint, custom_openai_endpoint, app_data_dir, cancellation_token,
    )
    .await?;
    let rule = proposal.lines().next().unwrap_or("").trim().trim_matches('"').to_string();
    if rule.is_empty() || rule.eq_ignore_ascii_case("none") || rule.len() > MAX_LESSON_CHARS {
        info!("self-improve: no lesson proposed ({:?})", rule);
        return Ok((base_markdown.to_string(), None));
    }

    // PROBE — regenerate the report with the candidate rule injected
    let probe_system = report_prompt_for(&format!("\n- {}\n", rule));
    let probe_markdown = llm(
        client, provider, model_name, api_key, &probe_system, report_user_prompt,
        ollama_endpoint, custom_openai_endpoint, app_data_dir, cancellation_token,
    )
    .await?;
    if probe_markdown.is_empty() {
        return Ok((base_markdown.to_string(), None));
    }

    // COMPOSE — judge A vs B
    let verdict = llm(
        client, provider, model_name, api_key, JUDGE_SYSTEM,
        &format!(
            "<source>\n{}\n</source>\n\n<summary_a>\n{}\n</summary_a>\n\n<summary_b>\n{}\n</summary_b>",
            excerpt(source_text, JUDGE_SOURCE_CHARS),
            excerpt(base_markdown, JUDGE_SOURCE_CHARS),
            excerpt(&probe_markdown, JUDGE_SOURCE_CHARS)
        ),
        ollama_endpoint, custom_openai_endpoint, app_data_dir, cancellation_token,
    )
    .await?;
    let winner = verdict.trim_start().chars().next().unwrap_or('A').to_ascii_uppercase();
    info!("self-improve: judge picked {} ({}) — rule: {}", winner, verdict.trim(), rule);

    if winner == 'B' {
        if let Some(dir) = app_data_dir {
            append_lesson(dir, template_id, rule.clone());
        }
        Ok((probe_markdown, Some(rule)))
    } else {
        Ok((base_markdown.to_string(), None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_lessons(dir: &Path, template_id: &str, rules: &[&str]) {
        let lessons: Vec<Lesson> = rules
            .iter()
            .map(|r| Lesson {
                rule: r.to_string(),
                wins: 1,
            })
            .collect();
        let mut all = LessonsFile::new();
        all.insert(template_id.to_string(), lessons);
        std::fs::write(
            dir.join("summary_lessons.json"),
            serde_json::to_string(&all).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn lessons_block_is_none_without_dir_file_or_lessons() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(lessons_block(None, "standup").is_none());
        assert!(lessons_block(Some(&tmp.path().to_path_buf()), "standup").is_none());

        write_lessons(tmp.path(), "standup", &[]);
        // An empty lessons list still yields None.
        let all: LessonsFile =
            serde_json::from_str(r#"{"standup": []}"#).unwrap();
        std::fs::write(
            tmp.path().join("summary_lessons.json"),
            serde_json::to_string(&all).unwrap(),
        )
        .unwrap();
        assert!(lessons_block(Some(&tmp.path().to_path_buf()), "standup").is_none());
    }

    #[test]
    fn lessons_block_renders_rules_for_template_only() {
        let tmp = tempfile::tempdir().unwrap();
        write_lessons(
            tmp.path(),
            "standup",
            &["always capture decision owners", "keep action items verbatim"],
        );

        let block = lessons_block(Some(&tmp.path().to_path_buf()), "standup")
            .expect("lessons block for standup");
        assert!(block.contains("LEARNED RULES"));
        assert!(block.contains("- always capture decision owners"));
        assert!(block.contains("- keep action items verbatim"));

        // Other templates and unknown ids get nothing.
        assert!(lessons_block(Some(&tmp.path().to_path_buf()), "retro").is_none());
        assert!(lessons_block(Some(&tmp.path().to_path_buf()), "nope").is_none());
    }

    #[test]
    fn is_enabled_uses_persisted_store_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();

        // No store file -> disabled (assuming env flag unset).
        std::env::remove_var("SADACRAFT_SUMMARY_SELF_IMPROVE");
        assert!(!is_enabled(Some(&dir)));

        std::fs::write(dir.join(STORE_FILE), r#"{"summary_self_improve": true}"#).unwrap();
        assert!(is_enabled(Some(&dir)));

        std::fs::write(dir.join(STORE_FILE), r#"{"summary_self_improve": false}"#).unwrap();
        assert!(!is_enabled(Some(&dir)));
    }

    #[test]
    fn is_enabled_honours_env_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();

        std::env::set_var("SADACRAFT_SUMMARY_SELF_IMPROVE", "1");
        assert!(is_enabled(Some(&dir)));
        std::env::remove_var("SADACRAFT_SUMMARY_SELF_IMPROVE");
    }

    #[test]
    fn append_dedupes_and_delete_and_clear_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();

        append_lesson(&dir, "standup", "rule one".to_string());
        append_lesson(&dir, "standup", "rule one".to_string());
        append_lesson(&dir, "standup", "rule two".to_string());

        let lessons = list_lessons(&dir);
        let list = lessons.get("standup").unwrap();
        assert_eq!(list.len(), 2, "duplicate rule must not be appended");

        assert!(delete_lesson(&dir, "standup", "rule one"));
        assert!(!delete_lesson(&dir, "standup", "rule one"));
        assert_eq!(list_lessons(&dir)["standup"].len(), 1);

        assert!(clear_lessons(&dir, "standup"));
        assert!(!clear_lessons(&dir, "standup"));
        assert!(list_lessons(&dir).get("standup").is_none());
        assert!(lessons_block(Some(&dir), "standup").is_none());
    }

    #[test]
    fn excerpt_respects_char_boundaries() {
        let s = "héllo wörld — emoji: 🎙️ end";
        let cut = excerpt(s, 8);
        assert!(cut.len() <= 8);
        assert!(s.starts_with(cut));
        assert_eq!(excerpt("short", 100), "short");
    }

    /// Live Propose–Probe–Compose pass through the real BuiltInAI path
    /// (llama-helper sidecar + local GGUF). Ignored by default; run with:
    ///   cargo test --lib self_improve::tests::run_pass_live_builtin -- --ignored
    /// Requires `MEETILY_LLAMA_HELPER` (defaults to the bundled sidecar in
    /// `binaries/`) and a downloaded `qwen3.5:4b` model under the app data dir.
    #[tokio::test]
    #[ignore]
    async fn run_pass_live_builtin() {
        if std::env::var_os("MEETILY_LLAMA_HELPER").is_none() {
            #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
            let triple = "x86_64-pc-windows-msvc";
            #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
            let triple = "aarch64-pc-windows-msvc";
            #[cfg(not(target_os = "windows"))]
            let triple = "";
            let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("binaries")
                .join(format!("llama-helper-{triple}.exe"));
            if helper.exists() {
                std::env::set_var("MEETILY_LLAMA_HELPER", &helper);
            }
        }

        // Model resolution is app_data_dir/models/summary/<gguf>. Link the
        // real downloaded model into a temp app data dir so a winning probe
        // only writes lessons to a throwaway file.
        let real_dir = dirs::data_dir()
            .expect("data dir")
            .join("com.sadacraft.app");
        let gguf = real_dir
            .join("models")
            .join("summary")
            .join("Qwen3.5-4B-Q4_K_M.gguf");
        if !gguf.exists() {
            eprintln!("skipping: {} not downloaded", gguf.display());
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let summary_dir = tmp.path().join("models").join("summary");
        std::fs::create_dir_all(&summary_dir).unwrap();
        if let Err(e) = std::fs::hard_link(&gguf, summary_dir.join("Qwen3.5-4B-Q4_K_M.gguf"))
        {
            eprintln!("skipping: cannot link model into temp dir: {e}");
            return;
        }
        let app_data_dir = tmp.path().to_path_buf();

        let source = "Alice: We need to ship the SDK by Friday. \
                      Bob: I'll own the docs, due Thursday. \
                      Alice: Decision — we cut the plugin system for v1.";
        let base = "# Report\n## Decisions\n- Cut plugin system for v1\n";
        let client = Client::new();
        let report_prompt_for = |extra: &str| {
            format!("You summarize meetings.{extra}\nOutput only markdown.")
        };
        let user_prompt = format!("<transcript_chunks>\n{source}\n</transcript_chunks>\n");

        let result = run_pass(
            &client,
            &LLMProvider::BuiltInAI,
            "qwen3.5:4b",
            "",
            source,
            base,
            "verify_live_test",
            report_prompt_for,
            &user_prompt,
            None,
            None,
            Some(&app_data_dir),
            None,
        )
        .await;

        let (winning, learned) = result.expect("run_pass should not hard-fail");
        assert!(!winning.is_empty());
        eprintln!("live run_pass learned rule: {learned:?}");
        if let Some(rule) = &learned {
            let block = lessons_block(Some(&app_data_dir), "verify_live_test")
                .expect("learned rule must persist to summary_lessons.json");
            assert!(block.contains(rule.as_str()));
        }
    }
}
