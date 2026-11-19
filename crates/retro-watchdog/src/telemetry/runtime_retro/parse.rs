//! Incremental transcript reader for the runtime retro.
//!
//! Model time is the span from a model input (a user message or a tool result) to the last model
//! output before the next input; it is credited as outputs arrive, so a turn still running counts.
//! Tool wait is the span from the last output to the tool result that answers it, charged to the
//! first tool of that output that is not a plain read. Spans of CAP_MS or more are idle time.
pub(in crate::telemetry) use super::label::command_label;
use super::super::conversation_retro::{Layout, SOURCES};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub(in crate::telemetry) const CAP_MS: f64 = 30.0 * 60_000.0;
/// Bytes read from one transcript per scan, so one huge append cannot stall the thread.
const MAX_READ: u64 = 64 << 20;
/// A file not written for this long is forgotten; a later append starts at its end.
const KEEP_MS: f64 = 7.0 * 86_400_000.0;
/// Tools whose wait says nothing about where time goes when another tool ran beside them.
const PLAIN_READS: [&str; 5] = ["Read", "Edit", "Write", "Glob", "Grep"];

#[derive(Default, Clone, Serialize, Deserialize)]
pub(in crate::telemetry) struct Cursor {
    pub offset: u64,
    pub alias: String,
    pub sub: bool,
    pub codex: bool,
    pub mtime_ms: f64,
    /// Model ms credited from this file inside the current window.
    pub model_ms: f64,
    last_input: f64,
    last_output: f64,
    credited: f64,
    family: String,
    labels: Vec<String>,
    msg_id: String,
    msg_out: f64,
}

/// Window totals. Keys of the model maps are `alias side family` (side: lead or sub).
#[derive(Default, Clone, Serialize, Deserialize)]
pub(in crate::telemetry) struct Tally {
    pub model_ms: BTreeMap<String, f64>,
    pub turns: BTreeMap<String, f64>,
    pub output_tokens: BTreeMap<String, f64>,
    pub tool_ms: BTreeMap<String, f64>,
    pub tool_calls: BTreeMap<String, f64>,
}

fn add(map: &mut BTreeMap<String, f64>, key: &str, value: f64) {
    *map.entry(key.to_string()).or_default() += value;
}

impl Tally {
    pub fn model_hours(&self) -> f64 {
        // An empty f64 sum is -0.0.
        self.model_ms.values().sum::<f64>() / 3_600_000.0 + 0.0
    }

    /// Model calls: one per turn.
    pub fn model_calls(&self) -> f64 {
        self.turns.values().sum::<f64>() + 0.0
    }

    pub fn merge(&mut self, other: Tally) {
        for (mine, theirs) in [(&mut self.model_ms, other.model_ms), (&mut self.turns, other.turns),
            (&mut self.output_tokens, other.output_tokens), (&mut self.tool_ms, other.tool_ms), (&mut self.tool_calls, other.tool_calls)] {
            for (key, value) in theirs { add(mine, &key, value); }
        }
    }
}

impl Cursor {
    fn key(&self) -> String {
        format!("{} {} {}", self.alias, if self.sub { "sub" } else { "lead" }, self.family)
    }

    /// A new model input; `tool_result` charges the wait since the last output to its tool.
    fn input(&mut self, at: f64, tool_result: bool, tally: &mut Tally) {
        if tool_result && self.last_output > 0.0 {
            let wait = at - self.last_output;
            let label = self.labels.iter().find(|l| !PLAIN_READS.contains(&l.as_str())).or(self.labels.first());
            if let (Some(label), true) = (label, wait > 0.0 && wait < CAP_MS) {
                add(&mut tally.tool_ms, label, wait);
                add(&mut tally.tool_calls, label, 1.0);
            }
        }
        self.last_input = at;
        self.last_output = 0.0;
        self.credited = 0.0;
        self.labels.clear();
    }

