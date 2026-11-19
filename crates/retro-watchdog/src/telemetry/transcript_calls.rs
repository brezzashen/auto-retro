//! Tool calls and tool results of agent transcripts, for deterministic probes (Docker restart
//! attribution, retro follow-ups). A call's `text` is the shell command when the tool has one
//! (Claude `input.command`, Codex `exec_command` code), else the input as JSON; a result's `text`
//! is the tool output. Only main transcripts count (no subagent logs), as in conversation_retro.
use super::conversation_retro::{is_conversation, Layout, SOURCES};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Characters of one result kept for matching; outputs past this are truncated.
const RESULT_CHARS: usize = 4_000;

pub(crate) struct Call<'a> {
    pub at_ms: f64,
    /// Transcript file stem: the Claude session id or the Codex rollout name.
    pub session: &'a str,
    pub path: &'a Path,
    pub line: usize,
    pub result: bool,
    pub text: String,
}

/// Main transcripts written at or after `from_ms`: `(source alias, path)`.
pub(crate) fn files(home: &Path, from_ms: f64) -> Vec<(&'static str, PathBuf)> {
    let mut out = Vec::new();
    for (alias, dir, layout) in SOURCES {
        let depth = match layout { Layout::Claude => 1, Layout::Codex => 3 };
        walk(&home.join(dir), depth, layout, from_ms, &mut |path| out.push((alias, path)));
    }
    out
}

fn walk(dir: &Path, depth: usize, layout: Layout, from_ms: f64, found: &mut dyn FnMut(PathBuf)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            if depth > 0 && entry.file_name() != "subagents" { walk(&entry.path(), depth - 1, layout, from_ms, found); }
            continue;
        }
        let modified = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        if modified >= from_ms && is_conversation(layout, &entry.file_name().to_string_lossy()) { found(entry.path()); }
    }
}

/// Every tool call and result of `path` timestamped inside `from_ms..=to_ms`, on lines holding at
/// least one of `needles` (all lines when empty); the check runs before any JSON parsing.
pub(crate) fn scan(path: &Path, from_ms: f64, to_ms: f64, needles: &[&str], each: &mut dyn FnMut(Call)) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let session = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    for (index, line) in text.lines().enumerate() {
        if !line.contains("tool") || (!needles.is_empty() && !needles.iter().any(|n| line.contains(n))) { continue; }
        let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
        let Some(at_ms) = row["timestamp"].as_str().and_then(crate::cause_tree::attribute::parse_time) else { continue };
        if at_ms < from_ms || at_ms > to_ms { continue; }
        for (result, text) in entries(&row) {
            each(Call { at_ms, session, path, line: index + 1, result, text });
        }
    }
}

/// Calls already counted from another transcript file. A session resumed under another account copies
/// its history into that profile's transcript (same session id, same timestamps), so a census over
/// every source would count those calls twice (retro 85719030010337260530: 13-31 % of a signature's calls).
#[derive(Default)]
pub(crate) struct Copies(std::collections::HashSet<(String, u64, bool, u64)>);

impl Copies {
    /// True the first time this session's call or result at this instant with this text is seen.
    pub(crate) fn first(&mut self, call: &Call) -> bool {
        use std::hash::{Hash, Hasher};
        let mut text = std::collections::hash_map::DefaultHasher::new();
        call.text.hash(&mut text);
        self.0.insert((call.session.to_string(), call.at_ms.to_bits(), call.result, text.finish()))
    }
}

/// `(is_result, text)` for each tool call or result in one transcript row.
pub(crate) fn entries(row: &Value) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    for item in row["message"]["content"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("tool_use") => out.push((false, item["input"]["command"].as_str().map_or_else(|| item["input"].to_string(), str::to_string))),
            Some("tool_result") => out.push((true, clip(&flatten(&item["content"])))),
            _ => {}
        }
    }
    let payload = &row["payload"];
    match payload["type"].as_str() {
        Some("function_call") => out.push((false, payload["arguments"].as_str().unwrap_or_default().to_string())),
        Some("custom_tool_call") => out.push((false, payload["input"].as_str().unwrap_or_default().to_string())),
        Some("function_call_output" | "custom_tool_call_output") => out.push((true, clip(&flatten(&payload["output"])))),
        _ => {}
    }
    out
}

fn flatten(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts.iter().map(|p| p["text"].as_str().map_or_else(|| flatten(p), str::to_string)).collect::<Vec<_>>().join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Past the cut, the `==> path:start-end` headers of local-search bodies are kept, because they end
/// a long result and the R11 probe matches reads against them (retro_followup_printed).
fn clip(text: &str) -> String {
    let mut out: String = text.chars().take(RESULT_CHARS).collect();
    if out.len() < text.len() {
        for line in text[out.len()..].lines().filter(|l| l.starts_with("==> ")) { out.push('\n'); out.push_str(line); }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_claude_and_codex_calls_and_results() {
        let claude = json!({"message": {"content": [{"type": "tool_use", "input": {"command": "docker desktop stop"}},
            {"type": "tool_use", "input": {"file_path": "/a/SKILL.md"}}]}});
        assert_eq!(entries(&claude), vec![(false, "docker desktop stop".into()), (false, r#"{"file_path":"/a/SKILL.md"}"#.into())]);
        let result = json!({"message": {"content": [{"type": "tool_result", "content": [{"type": "text", "text": "collector is stale"}]}]}});
        assert_eq!(entries(&result), vec![(true, "collector is stale".into())]);
        let codex = json!({"payload": {"type": "custom_tool_call", "input": "tools.exec_command({cmd:\"open -a Docker\"})"}});
        assert_eq!(entries(&codex)[0].1, "tools.exec_command({cmd:\"open -a Docker\"})");
        let output = json!({"payload": {"type": "function_call_output", "output": "ok"}});
        assert_eq!(entries(&output), vec![(true, "ok".into())]);
    }

    #[test]
    fn clip_keeps_body_headers_past_the_cut() {
        let long = format!("{}\n==> /h/f.rs:1-9\n  1\tx\n", "a".repeat(RESULT_CHARS + 10));
        let out = clip(&long);
        assert!(out.ends_with("\n==> /h/f.rs:1-9"));
        assert_eq!(clip("short"), "short");
    }

    #[test]
    fn copies_keep_the_first_of_a_resumed_transcripts_calls() {
        let (a, b) = (Path::new("/p1/s.jsonl"), Path::new("/p2/s.jsonl"));
        let call = |path, at_ms, text: &str| Call { at_ms, session: "s", path, line: 3, result: false, text: text.into() };
        let mut copies = Copies::default();
        assert!(copies.first(&call(a, 1.0, "ls")));
        assert!(!copies.first(&call(b, 1.0, "ls")));
        assert!(copies.first(&call(b, 2.0, "ls")));
        assert!(copies.first(&call(b, 1.0, "pwd")));
    }
}
