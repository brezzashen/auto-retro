//! Attribute a known SpringBoard boot abort on a guarded runtime without opening a session (founder,
//! 2026-11-13, incident 816183591079107939622, report springboard-verify-lane-816183591079107939622).
//! The simulator tooling already waits out the boot crash window on the runtimes listed in
//! `crash-repair-policy.json` `springboard.guarded_runtimes`. An abort labelled
//! `springboard_poster_snapshot_rbsassertion` on such a runtime, with no app crash and no failed test
//! run on that simulator in the 15 minutes after it, is acknowledged as `known_issue_guarded` and
//! counted in `crash-repair-guarded.json`. An abort that hurt something queues as before, and so does
//! every abort while the policy key is absent.
use super::root;
use crate::monitor::write_json;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const LABEL: &str = "springboard_poster_snapshot_rbsassertion";
/// How long after the abort a crash or a failed test run on the simulator counts as its damage.
const WINDOW_MS: f64 = 15.0 * 60_000.0;
/// Crash reports and result bundles are written late; the abort precedes its own report.
const SLACK_MS: f64 = 120_000.0;
const DAY_MS: f64 = 86_400_000.0;
const MAX_BUNDLES: usize = 400;
const KEEP_RECENT: usize = 100;

/// Outcome of one pass: aborts acknowledged, aborts still inside their window, and the running count.
pub(super) struct Pass {
    pub attributed: Vec<Value>,
    pub held: Vec<String>,
    pub counts: Value,
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn millis(time: std::io::Result<std::time::SystemTime>) -> Option<f64> {
    Some(time.ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64() * 1000.0)
}

fn guarded(policy: &Value, report: &Value) -> bool {
    report["known_issue"] == LABEL
        && super::springboard::eligible(policy, report)
        && policy["springboard"]["guarded_runtimes"].as_array().is_some_and(|runtimes| {
            report["simulator_runtime"].is_string() && runtimes.iter().any(|runtime| runtime == &report["simulator_runtime"])
        })
}

/// An Xcode runtime crash incident on `udid` from just before the abort to the end of its window.
fn app_crash(home: &Path, udid: &str, at: f64) -> Option<String> {
    std::fs::read_dir(root(home).join("incidents")).ok()?.flatten().take(8192)
        .map(|entry| entry.path())
        .filter(|file| file.extension().is_some_and(|e| e == "json"))
        .filter(|file| millis(std::fs::metadata(file).and_then(|m| m.modified())).is_some_and(|m| m >= at - SLACK_MS))
        .filter_map(|file| read(&file))
        .find(|event| {
            let report = &event["report"];
            event["trigger"] == "xcode_runtime_crash" && report["simulator_udid"] == udid
                && report["modified_at_ms"].as_f64().or(event["detected_at_ms"].as_f64())
                    .is_some_and(|t| t >= at - SLACK_MS && t <= at + WINDOW_MS)
        })
        .and_then(|event| event["id"].as_str().map(str::to_string))
}

/// `xcresulttool get test-results summary` JSON; tests read `summary.json` inside the fixture bundle.
fn summary(bundle: &Path) -> Option<Value> {
    if cfg!(test) {
        return read(&bundle.join("summary.json"));
    }
    let out = std::process::Command::new("/usr/bin/xcrun")
        .args(["xcresulttool", "get", "test-results", "summary", "--compact", "--path"])
        .arg(bundle).output().ok()?;
    out.status.success().then(|| serde_json::from_slice(&out.stdout).ok()).flatten()
}

/// A test result bundle on `udid` overlapping the window whose result is not `Passed`.
/// A bundle `xcresulttool` cannot read names no device, so it never counts.
fn failed_run(home: &Path, udid: &str, at: f64) -> Option<PathBuf> {
    let projects = std::fs::read_dir(home.join("Library/Developer/Xcode/DerivedData")).ok()?;
    projects.flatten()
        .filter_map(|p| std::fs::read_dir(p.path().join("Logs/Test")).ok())
        .flat_map(|d| d.flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "xcresult"))
        .take(MAX_BUNDLES)
        .filter(|p| std::fs::metadata(p).ok().is_some_and(|m| {
            millis(m.created()).is_some_and(|start| start <= at + WINDOW_MS) && millis(m.modified()).is_some_and(|end| end >= at - SLACK_MS)
        }))
        .find(|bundle| summary(bundle).is_some_and(|s| {
            s["result"] != "Passed" && s["devicesAndConfigurations"].as_array()
                .is_some_and(|rows| rows.iter().any(|row| row["device"]["deviceId"] == udid))
        }))
}

