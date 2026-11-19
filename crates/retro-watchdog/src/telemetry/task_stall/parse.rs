//! Incremental transcript reader for the task stall watch: the task each agent session is working
//! on right now.
//!
//! A task starts at a human prompt (Claude: a `user` line whose content is text, not a tool result;
//! Codex: `task_started` or a user message) and ends when the agent hands the turn back (Claude: an
//! assistant line whose `stop_reason` is not `tool_use`, the synthetic usage-limit reply included,
//! which is kept in `limit_ms`; Codex: `task_complete`). While it is open the
//! cursor keeps the tool calls still waiting for their result and, for Claude, how often each
//! command signature came back as an error inside this task.
use super::super::runtime_retro::parse::{claude_label, codex_label, timestamp_ms};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const TEXT_CHARS: usize = 240;
/// Signature length of a failing command: the same command with other trailing arguments repeats.
/// The first word is never cut, so a file tool's path stays whole (`signature_of`).
const SIGNATURE_CHARS: usize = 80;
/// Calls that wait on the user or on a subagent's own transcript, never on a queue.
pub(super) const NOT_STALLS: [&str; 7] = ["AskUserQuestion", "ExitPlanMode", "EnterPlanMode", "Agent", "Task", "request_user_input", "request_user_input_async"];
/// Codex questions whose call returns `{"accepted":true}` at once while the turn stays open until
/// the user answers: the quiet that follows waits on the user, not a stall (incident
/// 82196204334637140999: a Codex task read "silent" for 27 min after asking which device to check).
const ASYNC_ASKS: [&str; 1] = ["request_user_input_async"];

#[derive(Default, Clone, Serialize, Deserialize, PartialEq, Debug)]
pub(in crate::telemetry) struct Pending {
    pub id: String,
    pub label: String,
    pub command: String,
    pub at_ms: f64,
    /// Key for `failures`: label plus the first real command (`signature::core`), computed from
    /// the full multi-line command before `command` is flattened and cut.
    #[serde(default)]
    pub signature: String,
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub(in crate::telemetry) struct TaskCursor {
    pub offset: u64,
    pub alias: String,
    pub codex: bool,
    pub sub: bool,
    pub cwd: String,
    pub mtime_ms: f64,
    /// Start of the open task; 0 when the agent has handed the turn back.
    pub task_ms: f64,
    pub prompt: String,
    /// Last transcript line of any kind.
    pub last_ms: f64,
    pub calls: u64,
    pub pending: Vec<Pending>,
    /// Errors per command signature inside the open task.
    pub failures: BTreeMap<String, u64>,
    /// The task start already reported in an incident.
    pub reported_ms: f64,
    /// Last turn Claude Code ended with its synthetic usage-limit reply, and that reply.
    pub limit_ms: f64,
    pub limit_text: String,
    /// Time of an async question (`ASYNC_ASKS`) still unanswered; 0 once the task moves on.
    #[serde(default)]
    pub asked_ms: f64,
}

pub(super) fn excerpt(text: &str, chars: usize) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(chars).collect()
}

/// What a Claude tool call runs: Bash's `command`, an MCP JavaScript call's `code`, a file tool's
/// `file_path`/`notebook_path`, else the whole input. Keyed on the JSON input cut at 80 chars, three
/// Reads of different files under one scratchpad (its path alone is ~100 chars) shared one
/// signature and raised a false repeat failure (incident 815835349241340550064). Keyed on `command` alone, every MCP call had an empty command, so three unrelated
/// cc-computer errors (a timeout, a Keychain lookup, a file unlink) shared one signature and raised
/// a false repeat failure (incident 81184478046563301748).
fn claude_command(input: &Value) -> String {
    match input["command"].as_str().or(input["code"].as_str()).or(input["file_path"].as_str()).or(input["notebook_path"].as_str()) {
        Some(s) => s.to_string(),
        None if input.is_null() => String::new(),
        None => input.to_string(),
    }
}

