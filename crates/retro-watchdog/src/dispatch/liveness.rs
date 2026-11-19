//! Liveness of the session holding the analysis slot, read through `session-status`.
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::Path;

/// A launched job younger than this is never probed.
pub(super) const PROBE_AFTER_MS: f64 = 7_200_000.0;
/// At most one probe per job in this interval.
pub(super) const PROBE_EVERY_MS: f64 = 600_000.0;
/// A session with no live process and no transcript write for this long has ended.
pub(super) const DEAD_IDLE_MIN: f64 = 60.0;
/// A job whose session ended this many times without its ack is abandoned.
pub(super) const MAX_TAKEOVERS: u64 = 1;
/// Leading characters of the launched prompt that identify its session.
const PROMPT_MATCH_CHARS: usize = 120;

/// The newest session whose first prompt is the job's launched prompt: `{path, idle_min, live}`.
pub(super) fn probe(home: &Path, incident: &str, prompt: &str) -> Result<Option<Value>> {
    let head: String = prompt.trim().chars().take(PROMPT_MATCH_CHARS).collect();
    Ok(pick(&sessions(home, incident)?, |p| !head.is_empty() && p.trim().starts_with(&head)))
}

/// The newest session opened for the incident (its first prompt names `incident <id>`), when the
/// launched prompt is no longer known.
pub(crate) fn opened_for(home: &Path, incident: &str) -> Result<Option<Value>> {
    let needle = format!("incident {incident}");
    Ok(pick(&sessions(home, incident)?, |p| p.contains(&needle)))
}

fn sessions(home: &Path, incident: &str) -> Result<Value> {
    let output = std::process::Command::new(home.join(".local/bin/session-status"))
        .args([incident, "--json", "--limit", "5", "--since", "30d"])
        .output().context("run session-status")?;
    anyhow::ensure!(output.status.success(), "session-status failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    serde_json::from_slice(&output.stdout).context("parse session-status JSON")
}

fn pick(sessions: &Value, first_prompt: impl Fn(&str) -> bool) -> Option<Value> {
    sessions.as_array()?.iter()
        .find(|s| s["prompts"][0].as_str().is_some_and(&first_prompt))
        .map(|s| json!({"path": s["path"], "idle_min": s["idle_minutes"], "live": s["live"]}))
}

pub(super) fn dead(session: &Value) -> bool {
    session["live"].is_null() && session["idle_min"].as_f64().is_some_and(|m| m >= DEAD_IDLE_MIN)
}

pub(super) fn takeover_prompt(incident: &str, original: &str, session: &Value, attempt: u64) -> String {
    format!("TAKEOVER {attempt} of cpu-watchdog incident {incident}: the session that held it ({path}) ended {idle:.0} min ago without monitor-ack. \
        First read the tail of that transcript and ~/.local/state/cpu-watchdog/analysis/{incident}/; its uncommitted edits may be half-finished, so check them with git diff before building on them. \
        Finish only what is missing, then run cpu-watchdog monitor-ack {incident}. The original task follows. {original}",
        path = session["path"].as_str().unwrap_or("unknown"), idle = session["idle_min"].as_f64().unwrap_or(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_matches_the_dispatched_prompt_not_a_mention() {
        let sessions = json!([
            {"path":"/mention.jsonl","idle_minutes":1,"live":"running","prompts":["why is cpu-watchdog incident 4242 route perf stuck?"]},
            {"path":"/a.jsonl","idle_minutes":90,"live":null,"prompts":["cpu-watchdog incident 4242 route perf"]}]);
        let launched = |p: &str| p.starts_with("cpu-watchdog incident 4242 route perf");
        assert!(pick(&sessions, launched).is_some_and(|s| s["path"] == "/a.jsonl" && dead(&s)));
        assert!(pick(&sessions, |p| p.starts_with("cpu-watchdog incident 9999")).is_none());
        assert!(!dead(&json!({"live":"running","idle_min":500})));
        assert!(!dead(&json!({"live":null,"idle_min":10})));
    }
}
