//! Transcript items for the search quality measure: shell commands, other tool calls, their results
//! and assistant text, in file order, from Claude main and subagent transcripts and Codex rollouts.
//! Unlike `transcript_calls`, subagent logs count: subagents run many of the searches, and the
//! pairing of a call with its result goes by call id, which survives a resumed copy.
use serde_json::Value;
use std::path::{Path, PathBuf};

/// What one transcript item is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Kind { Shell, Tool, Result, Text }

pub(super) struct Step {
    pub at_ms: f64,
    pub id: String,
    pub kind: Kind,
    pub text: String,
    pub line: usize,
    /// The assistant turn that issued a tool call: Claude's `message.id`, shared by the calls one
    /// message runs in parallel; else the transcript line (`adopt.rs` window).
    pub turn: String,
}

/// Transcripts under every agent source written at or after `from_ms`.
pub(super) fn transcripts(home: &Path, from_ms: f64) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for (_, dir, _) in super::super::conversation_retro::SOURCES {
        collect(&home.join(dir), 4, from_ms, &mut out);
    }
    out
}

fn collect(dir: &Path, depth: usize, from_ms: f64, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let path = entry.path();
        if meta.is_dir() {
            if depth > 0 && entry.file_name() != "tool-results" { collect(&path, depth - 1, from_ms, out); }
            continue;
        }
        let modified = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        if modified >= from_ms && path.extension().is_some_and(|e| e == "jsonl") { out.push(path); }
    }
}

/// Items of `path` from `from_ms` up to `tail_ms` after `to_ms`, so a call near the end still has
/// its answer and follow-up. Shell commands after `to_ms` stay too: they are a call's adoption or
/// fallback evidence, and the caller skips them as calls. Search-retro 811204797093281505698
/// (20ab51b1…jsonl:1275): the window ended 5 s after a `query`, the agent read its hit with
/// `sed -n 165,175p …/settings/CreditUsageLedger.tsx` at :1299, and the dropped `sed` left the hit unadopted.
pub(super) fn read(path: &Path, from_ms: f64, to_ms: f64, tail_ms: f64) -> Vec<Step> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if !line.contains("tool") && !line.contains("call_id") && !line.contains("\"text\"") { continue; }
        let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
        let Some(at_ms) = row["timestamp"].as_str().and_then(crate::cause_tree::attribute::parse_time) else { continue };
        if at_ms < from_ms || at_ms > to_ms + tail_ms { continue; }
        let turn = row["message"]["id"].as_str().map_or_else(|| format!("line{index}"), str::to_string);
        for (id, kind, text) in entries(&row) {
            out.push(Step { at_ms, id, kind, text, line: index + 1, turn: turn.clone() });
        }
    }
    out
}

/// The shell directory Claude Code recorded (`cwd`) on each line holding a tool call, by line
/// number; Codex rollouts record none. `scoped.rs` places relative paths with it (search-retro
/// 816759169077413470494, 79a9b918…jsonl:1127).
pub(super) fn cwds(path: &Path) -> std::collections::HashMap<usize, String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines().enumerate()
        .filter(|(_, l)| l.contains("\"tool_use\"") && l.contains("\"cwd\""))
        .filter_map(|(i, l)| Some((i + 1, serde_json::from_str::<Value>(l).ok()?["cwd"].as_str()?.to_string())))
        .collect()
}

/// `(call id, kind, text)` of one transcript row.
pub(super) fn entries(row: &Value) -> Vec<(String, Kind, String)> {
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let mut out = Vec::new();
    for item in row["message"]["content"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("tool_use") => out.push(match item["input"]["command"].as_str() {
                Some(command) => (s(&item["id"]), Kind::Shell, command.to_string()),
                None => (s(&item["id"]), Kind::Tool, item["input"].to_string()),
            }),
            Some("tool_result") => out.push((s(&item["tool_use_id"]), Kind::Result, flatten(&item["content"]))),
            Some("text") if row["type"] == "assistant" => out.push((String::new(), Kind::Text, s(&item["text"]))),
            _ => {}
        }
    }
    let p = &row["payload"];
    match p["type"].as_str() {
        Some("function_call") => out.push((s(&p["call_id"]), Kind::Shell, s(&p["arguments"]))),
        Some("custom_tool_call") => out.push((s(&p["call_id"]), Kind::Shell, s(&p["input"]))),
        Some("function_call_output" | "custom_tool_call_output") => out.push((s(&p["call_id"]), Kind::Result, flatten(&p["output"]))),
        Some("message") if p["role"] == "assistant" => out.push((String::new(), Kind::Text, flatten(&p["content"]))),
        _ => {}
    }
    out
}