/// `label: <first real command>` flattened and cut at SIGNATURE_CHARS, but never inside its first
/// word: a long path or binary is the identity, only trailing arguments may merge.
pub(super) fn signature_of(label: &str, command: &str) -> String {
    let flat = excerpt(super::signature::core(command), usize::MAX);
    let first = flat.split(' ').next().map_or(0, |w| w.chars().count());
    format!("{label}: {}", flat.chars().take(SIGNATURE_CHARS.max(first)).collect::<String>())
}

fn text_of(content: &Value) -> Option<String> {
    if let Some(s) = content.as_str() { return Some(s.to_string()); }
    let blocks = content.as_array()?;
    if blocks.iter().any(|b| b["type"] == "tool_result") { return None; }
    let text: Vec<&str> = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
    (!text.is_empty()).then(|| text.join(" "))
}

impl TaskCursor {
    pub fn new(alias: &str, codex: bool, sub: bool) -> Self {
        TaskCursor { alias: alias.to_string(), codex, sub, ..TaskCursor::default() }
    }

    pub fn open(&self) -> bool { self.task_ms > 0.0 }

    fn start(&mut self, at: f64, prompt: &str) {
        (self.task_ms, self.calls) = (at, 0);
        self.prompt = excerpt(prompt, TEXT_CHARS);
        self.pending.clear();
        self.failures.clear();
        self.asked_ms = 0.0;
    }

    fn end(&mut self) {
        self.task_ms = 0.0;
        self.pending.clear();
        self.failures.clear();
        self.asked_ms = 0.0;
    }

    fn call(&mut self, at: f64, id: &str, label: String, command: &str) {
        if !self.open() { self.start(at, ""); }
        self.calls += 1;
        self.asked_ms = if ASYNC_ASKS.contains(&label.as_str()) { at } else { 0.0 };
        let signature = signature_of(&label, command);
        self.pending.push(Pending { id: id.to_string(), label, command: excerpt(command, TEXT_CHARS), at_ms: at, signature });
    }

    fn result(&mut self, id: &str, error: bool) {
        let Some(i) = self.pending.iter().position(|p| p.id == id) else { return };
        let call = self.pending.remove(i);
        if error {
            let signature = if call.signature.is_empty() { format!("{}: {}", call.label, excerpt(&call.command, SIGNATURE_CHARS)) } else { call.signature };
            *self.failures.entry(signature).or_default() += 1;
        }
    }

    pub fn feed(&mut self, line: &Value) {
        let Some(at) = line["timestamp"].as_str().and_then(timestamp_ms) else { return };
        self.last_ms = self.last_ms.max(at);
        if self.cwd.is_empty() { if let Some(cwd) = line["cwd"].as_str().or(line["payload"]["cwd"].as_str()) { self.cwd = cwd.to_string(); } }
        if self.codex { self.feed_codex(line, at) } else { self.feed_claude(line, at) }
    }

