//! Attribute crashes that a repair session caused itself (pre-fix reproductions, bisection probes,
//! baseline tests) to that session instead of queueing a new repair for each of them.
use super::{finished, root};
use crate::monitor::write_json;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

/// One repair session's span on its own device and process, with the crash family it owns.
struct Window {
    owner: String,
    udid: String,
    family: String,
    start: f64,
    end: f64,
}

/// Crash family: process, exception, signal and the first frame past the abort/kill and
/// release frames. Coarser than the queue signature so assertion variants of one fault
/// (resolve_offset or not) and release crashes whose top frame varies between `doDecrementSlow`
/// and `_swift_release_dealloc` (incident 814504369575276730715) match.
pub(super) fn family(report: &Value) -> Option<String> {
    let frame = report["frames"]
        .as_array()?
        .iter()
        .filter_map(|f| f["symbol"].as_str())
        .find(|s| !runtime_frame(s))?;
    Some(format!(
        "{}|{}|{}|{frame}",
        report["process"].as_str()?,
        report["exception_type"].as_str().unwrap_or_default(),
        report["signal"].as_str().unwrap_or_default()
    ))
}

/// Abort, kill and reference-count release frames, which sit above the frame that owns the fault.
fn runtime_frame(symbol: &str) -> bool {
    matches!(symbol, "__pthread_kill" | "pthread_kill" | "abort" | "<deduplicated_symbol>")
        || symbol.contains("RefCounts<")
        || ["swift_release", "_swift_release", "swift_unknownObjectRelease", "swift_bridgeObjectRelease", "objc_release"]
            .iter()
            .any(|p| symbol.starts_with(p))
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn windows(home: &Path, dispatches: &[Value]) -> Vec<Window> {
    let incidents = root(home).join("incidents");
    dispatches
        .iter()
        .filter(|d| d["started"] == true)
        .filter_map(|d| {
            let owner = d["incident"].as_str()?.to_string();
            let report = read(&incidents.join(format!("{owner}.json")))?["report"].clone();
            let end = if finished(home, d) {
                read(&incidents.join(format!("{owner}.ack")))?["acknowledged_at_ms"].as_f64()?
            } else {
                f64::INFINITY
            };
            Some(Window {
                udid: report["simulator_udid"].as_str()?.to_string(),
                family: family(&report)?,
                start: d["at_ms"].as_f64()?,
                end,
                owner,
            })
        })
        .collect()
}

/// Acknowledge each unacknowledged crash that happened inside a repair window on the same simulator
/// with the same crash family; the ack names the owning repair. Returns the attributions made.
pub(super) fn attribute(home: &Path, dispatches: &[Value], now: f64) -> Result<Vec<Value>> {
    let spans = windows(home, dispatches);
    let directory = root(home).join("incidents");
    let mut made = Vec::new();
    if spans.is_empty() || !directory.try_exists()? {
        return Ok(made);
    }
    for entry in std::fs::read_dir(&directory)?.take(4096) {
        let file = entry?.path();
        if file.extension().is_none_or(|e| e != "json") || file.with_extension("ack").exists() {
            continue;
        }
        let Some(event) = read(&file).filter(|e| e["trigger"] == "xcode_runtime_crash") else {
            continue;
        };
        let report = &event["report"];
        let (Some(id), Some(udid), Some(kind)) =
            (event["id"].as_str(), report["simulator_udid"].as_str(), family(report))
        else {
            continue;
        };
        let at = report["modified_at_ms"].as_f64().or(event["detected_at_ms"].as_f64());
        let Some(span) = spans.iter().find(|w| {
            w.owner != id && w.udid == udid && w.family == kind
                && at.is_some_and(|t| t >= w.start && t <= w.end)
        }) else {
            continue;
        };
        if dispatches.iter().any(|d| d["incident"] == id && d["started"] == true) {
            continue;
        }
        let record = json!({"acknowledged_at_ms":now,"attributed_to":span.owner,
            "reason":"crash on the repair's simulator with its crash family while that repair was open"});
        write_json(&file.with_extension("ack"), &record)?;
        made.push(json!({"incident":id,"attributed_to":span.owner}));
    }
    Ok(made)
}
