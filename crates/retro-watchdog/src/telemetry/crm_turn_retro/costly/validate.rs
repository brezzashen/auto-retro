//! `monitor-ack` gate of a costly-turn review (incidents with `requirements.crm_costly_review`).
//! report.json must judge every required turn (`turns[] {turn, ...}`) and every required signature group
//! (`groups[] {signature, ...}`) with:
//! - `verdict`: `normal`, `abnormal` or `inconclusive`;
//! - `cache`: `invalidated`, `intact` or `unknown`; `invalidated` also names `cache_cause` (a causeCatalog cause);
//! - `evidence` and `reason`;
//! - `abnormal` also names `cause` and `handoff` (`crm-turn-retro`, `needs_founder` or `none` with why);
//! - `inconclusive` also names `next` (the evidence that settles it).
//!
//! Every account in `slow_accounts` (turns that waited `slow::SLOW_FIRST_TEXT_MS` for their first text) needs a
//! `slow[] {account, cause, evidence, reason, action}` entry, `cause` one of `slow::CAUSES`, and report.md a 首字等待 section.
//!
//! It also links the published page (`report_url`), and report.md carries the SECTIONS headings.
use crate::monitor::root;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::path::Path;

const VERDICTS: [&str; 3] = ["normal", "abnormal", "inconclusive"];
const CACHE: [&str; 3] = ["invalidated", "intact", "unknown"];
const SECTIONS: [&str; 3] = ["判定", "缓存", "原因"];

fn text(v: &Value) -> bool { v.as_str().is_some_and(|s| !s.trim().is_empty()) }

pub(in crate::telemetry) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    let review = &event["requirements"]["crm_costly_review"];
    if review.is_null() { return Ok(()); }
    let dir = root(home).join("analysis").join(id);
    let report: Value = serde_json::from_slice(&std::fs::read(dir.join("report.json")).context("report.json missing")?)?;
    validate_report(review, &report, &std::fs::read_to_string(dir.join("report.md")).unwrap_or_default())
}

fn judged(entry: &Value, label: &str) -> Result<()> {
    let verdict = entry["verdict"].as_str().unwrap_or("");
    ensure!(VERDICTS.contains(&verdict), "{label}: verdict must be one of {VERDICTS:?}");
    let cache = entry["cache"].as_str().unwrap_or("");
    ensure!(CACHE.contains(&cache), "{label}: cache must be one of {CACHE:?}");
    ensure!(cache != "invalidated" || text(&entry["cache_cause"]), "{label}: cache invalidated needs cache_cause (the causeCatalog cause)");
    ensure!(text(&entry["evidence"]), "{label}: evidence is empty (thread ids, steps, tokens, cache rows, tool errors)");
    ensure!(text(&entry["reason"]), "{label}: reason is empty");
    if verdict == "abnormal" {
        ensure!(text(&entry["cause"]), "{label}: abnormal needs cause");
        ensure!(text(&entry["handoff"]), "{label}: abnormal needs handoff (crm-turn-retro, needs_founder, or none with why)");
    }
    ensure!(verdict != "inconclusive" || text(&entry["next"]), "{label}: inconclusive needs next (the evidence that settles it)");
    Ok(())
}

pub(super) fn validate_report(review: &Value, report: &Value, markdown: &str) -> Result<()> {
    let turns = report["turns"].as_array().context("report.json turns[] is missing")?;
    for key in review["turns"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let entry = turns.iter().find(|e| e["turn"] == key).with_context(|| format!("turns[] has no verdict for {key}"))?;
        judged(entry, &format!("turns[] {key}"))?;
    }
    let groups = report["groups"].as_array().context("report.json groups[] is missing")?;
    for sig in review["groups"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let entry = groups.iter().find(|e| e["signature"] == sig).with_context(|| format!("groups[] has no verdict for {sig}"))?;
        judged(entry, &format!("groups[] {sig}"))?;
    }
    let slow = review["slow_accounts"].as_array().filter(|a| !a.is_empty());
    for account in slow.into_iter().flatten().filter_map(Value::as_str) {
        let entry = report["slow"].as_array().and_then(|s| s.iter().find(|e| e["account"] == account))
            .with_context(|| format!("slow[] has no cause for account {account}"))?;
        let label = format!("slow[] {account}");
        ensure!(entry["cause"].as_str().is_some_and(|c| super::slow::CAUSES.contains(&c)), "{label}: cause must be one of {:?}", super::slow::CAUSES);
        for field in ["evidence", "reason", "action"] { ensure!(text(&entry[field]), "{label}: {field} is empty"); }
    }
    ensure!(slow.is_none() || markdown.contains("首字等待"), "report.md needs a 首字等待 section");
    ensure!(text(&report["report_url"]), "report.json report_url is empty");
    for section in SECTIONS { ensure!(markdown.contains(section), "report.md needs a {section} section"); }
    Ok(())
}
