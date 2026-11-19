//! Bounded Apple crash-report indexing; only reports arriving after activation alert.
use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, io::Read, path::Path, time::UNIX_EPOCH};

#[derive(Default, Deserialize, Serialize)]
struct Cursor {
    #[serde(default)]
    schema: u32,
    started_at_ms: f64,
    files: BTreeMap<String, u128>,
    reports: Vec<Value>,
}

/// Writes the incident once and returns its stable id.
pub(crate) fn incident(home: &Path, key: &str, event: Value) -> Result<String> {
    let hash = key.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    });
    let id = format!("8{hash}");
    let path = root(home).join("incidents").join(format!("{id}.json"));
    if !path.exists() {
        let mut event = event;
        event["id"] = json!(id);
        event["detected_at_ms"] = json!(crate::unix_now() * 1000.0);
        write_json(&path, &event)?;
    }
    Ok(id)
}

/// Simulator SpringBoard aborting in `-[RBSAssertion dealloc]` while PaperBoardUI snapshots the
/// wallpaper poster, 15 to 30 seconds after boot: an Apple runtime fault seen on iOS 27.0
/// (dotnet/macios#26227). Classification does not suppress the opt-in SpringBoard queue.
const POSTER_SNAPSHOT_ABORT: &str = "springboard_poster_snapshot_rbsassertion";

fn known_issue(body: &Value) -> Option<&'static str> {
    if body["procName"] != "SpringBoard" || body["parentProc"] != "launchd_sim"
        || body["exception"]["type"] != "EXC_CRASH" || body["exception"]["signal"] != "SIGABRT" {
        return None;
    }
    let symbols: Vec<&str> = body["lastExceptionBacktrace"].as_array().into_iter().flatten()
        .filter_map(|frame| frame["symbol"].as_str()).collect();
    let poster = symbols.iter().any(|symbol| symbol.contains("pui_executeSnapshotForDescriptor"));
    (poster && symbols.contains(&"-[RBSAssertion dealloc]")).then_some(POSTER_SNAPSHOT_ABORT)
}

/// The runtime identifier CoreSimulator or Xcode Previews records for the device, e.g. `...SimRuntime.iOS-27-1`.
fn simulator_runtime(home: &Path, udid: &str) -> Option<String> {
    for root in [
        "Library/Developer/CoreSimulator/Devices",
        "Library/Developer/Xcode/UserData/Previews/Simulator Devices",
    ] {
        let plist = home.join(root).join(udid).join("device.plist");
        if !plist.is_file() {
            continue;
        }
        let output = std::process::Command::new("/usr/bin/plutil")
            .args(["-extract", "runtime", "raw", "-o", "-"]).arg(plist).output().ok()?;
        let runtime = String::from_utf8(output.stdout).ok()?.trim().to_owned();
        if output.status.success() && !runtime.is_empty() {
            return Some(runtime);
        }
    }
    None
}

fn summary(home: &Path, path: &Path) -> Result<Value> {
    let file = std::fs::File::open(path)?;
    anyhow::ensure!(file.metadata()?.len() <= 4 * 1024 * 1024, "Crash report exceeds 4 MiB");
    let mut text = String::new();
    file.take(4 * 1024 * 1024).read_to_string(&mut text)?;
    let mut values = serde_json::Deserializer::from_str(&text).into_iter::<Value>();
    let header = values.next().context("Empty crash report")??;
    let body = values.next().context("Incomplete crash report")??;
    let process = body["procName"].as_str().or(header["app_name"].as_str());
    let simulated = body["parentProc"] == "launchd_sim";
    anyhow::ensure!(
        matches!(process, Some("MarsCRM" | "Xcode")) || (process == Some("SpringBoard") && simulated),
        "Unexpected crash process"
    );
    let frames: Vec<_> = body["threads"].as_array().into_iter().flatten()
        .find(|thread| thread["triggered"] == true)
        .and_then(|thread| thread["frames"].as_array())
        .into_iter().flatten().take(12).map(|frame| {
            json!({"symbol":frame["symbol"],"file":frame["sourceFile"],"line":frame["sourceLine"]})
        }).collect();
    // A simulator system process runs from the runtime volume, so only its coalition names the device.
    let proc_path = body["procPath"].as_str().unwrap_or_default();
    let device = proc_path.split("/Devices/").nth(1)
        .or_else(|| proc_path.split("/Simulator Devices/").nth(1))
        .and_then(|path| path.split('/').next())
        .or_else(|| body["coalitionName"].as_str()?.strip_prefix("com.apple.CoreSimulator.SimDevice."))
        .filter(|id| id.len() == 36);
    let preview = proc_path.contains("/Previews/Simulator Devices/")
        || body["usedImages"].as_array().into_iter().flatten()
            .any(|image| image["name"].as_str() == Some("__preview.dylib"));
    let image = body["usedImages"].as_array().into_iter().flatten().find(|image| {
        image["name"].as_str().is_some_and(|name| name == "MarsCRM.debug.dylib" || Some(name) == process)
    });
    Ok(json!({"path":path,"incident":body["incident"],"process":process,
        "simulator_udid":device,"simulator_runtime":device.and_then(|id| simulator_runtime(home, id)),
        "preview_injection":preview,
        "known_issue":known_issue(&body),"image_uuid":image.map(|image| &image["uuid"]),
        "app_version":header["app_version"],"build_version":header["build_version"],
        "capture_time":body["captureTime"],"process_launch_time":body["procLaunch"],"pid":body["pid"],
        "exception_frames":body["lastExceptionBacktrace"].as_array().map(|frames| frames.iter().take(32)
            .map(|frame| json!({"symbol":frame["symbol"],"image_index":frame["imageIndex"]})).collect::<Vec<_>>()),
        "exception_type":body["exception"]["type"],"signal":body["exception"]["signal"],
        "exception_subtype":body["exception"]["subtype"],"frames":frames}))
}

