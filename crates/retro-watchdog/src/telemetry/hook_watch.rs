//! Daily hook defect watch (founder request, 2026-11-10: the watchdog must find and fix hook
//! prompts like the skill-hub reconcile reminder that named `references/watch.md` without its skill).
//!
//! 1. Every SAMPLE_SECS the schedule is read in Singapore time. The run is due from DAILY_AT_MIN
//!    until one succeeds that day; a failed run is retried after RETRY_SECS, at most MAX_ATTEMPTS.
//! 2. A run is `hook-audit defects --hours WINDOW_HOURS --json` (crate ~/automation/crates/hook-audit):
//!    Stop prompts naming paths that do not resolve or files without a verified date, and hooks that
//!    time out or error repeatedly. Its last stdout line is the receipt.
//! 3. A defect is fresh unless an acknowledged hook-retro already listed its signature and every
//!    occurrence predates that acknowledgement (`resolved`). Fresh defects open one
//!    `hook_defects_found` incident and one session on route `hook-retro`, unless the previous one is
//!    still unacknowledged (state `blocked`).
//!
//! State: `hook-watch.json`; each run's stdout and log in `hook-watch/runs/`.
pub(super) mod validate;

use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::{
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

pub(super) const SAMPLE_SECS: f64 = 300.0;
/// 11:00 Singapore time, after the morning retros.
pub(super) const DAILY_AT_MIN: f64 = 11.0 * 60.0;
const RETRY_SECS: f64 = 3600.0;
const MAX_ATTEMPTS: usize = 3;
/// Overlaps the previous day's run; `resolved` keeps an overlap from reopening a fixed defect.
const WINDOW_HOURS: f64 = 26.0;
const RUN_TIMEOUT: Duration = Duration::from_secs(600);
const KEEP_RUNS: usize = 60;
const BIN: &str = "/home/user/.local/bin/hook-audit";

#[derive(Default)]
pub(super) struct HookWatch {
    loaded: bool,
    last_sample: f64,
    /// One record per run, newest last: `{day, at_ms, ok, error, receipt}`.
    pub(super) runs: Vec<Value>,
    pub(super) dispatches: Vec<Value>,
    /// Signature → acknowledgement time (ms) of the hook-retro that listed it.
    pub(super) resolved: Map<String, Value>,
    blocked: Option<Value>,
    worker: Option<Receiver<Value>>,
}

impl HookWatch {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded {
            self.loaded = true;
            if let Some(state) = super::read(&root(home).join("hook-watch.json")) {
                self.runs = state["runs"].as_array().cloned().unwrap_or_default();
                self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
                self.resolved = state["resolved"].as_object().cloned().unwrap_or_default();
            }
        }
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(run) => { self.worker = None; self.absorb(home, run)?; }
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS {
            return Ok(());
        }
        self.last_sample = now;
        self.settle_acks(home);
        if self.worker.is_none() && !cfg!(test) && self.due(now * 1000.0).is_some() {
            let (sender, receiver) = mpsc::sync_channel(1);
            let home_path = home.to_path_buf();
            std::thread::Builder::new().name("watchdog-hook-watch".into()).spawn(move || {
                let _ = sender.send(run(&home_path));
            })?;
            self.worker = Some(receiver);
        }
        self.write(home, now)
    }

    /// The Singapore day to run now, if any.
    pub(super) fn due(&self, now_ms: f64) -> Option<i64> {
        let (day, minutes) = super::quota_watch::singapore(now_ms);
        if minutes < DAILY_AT_MIN {
            return None;
        }
        let tries: Vec<&Value> = self.runs.iter().filter(|r| r["day"].as_i64() == Some(day)).collect();
        let last = tries.iter().filter_map(|r| r["at_ms"].as_f64()).fold(0.0, f64::max);
        (!tries.iter().any(|r| r["ok"] == true) && tries.len() < MAX_ATTEMPTS && now_ms - last >= RETRY_SECS * 1000.0).then_some(day)
    }

    /// Records the acknowledgement time of every acknowledged dispatch against its signatures.
    pub(super) fn settle_acks(&mut self, home: &Path) {
        for d in self.dispatches.iter_mut().filter(|d| d["started"] == true && d["acked_ms"].is_null()) {
            let ack = root(home).join("incidents").join(format!("{}.ack", d["incident"].as_str().unwrap_or("")));
            let Some(ms) = std::fs::metadata(&ack).and_then(|m| m.modified()).ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as f64) else { continue };
            d["acked_ms"] = json!(ms);
            for sig in d["signatures"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                self.resolved.insert(sig.to_string(), json!(ms));
            }
        }
    }

    /// Defects of the receipt that no acknowledged hook-retro already covers.
    pub(super) fn fresh(&self, receipt: &Value) -> Vec<Value> {
        receipt["defects"].as_array().into_iter().flatten().filter(|d| {
            let resolved = d["signature"].as_str().and_then(|s| self.resolved.get(s)).and_then(Value::as_f64).unwrap_or(0.0);
            d["last_seen_ms"].as_f64().unwrap_or(f64::MAX) > resolved
        }).cloned().collect()
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.iter().rev().find(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    pub(super) fn absorb(&mut self, home: &Path, run: Value) -> Result<()> {
        let receipt = run["receipt"].clone();
        self.runs.push(run.clone());
        let excess = self.runs.len().saturating_sub(KEEP_RUNS);
        self.runs.drain(..excess);
        if run["ok"] != true {
            crate::log(&format!("hook watch: run failed: {}", run["error"]));
            return Ok(());
        }
        let fresh = self.fresh(&receipt);
        crate::log(&format!("hook watch: {} defects, {} fresh", receipt["defects"].as_array().map_or(0, Vec::len), fresh.len()));
        if fresh.is_empty() {
            self.blocked = None;
            return Ok(());
        }
        if let Some(id) = self.open_incident(home) {
            self.blocked = Some(json!({"day": run["day"], "by": id, "fresh": fresh.len()}));
            return Ok(());
        }
        self.blocked = None;
        let signatures: Vec<Value> = fresh.iter().map(|d| d["signature"].clone()).collect();
        let id = super::diagnostics::incident(home, &format!("hook_defects_found:{}", run["day"]), json!({
            "trigger": "hook_defects_found", "day": run["day"], "timezone": "Asia/Singapore", "window_hours": WINDOW_HOURS,
            "stop_blocks": receipt["stop_blocks"], "defects": fresh, "output": run["output"],
        }))?;
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, "看门狗：钩子缺陷复盘", &prompt(home, &id, fresh.len()))
        };
        self.dispatches.push(match &result {
            Ok(agent) => json!({"incident": id, "day": run["day"], "at_ms": run["at_ms"], "started": true, "agent": agent, "signatures": signatures}),
            Err(error) => json!({"incident": id, "day": run["day"], "at_ms": run["at_ms"], "started": false, "error": format!("{error:#}")}),
        });
        let opened = if result.is_ok() { "已开钩子复盘会话" } else { "会话未能打开，见 hook-watch.json" };
        super::conversation_retro::notify("看门狗：钩子缺陷", &format!("{} 个新缺陷。{opened}。", fresh.len()));
        Ok(())
    }

    fn write(&self, home: &Path, now: f64) -> Result<()> {
        write_json(&root(home).join("hook-watch.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "running": self.worker.is_some(), "open_incident": self.open_incident(home), "blocked": self.blocked,
            "runs": self.runs, "dispatches": self.dispatches, "resolved": self.resolved,
            "schedule": {"timezone": "Asia/Singapore", "at_min": DAILY_AT_MIN, "window_hours": WINDOW_HOURS,
                "retry_s": RETRY_SECS, "max_attempts": MAX_ATTEMPTS, "sample_s": SAMPLE_SECS}}))
    }
}