    fn feed_claude(&mut self, line: &Value, at: f64) {
        let content = &line["message"]["content"];
        match line["type"].as_str() {
            // A compaction summary (`isCompactSummary`, also written by idle compaction after the turn
            // ended) is not a human prompt: it neither starts nor restarts a task.
            // An interruption marker (`[Request interrupted by user...]`) closes the turn; it is no prompt
            // (incident 812669328284927824950: a finished subagent stayed "silent" for 31 min).
            // A transcript-only queue entry (`queueTranscriptOnly`, e.g. the stale background-task
            // notification Claude Code records on resume) never reaches the model, so no turn follows
            // (incident 88333634518963469813: a finished session stayed "silent" for 31 min).
            Some("user") if line["isMeta"] != true && line["isCompactSummary"] != true && line["queueTranscriptOnly"] != true => match text_of(content) {
                Some(text) if text.starts_with("[Request interrupted") => self.end(),
                Some(prompt) => self.start(at, &prompt),
                None => for block in content.as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    self.result(block["tool_use_id"].as_str().unwrap_or(""), block["is_error"] == true);
                },
            },
            Some("assistant") => {
                for block in content.as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use") {
                    self.call(at, block["id"].as_str().unwrap_or(""), claude_label(block), &claude_command(&block["input"]));
                }
                if line["message"]["model"] == "<synthetic>" {
                    let text: Vec<&str> = content.as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect();
                    let text = text.join(" ");
                    if text.contains("limit") { (self.limit_ms, self.limit_text) = (at, excerpt(&text, TEXT_CHARS)); }
                }
                // Any stop other than a tool call hands the turn back (`end_turn`, a synthetic
                // limit reply's `stop_sequence`, `max_tokens`).
                let stop = line["message"]["stop_reason"].as_str().unwrap_or("");
                if !stop.is_empty() && stop != "tool_use" && self.pending.is_empty() { self.end(); }
            }
            _ => {}
        }
    }

    fn feed_codex(&mut self, line: &Value, at: f64) {
        let payload = &line["payload"];
        match (line["type"].as_str(), payload["type"].as_str()) {
            (Some("event_msg"), Some("task_started")) => self.start(at, ""),
            (Some("event_msg"), Some("task_complete" | "turn_aborted")) => self.end(),
            (Some("response_item"), Some("message")) if payload["role"] == "user" && self.prompt.is_empty() => {
                let text: Vec<&str> = payload["content"].as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect();
                if !self.open() { self.start(at, ""); }
                self.prompt = excerpt(&text.join(" "), TEXT_CHARS);
            }
            (Some("response_item"), Some("function_call" | "custom_tool_call")) => {
                let raw = payload["input"].as_str().or(payload["arguments"].as_str()).unwrap_or("");
                self.call(at, payload["call_id"].as_str().unwrap_or(""), codex_label(payload), raw);
            }
            (Some("response_item"), Some("function_call_output" | "custom_tool_call_output")) => {
                self.result(payload["call_id"].as_str().unwrap_or(""), false);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TaskCursor;
    use serde_json::{json, Value};

    fn mcp(secs: u32, id: &str, input: Value) -> [Value; 2] {
        let t = format!("2026-11-07T01:{secs:02}:00.000Z");
        [json!({"type": "assistant", "timestamp": t, "message": {"stop_reason": "tool_use", "content": [{"type": "tool_use", "id": id, "name": "mcp__cc-computer__js", "input": input}]}}),
         json!({"type": "user", "timestamp": t, "message": {"content": [{"type": "tool_result", "tool_use_id": id, "is_error": true}]}})]
    }

    fn failures(inputs: [Value; 3]) -> Vec<u64> {
        let mut c = TaskCursor::new("cc6", false, false);
        c.feed(&json!({"type": "user", "timestamp": "2026-11-07T01:00:00.000Z", "message": {"content": "e2e"}}));
        for (i, input) in inputs.into_iter().enumerate() { mcp(i as u32 + 1, &format!("c{i}"), input).iter().for_each(|l| c.feed(l)); }
        c.failures.values().copied().collect()
    }

    #[test]
    fn unrelated_mcp_errors_are_keyed_by_their_code_not_one_empty_command() {
        let js = |code: &str| json!({"code": code, "title": "t"});
        assert_eq!(failures([js("await cua.getApp(\"x\");"), js("execSync(\"security find\")"), js("fs.unlinkSync(p)")]), vec![1, 1, 1]);
        assert_eq!(failures([js("await cua.getApp(\"x\");"), js("await cua.getApp(\"x\");"), js("await cua.getApp(\"x\");")]), vec![3]);
        assert_eq!(failures([json!({"file_path": "/a"}), json!({"file_path": "/b"}), json!({})]), vec![1, 1, 1]);
    }
}