pub fn collect(home: &Path) -> Result<()> {
    let now = crate::unix_now() * 1000.0;
    let checkpoint = root(home).join("xcode-errors-cursor.json");
    let mut cursor: Cursor = match std::fs::read(&checkpoint) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("Invalid Xcode error cursor")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Cursor { started_at_ms: now, ..Cursor::default() },
        Err(e) => return Err(e.into()),
    };
    if cursor.schema < 2 {
        cursor.files.clear();
        cursor.schema = 2;
    }
    let directory = home.join("Library/Logs/DiagnosticReports");
    let mut files = Vec::new();
    let mut entries = std::fs::read_dir(&directory)?;
    for entry in entries.by_ref().take(4096) {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let tracked = ["MarsCRM-", "Xcode-", "SpringBoard-"].iter().any(|prefix| name.starts_with(prefix));
        if !tracked || !name.ends_with(".ips") {
            continue;
        }
        let metadata = entry.metadata()?;
        if metadata.is_file() {
            let modified = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_nanos();
            if cursor.files.get(&name) != Some(&modified) {
                files.push((modified, name, entry.path()));
            }
        }
    }
    let mut errors = Vec::new();
    if entries.next().is_some() { errors.push(json!({"error":"Diagnostic directory exceeds 4096 entries"})); }
    files.sort_by_key(|(modified, _, _)| std::cmp::Reverse(*modified));
    let pending = files.len().saturating_sub(8);
    for (modified, name, path) in files.into_iter().take(8) {
        match summary(home, &path) {
            Ok(mut report) => {
                let historical = modified as f64 / 1_000_000.0 <= cursor.started_at_ms;
                report["historical_at_activation"] = json!(historical);
                report["modified_at_ms"] = json!(modified as f64 / 1_000_000.0);
                let record = root(home).join("xcode-error-records").join(format!("{name}.json"));
                write_json(&record, &report)?;
                if !historical && report["known_issue"].is_null() {
                    incident(home, &format!("runtime:{name}"), json!({"trigger":"xcode_runtime_crash", "report":report}))?;
                }
                cursor.reports.retain(|old| old["path"] != report["path"]);
                cursor.reports.insert(0, report);
                cursor.files.insert(name, modified);
            }
            Err(e) => errors.push(json!({"path":path,"error":format!("{e:#}")})),
        }
    }
    cursor.reports.sort_by(|a,b| b["capture_time"].as_str().cmp(&a["capture_time"].as_str()));
    let springboard_watch = crate::crash_repair::springboard::observe(home, &cursor.reports)?;
    cursor.reports.truncate(32);
    write_json(&checkpoint, &cursor)?;
    write_json(&root(home).join("xcode-errors.json"), &json!({"at_ms":now,"error":null,
        "started_at_ms":cursor.started_at_ms,"probe_errors":errors,"pending_reports":pending,
        "reports":cursor.reports,"springboard_watch":springboard_watch,"semantics":"MarsCRM, Xcode and simulator SpringBoard .ips only; SpringBoard repair requires explicit opt-in, including known runtime faults; newest 8 changed reports per tick, 4 MiB each; first activation baselines historical reports unless explicitly included; original reports remain private; no process arguments or environment"}))
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