    fn output(&mut self, at: f64, labels: Vec<String>, tally: &mut Tally) {
        self.labels.extend(labels);
        self.last_output = at;
        let span = at - self.last_input;
        if self.last_input <= 0.0 || span <= 0.0 || span >= CAP_MS || self.family.is_empty() {
            return;
        }
        let key = self.key();
        if self.credited == 0.0 { add(&mut tally.turns, &key, 1.0); }
        add(&mut tally.model_ms, &key, span - self.credited);
        self.model_ms += span - self.credited;
        self.credited = span;
    }

    /// Streamed records of one message repeat its usage; early ones carry a partial count.
    fn tokens(&mut self, id: &str, out: f64, tally: &mut Tally) {
        let key = self.key();
        if id.is_empty() || id != self.msg_id {
            self.msg_id = id.to_string();
            self.msg_out = out;
            add(&mut tally.output_tokens, &key, out);
        } else if out > self.msg_out {
            add(&mut tally.output_tokens, &key, out - self.msg_out);
            self.msg_out = out;
        }
    }

    pub(in crate::telemetry) fn feed(&mut self, line: &Value, tally: &mut Tally) {
        let Some(at) = line["timestamp"].as_str().and_then(timestamp_ms) else { return };
        if self.codex { self.feed_codex(line, at, tally) } else { self.feed_claude(line, at, tally) }
    }

    fn feed_claude(&mut self, line: &Value, at: f64, tally: &mut Tally) {
        let message = &line["message"];
        let blocks = message["content"].as_array().map(Vec::as_slice).unwrap_or_default();
        match line["type"].as_str() {
            Some("user") => self.input(at, blocks.iter().any(|b| b["type"] == "tool_result"), tally),
            Some("assistant") => {
                let model = message["model"].as_str().unwrap_or("");
                if model.is_empty() || model.starts_with('<') { return; }
                self.family = family(model);
                let labels = blocks.iter().filter(|b| b["type"] == "tool_use").map(claude_label).collect();
                self.output(at, labels, tally);
                if let Some(out) = message["usage"]["output_tokens"].as_f64() {
                    self.tokens(message["id"].as_str().unwrap_or(""), out, tally);
                }
            }
            _ => {}
        }
    }

    fn feed_codex(&mut self, line: &Value, at: f64, tally: &mut Tally) {
        let payload = &line["payload"];
        match (line["type"].as_str(), payload["type"].as_str()) {
            (Some("turn_context"), _) => if let Some(model) = payload["model"].as_str() { self.family = family(model) },
            (Some("response_item"), Some("message")) => match payload["role"].as_str() {
                Some("user") => self.input(at, false, tally),
                Some("assistant") => self.output(at, vec![], tally),
                _ => {}
            },
            (Some("response_item"), Some("function_call_output" | "custom_tool_call_output")) => self.input(at, true, tally),
            (Some("response_item"), Some("reasoning")) => self.output(at, vec![], tally),
            (Some("response_item"), Some("function_call" | "custom_tool_call")) => self.output(at, vec![codex_label(payload)], tally),
            (Some("event_msg"), Some("token_count")) => if let Some(out) = payload["info"]["last_token_usage"]["output_tokens"].as_f64() {
                self.tokens("", out, tally);
            },
            _ => {}
        }
    }
}

/// `claude-haiku-5-5` → `haiku`; other providers keep their model id.
pub(in crate::telemetry) fn family(model: &str) -> String {
    ["haiku", "sonnet", "opus", "fable"].iter().find(|f| model.contains(*f)).map_or(model.to_string(), |f| f.to_string())
}

pub(in crate::telemetry) fn claude_label(block: &Value) -> String {
    let name = block["name"].as_str().unwrap_or("?");
    if let Some(server) = name.strip_prefix("mcp__") {
        return format!("mcp:{}", server.split("__").next().unwrap_or(""));
    }
    match block["input"]["command"].as_str() {
        Some(command) if name == "Bash" => format!("Bash:{}", command_label(command)),
        _ => name.to_string(),
    }
}

