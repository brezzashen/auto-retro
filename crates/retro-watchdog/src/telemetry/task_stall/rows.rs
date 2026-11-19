//! The open tasks of one scan and their stalls.
//!
//! Only the newest transcript of a session id counts: a session moved to another account
//! (limit-resume) continues under another profile and leaves the old file behind. A stalled task
//! whose agent process has ended (`session-status`: no live Claude or Codex process) is listed as
//! `alive: false` and never escalated; that is an abandoned or handed-off task, not a stall.
//! `session-status` reports a live process only for Claude sessions, so a Codex task reads as
//! ended from its own transcript instead (`codex_turn_gone`) or from the pid that last logged
//! its thread (`codex_live`, for code-mode `exec` calls whose clock sits in script text).
//!
//! A recurring job whose runs end on the synthetic usage-limit reply (a launchd backstop on an
//! exhausted account) never runs a task long, yet its work has not happened for as long as the
//! replies span: QUOTA_HITS replies with the same prompt over TASK_MIN or more form one
//! `quota_blocked` row.
use super::{codex_live, exclusive, heal, simq, parse::{TaskCursor, NOT_STALLS}, BLOCK_MIN, DROP_MS, REPEAT_FAILS, SILENT_MIN, TASK_MIN};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

const QUOTA_HITS: usize = 2;
/// Prompt prefix that names a recurring job.
const JOB_PROMPT_CHARS: usize = 80;

fn minutes(ms: f64) -> f64 { (ms / 6_000.0).round() / 10.0 }

/// The stalls of one task at `now`; empty while it is short, closed or moving.
pub(super) fn stalls(home: &Path, session: &str, c: &TaskCursor, now: f64, bus: &BTreeMap<String, heal::JobState>) -> Vec<Value> {
    if !c.open() || now - c.task_ms < TASK_MIN * 60_000.0 || now - c.last_ms.max(c.mtime_ms) >= DROP_MS { return vec![]; }
    let mut out = Vec::new();
    for p in c.pending.iter().filter(|p| !NOT_STALLS.contains(&p.label.as_str()) && now - p.at_ms >= BLOCK_MIN * 60_000.0) {
        out.push(json!({"kind": "blocked_call", "label": p.label, "command": p.command, "call": p.id, "min": minutes(now - p.at_ms),
            "waits_on": waits_on(home, session, &p.command, now, bus)}));
    }
    // Quiet after an async question waits on the user's answer (`TaskCursor::asked_ms`).
    if c.pending.is_empty() && c.asked_ms == 0.0 && now - c.last_ms >= SILENT_MIN * 60_000.0 {
        out.push(json!({"kind": "silent", "min": minutes(now - c.last_ms)}));
    }
    for (signature, n) in c.failures.iter().filter(|(_, n)| **n >= REPEAT_FAILS) {
        out.push(json!({"kind": "repeat_failure", "signature": signature, "errors": n}));
    }
    out
}

/// What a blocked call waits on: the job-bus job or release run its command names, else the
/// session's simulator lease queue ticket (`simq.rs`), else the command class. A render-queue
/// exclusive window open now rides along as `exclusive_window` (`exclusive.rs`): it holds new builds.
fn waits_on(home: &Path, session: &str, command: &str, now: f64, bus: &BTreeMap<String, heal::JobState>) -> Value {
    let named = heal::waits_on(home, command, bus);
    if matches!(named["kind"].as_str(), Some("job-bus" | "release")) { return named; }
    let mut out = match simq::waiting(home, session, now / 1000.0) {
        Some(mut q) => { q["class"] = named["kind"].clone(); q }
        None => named,
    };
    if let Some(window) = exclusive::open(home, now, |pid| cfg!(test) || heal::alive(pid)) { out["exclusive_window"] = window; }
    out
}

pub(super) fn row(path: &str, c: &TaskCursor, now: f64, stalls: Vec<Value>) -> Value {
    json!({"path": path, "source": c.alias, "subagent": c.sub, "cwd": c.cwd, "prompt": c.prompt, "task_start_ms": c.task_ms, "report_ms": c.task_ms,
        "open_min": minutes(now - c.task_ms), "calls": c.calls, "pending": c.pending, "stalls": stalls, "alive": true})
}

fn stem(path: &str) -> &str {
    Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or(path)
}

/// Whether a Claude or Codex process still serves the session, and the Orca tab title it runs in
/// (`session-status --json` → `live`); an unreadable answer counts as alive with no tab.
fn live(home: &Path, path: &str) -> (bool, Value) {
    if cfg!(test) { return (true, Value::Null); }
    let out = std::process::Command::new(home.join(".local/bin/session-status"))
        .args([stem(path), "--json", "--limit", "10", "--since", "6h"]).output();
    let Ok(out) = out else { return (true, Value::Null) };
    let Ok(value) = serde_json::from_slice::<Value>(&out.stdout) else { return (true, Value::Null) };
    live_of(&value, stem(path))
}

