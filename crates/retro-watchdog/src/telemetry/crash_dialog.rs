//! Suppress and auto-dismiss macOS CrashReporter / Problem Reporter modal popups ("<App> quit
//! unexpectedly.") while keeping `.ips` crash log generation intact in `~/Library/Logs/DiagnosticReports`.
use crate::{
    monitor::{root, write_json},
    procs::Proc,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::{path::Path, process::Command};

const CHECK_INTERVAL_SECS: f64 = 60.0;

#[derive(Default)]
pub(super) struct CrashDialog {
    last_defaults_check: f64,
    dismissed_total: u64,
    last_dismissed: Option<Value>,
}

pub(crate) fn problem_reporter_pids(processes: &[Proc]) -> Vec<(i32, i64)> {
    processes
        .iter()
        .filter(|p| p.name == "Problem Reporter")
        .map(|p| (p.pid, p.start))
        .collect()
}

fn ensure_headless_crash_reporter() -> (String, bool) {
    let current = Command::new("/usr/bin/defaults")
        .args(["read", "com.apple.CrashReporter", "DialogType"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_default();
    if current.eq_ignore_ascii_case("none") {
        return (current, false);
    }
    let _ = Command::new("/usr/bin/defaults")
        .args(["write", "com.apple.CrashReporter", "DialogType", "none"])
        .status();
    let _ = Command::new("/usr/bin/defaults")
        .args(["write", "com.apple.CrashReporter", "UseUNC", "-int", "0"])
        .status();
    ("none".to_owned(), true)
}

impl CrashDialog {
    pub(super) fn observe(&mut self, home: &Path, now: f64, processes: &[Proc]) -> Result<()> {
        let reporters = problem_reporter_pids(processes);
        let need_check = !reporters.is_empty()
            || self.last_defaults_check == 0.0
            || now - self.last_defaults_check >= CHECK_INTERVAL_SECS;
        if !need_check {
            return Ok(());
        }
        self.last_defaults_check = now;
        let (dialog_type, updated_defaults) = if cfg!(test) {
            ("none".to_owned(), false)
        } else {
            ensure_headless_crash_reporter()
        };
        let mut dismissed_now = Vec::new();
        for (pid, start) in &reporters {
            let killed = if cfg!(test) {
                true
            } else {
                Command::new("/bin/kill")
                    .args(["-TERM", &pid.to_string()])
                    .status()
                    .is_ok_and(|s| s.success())
            };
            if killed {
                self.dismissed_total += 1;
                let event = json!({"at_ms": now * 1000.0, "pid": pid, "process_start": start});
                self.last_dismissed = Some(event.clone());
                dismissed_now.push(event);
            }
        }
        write_json(
            &root(home).join("crash-dialog-heal.json"),
            &json!({
                "at_ms": now * 1000.0,
                "error": null,
                "dialog_type": dialog_type,
                "defaults_repaired": updated_defaults,
                "active_problem_reporters": reporters.len(),
                "dismissed_now": dismissed_now,
                "dismissed_total": self.dismissed_total,
                "last_dismissed": self.last_dismissed,
                "semantics": "Enforces com.apple.CrashReporter DialogType=none + UseUNC=0 and terminates Problem Reporter GUI popups while preserving DiagnosticReports .ips files"
            }),
        )
    }
}