/// The text of a shell word opened by `q`, up to its closing quote; inside double quotes a backslash
/// escapes the next character. Search-retro 84763104493312366879 (be7a95f1…jsonl:136): the query
/// `"\"/me/subscribe\"|/subscribe\""` was read as `\` and had no names, so it could not be adopted.
pub(super) fn quoted(text: &str, q: char) -> String {
    let (mut out, mut chars) = (String::new(), text.chars());
    while let Some(c) = chars.next() {
        match c {
            '\\' if q == '"' => out.extend(chars.next()),
            c if c == q => break,
            c => out.push(c),
        }
    }
    out
}

fn flatten(content: &Value) -> String {
    match content {
        Value::String(text) => unwrap_settled(text),
        Value::Array(parts) => parts.iter().map(|p| p["text"].as_str().map_or_else(|| flatten(p), unwrap_settled)).collect::<Vec<_>>().join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A Codex JavaScript tool call prints each `tools.exec_command` result as one JSON line:
/// `{"chunk_id":…,"exit_code":0,"output":"<escaped text>"}` from `text(await …)`, or the settled
/// promise `{"status":"fulfilled","value":{…,"output":…}}` from `Promise.allSettled`. The command's
/// own lines are that `output`. Read whole, every `path:line` sits inside one escaped string and no
/// hit is seen. Search-retro 813242367899389877458: rollout-…01a1b500…jsonl:33 `search co2 --lines`
/// printed 40 lines and :58, :76, :287, :361, :455, :573, :689 printed `query` hits; all counted as misses.
fn exec_output(v: &Value) -> Option<&str> {
    (v.get("chunk_id").is_some() || v.get("exit_code").is_some()).then(|| v["output"].as_str()).flatten()
}

fn unwrap_settled(text: &str) -> String {
    let parsed = text.trim_start().starts_with('{').then(|| serde_json::from_str::<Value>(text.trim()).ok()).flatten();
    match parsed.as_ref().and_then(|v| exec_output(v).or_else(|| exec_output(&v["value"])).or_else(|| (v["status"] == "rejected").then(|| v["reason"].as_str()).flatten())) {
        Some(output) => output.to_string(),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_settled_js_tool_result_is_read_as_its_command_output() {
        let row = json!({"type": "response_item", "payload": {"type": "custom_tool_call_output", "call_id": "c1", "output": [
            {"type": "input_text", "text": "Script completed\nOutput:\n"},
            {"type": "input_text", "text": "{\"status\":\"fulfilled\",\"value\":{\"exit_code\":0,\"output\":\"a/b.rs:8:    let co2 = 1;\\nc/d.rs:9:x\"}}"},
            {"type": "input_text", "text": "{\"status\":\"rejected\",\"reason\":\"timeout\"}"}]}});
        let (_, kind, text) = entries(&row).remove(0);
        assert_eq!(kind, Kind::Result);
        assert_eq!(text, "Script completed\nOutput:\n\na/b.rs:8:    let co2 = 1;\nc/d.rs:9:x\ntimeout");
        // Plain text, and JSON that is no settled promise, stay as printed.
        assert_eq!(unwrap_settled("{\"a\":1}"), "{\"a\":1}");
        assert_eq!(unwrap_settled("x/y.rs:1:z"), "x/y.rs:1:z");
        // rollout-…:58: `text(await tools.exec_command(…))` prints the result object itself.
        assert_eq!(unwrap_settled("{\"chunk_id\":\"7b\",\"exit_code\":0,\"output\":\"  /r/a.ts:1-25  [code/block] x\"}"), "  /r/a.ts:1-25  [code/block] x");
    }
}
