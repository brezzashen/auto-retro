//! Attribute an XCTest host crash to the test run that produced it (founder, 2026-11-05, incident
//! 815379692640138682000). A crash whose stack holds a `<Class>Tests.test…` frame and that the run's
//! own xcresult already records as `Crash: <process> …` on the same simulator is that run's test
//! failure: the session that ran it sees it in its result, so no repair session opens.
use super::root;
use crate::monitor::write_json;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// How long a test-frame crash waits for its run's xcresult before it queues as an ordinary crash.
const GRACE_MS: f64 = 15.0 * 60_000.0;
/// Crash report mtime may trail the bundle's last write by the report writer's delay.
const SLACK_MS: f64 = 120_000.0;
const MAX_BUNDLES: usize = 400;

/// Outcome of one pass: crashes acknowledged as test failures, and crashes still waiting for a result.
pub(super) struct Pass {
    pub attributed: Vec<Value>,
    pub held: Vec<String>,
}

/// First `<Class>Tests.test<Name>` in the stack, e.g. `TranscriptRowsTests.testLinkedProse…`.
pub(super) fn test_frame(report: &Value) -> Option<String> {
    report["frames"].as_array()?.iter().filter_map(|f| f["symbol"].as_str()).find_map(|symbol| {
        let at = symbol.find("Tests.test")?;
        let start = symbol[..at].rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
        let end = symbol[at..].find('(').map_or(symbol.len(), |i| at + i);
        (start < at).then(|| symbol[start..end].to_string())
    })
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn millis(time: std::io::Result<std::time::SystemTime>) -> Option<f64> {
    Some(time.ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64() * 1000.0)
}

/// Test result bundles under every DerivedData folder whose span (bundle birth to last write) holds `at`.
fn bundles(home: &Path, at: f64) -> Vec<PathBuf> {
    let Ok(projects) = std::fs::read_dir(home.join("Library/Developer/Xcode/DerivedData")) else { return vec![] };
    projects.flatten()
        .filter_map(|p| std::fs::read_dir(p.path().join("Logs/Test")).ok())
        .flat_map(|d| d.flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "xcresult"))
        .take(MAX_BUNDLES)
        .filter(|p| std::fs::metadata(p).ok().is_some_and(|m| {
            millis(m.created()).is_some_and(|s| s <= at) && millis(m.modified()).is_some_and(|e| at <= e + SLACK_MS)
        }))
        .collect()
}

/// `xcresulttool get test-results tests` JSON; tests read `tests.json` inside the fixture bundle.
fn results(bundle: &Path) -> Option<Value> {
    if cfg!(test) {
        return read(&bundle.join("tests.json"));
    }
    let out = std::process::Command::new("/usr/bin/xcrun")
        .args(["xcresulttool", "get", "test-results", "tests", "--compact", "--path"])
        .arg(bundle).output().ok()?;
    out.status.success().then(|| serde_json::from_slice(&out.stdout).ok()).flatten()
}

/// The test case whose failure records `Crash: <process> …`, when the bundle ran on `udid`.
fn crashed_case(tests: &Value, udid: &str, process: &str) -> Option<String> {
    let on_device = tests["devices"].as_array()?.iter().any(|d| d["deviceId"] == udid);
    let prefix = format!("Crash: {process} ");
    fn walk(node: &Value, prefix: &str, case: Option<&str>) -> Option<String> {
        let case = if node["nodeType"] == "Test Case" { node["nodeIdentifier"].as_str().or(case) } else { case };
        if node["nodeType"] == "Failure Message" && node["name"].as_str().is_some_and(|n| n.starts_with(prefix)) {
            return Some(case.unwrap_or_default().to_string());
        }
        node["children"].as_array()?.iter().find_map(|c| walk(c, prefix, case))
    }
    on_device.then(|| tests["testNodes"].as_array()?.iter().find_map(|n| walk(n, &prefix, None))).flatten()
}

/// The agent session holding the simulator lease since before the crash, if the lease still stands.
fn lease_owner(home: &Path, udid: &str, at: f64) -> Option<String> {
    let lease = read(&home.join(".local/state/sim-lease/leases").join(format!("{udid}.json")))?;
    let since = lease["since"].as_f64()? * 1000.0;
    (since <= at).then(|| lease["owner"].as_str().map(str::to_string)).flatten()
}

pub(super) fn attribute(home: &Path, dispatches: &[Value], now: f64) -> Result<Pass> {
    let mut pass = Pass { attributed: vec![], held: vec![] };
    let taken = super::batch::covered(dispatches);
    let owners = root(home).join("crash-repair-test-runs.json");
    let seen = read(&owners).unwrap_or_else(|| json!({}));
    let mut next = json!({});
    let directory = root(home).join("incidents");
    if !directory.try_exists()? {
        return Ok(pass);
    }
    for entry in std::fs::read_dir(&directory)?.take(4096) {
        let file = entry?.path();
        if file.extension().is_none_or(|e| e != "json") || file.with_extension("ack").exists() {
            continue;
        }
        let Some(event) = read(&file).filter(|e| e["trigger"] == "xcode_runtime_crash") else { continue };
        let report = &event["report"];
        let (Some(id), Some(test), Some(udid), Some(process)) = (event["id"].as_str(), test_frame(report),
            report["simulator_udid"].as_str(), report["process"].as_str()) else { continue };
        // Only inside the grace window: later the crash is an ordinary queue entry and is not rescanned.
        if event["detected_at_ms"].as_f64().is_none_or(|d| now - d >= GRACE_MS) || taken.contains(id)
            || dispatches.iter().any(|d| d["incident"] == id && d["started"] == true) {
            continue;
        }
        let Some(at) = report["modified_at_ms"].as_f64().or(event["detected_at_ms"].as_f64()) else { continue };
        let found = bundles(home, at).into_iter()
            .find_map(|b| results(&b).and_then(|t| crashed_case(&t, udid, process)).map(|case| (b, case)));
        // The run usually still holds its simulator lease while its result is being written; keep the owner.
        let session = lease_owner(home, udid, at).or_else(|| seen[id].as_str().map(str::to_string));
        if let Some(owner) = &session { next[id] = json!(owner); }
        let Some((bundle, case)) = found else {
            pass.held.push(id.to_string());
            continue;
        };
        write_json(&file.with_extension("ack"), &json!({"acknowledged_at_ms": now, "attributed_to": "test_run",
            "test": test, "test_case": case, "xcresult": bundle, "session": session,
            "reason": "XCTest host crash that its own test run already records as a crash on the same simulator"}))?;
        pass.attributed.push(json!({"incident": id, "attributed_to": "test_run", "test": test, "session": session}));
    }
    // Only crashes still waiting keep an entry, so the file stays as small as the grace window.
    let waiting: serde_json::Map<String, Value> = next.as_object().unwrap().iter()
        .filter(|(id, _)| pass.held.contains(id)).map(|(k, v)| (k.clone(), v.clone())).collect();
    if !waiting.is_empty() || seen.as_object().is_some_and(|m| !m.is_empty()) {
        write_json(&owners, &Value::Object(waiting))?;
    }
    Ok(pass)
}
