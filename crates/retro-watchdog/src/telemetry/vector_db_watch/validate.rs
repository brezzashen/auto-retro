//! `monitor-ack` gate of a vector database retro (`vector_db_growth_reached`): report.json names the
//! published page, attributes the growth, gives the split decision with its reason (a proposal also
//! names its plan; a split is never executed by the session), judges query performance with evidence,
//! and lists the optimizations considered; an applied one states before, after and recall.
use crate::monitor::root;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::path::Path;

pub(in crate::telemetry) const SPLIT: [&str; 2] = ["keep", "propose"];
pub(in crate::telemetry) const PERFORMANCE: [&str; 2] = ["ok", "regressed"];
pub(in crate::telemetry) const OPTIMIZATION: [&str; 3] = ["applied", "proposed", "rejected"];

fn text(v: &Value, min: usize) -> bool {
    v.as_str().is_some_and(|s| s.trim().chars().count() >= min)
}

pub(in crate::telemetry) fn check(report: &Value) -> Result<()> {
    ensure!(report["report_url"].as_str().is_some_and(|u| u.starts_with("https://reports.example.com")), "report.json report_url is missing");
    let growth = report["growth"].as_array().filter(|g| !g.is_empty())
        .context("report.json growth must list {scope, bytes, cause} for the corpora or files that grew")?;
    for (i, row) in growth.iter().enumerate() {
        ensure!(text(&row["scope"], 2) && row["bytes"].is_number() && text(&row["cause"], 10),
            "report.json growth[{i}] needs scope, bytes and a cause of 10+ characters");
    }
    let split = &report["split"];
    let verdict = split["verdict"].as_str().unwrap_or("");
    ensure!(SPLIT.contains(&verdict), "report.json split.verdict must be one of {SPLIT:?}");
    ensure!(text(&split["reason"], 20), "report.json split.reason must give the measured reason (20+ characters)");
    if verdict == "propose" {
        ensure!(text(&split["plan"], 20), "report.json split.plan must say which corpora move to which file and how recall is proven");
    }
    let perf = &report["performance"];
    ensure!(PERFORMANCE.contains(&perf["verdict"].as_str().unwrap_or("")), "report.json performance.verdict must be one of {PERFORMANCE:?}");
    ensure!(text(&perf["evidence"], 20), "report.json performance.evidence must cite the latency or lock figures (20+ characters)");
    let rows = report["optimizations"].as_array().filter(|o| !o.is_empty())
        .context("report.json optimizations must list every optimization considered, applied or not")?;
    for (i, row) in rows.iter().enumerate() {
        let verdict = row["verdict"].as_str().unwrap_or("");
        ensure!(OPTIMIZATION.contains(&verdict), "report.json optimizations[{i}].verdict must be one of {OPTIMIZATION:?}");
        ensure!(text(&row["action"], 5) && text(&row["reason"], 10), "report.json optimizations[{i}] needs action and a reason of 10+ characters");
        if verdict == "applied" {
            ensure!(text(&row["before"], 3) && text(&row["after"], 3), "report.json optimizations[{i}] is applied but lacks before/after measurements");
            ensure!(text(&row["recall"], 10), "report.json optimizations[{i}] is applied but does not state the recall check (vector-recall golden sets)");
        }
    }
    Ok(())
}

pub(in crate::telemetry) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    if event["trigger"] != "vector_db_growth_reached" {
        return Ok(());
    }
    let path = root(home).join("analysis").join(id).join("report.json");
    let report: Value = serde_json::from_slice(&std::fs::read(&path).with_context(|| format!("{} missing", path.display()))?)?;
    check(&report)
}
