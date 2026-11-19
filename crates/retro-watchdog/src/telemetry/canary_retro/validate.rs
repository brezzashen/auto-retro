//! `monitor-ack` gate of a Canary retro (incidents with `requirements.canary_review`): report.json
//! must name the cause of every run over the hard limit and answer the founder's four checks
//! (errors, cache, queue, and since 2026-11-02 ci: what belongs in CI or a check ahead of the
//! release), each defect with its change or the reason there is none. A CI change also states
//! `release_cost_s`, the seconds it adds to a release's critical path (0 off the path, negative
//! when it saves), so no check is added that slows every release more than it saves.
use crate::monitor::root;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::path::Path;

const CAUSES: [&str; 8] = ["cache", "build", "upload", "queue", "preflight", "rollout", "error", "other"];
const CHECKS: [&str; 4] = ["errors", "cache", "queue", "ci"];
const SECTIONS: [&str; 5] = ["发布时长", "错误", "缓存", "队列", "CI 前移"];

fn text(v: &Value) -> bool { v.as_str().is_some_and(|s| !s.trim().is_empty()) }

pub(in crate::telemetry) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    let review = &event["requirements"]["canary_review"];
    if review.is_null() { return Ok(()); }
    let dir = root(home).join("analysis").join(id);
    let report: Value = serde_json::from_slice(&std::fs::read(dir.join("report.json")).context("report.json missing")?)?;
    let runs = report["runs"].as_array().context("report.json runs[] is missing")?;
    for run in review["over_hard"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let entry = runs.iter().find(|r| r["id"] == run).with_context(|| format!("runs[] has no cause for {run}, which took over the hard limit"))?;
        ensure!(entry["cause"].as_str().is_some_and(|c| CAUSES.contains(&c)), "runs[] {run}: cause must be one of {CAUSES:?}");
        ensure!(text(&entry["evidence"]), "runs[] {run}: evidence is empty");
    }
    for key in CHECKS {
        let check = &report["checks"][key];
        ensure!(matches!(check["verdict"].as_str(), Some("ok" | "defect")), "checks.{key}.verdict must be ok or defect");
        ensure!(text(&check["evidence"]), "checks.{key}.evidence is empty");
        ensure!(check["verdict"] == "ok" || text(&check["change"]) || text(&check["no_change_reason"]),
            "checks.{key} is a defect and needs change or no_change_reason");
        ensure!(key != "ci" || !text(&check["change"]) || check["release_cost_s"].is_number(),
            "checks.ci.release_cost_s must give the seconds the change adds to a release's critical path (0 off the path, negative when it saves)");
    }
    ensure!(text(&report["report_url"]), "report.json report_url is empty");
    let markdown = std::fs::read_to_string(dir.join("report.md")).unwrap_or_default();
    for section in SECTIONS { ensure!(markdown.contains(section), "report.md needs a {section} section"); }
    Ok(())
}