/// Runs `hook-audit defects` once and returns its record; never panics on a failed or stuck run.
fn run(home: &Path) -> Value {
    let started_ms = crate::unix_now() * 1000.0;
    let started = Instant::now();
    let runs = root(home).join("hook-watch").join("runs");
    let output = runs.join(format!("{}.json", started_ms as u64));
    let result = (|| -> Result<Option<i32>> {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&runs)?;
        let file = |path: &Path| std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(path);
        let mut child = Command::new(BIN).args(["defects", "--hours", &WINDOW_HOURS.to_string(), "--json"])
            .env("PATH", super::MARS_SCRIPT_PATH).stdin(Stdio::null())
            .stdout(file(&output)?).stderr(file(&output.with_extension("log"))?).spawn()?;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status.code());
            }
            if started.elapsed() >= RUN_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("timed out");
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    })();
    let receipt = super::quota_watch::receipt(&std::fs::read_to_string(&output).unwrap_or_default());
    let (exit, error) = match result {
        Ok(code) => (code, None),
        Err(e) => (None, Some(format!("{e:#}"))),
    };
    let ok = exit == Some(0) && receipt["ok"] == true;
    let error = error.or_else(|| (!ok).then(|| "no receipt".to_string()));
    json!({"day": super::quota_watch::singapore(started_ms).0, "at_ms": crate::unix_now() * 1000.0,
        "seconds": started.elapsed().as_secs(), "exit": exit, "ok": ok, "error": error, "output": output, "receipt": receipt})
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
pub(super) fn prompt(home: &Path, id: &str, count: usize) -> String {
    format!("cpu-watchdog incident {id}: the daily hook audit found {count} hook defects in real Claude sessions (Stop prompts naming paths that do not resolve or files without a verified date to bump, or hooks that time out or error repeatedly). {header} \
        Incident: {s}/incidents/{id}.json (defects lists each signature, kind, hook, token, count, detail and an example transcript locator). \
        For each defect read its example, find the hook's source (hook-audit skill: where hooks live; skill-hub, bg-task-guard and the other ~/automation crates; Mars_agent .claude/hooks), fix it under its owning crate or skill with a test, reinstall through cpu-build-gate and replay the example payload; leave files a live session is editing to it (session-status). \
        Write {s}/analysis/{id}/report.md in Chinese and report.json with report_url, defects[] {{signature, verdict fixed|proposed|kept|owned_elsewhere, reason, commit when fixed, owner when owned_elsewhere}} and, when anything was fixed, after {{check: \"pass\" from hook-audit check, replay: what the replayed hook printed}}, publish the encrypted report page, then run cpu-watchdog monitor-ack {id}.",
        header = crate::dispatch::route_header("hook-retro"), s = root(home).display())
}

#[cfg(test)]
#[path = "hook_watch/tests.rs"]
mod tests;
