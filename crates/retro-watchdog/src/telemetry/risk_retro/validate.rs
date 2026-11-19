//! `monitor-ack` gate of a risk retro (incidents with `requirements.risk_review`).
//! report.json must judge every event key the incident listed:
//! `events[] {key, verdict, evidence, reason}` with verdict `real_risk`, `false_positive` or `inconclusive`;
//! - `false_positive` also names the misfiring rule (`rule`, source file:line), a fix `proposal` for it and the
//!   `account` action the founder should take (the exact `mars canary abuse ...` command, or `none` with why);
//! - `inconclusive` also names `next`: the evidence that would settle it and when it is checked.
//! It also links the published page (`report_url`), and report.md carries the SECTIONS headings.
use crate::monitor::root;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::path::Path;

const VERDICTS: [&str; 3] = ["real_risk", "false_positive", "inconclusive"];
const SECTIONS: [&str; 3] = ["判定", "证据", "处置"];

fn text(v: &Value) -> bool { v.as_str().is_some_and(|s| !s.trim().is_empty()) }

/// The rule a false positive names must point at source: `path:line`.
fn file_line(v: &Value) -> bool {
    v.as_str().is_some_and(|s| s.split_whitespace().any(|w| w.rsplit_once(':').is_some_and(|(p, l)| p.contains('.') && l.trim_end_matches([',', ')', '.']).parse::<u32>().is_ok())))
}

pub(in crate::telemetry) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    let review = &event["requirements"]["risk_review"];
    if review.is_null() { return Ok(()); }
    let dir = root(home).join("analysis").join(id);
    let report: Value = serde_json::from_slice(&std::fs::read(dir.join("report.json")).context("report.json missing")?)?;
    validate_report(review, &report, &std::fs::read_to_string(dir.join("report.md")).unwrap_or_default())
}

pub(super) fn validate_report(review: &Value, report: &Value, markdown: &str) -> Result<()> {
    let entries = report["events"].as_array().context("report.json events[] is missing")?;
    for key in review["events"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let entry = entries.iter().find(|e| e["key"] == key).with_context(|| format!("events[] has no verdict for {key}"))?;
        let verdict = entry["verdict"].as_str().unwrap_or("");
        ensure!(VERDICTS.contains(&verdict), "events[] {key}: verdict must be one of {VERDICTS:?}");
        ensure!(text(&entry["evidence"]), "events[] {key}: evidence is empty (turn or request times, thread ids, signal features, rule hit)");
        ensure!(text(&entry["reason"]), "events[] {key}: reason is empty");
        if verdict == "false_positive" {
            ensure!(file_line(&entry["rule"]), "events[] {key}: false_positive needs rule as the misfiring source file:line");
            ensure!(text(&entry["proposal"]), "events[] {key}: false_positive needs a proposal that fixes the rule");
            ensure!(text(&entry["account"]), "events[] {key}: false_positive needs account (the founder's command for this account, or none with why)");
        }
        ensure!(verdict != "inconclusive" || text(&entry["next"]), "events[] {key}: inconclusive needs next (the evidence that settles it)");
    }
    ensure!(text(&report["report_url"]), "report.json report_url is empty");
    for section in SECTIONS { ensure!(markdown.contains(section), "report.md needs a {section} section"); }
    Ok(())
}
