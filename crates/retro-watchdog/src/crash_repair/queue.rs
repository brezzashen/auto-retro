//! Select only authorized runtime crashes and preserve a bounded number of new reports per pass.
use super::{evidence, root};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{collections::HashSet, path::Path};

pub(super) fn pending(home: &Path, policy: &Value, dispatches: &[Value]) -> Result<Vec<Value>> {
    let since = policy["enabled_at_ms"]
        .as_f64()
        .context("Missing crash repair activation time")?;
    let directory = root(home).join("incidents");
    let mut queue = Vec::new();
    if !directory.try_exists()? {
        return Ok(queue);
    }
    // Crashes covered by a session (`batch.rs`) belong to it, duplicates of its signature included.
    let covered: HashSet<String> = super::batch::covered(dispatches);
    for (index, entry) in std::fs::read_dir(&directory)?.take(4097).enumerate() {
        anyhow::ensure!(
            index < 4096,
            "Crash incident directory exceeds 4096 entries"
        );
        let file = entry?.path();
        if file.extension().is_none_or(|e| e != "json") || file.with_extension("ack").exists() {
            continue;
        }
        let event: Value = serde_json::from_slice(&std::fs::read(&file)?)?;
        let Some(id) = event["id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        let springboard = super::springboard::eligible(policy, &event["report"]);
        let ordinary = event["trigger"] == "xcode_runtime_crash" && event["report"]["known_issue"].is_null();
        if !ordinary && !(event["trigger"] == "springboard_runtime_crash" && springboard) {
            continue;
        }
        let explicit = policy["include_incidents"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|item| item == id));
        if !explicit && !springboard && event["detected_at_ms"].as_f64().is_none_or(|at| at < since) {
            continue;
        }
        if covered.contains(id) || dispatches
            .iter()
            .any(|d| d["incident"] == id && d["started"] == true)
        {
            continue;
        }
        queue.push(event);
    }
    queue.sort_by(|a, b| {
        a["detected_at_ms"]
            .as_f64()
            .partial_cmp(&b["detected_at_ms"].as_f64())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(queue)
}

/// A crash whose report has been missing this long will never have evidence.
const LOST_AFTER_MS: f64 = 3_600_000.0;

/// Saves evidence for up to eight queued crashes that lack it and returns (errors, lost).
/// A crash whose report file is gone an hour after detection is `lost`: it is acknowledged as
/// `evidence_lost` and leaves the queue. Two crashes of 2026-11-02 whose `.ips` files were gone
/// failed this step on every pass for eight days and kept `crash-repair-health` unhealthy, which
/// also hid any new evidence error (report history-recall-audit-20261110, founder 2026-11-10).
pub(super) fn preserve(home: &Path, queue: &[Value], now: f64) -> (Vec<Value>, Vec<Value>) {
    let (mut errors, mut lost) = (Vec::new(), Vec::new());
    for event in queue
        .iter()
        .filter(|event| {
            !root(home)
                .join("analysis")
                .join(event["id"].as_str().unwrap())
                .join("evidence.json")
                .exists()
        })
        .take(8)
    {
        let Err(error) = evidence::save(home, event) else { continue };
        let report = event["report"]["path"].as_str().unwrap_or("");
        let gone = !report.is_empty() && std::fs::symlink_metadata(report).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
        let old = event["detected_at_ms"].as_f64().is_some_and(|at| now - at >= LOST_AFTER_MS);
        if gone && old && acknowledge_lost(home, event, report, now).is_ok() {
            lost.push(json!({"incident":event["id"],"report":report}));
        } else {
            errors.push(json!({"incident":event["id"],"error":format!("{error:#}")}));
        }
    }
    (errors, lost)
}

fn acknowledge_lost(home: &Path, event: &Value, report: &str, now: f64) -> Result<()> {
    let id = event["id"].as_str().context("Missing incident id")?;
    let record = json!({"acknowledged_at_ms":now,"attributed_to":"evidence_lost","report":report,
        "reason":"the crash report is no longer on disk, so no evidence can be preserved and no repair can start from it"});
    crate::monitor::write_json(&root(home).join("analysis").join(id).join("evidence-lost.json"), &record)?;
    crate::monitor::write_json(&root(home).join("incidents").join(format!("{id}.ack")), &record)
}
