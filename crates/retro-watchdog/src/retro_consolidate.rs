//! Probe lifecycle, the consolidation step of the retro insight store: each probe's verdict streak
//! (retro_scorecard) sets its status. `graduated` probes (three effective in a row) are still
//! measured but need no verdict until they measure ineffective or regressed; `redefine` (three insufficient_data) needs a
//! new pattern; `escalate` (three ineffective or regressed) is the next retro's first finding.
//! The outcome lives in `probe-lifecycle.json`, so the hand-edited retro-probes.json is never rewritten.
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::path::Path;

const FILE: &str = "probe-lifecycle.json";
/// Probes that still need a verdict every retro; above this the scorecard asks for retirement.
pub(crate) const ACTIVE_CAP: usize = 12;

pub(crate) fn lifecycle(home: &Path) -> Value {
    std::fs::read(root(home).join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({"probes": {}}))
}

pub(crate) fn status(lifecycle: &Value, probe: &str) -> String {
    lifecycle["probes"][probe]["status"].as_str().unwrap_or("active").to_string()
}

fn status_of(action: &str) -> &'static str {
    match action { "graduate" => "graduated", "redefine" => "redefine", "escalate" => "escalate", _ => "active" }
}

/// The lifecycle the current streaks call for, and the probes whose status changes.
fn plan(home: &Path, now: f64) -> Result<(Value, Vec<Value>)> {
    let current = lifecycle(home);
    let mut probes = Map::new();
    let mut changes = Vec::new();
    for s in crate::retro_scorecard::probe_streaks(home)? {
        let id = s["probe"].as_str().unwrap_or_default().to_string();
        let next = status_of(s["action"].as_str().unwrap_or("keep"));
        let before = status(&current, &id);
        let since = if before == next { current["probes"][&id]["since_ms"].clone() } else { json!(now) };
        if before != next { changes.push(json!({"probe": id, "from": before, "to": next, "streak": s["streak"], "last": s["last"]})); }
        probes.insert(id, json!({"status": next, "streak": s["streak"], "last": s["last"], "since_ms": since}));
    }
    let active = probes.values().filter(|p| p["status"] != "graduated").count();
    Ok((json!({"at_ms": now, "active": active, "active_cap": ACTIVE_CAP, "probes": probes}), changes))
}

/// Recompute and save the lifecycle; called after every retro acknowledgement.
pub(crate) fn apply(home: &Path, now: f64) -> Result<Vec<Value>> {
    let (next, changes) = plan(home, now)?;
    write_json(&root(home).join(FILE), &next)?;
    Ok(changes)
}

/// `cpu-watchdog insight-consolidate [--apply]`: the status changes the streaks call for.
pub fn run(home: &Path, args: Vec<String>) -> Result<()> {
    let now = crate::unix_now() * 1000.0;
    let applied = args.iter().any(|a| a == "--apply");
    let (next, changes) = plan(home, now)?;
    if applied { write_json(&root(home).join(FILE), &next)?; }
    let mut counts = Map::new();
    for p in next["probes"].as_object().into_iter().flat_map(|m| m.values()) {
        let key = p["status"].as_str().unwrap_or("active").to_string();
        counts.insert(key.clone(), json!(counts.get(&key).and_then(Value::as_u64).unwrap_or(0) + 1));
    }
    println!("{}", serde_json::to_string_pretty(&json!({"applied": applied, "counts": counts, "active": next["active"],
        "active_cap": ACTIVE_CAP, "over_cap": next["active"].as_u64().unwrap_or(0) as usize > ACTIVE_CAP, "changes": changes}))?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_map_to_statuses_and_unknown_probes_are_active() {
        assert_eq!(status_of("graduate"), "graduated");
        assert_eq!(status_of("redefine"), "redefine");
        assert_eq!(status_of("escalate"), "escalate");
        assert_eq!(status_of("keep"), "active");
        assert_eq!(status(&json!({"probes": {"R1": {"status": "graduated"}}}), "R1"), "graduated");
        assert_eq!(status(&json!({"probes": {}}), "R9"), "active");
    }
}
