//! Task pace of one window (founder, 2026-11-02): for each prompt the founder typed in a Claude
//! session, how many searches ran and how many minutes passed before the first code edit. The
//! policy score says which tool ran; this says whether searching got the founder's task to the
//! change sooner, the effect the founder can feel.
//!
//! A task starts at a human prompt (a `user` line whose text is not a tool result, a meta line, a
//! `<…>` system or command block, a `[hostq]` notice, a compaction summary or a watchdog dispatch) inside the window and
//! ends at the next one. Its first edit is the first Edit, Write, MultiEdit or NotebookEdit call.
//! Searches are the window's search-watch events of that session between the prompt and the edit.
//! Tasks without an edit (questions, reviews, waits) are counted, not timed. Codex sessions are
//! skipped: their rollouts edit through apply_patch inside shell calls.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const EDITS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];
const EXAMPLES: usize = 5;

struct Task {
    session: String,
    locator: String,
    prompt: String,
    start_ms: f64,
    edit_ms: Option<f64>,
}

pub(super) fn measure(home: &Path, events: &[Value], from_ms: f64, to_ms: f64) -> Value {
    let mut sessions: Vec<&str> = events.iter().filter(|e| e["agent"] == "claude").filter_map(|e| e["session"].as_str()).filter(|s| !s.is_empty()).collect();
    sessions.sort_unstable();
    sessions.dedup();
    let (mut tasks, mut missing) = (Vec::new(), 0);
    for session in &sessions {
        match transcript(home, session) {
            Some(path) => tasks.extend(tasks_of(&path, session, from_ms, to_ms)),
            None => missing += 1,
        }
    }
    let searches = |t: &Task, local: bool| {
        let end = t.edit_ms.unwrap_or(f64::MAX);
        events.iter().filter(|e| e["session"] == t.session.as_str())
            .filter(|e| e["ts_ms"].as_f64().is_some_and(|at| at >= t.start_ms && at <= end))
            .filter(|e| !local || e["kind"] == "local_search").count()
    };
    let edited: Vec<&Task> = tasks.iter().filter(|t| t.edit_ms.is_some()).collect();
    let counts: Vec<f64> = edited.iter().map(|t| searches(t, false) as f64).collect();
    let minutes: Vec<f64> = edited.iter().filter_map(|t| t.edit_ms.map(|e| (e - t.start_ms) / 60_000.0)).collect();
    let total: usize = edited.iter().map(|t| searches(t, false)).sum();
    let local: usize = edited.iter().map(|t| searches(t, true)).sum();
    let mut ranked: Vec<&&Task> = edited.iter().collect();
    ranked.sort_by_key(|t| std::cmp::Reverse(searches(t, false)));
    let examples: Vec<Value> = ranked.iter().take(EXAMPLES).map(|t| json!({
        "session": t.session, "transcript": t.locator, "prompt": t.prompt, "searches": searches(t, false),
        "local_search": searches(t, true), "minutes": round((t.edit_ms.unwrap_or(t.start_ms) - t.start_ms) / 60_000.0)})).collect();
    json!({
        "sessions": sessions.len(), "missing_transcripts": missing, "tasks": tasks.len(), "edited": edited.len(),
        "searches_to_edit": {"p50": pct(&counts, 0.5), "p90": pct(&counts, 0.9), "mean": mean(&counts)},
        "minutes_to_edit": {"p50": pct(&minutes, 0.5), "p90": pct(&minutes, 0.9)},
        "local_search_share": if total == 0 { Value::Null } else { json!(round(local as f64 / total as f64)) },
        "examples": examples,
        "definition": "per human prompt in a Claude session: search-watch events and minutes until the first Edit/Write/MultiEdit/NotebookEdit; tasks without an edit are not timed",
    })
}

/// `cpu-watchdog search-pace <incident-id>`: recomputes an incident's task pace from its batch.
pub(super) fn cli(home: &Path, args: Vec<String>) -> anyhow::Result<()> {
    let [id] = args.as_slice() else { anyhow::bail!("usage: cpu-watchdog search-pace <incident-id>") };
    let batch = crate::monitor::root(home).join("search-retro").join(format!("{id}.jsonl"));
    let text = std::fs::read_to_string(&batch).map_err(|e| anyhow::anyhow!("read {}: {e}", batch.display()))?;
    let events: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let at = |e: Option<&Value>| e.and_then(|e| e["ts_ms"].as_f64()).unwrap_or(0.0);
    println!("{}", serde_json::to_string_pretty(&measure(home, &events, at(events.first()), at(events.last())))?);
    Ok(())
}

/// The session's main transcript under `~/.claude/projects` or a profile's `projects`.
fn transcript(home: &Path, session: &str) -> Option<PathBuf> {
    let name = format!("{session}.jsonl");
    let mut roots = vec![home.join(".claude/projects")];
    if let Ok(profiles) = std::fs::read_dir(home.join(".claude-profiles")) {
        roots.extend(profiles.flatten().map(|p| p.path().join("projects")));
    }
    roots.iter().filter_map(|r| std::fs::read_dir(r).ok()).flat_map(|d| d.flatten()).map(|p| p.path().join(&name)).find(|p| p.is_file())
}

fn tasks_of(path: &Path, session: &str, from_ms: f64, to_ms: f64) -> Vec<Task> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut tasks: Vec<Task> = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let Ok(line) = serde_json::from_str::<Value>(raw) else { continue };
        let Some(at) = line["timestamp"].as_str().and_then(crate::telemetry::transcript_ms) else { continue };
        if line["type"] == "user" {
            if let Some(prompt) = human_prompt(&line) {
                if (from_ms..=to_ms).contains(&at) {
                    tasks.push(Task { session: session.to_string(), locator: format!("{}:{}", path.display(), n + 1), prompt, start_ms: at, edit_ms: None });
                } else if at > to_ms {
                    break;
                }
            }
        } else if line["type"] == "assistant" {
            let edits = line["message"]["content"].as_array().is_some_and(|c| c.iter().any(|b| b["type"] == "tool_use" && b["name"].as_str().is_some_and(|n| EDITS.contains(&n))));
            if let Some(task) = tasks.last_mut().filter(|t| edits && t.edit_ms.is_none()) {
                task.edit_ms = Some(at);
            }
        }
    }
    tasks
}

/// The text the founder typed, clipped; `None` for tool results and injected lines.
pub(super) fn human_prompt(line: &Value) -> Option<String> {
    if line["isMeta"] == true || line["isCompactSummary"] == true {
        return None;
    }
    let content = &line["message"]["content"];
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) if blocks.iter().all(|b| b["type"] != "tool_result") => {
            blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(" ")
        }
        _ => return None,
    };
    let text = text.trim();
    let injected = text.is_empty() || text.starts_with('<') || text.starts_with("[hostq]") || text.starts_with("[Request interrupted")
        || text.starts_with("This session is being continued") || text.starts_with("cpu-watchdog incident");
    (!injected).then(|| text.chars().take(80).collect())
}

fn pct(values: &[f64], q: f64) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    json!(round(sorted[((sorted.len() - 1) as f64 * q).round() as usize]))
}

fn mean(values: &[f64]) -> Value {
    if values.is_empty() { Value::Null } else { json!(round(values.iter().sum::<f64>() / values.len() as f64)) }
}

fn round(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
#[path = "task_pace_tests.rs"]
mod tests;
