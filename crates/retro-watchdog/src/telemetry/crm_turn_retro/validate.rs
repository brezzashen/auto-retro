//! `monitor-ack` gate of a CRM turn retro (incidents with `requirements.crm_turn_review`).
//! report.json must:
//! - judge every waste class the incident ranked (`classes[] {class, verdict, evidence}`; verdict
//!   `fixed`, `not_waste`, `deferred` or `needs_founder`, the last three with a `reason`);
//! - give each shipped fix as `findings[] {cause, layer, evidence, change, commit, before, after,
//!   quality, production {digest | not_promoted_reason}, probe | no_probe_reason}` with `before` and
//!   `after` measured on `beta` and `canary`, or state `no_fix_reason` when nothing qualified;
//! - give a verdict for every probe of the follow-up receipt (`followups[] {probe, verdict, analysis, next}`);
//! - judge every cache cause the incident required while the main hit rate was under target
//!   (`cache.causes[] {cause, verdict, evidence, reason}`) and every required cost line
//!   (`cost.lines[] {line, verdict, evidence, reason}`), with the class verdicts;
//! - link the published page (`report_url`).
//! Incidents opened before the cache and cost review carry no `cache` / `cost` requirement and pass without them.
use super::probes;
use crate::monitor::root;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::path::Path;

const CLASS_VERDICTS: [&str; 4] = ["fixed", "not_waste", "deferred", "needs_founder"];
const LAYERS: [&str; 5] = ["tool", "route", "processor", "prompt", "database"];
const SECTIONS: [&str; 4] = ["无效消耗", "修复", "上线", "后续探针"];

fn text(v: &Value) -> bool { v.as_str().is_some_and(|s| !s.trim().is_empty()) }

/// Every name of `required` needs an entry in `entries` (`key` = the name) with a class verdict, evidence and,
/// unless fixed, a reason.
fn judged(entries: &Value, required: &Value, path: &str, key: &str) -> Result<()> {
    for name in required.as_array().into_iter().flatten().filter_map(Value::as_str) {
        let entry = entries.as_array().into_iter().flatten().find(|e| e[key] == name)
            .with_context(|| format!("{path}[] has no verdict for {name}, which the incident required"))?;
        let verdict = entry["verdict"].as_str().unwrap_or("");
        ensure!(CLASS_VERDICTS.contains(&verdict), "{path}[] {name}: verdict must be one of {CLASS_VERDICTS:?}");
        ensure!(text(&entry["evidence"]), "{path}[] {name}: evidence is empty (thread ids, tokens and US$ from stats)");
        ensure!(verdict == "fixed" || text(&entry["reason"]), "{path}[] {name}: {verdict} needs a reason");
    }
    Ok(())
}