pub(in crate::telemetry) fn codex_label(payload: &Value) -> String {
    let name = payload["name"].as_str().unwrap_or("?");
    // `exec` carries JavaScript that calls `exec_command({cmd:"…"})`; function calls carry JSON arguments.
    let raw = payload["input"].as_str().or(payload["arguments"].as_str()).unwrap_or("");
    let command = raw.find("cmd\":\"").map(|i| i + 6).or(raw.find("cmd:\"").map(|i| i + 5))
        .map(|start| raw[start..].split('"').next().unwrap_or("").replace("\\\\", "\\"));
    match command {
        Some(command) if !command.is_empty() => format!("{name}:{}", command_label(&command)),
        _ => name.to_string(),
    }
}

/// `2026-10-22T14:57:39.174Z` → Unix ms; any other shape is skipped.
pub(crate) fn timestamp_ms(text: &str) -> Option<f64> {
    let b = text.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[10] != b'T' || !text.ends_with('Z') { return None; }
    let n = |r: std::ops::Range<usize>| text.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm) = (n(11..13)?, n(14..16)?);
    let secs: f64 = text.get(17..text.len() - 1)?.parse().ok()?;
    // Days from civil (Howard Hinnant), proleptic Gregorian.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some(((days * 86_400 + hh * 3600 + mm * 60) as f64 + secs) * 1000.0)
}

/// Reads every transcript written since `last_scan_ms`. A file first seen that was created before
/// the window starts at its end, so history before the window never counts.
pub(in crate::telemetry) fn scan(home: &Path, mut files: BTreeMap<String, Cursor>, window_from: f64, last_scan_ms: f64, now_ms: f64)
    -> (BTreeMap<String, Cursor>, Tally) {
    let mut tally = Tally::default();
    for (alias, dir, layout) in SOURCES {
        walk(&home.join(dir), 3, &mut |path, created, modified, len| {
            let key = path.to_string_lossy().into_owned();
            if modified < last_scan_ms - 120_000.0 && files.contains_key(&key) { return; }
            let cursor = files.entry(key).or_insert_with(|| Cursor {
                offset: if created >= window_from { 0 } else { len }, alias: alias.to_string(),
                sub: path.components().any(|c| c.as_os_str() == "subagents"), codex: layout == Layout::Codex, ..Cursor::default()
            });
            cursor.mtime_ms = modified;
            read_new(path, len, cursor, &mut tally);
        }, &|name| match layout {
            Layout::Codex => name.starts_with("rollout-") && name.ends_with(".jsonl"),
            _ => name.ends_with(".jsonl") && !name.starts_with("ag-boot"),
        });
    }
    files.retain(|_, c| now_ms - c.mtime_ms < KEEP_MS);
    (files, tally)
}

fn read_new(path: &Path, len: u64, cursor: &mut Cursor, tally: &mut Tally) {
    let mut offset = cursor.offset;
    read_lines(path, len, &mut offset, &mut |value| cursor.feed(value, tally));
    cursor.offset = offset;
}

/// Feeds the whole JSON lines appended after `offset` and advances it past them.
pub(in crate::telemetry) fn read_lines(path: &Path, len: u64, offset: &mut u64, feed: &mut dyn FnMut(&Value)) {
    if len < *offset { *offset = len; }
    if len == *offset { return; }
    let Ok(mut file) = std::fs::File::open(path) else { return };
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(*offset)).is_err() || file.take(MAX_READ).read_to_end(&mut bytes).is_err() { return; }
    // Only whole lines; a line still being written is read next time.
    let Some(end) = bytes.iter().rposition(|b| *b == b'\n') else {
        if bytes.len() as u64 >= MAX_READ { *offset += MAX_READ; }
        return;
    };
    for line in bytes[..end].split(|b| *b == b'\n') {
        if let Ok(value) = serde_json::from_slice::<Value>(line) { feed(&value); }
    }
    *offset += end as u64 + 1;
}

pub(in crate::telemetry) fn walk(dir: &Path, depth: usize, found: &mut dyn FnMut(&Path, f64, f64, u64), wanted: &dyn Fn(&str) -> bool) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let ms = |t: std::io::Result<std::time::SystemTime>| t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64() * 1000.0);
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            if depth > 0 { walk(&entry.path(), depth - 1, found, wanted); }
        } else if wanted(&entry.file_name().to_string_lossy()) {
            found(&entry.path(), ms(meta.created()), ms(meta.modified()), meta.len());
        }
    }
}
