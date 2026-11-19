//! Incremental transcript reader for the wait retro: the time each agent session sat blocked on a
//! tool call.
//!
//! A wait is the span from the last model output to the tool result that answers it, the same span
//! the runtime retro charges as tool wait, so parallel calls count once. It is charged to the first
//! call of that output that is not a plain read. Questions to the user and subagent dispatches are
//! not waits (the subagent's own transcript counts its waits). Spans under MIN_MS are ordinary tool
//! work; spans of CAP_MS or more are a sleeping host or a resumed session.
use super::super::conversation_retro::{Layout, SOURCES};
use super::super::runtime_retro::parse::{claude_label, codex_label, read_lines, timestamp_ms, walk};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

mod bridge;

pub(in crate::telemetry) const MIN_MS: f64 = 10_000.0;
pub(in crate::telemetry) const CAP_MS: f64 = 4.0 * 3_600_000.0;
/// Longest waits kept per session as evidence.
const TOP: usize = 8;
const COMMAND_CHARS: usize = 240;
/// A file not written for this long is forgotten; a later append starts at its end.
const KEEP_MS: f64 = 7.0 * 86_400_000.0;
/// Antigravity's read_file, edit_file and write_file are its Read, Edit and Write.
const PLAIN_READS: [&str; 8] = ["Read", "Edit", "Write", "Glob", "Grep", "read_file", "edit_file", "write_file"];
const NOT_WAITS: [&str; 7] = ["AskUserQuestion", "ExitPlanMode", "EnterPlanMode", "Agent", "Task", "request_user_input", "invoke_subagent"];
/// First matching class wins; needles are matched against the label, then the whole command text
/// (a chained build or deliver step often sits past the stored excerpt). Cargo's `--release` profile
/// flag and its output directory (`target/release/<bin>`, a built tool being run) are not a release.
const CLASSES: [(&str, &[&str]); 6] = [
    // Driving a leased page is page work even when the script also takes the lease or loops over lines
    // (wait retro 815645674916169722875: 42 min of agent-browser forms were filed as queue_lease and polling).
    ("browser", &["agent-browser", "AB open", "AB click", "AB fill", "AB eval", "AB type", "AB wait", "lease-page.js", "connectOverCDP"]),
    ("polling", &["until", "while", "sleep", "select(undef", "Bun.sleep", "kill -0", "Monitor", "TaskOutput", "write_stdin", "job-bus wait", "session-status"]),
    ("release_deploy", &["deliver", "bun run canary", "bun run k8s", "release", "deploy", "testflight", "kubectl rollout", "docker push"]),
    ("queue_lease", &["sim-lease", "browser-lease", "render-queue", "job-bus", "acquire", "lease", "queue", "offload"]),
    ("build_test", &["cpu-build-gate", "cargo", "xcodebuild", "bun test", "bun run build", "tsc", "swift build", "simRun", "capture.js", "run.js", "docker build", "docker compose"]),
    ("network", &["ssh", "curl", "gcloud", "git fetch", "git push", "git pull", "scp", "rsync"]),
];