pub(in crate::telemetry) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    let review = &event["requirements"]["crm_turn_review"];
    if review.is_null() { return Ok(()); }
    let dir = root(home).join("analysis").join(id);
    let report: Value = serde_json::from_slice(&std::fs::read(dir.join("report.json")).context("report.json missing")?)?;
    let classes = report["classes"].as_array().context("report.json classes[] is missing")?;
    for class in review["classes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let entry = classes.iter().find(|c| c["class"] == class).with_context(|| format!("classes[] has no verdict for {class}, which the incident ranked"))?;
        let verdict = entry["verdict"].as_str().unwrap_or("");
        ensure!(CLASS_VERDICTS.contains(&verdict), "classes[] {class}: verdict must be one of {CLASS_VERDICTS:?}");
        ensure!(text(&entry["evidence"]), "classes[] {class}: evidence is empty (thread ids and the analyze-rounds finding)");
        ensure!(verdict == "fixed" || text(&entry["reason"]), "classes[] {class}: {verdict} needs a reason");
    }
    judged(&report["cache"]["causes"], &review["cache"]["causes"], "cache.causes", "cause")?;
    // The founder keeps the 90% target (2026-11-12): under it no cache cause is "not waste"; an unsolved one is
    // deferred with the lever under trial, or needs_founder with a costed proposal.
    let below = review["cache"]["hit"].as_f64().zip(review["cache"]["target"].as_f64()).is_some_and(|(hit, target)| hit < target);
    if let Some(c) = report["cache"]["causes"].as_array().into_iter().flatten().find(|c| below && c["verdict"] == "not_waste") {
        anyhow::bail!("cache.causes[] {}: not_waste is refused while the hit rate is under target; use deferred with the lever under trial or needs_founder with a costed proposal", c["cause"]);
    }
    judged(&report["cost"]["lines"], &review["cost"]["lines"], "cost.lines", "line")?;
    let findings = report["findings"].as_array().cloned().unwrap_or_default();
    ensure!(!findings.is_empty() || text(&report["no_fix_reason"]), "report.json needs findings[] or no_fix_reason");
    let fixed = |v: &Value| v.as_array().into_iter().flatten().any(|c| c["verdict"] == "fixed");
    ensure!(!(fixed(&report["classes"]) || fixed(&report["cache"]["causes"]) || fixed(&report["cost"]["lines"])) || !findings.is_empty(), "a class, cache cause or cost line is marked fixed but findings[] is empty");
    for (i, finding) in findings.iter().enumerate() {
        for key in ["cause", "evidence", "change", "commit", "quality"] { ensure!(text(&finding[key]), "findings[{i}].{key} is empty"); }
        ensure!(finding["layer"].as_str().is_some_and(|l| LAYERS.contains(&l)), "findings[{i}].layer must be one of {LAYERS:?}");
        for side in ["before", "after"] {
            for env in ["beta", "canary"] { ensure!(!finding[side][env].is_null(), "findings[{i}].{side}.{env} is missing: measure before and after in the same environment"); }
        }
        let production = &finding["production"];
        ensure!(text(&production["digest"]) || text(&production["not_promoted_reason"]), "findings[{i}].production needs digest (after mars canary promote) or not_promoted_reason");
        ensure!(text(&finding["probe"]) || text(&finding["no_probe_reason"]), "findings[{i}] needs probe (its id in crm-turn-probes.json) or no_probe_reason");
        if let Some(probe) = finding["probe"].as_str() {
            ensure!(probes::load(home).iter().any(|p| p["id"] == probe), "findings[{i}].probe {probe} is not in crm-turn-probes.json");
        }
    }
    let receipt: Value = serde_json::from_slice(&std::fs::read(probes::receipt(home, id))
        .with_context(|| format!("run cpu-watchdog crm-turn-stats --followup {id} before new findings"))?)?;
    let followups = report["followups"].as_array().cloned().unwrap_or_default();
    for probe in receipt["probes"].as_array().into_iter().flatten().filter_map(|p| p["probe"].as_str()) {
        let entry = followups.iter().find(|f| f["probe"] == probe).with_context(|| format!("followups[] has no verdict for probe {probe}"))?;
        let verdict = entry["verdict"].as_str().unwrap_or("");
        ensure!(probes::VERDICTS.contains(&verdict), "followups[] {probe}: verdict must be one of {:?}", probes::VERDICTS);
        ensure!(text(&entry["analysis"]), "followups[] {probe}: analysis is empty");
        ensure!(!matches!(verdict, "ineffective" | "regressed") || text(&entry["next"]), "followups[] {probe}: {verdict} needs next (why the waste persisted)");
    }
    ensure!(text(&report["report_url"]), "report.json report_url is empty");
    let markdown = std::fs::read_to_string(dir.join("report.md")).unwrap_or_default();
    for section in SECTIONS { ensure!(markdown.contains(section), "report.md needs a {section} section"); }
    for (key, section) in [("cache", "缓存命中"), ("cost", "成本结构")] {
        ensure!(review[key].is_null() || markdown.contains(section), "report.md needs a {section} section");
    }
    Ok(())
}
