//! `monitor-ack` gate of a hook retro (`hook_defects_found`): report.json names the published page
//! and gives every defect of the incident a verdict with a reason; a fixed defect names its commit,
//! and a run that fixed anything proves `hook-audit check` passes and the replayed hook's output.
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
    let rows = report["defects"].as_array().context("report.json defects must be an array of {signature, verdict, reason}")?;
    for (i, row) in rows.iter().enumerate() {
        let verdict = row["verdict"].as_str().unwrap_or("");
        ensure!(VERDICTS.contains(&verdict), "report.json defects[{i}].verdict must be one of {VERDICTS:?}");
        ensure!(text(&row["signature"], 3) && text(&row["reason"], 10), "report.json defects[{i}] needs signature and a reason of 10+ characters");
        if verdict == "fixed" {
            ensure!(row["commit"].as_str().is_some_and(|c| c.len() >= 7 && c.chars().all(|c| c.is_ascii_hexdigit())), "report.json defects[{i}] is fixed but names no commit sha");
        }
        if verdict == "owned_elsewhere" {
            ensure!(text(&row["owner"], 3), "report.json defects[{i}] is owned_elsewhere but names no owner session");
        }
    }
    for defect in incident["defects"].as_array().into_iter().flatten() {
        let sig = defect["signature"].as_str().unwrap_or("");
        ensure!(rows.iter().any(|r| r["signature"] == sig), "report.json defects has no verdict for {sig}");
    }
    if rows.iter().any(|r| r["verdict"] == "fixed") {
        ensure!(report["after"]["check"] == "pass", "report.json after.check must be \"pass\" (hook-audit check)");
        ensure!(text(&report["after"]["replay"], 10), "report.json after.replay must state what the replayed hook printed");
    }
    Ok(())
}

pub(in crate::telemetry) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    if event["trigger"] != "hook_defects_found" {
        return Ok(());
    }
    let path = root(home).join("analysis").join(id).join("report.json");
    let report: Value = serde_json::from_slice(&std::fs::read(&path).with_context(|| format!("{} missing", path.display()))?)?;
    check(&event, &report)
}
