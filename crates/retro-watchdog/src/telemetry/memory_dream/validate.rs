//! `monitor-ack` gate of a memory dream (`memory_dream_due`, formerly `memory_changes_reached`):
//! recall was measured and the store scanned both before and after (files written by
//! `agent-memory eval` / `dream --out`), report.json copies those numbers, counts every action,
//! and recall did not drop by more than one point without a stated reason; broken links may not
//! grow. The recall misses were reviewed (`misses_reviewed`), and every recorded recall gap and
//! refreshed hub note is listed (`gaps[]` with what was asked and where to get it instead, `hubs[]`).
//! An incident with `contract: 2` also passes the session audit gate (`audit.rs`).
use crate::monitor::root;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::path::Path;

pub(in crate::telemetry) const ACTIONS: [&str; 10] = ["links_repointed", "links_removed", "merged", "superseded", "verified",
    "forgotten", "rewritten", "paths_fixed", "gaps_recorded", "hubs_updated"];

/// `gaps[]` and `hubs[]` match their action counts and carry the fields a later agent needs.
fn check_lists(report: &Value) -> Result<()> {
    ensure!(report["misses_reviewed"].is_u64(), "report.json misses_reviewed must count the recall-miss lines read (jq 'select(.miss)' on recalls.jsonl)");
    for (list, action, fields) in [("gaps", "gaps_recorded", &["name", "asked", "instead"][..]), ("hubs", "hubs_updated", &["name"][..])] {
        let rows = report[list].as_array().with_context(|| format!("report.json {list} must be an array (empty when none)"))?;
        ensure!(Some(rows.len() as u64) == report["actions"][action].as_u64(), "report.json {list} has {} rows but actions.{action} is {}", rows.len(), report["actions"][action]);
        for (i, row) in rows.iter().enumerate() {
            for f in fields {
                ensure!(row[*f].as_str().is_some_and(|s| !s.trim().is_empty()), "report.json {list}[{i}].{f} must be a non-empty string");
            }
        }
    }
    Ok(())
}
/// Largest tolerated drop in percentage points (one single-set case is 0.67).
const DROP: f64 = 1.0;

fn load(dir: &Path, name: &str, how: &str) -> Result<Value> {
    let bytes = std::fs::read(dir.join(name)).with_context(|| format!("{name} missing: {how}"))?;
    serde_json::from_slice(&bytes).with_context(|| format!("{name} is not JSON"))
}

fn num(v: &Value, path: &[&str]) -> Option<f64> {
    path.iter().try_fold(v, |v, k| v.get(*k)).and_then(Value::as_f64)
}

pub(in crate::telemetry) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    if event["trigger"] != "memory_dream_due" && event["trigger"] != "memory_changes_reached" {
        return Ok(());
    }
    let dir = root(home).join("analysis").join(id);
    let eval = "agent-memory eval --set <golden-memory-single> --set <golden-memory-multi> > analysis/<id>/eval-<before|after>.json";
    let scan = "agent-memory dream --out analysis/<id>/scan-<before|after>.json";
    let (eb, ea) = (load(&dir, "eval-before.json", eval)?, load(&dir, "eval-after.json", eval)?);
    let (sb, sa) = (load(&dir, "scan-before.json", scan)?, load(&dir, "scan-after.json", scan)?);
    let report = load(&dir, "report.json", "write report.json")?;
    ensure!(report["report_url"].as_str().is_some_and(|u| u.starts_with("https://reports.example.com")), "report.json report_url is missing");
    let metrics = [("single", "top5_pct"), ("single", "hook_injected_pct"), ("multi", "coverage_top5_pct")];
    for (when, file) in [("before", &eb), ("after", &ea)] {
        for (set, key) in metrics {
            let measured = num(file, &["sets", set, key]).with_context(|| format!("eval-{when}.json lacks sets.{set}.{key}"))?;
            let declared = num(&report, &["eval", when, set, key]).with_context(|| format!("report.json eval.{when}.{set}.{key} must be a number"))?;
            ensure!((measured - declared).abs() < 0.05, "report.json eval.{when}.{set}.{key} is {declared}, eval-{when}.json measured {measured}");
        }
    }
    let reason = report["regression_reason"].as_str().is_some_and(|r| r.trim().len() >= 20);
    for (set, key) in metrics {
        let (b, a) = (num(&eb, &["sets", set, key]).unwrap_or(0.0), num(&ea, &["sets", set, key]).unwrap_or(0.0));
        ensure!(a + DROP >= b || reason, "recall dropped: {set}.{key} {b} -> {a}; undo the change or give regression_reason");
    }
    for (when, file) in [("before", &sb), ("after", &sa)] {
        for key in ["broken_link", "superseded_link", "missing_path", "near_duplicate"] {
            let measured = num(file, &["stats", key]).with_context(|| format!("scan-{when}.json lacks stats.{key}"))?;
            ensure!(num(&report, &["scan", when, key]) == Some(measured), "report.json scan.{when}.{key} must equal scan-{when}.json ({measured})");
        }
    }
    ensure!(num(&sa, &["stats", "broken_link"]) <= num(&sb, &["stats", "broken_link"]), "broken links grew during the dream");
    for key in ACTIONS {
        ensure!(report["actions"][key].is_u64(), "report.json actions.{key} must be a count");
    }
    check_lists(&report)?;
    if event["contract"].as_u64() >= Some(2) {
        let store = event["store"].as_str().map_or_else(|| super::store(home), std::path::PathBuf::from);
        super::audit::validate(home, id, &dir, (&store, event["head"].as_str().unwrap_or_default()), &report, &eb, &ea)?;
    }
    Ok(())
}
