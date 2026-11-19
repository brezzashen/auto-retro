//! `monitor-ack` gate of a wait retro (incidents with `requirements.wait_review`): report.json must
//! judge every covered session, cover the three review parts the founder asked for, and point at
//! the infrastructure skill's wait ledger holding this incident's input-error rows.
use crate::monitor::root;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::path::Path;

const VERDICTS: [&str; 4] = ["necessary", "avoidable", "stuck", "input_error"];
const FINDINGS: [&str; 5] = ["stuck", "unreleased", "silent", "slow", "ok"];
const SECTIONS: [&str; 3] = ["信息填写", "队列机制", "脚本优化"];

fn text(v: &Value) -> bool { v.as_str().is_some_and(|s| !s.trim().is_empty()) }

fn items<'a>(report: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    report[key].as_array().with_context(|| format!("report.json {key}[] is missing"))
}

pub(in crate::telemetry) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    if event["requirements"]["wait_review"].is_null() { return Ok(()); }
    let dir = root(home).join("analysis").join(id);
    let report: Value = serde_json::from_slice(&std::fs::read(dir.join("report.json")).context("report.json missing")?)?;
    let sessions = items(&report, "sessions")?;
    for path in event["sessions"].as_array().into_iter().flatten().filter_map(|s| s["path"].as_str()) {
        let entry = sessions.iter().find(|s| s["path"] == path).with_context(|| format!("sessions[] has no verdict for {path}"))?;
        ensure!(entry["verdict"].as_str().is_some_and(|v| VERDICTS.contains(&v)), "sessions[] {path}: verdict must be one of {VERDICTS:?}");
        ensure!(text(&entry["cause"]), "sessions[] {path}: cause is empty");
    }
    let inputs = items(&report, "input_errors")?;
    for row in inputs {
        ensure!(text(&row["input"]) && text(&row["cost"]) && text(&row["guard"]), "input_errors[] rows need input, cost and guard");
    }
    let ledger = &report["infra_ledger"];
    if inputs.is_empty() {
        ensure!(text(&ledger["path"]) || text(&ledger["no_rows_reason"]), "infra_ledger needs path or no_rows_reason");
    }
    if let Some(path) = ledger["path"].as_str() {
        let body = std::fs::read_to_string(path).with_context(|| format!("infra_ledger.path {path} is not readable"))?;
        ensure!(body.contains(id), "the infrastructure wait ledger {path} has no row for incident {id}");
    } else {
        ensure!(inputs.is_empty(), "input_errors[] rows must be recorded in the infrastructure wait ledger (infra_ledger.path)");
    }
    let queues = items(&report, "queues")?;
    ensure!(!queues.is_empty(), "queues[] must review at least one queue mechanism");
    for q in queues {
        ensure!(text(&q["mechanism"]) && text(&q["evidence"]), "queues[] rows need mechanism and evidence");
        ensure!(q["finding"].as_str().is_some_and(|f| FINDINGS.contains(&f)), "queues[] finding must be one of {FINDINGS:?}");
        ensure!(q["finding"] == "ok" || text(&q["change"]) || text(&q["no_change_reason"]), "queues[] rows with a defect need change or no_change_reason");
    }
    let scripts = items(&report, "scripts")?;
    ensure!(!scripts.is_empty() || text(&report["scripts_none_reason"]), "scripts[] is empty and scripts_none_reason is missing");
    for s in scripts {
        ensure!(text(&s["repeat"]) && (text(&s["commit"]) || text(&s["no_change_reason"])), "scripts[] rows need repeat and commit or no_change_reason");
    }
    let markdown = std::fs::read_to_string(dir.join("report.md")).unwrap_or_default();
    for section in SECTIONS { ensure!(markdown.contains(section), "report.md needs a {section} section"); }
    Ok(())
}
