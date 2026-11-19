//! One Xcode watchdog session (founder, 2026-10-27): Xcode runtime crashes and SpringBoard crashes
//! are conditions of one trigger. The first eligible crash leads a session; every other eligible
//! crash, whatever its signature or process, joins that session's `covers` while it is open instead
//! of waiting for a session of its own. The session groups them by crash family and repairs them
//! together; `monitor-ack` of the lead requires a `crashes[]` entry per covered crash, and the ack
//! then closes the covered incidents.
use super::{attribution, root};
use crate::monitor::write_json;
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{collections::HashSet, path::Path};

/// Incidents already covered by a started session.
pub(super) fn covered(dispatches: &[Value]) -> HashSet<String> {
    dispatches.iter().filter(|d| d["started"] == true)
        .flat_map(|d| d["covers"].as_array().cloned().unwrap_or_default())
        .filter_map(|c| c["incident"].as_str().map(str::to_string)).collect()
}

fn entry(event: &Value, now: f64) -> Value {
    json!({"incident": event["id"], "trigger": event["trigger"], "process": event["report"]["process"],
        "family": attribution::family(&event["report"]), "joined_at_ms": now})
}

/// Adds `events` to the session `dispatch` covers; returns how many joined.
pub(super) fn join(dispatch: &mut Value, events: &[&Value], now: f64) -> usize {
    if !dispatch["covers"].is_array() { dispatch["covers"] = json!([]); }
    let lead = dispatch["incident"].clone();
    let covers = dispatch["covers"].as_array_mut().unwrap();
    let mut joined = 0;
    for event in events {
        if event["id"] != lead &&!covers.iter().any(|c| c["incident"] == event["id"]) {
            covers.push(entry(event, now));
            joined += 1;
        }
    }
    joined
}

fn session<'a>(state: &'a Value, id: &str) -> Option<&'a Value> {
    state["dispatches"].as_array()?.iter().find(|d| d["incident"] == id && d["started"] == true)
}

fn read(home: &Path) -> Value {
    crate::telemetry::read_state(&root(home).join("crash-repair.json")).unwrap_or(Value::Null)
}

fn acked(home: &Path, id: &str) -> bool { root(home).join("incidents").join(format!("{id}.ack")).exists() }

/// monitor-ack gate for a lead crash: report.json `crashes[]` names every covered crash that no
/// attribution acknowledged yet, with its family, status and cause.
pub(crate) fn validate(home: &Path, id: &str) -> Result<()> {
    let state = read(home);
    let Some(lead) = session(&state, id) else { return Ok(()) };
    let open: Vec<&str> = lead["covers"].as_array().into_iter().flatten().filter_map(|c| c["incident"].as_str())
        .filter(|c| !acked(home, c)).collect();
    if open.is_empty() { return Ok(()); }
    let report: Value = serde_json::from_slice(&std::fs::read(root(home).join("analysis").join(id).join("report.json"))
        .context("report.json missing")?)?;
    let listed: Vec<&str> = report["crashes"].as_array().into_iter().flatten()
        .filter(|c| ["status", "cause"].iter().all(|k| c[*k].as_str().is_some_and(|v| !v.trim().is_empty())))
        .filter_map(|c| c["incident"].as_str()).collect();
    let missing: Vec<&str> = open.into_iter().filter(|c| !listed.contains(c)).collect();
    ensure!(missing.is_empty(), "report.json crashes[] needs {{incident, family, status, cause}} for every crash this Xcode session covers (crash-repair.json covers); missing {}", missing.join(", "));
    Ok(())
}

/// After the lead's acknowledgement, closes the crashes it covered.
pub(crate) fn close_covered(home: &Path, id: &str) -> Result<()> {
    let state = read(home);
    let Some(lead) = session(&state, id) else { return Ok(()) };
    for covered in lead["covers"].as_array().into_iter().flatten().filter_map(|c| c["incident"].as_str()) {
        if !acked(home, covered) {
            write_json(&root(home).join("incidents").join(format!("{covered}.ack")),
                &json!({"acknowledged_at_ms": crate::unix_now() * 1000.0, "covered_by": id, "kind": "xcode_session"}))?;
        }
    }
    Ok(())
}
