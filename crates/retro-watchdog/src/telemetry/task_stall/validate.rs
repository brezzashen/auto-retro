//! `monitor-ack` gate of a task stall review (incidents with `requirements.task_review`): report.json
//! must judge every covered task, account for the heals, and record the fixes; report.md must carry
//! the three steps in the founder's order (stop the bleeding, optimize, review; see SECTIONS).
use crate::monitor::root;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::path::Path;

const VERDICTS: [&str; 5] = ["stuck_queue", "input_error", "silent_agent", "necessary", "false_alarm"];
const SECTIONS: [&str; 3] = ["止血", "优化", "复盘"];

fn text(v: &Value) -> bool { v.as_str().is_some_and(|s| !s.trim().is_empty()) }

pub(in crate::telemetry) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    if event["requirements"]["task_review"].is_null() { return Ok(()); }
    let dir = root(home).join("analysis").join(id);
    let report: Value = serde_json::from_slice(&std::fs::read(dir.join("report.json")).context("report.json missing")?)?;
    let tasks = report["tasks"].as_array().context("report.json tasks[] is missing")?;
    for path in event["tasks"].as_array().into_iter().flatten().filter_map(|t| t["path"].as_str()) {
        let entry = tasks.iter().find(|t| t["path"] == path).with_context(|| format!("tasks[] has no verdict for {path}"))?;
        ensure!(entry["verdict"].as_str().is_some_and(|v| VERDICTS.contains(&v)), "tasks[] {path}: verdict must be one of {VERDICTS:?}");
        ensure!(text(&entry["cause"]), "tasks[] {path}: cause is empty");
        ensure!(text(&entry["bleeding_stopped"]), "tasks[] {path}: bleeding_stopped must say how the stall ended or why it needed nothing");
    }
    let heals = event["heals"].as_array().map_or(0, Vec::len);
    ensure!(heals == 0 || text(&report["heals_review"]), "heals_review must judge the {heals} heal(s) the watch applied");
    let fixes = report["fixes"].as_array().context("report.json fixes[] is missing")?;
    ensure!(!fixes.is_empty() || text(&report["fixes_none_reason"]), "fixes[] is empty and fixes_none_reason is missing");
    for f in fixes {
        ensure!(text(&f["cause"]) && (text(&f["commit"]) || text(&f["no_change_reason"])), "fixes[] rows need cause and commit or no_change_reason");
    }
    let markdown = std::fs::read_to_string(dir.join("report.md")).unwrap_or_default();
    for section in SECTIONS { ensure!(markdown.contains(section), "report.md needs a {section} section"); }
    Ok(())
}
