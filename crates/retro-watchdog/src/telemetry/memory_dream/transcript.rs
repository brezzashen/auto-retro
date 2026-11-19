//! What one session transcript tells a memory dream: the notes the prompt hook injected, the skills
//! it loaded, its failed tool results, the task it started with, whether it delivered a report and
//! where the user approved a result. Read once per file; `sessions.rs` groups the sessions.
use super::super::transcript_calls;
use regex::Regex;
use serde_json::Value;
use std::{collections::BTreeMap, path::{Path, PathBuf}, sync::LazyLock};

/// A typed reaction, not a pasted document.
const REACTION_CHARS: usize = 120;
/// Characters of the first request that name its task family once numbers and ids are masked.
const FAMILY_CHARS: usize = 48;
/// Start-anchored only: the `retro_repeats` census also counts error words inside a log or transcript the
/// session was reading, which ranks the sessions that read them first.
static FAILED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(exit code [1-9]|<error>|error:)").unwrap());
/// Calls only a session that audits other sessions makes.
const AUDITOR: [&str; 4] = ["cpu-watchdog/analysis/", "cpu-watchdog retro-", "cpu-watchdog monitor-ack", "cpu-watchdog dream-sessions"];
static INJECTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"### ([a-z0-9][a-z0-9-]*) \(\d\.\d\d · ").unwrap());
static SKILL_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/skills/([A-Za-z0-9_-]+)/").unwrap());
/// Approval in the user's own short follow-up. A heuristic that selects candidates; the audit reads the turn.
static PRAISE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^对的|很好|非常好|挺好|不错|完美|太好了|太棒了|漂亮|厉害|就是这样|great|perfect|awesome").unwrap());
static VARIABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9a-f]{6,}|\d+").unwrap());
/// A working directory inside a session's scratchpad: `…/<session id>/scratchpad[/…]`.
static SCRATCHPAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})/scratchpad(/|$)").unwrap());
/// A compacted session opens with its summary, not with a task.
const CONTINUED: &str = "This session is being continued";
/// The page every delivered task ends with.
const DELIVERED: &str = "reports.example.com";

#[derive(Default)]
pub(super) struct Session {
    pub path: PathBuf,
    pub title: String,
    pub auditor: bool,
    /// Started inside another session's scratchpad: a harness run (A/B eval, probe) of that session's synthetic
    /// prompt, not a task of its own (2026-11-11: eight report-skill A/B runs formed a false `best:` family).
    pub harness: bool,
    pub calls: usize,
    /// `(call index, line)` of each failed result.
    pub failed: Vec<(usize, usize)>,
    /// `(name, call index, line)`: first injection of each note, first load of each skill.
    pub notes: Vec<(String, usize, usize)>,
    pub skills: Vec<(String, usize, usize)>,
    /// The first request with numbers and ids masked: runs of one recurring task share it. Empty for a
    /// session that began before the window or opens with a compaction summary (`no_task`).
    pub family: String,
    no_task: bool,
    /// The session ended with the report link: no tool call came after it.
    pub delivered: bool,
    /// `(line, message)` of each approving follow-up.
    pub praise: Vec<(usize, String)>,
}

fn first(list: &mut Vec<(String, usize, usize)>, name: &str, calls: usize, line: usize) {
    if !name.is_empty() && !list.iter().any(|(n, _, _)| n == name) { list.push((name.to_string(), calls, line)); }
}

/// The text a user typed in this row: not a tool result, hook output or command echo (those start with `<`).
fn typed(row: &Value) -> Option<String> {
    if row["type"] != "user" || row["isMeta"] == true { return None; }
    let content = &row["message"]["content"];
    let text = content.as_str().map(str::to_string).or_else(|| {
        let parts: Vec<&str> = content.as_array()?.iter().filter(|c| c["type"] == "text").filter_map(|c| c["text"].as_str()).collect();
        (!parts.is_empty()).then(|| parts.join(" "))
    })?;
    let text = text.trim();
    (!text.is_empty() && !text.starts_with('<')).then(|| text.to_string())
}

