//! `monitor-ack` gate of a weekly prompt review (`prompt_review_due`): report.json names the
//! published page and gives every new duplicate group of the incident a verdict with a reason; a
//! fixed group names its commit and the run re-measured the prefix and the prompt-dedup gate; a group
//! left to another session names that session.
use crate::monitor::root;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::path::Path;

pub(in crate::telemetry) const VERDICTS: [&str; 4] = ["fixed", "proposed", "kept", "owned_elsewhere"];

fn text(v: &Value, min: usize) -> bool {
    v.as_str().is_some_and(|s| s.trim().chars().count() >= min)
}

pub(in crate::telemetry) fn check(incident: &Value, report: &Value) -> Result<()> {
    ensure!(report["report_url"].as_str().is_some_and(|u| u.starts_with("https://reports.example.com")), "report.json report_url is missing");
    let rows = report["groups"].as_array().context("report.json groups must be an array of {id, verdict, reason}")?;
    for (i, row) in rows.iter().enumerate() {
        let verdict = row["verdict"].as_str().unwrap_or("");
        ensure!(VERDICTS.contains(&verdict), "report.json groups[{i}].verdict must be one of {VERDICTS:?}");
        ensure!(text(&row["id"], 1) && text(&row["reason"], 10), "report.json groups[{i}] needs id and a reason of 10+ characters");
        if verdict == "fixed" {
            ensure!(row["commit"].as_str().is_some_and(|c| c.len() >= 7 && c.chars().all(|c| c.is_ascii_hexdigit())), "report.json groups[{i}] is fixed but names no commit sha");
        }
        if verdict == "owned_elsewhere" {
            ensure!(text(&row["owner"], 3), "report.json groups[{i}] is owned_elsewhere but names no owner session");
        }
    }
    for group in incident["new_groups"].as_array().into_iter().flatten() {
        let id = group["id"].as_str().unwrap_or("");
        ensure!(rows.iter().any(|r| r["id"] == id), "report.json groups has no verdict for new group {id}");
    }
    if rows.iter().any(|r| r["verdict"] == "fixed") {
        ensure!(report["after"]["prefix"].is_u64(), "report.json after.prefix must be the re-measured prefix characters");
        ensure!(report["after"]["gate"] == "pass", "report.json after.gate must be \"pass\" (prompt-dedup analyze-prefix against origin/mastra)");
    }
    Ok(())
}

pub(in crate::telemetry) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    if event["trigger"] != "prompt_review_due" {
        return Ok(());
    }
    let path = root(home).join("analysis").join(id).join("report.json");
    let report: Value = serde_json::from_slice(&std::fs::read(&path).with_context(|| format!("{} missing", path.display()))?)?;
    check(&event, &report)
}
