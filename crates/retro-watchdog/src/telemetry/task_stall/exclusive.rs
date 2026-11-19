//! render-queue's exclusive memory window as context for a blocked call (incident
//! 811689766098363691416, 2026-11-06: a `bun run ui-tools ios testAffected` call waited 12 min while
//! cpu-build-gate held its xcodebuild behind window h3, and `waits_on` said only `other`).
//! While `~/.local/state/render-queue/exclusive.json` names a live owner and `until` has not passed,
//! cpu-build-gate holds every new non-release build (crates/cpu-build-gate/src/exclusive.rs), so any
//! build a stalled call runs may be waiting there. The gate prints that only once on stderr, which a
//! caller's pipe often filters out. Read only; nothing here changes the window.
use serde_json::{json, Value};
use std::path::Path;

/// The open window at `now_ms`, or None when there is none, its owner died or it expired.
pub(super) fn open(home: &Path, now_ms: f64, alive: impl Fn(i64) -> bool) -> Option<Value> {
    let path = home.join(".local/state/render-queue/exclusive.json");
    let marker: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let (pid, until) = (marker["pid"].as_i64()?, marker["until"].as_f64()?);
    if !alive(pid) || now_ms >= until { return None; }
    Some(json!({"owner_pid": pid, "shot": marker["shot"], "project": marker["project"], "budget_gib": marker["budgetGib"],
        "open_min": marker["since"].as_f64().map(|s| ((now_ms - s) / 6_000.0).round() / 10.0),
        "ends_in_max_min": ((until - now_ms) / 6_000.0).round() / 10.0,
        "holds": "new non-release cpu-build-gate builds until the owner exits"}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with(marker: Option<Value>) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("task-stall-exclusive-{}-{}", std::process::id(), marker.is_some()));
        let state = dir.join(".local/state/render-queue");
        std::fs::create_dir_all(&state).unwrap();
        match marker {
            Some(m) => std::fs::write(state.join("exclusive.json"), m.to_string()).unwrap(),
            None => { let _ = std::fs::remove_file(state.join("exclusive.json")); }
        }
        dir
    }

    #[test]
    fn a_live_unexpired_window_is_reported_and_a_dead_or_expired_one_is_not() {
        let home = home_with(Some(json!({"pid": 55130, "project": "/v/bench", "shot": "h3", "budgetGib": 50,
            "since": 1_000_000.0, "until": 1_000_000.0 + 150.0 * 60_000.0})));
        let now = 1_000_000.0 + 14.0 * 60_000.0;
        let w = open(&home, now, |_| true).expect("open window");
        assert_eq!((w["shot"].as_str(), w["owner_pid"].as_i64(), w["budget_gib"].as_i64()), (Some("h3"), Some(55130), Some(50)));
        assert_eq!((w["open_min"].as_f64(), w["ends_in_max_min"].as_f64()), (Some(14.0), Some(136.0)));
        assert!(open(&home, now, |_| false).is_none(), "a dead owner leaves the window inert");
        assert!(open(&home, 1_000_000.0 + 151.0 * 60_000.0, |_| true).is_none(), "a passed until leaves it inert");
        assert!(open(&home_with(None), now, |_| true).is_none());
    }
}
