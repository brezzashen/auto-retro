//! Re-judges queued port-leak jobs before they open a session. The one-slot analysis queue held
//! incident 816353310315624064394 for 12.1 h (2026-11-01 08:37 to 20:46 SGT); node N2.13 landed
//! at 20:33 and explains its stored attribution (every known window 1.00, residual -8), yet the
//! session opened because the closure ran only at detection time. The stored attribution is the
//! better evidence: ControlCenter module lines rotate out within about 9 h (N2.8), so a recompute
//! at launch sees less. A probe an older binary did not emit is derived from fields it did.
use super::{close, explaining, load};
use crate::monitor::{root, write_json};
use crate::telemetry::read_state;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

/// Adds `cc_known_windows` (growth minus `cc_residual`, as attribute.rs defines it) when missing.
pub(crate) fn backfill(found: &mut Value) {
    if !found["probes"]["cc_known_windows"].is_null() { return; }
    let (Some(growth), Some(residual)) = (found["growth"].as_i64().filter(|g| *g > 0),
        found["probes"]["cc_residual"]["growth"].as_i64()) else { return };
    found["probes"]["cc_known_windows"] = json!({"growth": growth - residual,
        "share": (growth - residual) as f64 / growth as f64, "derived": "growth - cc_residual"});
}

/// The ledger row of the restart the census made for incident `id`.
fn restart_for(home: &Path, id: &str) -> Option<Value> {
    let directory = root(home).join("controlcenter-restarts");
    let mut files: Vec<_> = std::fs::read_dir(directory).ok()?.flatten().map(|e| e.path()).collect();
    files.sort();
    files.iter().rev().take(3).filter_map(|f| std::fs::read_to_string(f).ok())
        .flat_map(|text| text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect::<Vec<_>>())
        .find(|row| row["incident"] == id)
}

/// Closes queued ControlCenter port-leak incident `id` by script when the current cause tree
/// explains the attribution stored at detection and that restart recovered. `now` is in seconds.
pub(crate) fn close_queued(home: &Path, now: f64, id: &str) -> Result<Option<Value>> {
    let Some(incident) = read_state(&root(home).join(format!("incidents/{id}.json"))) else { return Ok(None) };
    if incident["trigger"] != "controlcenter_port_leak" || root(home).join(format!("incidents/{id}.ack")).exists() {
        return Ok(None);
    }
    let directory = root(home).join("analysis").join(id);
    let Some(mut found) = read_state(&directory.join("attribution.json")) else { return Ok(None) };
    if found["restart_recovered"] != true { return Ok(None) }
    let Some(restart) = restart_for(home, id).filter(|r| r["outcome"] == "restarted") else { return Ok(None) };
    backfill(&mut found);
    let tree = load(home)?;
    let Some(node) = explaining(&tree, "controlcenter_port_leak", &found).cloned() else { return Ok(None) };
    found["explained_by"] = node["id"].clone();
    found["rejudged_at_ms"] = json!(now * 1000.0);
    write_json(&directory.join("attribution.json"), &found)?;
    close(home, now, id, &incident, &found, &restart, &node, "queue").map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backfill_derives_known_windows_from_the_residual() {
        // Incident 816353310315624064394 as stored at 08:37 by the binary before N2.13.
        let mut found = json!({"growth": 3083, "probes": {"cc_menubar_rebuild": {"share": 0.367},
            "cc_mic_sessions": {"share": 0.819}, "cc_residual": {"growth": -8}}});
        backfill(&mut found);
        assert_eq!(found["probes"]["cc_known_windows"]["growth"], 3091);
        assert!(found["probes"]["cc_known_windows"]["share"].as_f64().unwrap() > 1.0);
        let mut present = json!({"growth": 100, "probes": {"cc_known_windows": {"share": 0.5}, "cc_residual": {"growth": 0}}});
        backfill(&mut present);
        assert_eq!(present["probes"]["cc_known_windows"]["share"], 0.5);
        let mut empty = json!({"growth": 0, "probes": {"cc_residual": {"growth": 0}}});
        backfill(&mut empty);
        assert!(empty["probes"]["cc_known_windows"].is_null());
    }

    #[test]
    fn queued_job_closes_only_with_a_recovered_restart_and_an_explaining_node() {
        let home = crate::telemetry::test_home("queued-rejudge");
        let tree = home.join(".codex/skills/watchdog/references");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("cause-tree.json"), json!({"nodes": [
            {"id": "N2", "parent": null, "triggers": ["controlcenter_port_leak"], "status": "open"},
            {"id": "N2.13", "parent": "N2", "probe": "cc_known_windows", "close_share": 0.85, "status": "closed", "answer": "known"},
        ]}).to_string()).unwrap();
        let state = root(&home);
        for d in ["incidents", "analysis/7", "controlcenter-restarts"] { std::fs::create_dir_all(state.join(d)).unwrap(); }
        write_json(&state.join("incidents/7.json"), &json!({"trigger": "controlcenter_port_leak", "pid": 1, "ports": 4041})).unwrap();
        let stored = json!({"growth": 3083, "restart_recovered": true, "probes": {"cc_residual": {"growth": -8}}});
        write_json(&state.join("analysis/7/attribution.json"), &stored).unwrap();
        // No restart row for this incident: stays queued for a session.
        assert!(close_queued(&home, 10.0, "7").unwrap().is_none());
        std::fs::write(state.join("controlcenter-restarts/0.ndjson"),
            format!("{}\n", json!({"incident": "7", "outcome": "restarted", "pid": 1, "new_pid": 2, "new_ports": 1041}))).unwrap();
        let closed = close_queued(&home, 10.0, "7").unwrap().unwrap();
        assert_eq!(closed["node"], "N2.13");
        assert!(state.join("incidents/7.ack").exists() && state.join("analysis/7/report.md").exists());
        // Acknowledged already: never closed twice.
        assert!(close_queued(&home, 11.0, "7").unwrap().is_none());
        std::fs::remove_dir_all(home).unwrap();
    }
}
