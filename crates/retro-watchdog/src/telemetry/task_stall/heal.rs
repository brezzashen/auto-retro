//! Stop-bleeding for the task stall watch (founder, 2026-10-29: "优先保证止血"), and what a blocked
//! call is waiting on.
//!
//! A job-bus job that never got a terminal event keeps every `job-bus wait` and `deliver wait` on
//! it blocked until their timeout. A job is an orphan once its state event is not terminal for
//! ORPHAN_MIN and its producer is provably gone: a `release` job whose board file is closed
//! (`succeeded`/`detached` → `finished`, `failed` → `failed`), or a job of another topic whose
//! emitting pids are all dead (→ `cancelled`). Each orphan gets its terminal event through the
//! `job-bus emit` CLI, so subscribers are told too. A `running` board run is left to
//! `deliver hygiene`. Nothing else is touched.
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

pub(super) const ORPHAN_MIN: f64 = 30.0;
const TERMINAL: [&str; 3] = ["finished", "failed", "cancelled"];

pub(super) fn alive(pid: i64) -> bool {
    pid > 0 && (unsafe { libc::kill(pid as i32, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

#[derive(Default, Debug, Clone)]
pub(in crate::telemetry) struct JobState { pub topic: String, pub event: String, pub ms: f64, pub pids: Vec<i64> }

/// Each job's state event, a terminal event winning over later non-terminal ones (job-bus `latest`).
pub(super) fn jobs(events: &Path) -> BTreeMap<String, JobState> {
    let mut out: BTreeMap<String, JobState> = BTreeMap::new();
    for line in std::fs::read_to_string(events).unwrap_or_default().lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        let (Some(job), Some(event), Some(ms)) = (e["job"].as_str(), e["event"].as_str(), e["ms"].as_f64()) else { continue };
        let topic = e["topic"].as_str().unwrap_or("").to_string();
        let state = out.entry(format!("{topic}|{job}")).or_default();
        if let Some(pid) = e["pid"].as_i64() { if !state.pids.contains(&pid) { state.pids.push(pid); } }
        if TERMINAL.contains(&state.event.as_str()) && !TERMINAL.contains(&event) { continue; }
        (state.topic, state.event, state.ms) = (topic, event.to_string(), ms);
    }
    out
}

/// The terminal event an orphan should get, or None while its producer may still write.
pub(super) fn verdict(state: &JobState, job: &str, releases: &Path, now_ms: f64) -> Option<(&'static str, String)> {
    if TERMINAL.contains(&state.event.as_str()) || now_ms - state.ms < ORPHAN_MIN * 60_000.0 { return None; }
    if state.topic == "release" {
        let board = super::super::read(&releases.join(format!("{job}.json")))?;
        return match board["state"].as_str()? {
            "succeeded" | "detached" => Some(("finished", "board succeeded".into())),
            "failed" => Some(("failed", "board failed".into())),
            _ => None,
        };
    }
    (!state.pids.is_empty() && !state.pids.iter().any(|p| alive(*p))).then(|| ("cancelled", format!("producer pids {:?} gone", state.pids)))
}

/// Emits the terminal event of every orphan; returns one receipt per job.
pub(super) fn heal_bus(home: &Path, now_ms: f64) -> Vec<Value> {
    let state = home.join(".local/state");
    let releases = state.join("mars-deliver/releases");
    let mut out = Vec::new();
    for (key, job_state) in jobs(&state.join("job-bus/events.jsonl")) {
        let job = key.split_once('|').map_or(key.as_str(), |(_, j)| j);
        let Some((event, reason)) = verdict(&job_state, job, &releases, now_ms) else { continue };
        let data = json!({"by": "cpu-watchdog task-stall", "reason": reason, "last_event": job_state.event}).to_string();
        let ok = cfg!(test) || std::process::Command::new(home.join(".local/bin/job-bus"))
            .args(["emit", "--topic", &job_state.topic, "--job", job, "--event", event, "--data", &data])
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
        out.push(json!({"at_ms": now_ms, "topic": job_state.topic, "job": job, "last_event": job_state.event,
            "silent_min": ((now_ms - job_state.ms) / 60_000.0).round(), "emitted": event, "reason": reason, "ok": ok}));
    }
    out
}

fn arg_after<'a>(command: &'a str, flag: &str) -> Option<&'a str> {
    let mut words = command.split_whitespace();
    words.find(|w| *w == flag)?;
    words.next().filter(|w| !w.starts_with('-'))
}

/// What a blocked call waits on, read from its command: a job-bus job or a release board run with
/// its current state. `ended: true` means the call waits on something that already finished.
pub(super) fn waits_on(home: &Path, command: &str, bus: &BTreeMap<String, JobState>) -> Value {
    let state = home.join(".local/state");
    if let Some(job) = arg_after(command, "--job").filter(|_| command.contains("job-bus wait")) {
        let found = bus.iter().find(|(k, _)| k.split_once('|').is_some_and(|(_, j)| j == job)).map(|(_, s)| s);
        return json!({"kind": "job-bus", "job": job, "state": found.map(|s| s.event.clone()),
            "ended": found.is_some_and(|s| TERMINAL.contains(&s.event.as_str()))});
    }
    if let Some(run) = arg_after(command, "wait").filter(|r| command.contains("deliver wait") && !r.starts_with("--")) {
        let board = super::super::read(&state.join("mars-deliver/releases").join(format!("{run}.json")));
        let board_state = board.as_ref().and_then(|b| b["state"].as_str()).map(str::to_string);
        return json!({"kind": "release", "run": run, "state": board_state,
            "ended": board_state.as_deref().is_some_and(|s| s == "succeeded" || s == "failed")});
    }
    json!({"kind": super::super::wait_retro::parse::class_of("", command)})
}