pub(super) fn attribute(home: &Path, policy: &Value, dispatches: &[Value], now: f64) -> Result<Pass> {
    let ledger_path = root(home).join("crash-repair-guarded.json");
    let mut ledger = read(&ledger_path).unwrap_or_else(|| json!({"total": 0, "recent": [], "hurt": {}}));
    let mut pass = Pass { attributed: vec![], held: vec![], counts: Value::Null };
    let taken = super::batch::covered(dispatches);
    let directory = root(home).join("incidents");
    let mut hurt = serde_json::Map::new();
    let mut changed = false;
    for entry in std::fs::read_dir(&directory).into_iter().flatten().take(4096) {
        let file = entry?.path();
        if file.extension().is_none_or(|e| e != "json") || file.with_extension("ack").exists() {
            continue;
        }
        let Some(event) = read(&file).filter(|e| e["trigger"] == "springboard_runtime_crash") else { continue };
        let report = &event["report"];
        let (Some(id), Some(udid)) = (event["id"].as_str(), report["simulator_udid"].as_str()) else { continue };
        if !guarded(policy, report) || taken.contains(id) || dispatches.iter().any(|d| d["incident"] == id && d["started"] == true) {
            continue;
        }
        let Some(at) = report["modified_at_ms"].as_f64().or(event["detected_at_ms"].as_f64()) else { continue };
        if now < at + WINDOW_MS + SLACK_MS {
            pass.held.push(id.to_string());
            continue;
        }
        // Judged once: an abort that hurt something stays in the queue and is not rescanned each pass.
        if let Some(reason) = ledger["hurt"].get(id) {
            hurt.insert(id.to_string(), reason.clone());
            continue;
        }
        let damage = app_crash(home, udid, at).map(|crash| json!({"app_crash": crash}))
            .or_else(|| failed_run(home, udid, at).map(|bundle| json!({"failed_test_run": bundle})));
        changed = true;
        if let Some(damage) = damage {
            hurt.insert(id.to_string(), damage);
            continue;
        }
        let record = json!({"incident": id, "at_ms": at, "simulator_udid": udid, "simulator_runtime": report["simulator_runtime"],
            "process_launch_time": report["process_launch_time"], "capture_time": report["capture_time"]});
        let mut ack = record.clone();
        ack["acknowledged_at_ms"] = json!(now);
        ack["attributed_to"] = json!("known_issue_guarded");
        ack["known_issue"] = json!(LABEL);
        ack["reason"] = json!("known SpringBoard boot abort on a runtime the simulator tooling guards; no app crash and no failed test run on the simulator in the 15 minutes after it");
        write_json(&file.with_extension("ack"), &ack)?;
        ledger["total"] = json!(ledger["total"].as_u64().unwrap_or(0) + 1);
        let recent = ledger["recent"].as_array_mut().map(std::mem::take).unwrap_or_default();
        let skip = (recent.len() + 1).saturating_sub(KEEP_RECENT);
        ledger["recent"] = recent.into_iter().skip(skip).chain([record]).collect();
        pass.attributed.push(json!({"incident": id, "attributed_to": "known_issue_guarded"}));
    }
    if changed || ledger["hurt"].as_object().is_some_and(|old| old.len() != hurt.len()) {
        ledger["hurt"] = Value::Object(hurt);
        write_json(&ledger_path, &ledger)?;
    }
    let last_day = ledger["recent"].as_array().into_iter().flatten()
        .filter(|r| r["at_ms"].as_f64().is_some_and(|at| now - at < DAY_MS)).count();
    pass.counts = json!({"total": ledger["total"], "last_24h": last_day, "awaiting": pass.held});
    Ok(pass)
}