#[derive(Default, Clone, Serialize, Deserialize)]
pub(in crate::telemetry) struct Wait {
    pub at_ms: f64,
    pub seconds: f64,
    pub label: String,
    pub class: String,
    pub command: String,
    /// Tool call id (Claude `tool_use.id`, Codex `call_id`), to find the call in the transcript.
    pub call: String,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct Call {
    label: String,
    command: String,
    id: String,
    /// Class read from the whole command before it is cut to an excerpt.
    #[serde(default)]
    class: String,
}

impl Call {
    fn new(label: String, raw: &str, id: &str) -> Self {
        Call { class: class_of(&label, raw).to_string(), command: excerpt(raw), label, id: id.to_string() }
    }
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub(in crate::telemetry) struct Cursor {
    pub offset: u64,
    pub alias: String,
    pub sub: bool,
    pub codex: bool,
    pub mtime_ms: f64,
    pub cwd: String,
    /// Blocked ms since this session was last reviewed.
    pub wait_ms: f64,
    pub waits: f64,
    pub first_ms: f64,
    pub last_ms: f64,
    pub classes: BTreeMap<String, f64>,
    pub top: Vec<Wait>,
    /// Over the threshold and listed for the next retro.
    pub flagged: bool,
    /// Waits ending before this are skipped (`wait-stats` reads whole files).
    #[serde(skip)]
    since_ms: f64,
    last_output: f64,
    calls: Vec<Call>,
    /// An Orca-bridged Antigravity transcript (`bridge.rs`), set from the path on every scan.
    #[serde(skip)]
    bridge: bool,
    /// Stamped tool span of the bridged step still collecting results.
    #[serde(default)]
    stamp_from: f64,
    #[serde(default)]
    stamp_to: f64,
    /// Totals use the stamped measure; a bridged cursor saved before it counted model time and
    /// starts again from zero once (wait retro 83168070974659841531: 66 cursors, 456 min).
    #[serde(default)]
    stamped: bool,
}

/// Page work anywhere in the command wins over every label: a lease-page run whose label is the first word of a
/// CSV-building `while read` loop was filed as polling (wait retro 85883150729621603936: 6.6 of 6.8 polling minutes).
pub(in crate::telemetry) fn class_of(label: &str, command: &str) -> &'static str {
    let command = command.replace("--release", "").replace("/release/", "/");
    let (browser, browser_needles) = CLASSES[0];
    if browser_needles.iter().any(|n| label.contains(n) || command.contains(n)) {
        return browser;
    }
    CLASSES.iter().find(|(_, needles)| needles.iter().any(|n| label.contains(n)))
        .or_else(|| CLASSES.iter().find(|(_, needles)| needles.iter().any(|n| command.contains(n))))
        .map_or("other", |(class, _)| class)
}

fn excerpt(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(COMMAND_CHARS).collect()
}

impl Cursor {
    pub fn new(alias: &str, codex: bool, path: &Path) -> Self {
        Cursor { alias: alias.to_string(), codex, sub: path.components().any(|c| c.as_os_str() == "subagents"), bridge: is_bridge(path), ..Cursor::default() }
    }

    /// Clears the totals after a retro took this session; later waits count toward the next one.
    pub fn reset(&mut self) {
        (self.wait_ms, self.waits, self.first_ms, self.last_ms, self.flagged) = (0.0, 0.0, 0.0, 0.0, false);
        self.classes.clear();
        self.top.clear();
    }

    fn input(&mut self, at: f64, tool_result: bool) {
        if tool_result && self.last_output > 0.0 {
            let span = at - self.last_output;
            if let Some(call) = self.waiting_call() {
                if (MIN_MS..CAP_MS).contains(&span) && at >= self.since_ms { self.charge(at, span, call); }
            }
        }
        self.last_output = 0.0;
        self.calls.clear();
    }

    /// The call a step's wait is charged to: its first call that is not a plain read, unless the
    /// step only read or asked.
    fn waiting_call(&self) -> Option<Call> {
        let call = self.calls.iter().find(|c| !PLAIN_READS.contains(&c.label.as_str())).or(self.calls.first());
        call.filter(|c| !NOT_WAITS.contains(&c.label.as_str()) && !PLAIN_READS.contains(&c.label.as_str())).cloned()
    }

    fn charge(&mut self, at: f64, span: f64, call: Call) {
        let class = if call.class.is_empty() { class_of(&call.label, &call.command).to_string() } else { call.class.clone() };
        self.wait_ms += span;
        self.waits += 1.0;
        if self.first_ms == 0.0 { self.first_ms = at - span; }
        self.last_ms = at;
        *self.classes.entry(class.clone()).or_default() += span;
        self.top.push(Wait { at_ms: at - span, seconds: (span / 100.0).round() / 10.0, label: call.label, class, command: call.command, call: call.id });
        self.top.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
        self.top.truncate(TOP);
    }

    fn output(&mut self, at: f64, calls: Vec<Call>) {
        self.calls.extend(calls);
        self.last_output = at;
    }

    pub fn feed(&mut self, line: &Value) {
        let Some(at) = line["timestamp"].as_str().and_then(timestamp_ms) else { return };
        if self.cwd.is_empty() { if let Some(cwd) = line["cwd"].as_str().or(line["payload"]["cwd"].as_str()) { self.cwd = cwd.to_string(); } }
        if self.codex { self.feed_codex(line, at) } else if self.bridge { self.feed_bridge(line, at) } else { self.feed_claude(line, at) }
    }

