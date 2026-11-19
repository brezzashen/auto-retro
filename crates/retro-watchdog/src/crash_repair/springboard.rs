//! SpringBoard opt-in shares crash-repair deduplication, evidence and dispatch limits.
use crate::monitor::root;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::Path;

pub(crate) fn enabled(policy: &Value) -> bool {
    policy["enabled"] == true && policy["springboard"]["enabled"] == true
}

pub(crate) fn eligible(policy: &Value, report: &Value) -> bool {
    if !enabled(policy) || report["process"] != "SpringBoard" {
        return false;
    }
    let Some(since) = policy["springboard"]["enabled_at_ms"].as_f64()
        .filter(|since| since.is_finite() && *since > 0.0) else { return false; };
    let explicit = policy["springboard"]["include_reports"].as_array().is_some_and(|paths| {
        report["path"].is_string() && paths.iter().any(|path| path == &report["path"])
    });
    explicit || report["modified_at_ms"].as_f64().is_some_and(|at| at >= since)
}

pub(crate) fn observe(home: &Path, reports: &[Value]) -> Result<Value> {
    let path = root(home).join("crash-repair-policy.json");
    let policy: Value = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("Invalid crash repair policy")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(error) => return Err(error.into()),
    };
    if enabled(&policy) {
        anyhow::ensure!(policy["springboard"]["enabled_at_ms"].as_f64()
            .is_some_and(|since| since.is_finite() && since > 0.0), "Missing SpringBoard activation time");
    }
    let mut incidents = Vec::new();
    for report in reports.iter().filter(|report| eligible(&policy, report)) {
        let name = report["path"].as_str().and_then(|p| Path::new(p).file_name())
            .and_then(|n| n.to_str()).context("Missing SpringBoard report filename")?;
        incidents.push(super::super::telemetry::springboard_incident(home, &format!("runtime:{name}"),
            json!({"trigger":"springboard_runtime_crash","report":report}))?);
    }
    Ok(json!({"enabled":enabled(&policy),"enabled_at_ms":policy["springboard"]["enabled_at_ms"],
        "incidents":incidents,"route":"springboard-crash","queue":"crash-repair.json"}))
}
