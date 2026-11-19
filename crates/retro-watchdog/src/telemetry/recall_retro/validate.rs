//! `monitor-ack` gate of a recall retro (`recall_calls_reached`): the content audit ran, and
//! report.json gives judged recall@1/3/5 and the content pass rate for every audited corpus, plus
//! the report link. A declared number without the audit files is refused.
use crate::monitor::root;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::path::Path;

pub(in crate::telemetry) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    if event["trigger"] != "recall_calls_reached" {
        return Ok(());
    }
    let dir = root(home).join("analysis").join(id);
    let audit: Value = serde_json::from_slice(&std::fs::read(dir.join("audit/summary.json"))
        .context("audit/summary.json missing: run local-search-eval audit --window <window> --out analysis/<id>/audit")?)?;
    ensure!(dir.join("audit/queries.jsonl").exists(), "audit/queries.jsonl missing");
    let report: Value = serde_json::from_slice(&std::fs::read(dir.join("report.json")).context("report.json missing")?)?;
    ensure!(report["report_url"].as_str().is_some_and(|u| u.starts_with("https://reports.example.com")), "report.json report_url is missing");
    for (corpus, summary) in audit["corpora"].as_object().into_iter().flatten() {
        if summary["queries"].as_u64().unwrap_or(0) == 0 {
            continue;
        }
        let recall = &report["recall"][corpus];
        for key in ["judged", "at1", "at3", "at5"] {
            ensure!(recall[key].is_number(), "report.json recall.{corpus}.{key} must be a number");
        }
        ensure!(report["content"][corpus]["ok_pct"].is_number() || summary["hits"].as_u64() == Some(0),
            "report.json content.{corpus}.ok_pct must be a number");
    }
    Ok(())
}
