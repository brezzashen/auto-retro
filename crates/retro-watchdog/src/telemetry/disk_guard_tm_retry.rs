//! Snapshot retry after an S0 run that skipped Time Machine snapshots (founder decision, report
//! disk-low-81269512195071067984 item 2): on 2026-10-28 10:04 `recover-disk` found a backup running,
//! skipped `s0:tm-snapshots` ("snapshot cleanup requires an idle Time Machine") and a disk session
//! opened, although the snapshots it skipped were what had freed 65-131 GiB in the runs before.
//! The S0 worker thread now waits for Time Machine to go idle, prunes snapshots once and measures
//! again, so the session opens only when the disk is still short after that.
use serde_json::{json, Value};
use std::{process::Command, time::{Duration, Instant}};

/// A backup on this host took 35 min on 2026-10-29; two hours covers a slow one.
const IDLE_WAIT: Duration = Duration::from_secs(2 * 3600);
const POLL: Duration = Duration::from_secs(30);
/// APFS releases a deleted snapshot's blocks asynchronously.
const SETTLE: Duration = Duration::from_secs(60);
const TM_SCHEDULER: &str = "/home/user/.local/bin/tm-scheduler";

/// True when S0 did not recover and its snapshot step was refused because a backup was running.
pub(super) fn skipped_for_backup(report: &Value) -> bool {
    report["recovered"] != true
        && report["steps"].as_array().is_some_and(|steps| steps.iter().any(|step| {
            step["step"] == "s0:tm-snapshots"
                && step["details"].as_array().is_some_and(|d| d.iter().any(|x| x.as_str().is_some_and(|s| s.contains("idle Time Machine"))))
        }))
}

/// Returns the volumes to record as `after` and the retry record (`null` when no retry was due).
pub(super) fn run(report: &Value, after: Value) -> (Value, Value) {
    if cfg!(test) || !skipped_for_backup(report) || super::lowest(&after).is_some_and(|f| f >= super::MIN_FREE_GIB) {
        return (after, Value::Null);
    }
    let started = Instant::now();
    while running() && started.elapsed() < IDLE_WAIT {
        std::thread::sleep(POLL);
    }
    let waited_s = started.elapsed().as_secs();
    if running() {
        return (after, json!({"waited_s": waited_s, "idle": false}));
    }
    let output = Command::new(TM_SCHEDULER).arg("--prune-snapshots").output();
    std::thread::sleep(SETTLE);
    let record = match output {
        Ok(out) => json!({"waited_s": waited_s, "idle": true, "exit": out.status.code(),
            "output": tail(&String::from_utf8_lossy(&out.stdout)), "stderr": tail(&String::from_utf8_lossy(&out.stderr))}),
        Err(error) => json!({"waited_s": waited_s, "idle": true, "error": error.to_string()}),
    };
    (super::volumes(), record)
}

/// `tmutil status` reports `Running = 1;` while any backup runs; an unreadable status counts as running.
fn running() -> bool {
    Command::new("/usr/bin/tmutil").arg("status").output()
        .map(|out| !String::from_utf8_lossy(&out.stdout).contains("Running = 0;")).unwrap_or(true)
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(5)..].join("\n")
}