    fn feed_claude(&mut self, line: &Value, at: f64) {
        let blocks = line["message"]["content"].as_array().map(Vec::as_slice).unwrap_or_default();
        match line["type"].as_str() {
            Some("user") => self.input(at, blocks.iter().any(|b| b["type"] == "tool_result")),
            Some("assistant") => {
                let calls = blocks.iter().filter(|b| b["type"] == "tool_use")
                    .map(|b| Call::new(claude_label(b), b["input"]["command"].as_str().unwrap_or(""), b["id"].as_str().unwrap_or(""))).collect();
                self.output(at, calls);
            }
            _ => {}
        }
    }

    fn feed_codex(&mut self, line: &Value, at: f64) {
        let payload = &line["payload"];
        match (line["type"].as_str(), payload["type"].as_str()) {
            (Some("response_item"), Some("message")) => match payload["role"].as_str() {
                Some("user") => self.input(at, false),
                Some("assistant") => self.output(at, vec![]),
                _ => {}
            },
            (Some("response_item"), Some("function_call_output" | "custom_tool_call_output")) => self.input(at, true),
            (Some("response_item"), Some("reasoning")) => self.output(at, vec![]),
            (Some("response_item"), Some("function_call" | "custom_tool_call")) => {
                let raw = payload["input"].as_str().or(payload["arguments"].as_str()).unwrap_or("");
                self.output(at, vec![Call::new(codex_label(payload), raw, payload["call_id"].as_str().unwrap_or(""))]);
            }
            _ => {}
        }
    }
}

/// Reads every transcript written since `last_scan_ms`. A file first seen that was created before
/// `window_from` starts at its end, so history before the watch never counts.
pub(in crate::telemetry) fn scan(home: &Path, mut files: BTreeMap<String, Cursor>, window_from: f64, last_scan_ms: f64, now_ms: f64) -> BTreeMap<String, Cursor> {
    // A file first seen is read from 0 only when it was created after the last scan. One created
    // earlier lost its cursor (load error, panicked scan, KEEP_MS) or predates the watch; reading it
    // from 0 counted waits a retro had already reviewed (wait retro 86145098471064008529).
    let fresh = window_from.max(last_scan_ms - 120_000.0);
    each(home, &mut |alias, codex, path, created, modified, len| {
        let key = path.to_string_lossy().into_owned();
        if modified < last_scan_ms - 120_000.0 && files.contains_key(&key) { return; }
        let cursor = files.entry(key).or_insert_with(|| Cursor { offset: if created >= fresh { 0 } else { len }, ..Cursor::new(alias, codex, path) });
        cursor.mtime_ms = modified;
        cursor.bridge = is_bridge(path);
        if cursor.bridge && !cursor.stamped { cursor.reset(); cursor.stamped = true; }
        let mut offset = cursor.offset;
        read_lines(path, len, &mut offset, &mut |value| cursor.feed(value));
        cursor.offset = offset;
    });
    files.retain(|_, c| now_ms - c.mtime_ms < KEEP_MS || c.flagged);
    files
}

/// Every transcript written since `from`, read whole, counting only waits that end after `from`.
pub(in crate::telemetry) fn scan_since(home: &Path, from: f64) -> BTreeMap<String, Cursor> {
    let mut files = BTreeMap::new();
    each(home, &mut |alias, codex, path, _, modified, len| {
        if modified < from { return; }
        let mut cursor = Cursor { since_ms: from, mtime_ms: modified, ..Cursor::new(alias, codex, path) };
        let mut offset = 0;
        read_lines(path, len, &mut offset, &mut |value| cursor.feed(value));
        cursor.offset = offset;
        files.insert(path.to_string_lossy().into_owned(), cursor);
    });
    files
}

fn is_bridge(path: &Path) -> bool {
    path.parent().and_then(|p| p.file_name()).is_some_and(|n| n.to_string_lossy().ends_with(bridge::SLUG))
}

pub(in crate::telemetry) fn each(home: &Path, found: &mut dyn FnMut(&str, bool, &Path, f64, f64, u64)) {
    for (alias, dir, layout) in SOURCES {
        walk(&home.join(dir), 3, &mut |path, created, modified, len| found(alias, layout == Layout::Codex, path, created, modified, len), &|name| match layout {
            Layout::Codex => name.starts_with("rollout-") && name.ends_with(".jsonl"),
            _ => name.ends_with(".jsonl") && !name.starts_with("ag-boot"),
        });
    }
}

#[cfg(test)]
pub(in crate::telemetry) fn bridge_ms_for_test(text: &str) -> Option<f64> { bridge::rfc3339_ms(text) }