/// The liveness of session `id` in a `session-status --json` answer. The words match any session
/// whose prompts or answers mention the id (a handoff prompt quoting the parent's scratchpad path
/// ranks first), so only the row whose own `id` equals it counts; no such row reads as alive.
pub(super) fn live_of(value: &Value, id: &str) -> (bool, Value) {
    let rows = if value.is_array() { value.as_array().cloned().unwrap_or_default() } else { vec![value.clone()] };
    let Some(own) = rows.iter().find(|r| r["id"] == id) else { return (true, Value::Null) };
    (own["live"].is_object() && own["live"]["status"] != "shell", own["live"]["name"].clone())
}

/// Whether a Codex task's turn has ended without a closing event: a pending call that returns on
/// its own clock (`duration_ms` of `sleep`, `yield_time_ms` of `exec_command`/`write_stdin`)
/// has overrun that clock by BLOCK_MIN. Only a process that is gone (its Orca tab closed after a
/// handoff, incident 811852163878791633152) leaves such a call open; nothing still waits on it.
pub(super) fn codex_turn_gone(c: &TaskCursor, now: f64) -> bool {
    c.pending.iter().any(|p| {
        let Ok(args) = serde_json::from_str::<Value>(&p.command) else { return false };
        let Some(clock) = args["duration_ms"].as_f64().or(args["yield_time_ms"].as_f64()) else { return false };
        now - p.at_ms >= clock + BLOCK_MIN * 60_000.0
    })
}

fn quota_rows(files: &BTreeMap<String, TaskCursor>, now: f64) -> Vec<Value> {
    let mut jobs: BTreeMap<(String, String), Vec<(&String, &TaskCursor)>> = BTreeMap::new();
    for (path, c) in files.iter().filter(|(_, c)| c.limit_ms > 0.0 && now - c.limit_ms < DROP_MS) {
        jobs.entry((c.alias.clone(), c.prompt.chars().take(JOB_PROMPT_CHARS).collect())).or_default().push((path, c));
    }
    jobs.into_iter().filter_map(|((alias, prompt), mut hits)| {
        hits.sort_by(|a, b| a.1.limit_ms.total_cmp(&b.1.limit_ms));
        let (first, last) = (hits.first()?.1, hits.last()?);
        let span = last.1.limit_ms - first.limit_ms;
        (hits.len() >= QUOTA_HITS && span >= TASK_MIN * 60_000.0).then(|| json!({
            "path": last.0, "source": alias, "subagent": false, "cwd": last.1.cwd, "prompt": prompt, "task_start_ms": first.limit_ms, "report_ms": last.1.limit_ms,
            "open_min": minutes(now - first.limit_ms), "calls": 0, "pending": [], "alive": true,
            "stalls": [{"kind": "quota_blocked", "hits": hits.len(), "span_min": minutes(span), "first_ms": first.limit_ms,
                "last_ms": last.1.limit_ms, "text": last.1.limit_text}]}))
    }).collect()
}

/// Open tasks of at least `min` minutes, newest transcript per session, with stalls and liveness,
/// plus the `quota_blocked` jobs; longest first.
pub(super) fn open_rows(home: &Path, files: &BTreeMap<String, TaskCursor>, now: f64, min: f64) -> Vec<Value> {
    let bus = heal::jobs(&home.join(".local/state/job-bus/events.jsonl"));
    let mut newest: BTreeMap<&str, (&String, &TaskCursor)> = BTreeMap::new();
    for (path, c) in files {
        let slot = newest.entry(stem(path)).or_insert((path, c));
        if c.mtime_ms > slot.1.mtime_ms { *slot = (path, c); }
    }
    let mut rows: Vec<Value> = newest.into_values().filter(|(_, c)| c.open() && now - c.task_ms >= min * 60_000.0 && now - c.mtime_ms < DROP_MS)
        .map(|(p, c)| {
            let found = stalls(home, &simq::session_of(p), c, now, &bus);
            let mut r = row(p, c, now, found);
            if r["stalls"].as_array().is_some_and(|s| !s.is_empty()) {
                let gone = c.codex && (codex_turn_gone(c, now) || codex_live::process_alive(p, heal::alive) == Some(false));
                let (alive, tab) = if gone { (false, Value::Null) } else { live(home, p) };
                (r["alive"], r["tab"]) = (json!(alive), tab);
            }
            r
        }).collect();
    rows.extend(quota_rows(files, now));
    rows.sort_by(|a, b| b["open_min"].as_f64().unwrap_or(0.0).total_cmp(&a["open_min"].as_f64().unwrap_or(0.0)));
    rows
}

/// Rows that open an incident: stalled, and either alive or a `quota_blocked` job.
pub(super) fn escalating(row: &Value) -> bool {
    row["alive"] == true && row["stalls"].as_array().is_some_and(|s| !s.is_empty())
}
