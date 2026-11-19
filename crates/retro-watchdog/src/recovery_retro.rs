//! Every recorded forced recovery (headless page, Docker, daemon, Storybook) is a condition of the
//! one performance trigger (`wake.rs`, founder 2026-10-27): it joins the open performance session,
//! queues one, or waits for the next, and that session carries the optimization review. Surge
//! actions are reviewed the same way. A recovery on a node marked `script_close` closes by script
//! instead (`known.rs`).
use crate::monitor::root;
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

mod known;

pub(crate) fn record(home: &Path, kind: &str, event: &Value) -> Result<()> {
    let outcome = event["outcome"].as_str().unwrap_or_default();
    if outcome.is_empty() || outcome.starts_with("skipped") { return Ok(()); }
    if kind == "controlcenter" {
        if let Some(id) = event["incident"].as_str() {
            if crate::telemetry::read_state(&root(home).join(format!("incidents/{id}.json")))
                .is_some_and(|incident| incident["trigger"] == "controlcenter_port_leak") {
                return Ok(()); // Its existing port-leak analysis uses the same admission queue.
            }
        }
    }
    let digest = format!("{:x}", Sha256::digest(format!("{kind}:{event}")));
    let id = crate::telemetry::springboard_incident(home, &format!("forced-recovery:{digest}"),
        json!({"trigger":"forced_recovery","kind":kind,"recovery":event,
            "telemetry":crate::telemetry::evidence(home)}))?;
    match known::close(home, &id, kind, event, crate::unix_now() * 1000.0) {
        Ok(Some(closed)) => {
            let repeat = closed["decision"].as_str().map_or(String::new(), |d| format!("；24 小时内第 {} 次，待你决定：{d}", closed["recurrence_24h"]));
            crate::dispatch::notify("看门狗：恢复复盘", &format!("已知原因 {} 解释这次 {kind} 恢复，脚本结案{repeat}", closed["node"].as_str().unwrap_or("?")));
            return Ok(());
        }
        Ok(None) => {}
        Err(error) => crate::log(&format!("recovery retro {id}: cause tree: {error:#}")),
    }
    // An open recovery is reviewed inside the performance session with the load around it (`wake.rs`).
    let detail = format!("{kind} forced recovery {}", event["outcome"].as_str().unwrap_or("?"));
    crate::wake::trigger(home, crate::unix_now() * 1000.0, crate::wake::Condition::Recovery, &id, &detail, true, json!({"kind": kind}))?;
    Ok(())
}

pub(crate) fn validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    if event["trigger"] != "forced_recovery" && event["trigger"] != crate::surge::TRIGGER { return Ok(()); }
    review(&serde_json::from_slice(&std::fs::read(root(home).join("analysis").join(id).join("report.json"))?)?)
}

/// The optimization review every forced-recovery, surge and performance session must carry.
pub(crate) fn review(report: &Value) -> Result<()> {
    for key in ["resource_contention", "error_prevention", "change_or_no_change_reason", "validation", "remaining_risk"] {
        ensure!(report["optimization_review"][key].as_str().is_some_and(|s| !s.trim().is_empty()),
            "forced recovery review needs optimization_review.{key}");
    }
    ensure!(report["verification"]["passed"] == true, "forced recovery review not verified");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_once_and_joins_distinct_recoveries_into_one_performance_session() {
        let home = crate::telemetry::test_home("recovery-retro");
        let event = json!({"at_ms":1000,"outcome":"closed","url":"http://example.test"});
        record(&home, "headless", &event).unwrap();
        record(&home, "headless", &event).unwrap();
        record(&home, "daemon", &json!({"at_ms":1001,"outcome":"restarted"})).unwrap();
        let state = crate::telemetry::read_state(&root(&home).join("wake-perf.json")).unwrap();
        let episodes = state["episodes"].as_array().unwrap();
        assert_eq!(episodes.len(), 2, "the repeated headless record is one episode");
        assert_eq!((episodes[0]["outcome"].as_str(), episodes[1]["outcome"].as_str()), (Some("queued"), Some("joined")));
        let id = episodes[0]["incident"].as_str().unwrap();
        let joined = episodes[1]["incident"].as_str().unwrap();
        assert!(root(&home).join(format!("incidents/{joined}.ack")).exists(), "a joined recovery is acknowledged");
        assert!(validate(&home, id, &root(&home).join(format!("incidents/{id}.json"))).is_err());
        assert!(crate::wake::validate(&home, id).is_err(), "the session must cover both episodes");
        std::fs::remove_dir_all(home).unwrap();
    }
}
