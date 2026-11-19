//! Reading `recover-disk`'s output for the S0 run record.
use serde_json::{json, Value};

/// `recover-disk`'s JSON: the deploy lease prints its notices to stdout first, so parsing starts at the
/// first line that opens an object.
pub(in crate::telemetry) fn report_json(text: &str) -> Value {
    let start = if text.starts_with('{') { Some(0) } else { text.find("\n{").map(|at| at + 1) };
    start.and_then(|at| serde_json::from_str(&text[at..]).ok()).unwrap_or(Value::Null)
}

/// The recovery verdict and per-step counts of `recover-disk`'s JSON, without the item lists.
pub(in crate::telemetry) fn summarize(report: &Value) -> Value {
    if report.is_null() {
        return Value::Null;
    }
    let steps: Vec<Value> = report["steps"].as_array().map(|s| s.iter()
        .map(|step| json!({"step": step["step"], "reclaimed": step["reclaimed"]})).collect()).unwrap_or_default();
    json!({"recovered": report["recovered"], "reclaimed_items": report["totalReclaimedItems"], "steps": steps,
        "docker_vm": {"before_gib": report["dockerVm"]["before"]["freeGib"], "after_gib": report["dockerVm"]["after"]["freeGib"]}})
}
