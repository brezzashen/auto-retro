//! Labelled synthetic search traffic (founder decision 1A on search-retro 810495355461229102702,
//! 2026-11-05): a harness that runs agents or commands to test search itself (an A/B benchmark of
//! headless Claude runs, a hook probe, a replay through the installed guard) sets
//! `SEARCH_WATCH_SYNTHETIC=<label>`. `script-guard` copies the label into each event's `synthetic`
//! field and `local-search` into each usage row's, so the window's rates, score and call count leave
//! them out, and every report still carries the numbers with them (`with_synthetic`).
//!
//! That window held about 30 of 77 engine rows, all 4 fallbacks and the only history/report miss
//! from such traffic (f8ce3935…jsonl:584 `scratchpad/ab/run.mjs`, an Antigravity probe in
//! `scratchpad/agprobe`), and the previous window's 6 false NONE came from the same benchmark task.
//! Only the label counts: no session is guessed synthetic from its prompt or id.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// The label variable a test harness exports for every process it starts.
pub(super) const ENV: &str = "SEARCH_WATCH_SYNTHETIC";
/// A session labelled within this margin of the window counts as synthetic for the whole window.
const MARGIN_MS: f64 = 3_600_000.0;

/// The event's or usage row's label, when it carries one.
pub(super) fn label(row: &Value) -> Option<&str> {
    row["synthetic"].as_str().filter(|l| !l.trim().is_empty())
}

/// Splits events into the real ones and the labelled synthetic ones.
pub(super) fn split(events: &[Value]) -> (Vec<Value>, Vec<Value>) {
    events.iter().cloned().partition(|e| label(e).is_none())
}

/// Sessions with a labelled event in `events.jsonl` near the window, each with its label.
pub(super) fn sessions(home: &Path, from_ms: f64, to_ms: f64) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(super::log_path(home)).unwrap_or_default();
    let rows = text.lines().filter(|l| l.contains("\"synthetic\":")).filter_map(|l| serde_json::from_str::<Value>(l).ok());
    sessions_of(rows, from_ms, to_ms)
}

fn sessions_of(rows: impl Iterator<Item = Value>, from_ms: f64, to_ms: f64) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for row in rows {
        let near = row["ts_ms"].as_f64().is_some_and(|t| t >= from_ms - MARGIN_MS && t <= to_ms + MARGIN_MS);
        if let (true, Some(label), Some(session)) = (near, label(&row), row["session"].as_str().filter(|s| !s.is_empty())) {
            out.entry(session.to_string()).or_insert_with(|| label.to_string());
        }
    }
    out
}

/// True when a transcript path belongs to a synthetic session: the session id is its file stem or
/// one of its directories (a subagent transcript under `<session>/subagents/`).
pub(super) fn in_sessions(path: &Path, sessions: &BTreeMap<String, String>) -> bool {
    !sessions.is_empty()
        && path.iter().any(|c| {
            let c = c.to_string_lossy();
            sessions.contains_key(c.strip_suffix(".jsonl").unwrap_or(&c))
        })
}

/// The policy score without and with the synthetic events, plus what was left out.
pub(super) fn scores(events: &[Value]) -> (Value, Value, Value) {
    let (real, synthetic) = split(events);
    let mut labels: BTreeMap<String, u64> = BTreeMap::new();
    for e in &synthetic {
        *labels.entry(label(e).unwrap_or_default().to_string()).or_default() += 1;
    }
    let local = synthetic.iter().filter(|e| e["kind"] == "local_search").count();
    let summary = json!({"events": synthetic.len(), "local_search": local, "labels": labels});
    (super::score::score(&real), super::score::score(events), summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn labelled_events_leave_the_score_and_are_reported_apart() {
        let sweep = |s: &str, synthetic: Option<&str>| {
            let mut e = json!({"kind": "grep", "tool": "rg", "target": "report", "scope": "tree", "verdict": "blocked", "session": s, "ts_ms": 10.0});
            if let Some(l) = synthetic { e["synthetic"] = json!(l) }
            e
        };
        let local = json!({"kind": "local_search", "target": "report", "verdict": "allowed", "session": "a", "ts_ms": 10.0});
        // Search-retro 810495355461229102702: the only history/report miss was an Antigravity probe.
        let events = vec![local.clone(), local, sweep("probe", Some("agprobe"))];
        let (real, all, left) = scores(&events);
        assert_eq!(real["components"]["history_report"], json!(1.0));
        assert_eq!(all["components"]["history_report"], json!(0.667));
        assert_eq!(left, json!({"events": 1, "local_search": 0, "labels": {"agprobe": 1}}));
        let (r, s) = split(&[sweep("x", None), sweep("y", Some(" "))]);
        assert_eq!((r.len(), s.len()), (2, 0), "a blank label is no label");
    }

    #[test]
    fn a_labelled_session_marks_its_transcripts_and_subagents_only() {
        let rows = vec![
            json!({"session": "bench-1", "synthetic": "ab", "ts_ms": 1000.0}),
            json!({"session": "far", "synthetic": "ab", "ts_ms": 1000.0 + 3.0 * MARGIN_MS}),
            json!({"session": "real", "ts_ms": 1000.0}),
        ];
        let sessions = sessions_of(rows.into_iter(), 900.0, 1100.0);
        assert_eq!(sessions.keys().collect::<Vec<_>>(), ["bench-1"]);
        assert!(in_sessions(&PathBuf::from("/p/projects/x/bench-1.jsonl"), &sessions));
        assert!(in_sessions(&PathBuf::from("/p/projects/x/bench-1/subagents/agent-a.jsonl"), &sessions));
        assert!(!in_sessions(&PathBuf::from("/p/projects/x/real.jsonl"), &sessions));
        assert!(!in_sessions(&PathBuf::from("/p/projects/x/bench-10.jsonl"), &sessions));
    }
}