fn skill_of(item: &Value) -> Option<String> {
    match item["name"].as_str()? {
        "Skill" => item["input"]["skill"].as_str().map(str::to_string),
        "Read" => SKILL_PATH.captures(item["input"]["file_path"].as_str()?).map(|c| c[1].to_string()),
        _ => None,
    }
}

fn user_turn(s: &mut Session, said: String, line: usize) {
    if s.family.is_empty() && !s.no_task {
        if said.starts_with(CONTINUED) { s.no_task = true; return; }
        s.family = VARIABLE.replace_all(&said.chars().take(FAMILY_CHARS).collect::<String>(), "#").into_owned();
    } else if said.chars().count() <= REACTION_CHARS && PRAISE.is_match(&said) {
        s.praise.push((line, said));
    }
}

/// One transcript inside `from_ms..=to_ms`.
fn read(path: &Path, from_ms: f64, to_ms: f64) -> Session {
    let mut s = Session { path: path.to_path_buf(), ..Default::default() };
    let Ok(text) = std::fs::read_to_string(path) else { return s };
    for (index, line) in text.lines().enumerate() {
        if line.contains("\"custom-title\"") {
            if let Some(t) = serde_json::from_str::<Value>(line).ok().and_then(|v| v["customTitle"].as_str().map(str::to_string)) { s.title = t; }
            continue;
        }
        if !["hook_additional_context", "tool", "\"type\":\"user\"", DELIVERED].iter().any(|n| line.contains(n)) { continue; }
        let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
        let Some(at) = row["timestamp"].as_str().and_then(crate::cause_tree::attribute::parse_time) else { continue };
        if let Some(owner) = row["cwd"].as_str().and_then(|c| SCRATCHPAD.captures(c)) {
            // Its own session: the file stem, or the `<session>/subagents/` directory of a subagent transcript. The
            // project directory name of a harness run also holds the owner's id, so a substring test is not enough.
            let own = path.file_stem().into_iter().chain(path.iter()).any(|c| c.to_string_lossy() == owner[1]);
            s.harness |= !own;
        }
        if at < from_ms { s.no_task = true; }
        if at < from_ms || at > to_ms { continue; }
        if row["attachment"]["type"] == "hook_additional_context" {
            for part in row["attachment"]["content"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|p| p.contains("agent-memory")) {
                for c in INJECTED.captures_iter(part) { first(&mut s.notes, &c[1], s.calls, index + 1); }
            }
            continue;
        }
        if let Some(said) = typed(&row) { user_turn(&mut s, said, index + 1); }
        for item in row["message"]["content"].as_array().into_iter().flatten() {
            if let Some(skill) = skill_of(item) { first(&mut s.skills, &skill, s.calls, index + 1); }
            s.delivered |= row["type"] == "assistant" && item["text"].as_str().is_some_and(|t| t.contains(DELIVERED));
        }
        for (result, text) in transcript_calls::entries(&row) {
            if !result {
                s.calls += 1;
                s.delivered = false;
                s.auditor |= AUDITOR.iter().any(|m| text.contains(m));
            } else if FAILED.is_match(text.trim_start()) {
                s.failed.push((s.calls, index + 1));
            }
        }
    }
    s
}

/// Sessions of the window by id, auditors left out; a session resumed under another account keeps
/// its longest copy.
pub(super) fn sessions(home: &Path, from_ms: f64, to_ms: f64) -> BTreeMap<String, Session> {
    let mut out: BTreeMap<String, Session> = BTreeMap::new();
    for (_, path) in transcript_calls::files(home, from_ms) {
        let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        let s = read(&path, from_ms, to_ms);
        if s.calls > 0 && !s.auditor && out.get(&id).is_none_or(|old| old.calls < s.calls) { out.insert(id, s); }
    }
    out
}
