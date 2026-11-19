//! Canary prompt-cache watch (user request, 2026-10-24): find Mars CRM model groups on canary whose
//! cache hit rate fell to zero and have a model session explain each one.
//!
//! 1. Every SAMPLE_SECS one run of `mars-ops-crm cache-watch` in Mars_CRM (`mars-ops` `cache_watch/`, `crm_ops.rs`) reads
//!    the last day of canary billing rows read-only and prints one JSON receipt: every model and agent
//!    group and the `zero` groups whose newest eligible calls read no cache. Image models and requests
//!    below the cache minimum are expected zeros and never flagged (`mars-ops` `cache_watch/config.rs`).
//! 2. A receipt with zero groups opens one `canary_cache_zero` incident and one session on route
//!    `cache-zero`, which proves each zero normal (and records the exemption) or finds and fixes its cause.
//!    No new incident opens while one is unacknowledged; one signature (the sorted zero groups) opens
//!    again only after SAME_SIGNATURE_SECS; at most MAX_DISPATCH_PER_DAY a day.
//!
//! State goes to `cache-watch.json`; each run's receipt and log to `cache-watch/runs/`.
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    os::unix::{fs::{DirBuilderExt, OpenOptionsExt}, process::CommandExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

pub(super) const SAMPLE_SECS: f64 = 900.0;
pub(super) const SAME_SIGNATURE_SECS: f64 = 86_400.0;
pub(super) const MAX_DISPATCH_PER_DAY: usize = 3;
/// A session that failed to start is retried after this long.
const RETRY_SECS: f64 = 600.0;
const RUN_TIMEOUT: Duration = Duration::from_secs(300);
const DAY_MS: f64 = 86_400_000.0;
const KEEP_RUNS: usize = 96;
const KEEP_RUN_FILES: usize = 200;
use super::crm_ops::CRM;

#[derive(Default)]
pub(super) struct CacheWatch {
    loaded: bool,
    last_sample: f64,
    /// One summary per run, newest last: `{at_ms, seconds, exit, ok, error, output, calls, signature, zero}`.
    pub(super) runs: Vec<Value>,
    /// `{incident, signature, run_at_ms, at_ms, started, agent|error}` for every dispatched session.
    pub(super) dispatches: Vec<Value>,
    worker: Option<Receiver<(Value, Value)>>,
}

impl CacheWatch {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded {
            self.loaded = true;
            if let Some(state) = super::read(&root(home).join("cache-watch.json")) {
                self.runs = state["runs"].as_array().cloned().unwrap_or_default();
                self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
            }
        }
        let mut changed = false;
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok((summary, receipt)) => { self.worker = None; changed = true; self.absorb(home, now, summary, &receipt)?; }
                Err(TryRecvError::Disconnected) => { self.worker = None; changed = true; }
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.worker.is_none() && !cfg!(test) && !(self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS) {
            self.last_sample = now;
            changed = true;
            let home = home.to_path_buf();
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::Builder::new().name("watchdog-cache".into()).spawn(move || {
                let _ = sender.send(run(&home));
            })?;
            self.worker = Some(receiver);
        }
        if changed { self.write(home, now) } else { Ok(()) }
    }

    pub(super) fn absorb(&mut self, home: &Path, now: f64, summary: Value, receipt: &Value) -> Result<()> {
        crate::log(&format!("cache watch: {summary}"));
        self.runs.push(summary.clone());
        let excess = self.runs.len().saturating_sub(KEEP_RUNS);
        self.runs.drain(..excess);
        if summary["ok"] == true && summary["zero"].as_u64().unwrap_or(0) > 0 {
            self.escalate(home, now, &summary, receipt)?;
        }
        Ok(())
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.iter().rev().find(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    /// Whether a run with this zero signature may open a session now.
    pub(super) fn may_dispatch(&self, home: &Path, now: f64, signature: &str) -> bool {
        let now_ms = now * 1000.0;
        let started = |d: &&Value| d["started"] == true;
        let at = |d: &Value| d["at_ms"].as_f64().unwrap_or(0.0);
        let failed_recently = self.dispatches.last().is_some_and(|d| d["started"] != true && now_ms - at(d) < RETRY_SECS * 1000.0);
        let today = self.dispatches.iter().filter(started).filter(|d| now_ms - at(*d) < DAY_MS).count();
        let repeated = self.dispatches.iter().filter(started)
            .any(|d| d["signature"] == signature && now_ms - at(d) < SAME_SIGNATURE_SECS * 1000.0);
        !signature.is_empty() && self.open_incident(home).is_none() && !failed_recently && today < MAX_DISPATCH_PER_DAY && !repeated
    }

    fn escalate(&mut self, home: &Path, now: f64, summary: &Value, receipt: &Value) -> Result<()> {
        let signature = summary["signature"].as_str().unwrap_or("").to_string();
        if !self.may_dispatch(home, now, &signature) {
            return Ok(());
        }
        let zero: Vec<Value> = receipt["groups"].as_array().into_iter().flatten().filter(|g| g["status"] == "zero").cloned().collect();
        let previous: Vec<Value> = self.dispatches.iter().rev().filter(|d| d["started"] == true).take(5)
            .map(|d| json!({"incident": d["incident"], "signature": d["signature"],
                "report": root(home).join("analysis").join(d["incident"].as_str().unwrap_or("")).join("report.json")})).collect();
        let id = super::diagnostics::incident(home, &format!("canary_cache_zero:{signature}:{}", summary["at_ms"]), json!({
            "trigger": "canary_cache_zero", "tier": "canary", "signature": signature, "zero": zero,
            "config": receipt["config"], "exemptions": receipt["exemptions"], "receipt": summary["output"],
            "previous": previous, "evidence": evidence(home),
        }))?;
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, "看门狗：canary 缓存零命中", &prompt(home, &id, &signature))
        };
        let mut record = json!({"incident": id, "signature": signature, "run_at_ms": summary["at_ms"], "at_ms": now * 1000.0});
        let status = match &result {
            Ok(agent) => json!({"started": true, "agent": agent}),
            Err(error) => json!({"started": false, "error": format!("{error:#}")}),
        };
        record.as_object_mut().unwrap().extend(status.as_object().cloned().unwrap_or_default());
        self.dispatches.push(record);
        if result.is_ok() {
            super::conversation_retro::notify("看门狗：canary 缓存零命中", &format!("{signature}，已开会话分析原因"));
        }
        Ok(())
    }

    fn write(&self, home: &Path, now: f64) -> Result<()> {
        // A failed canary read is a broken source: monitor-check health shows it until a run succeeds.
        let error = self.runs.last().filter(|r| r["ok"] != true).map(|r| r["error"].clone());
        write_json(&root(home).join("cache-watch.json"), &json!({"at_ms": now * 1000.0, "error": error,
            "running": self.worker.is_some(), "open_incident": self.open_incident(home), "last": self.runs.last(),
            "runs": self.runs, "dispatches": self.dispatches,
            "threshold": {"sample_s": SAMPLE_SECS, "same_signature_s": SAME_SIGNATURE_SECS,
                "max_dispatch_per_day": MAX_DISPATCH_PER_DAY, "timeout_s": RUN_TIMEOUT.as_secs()}}))
    }
}

/// Runs `cache-watch` once and returns its summary and receipt; never panics on a failed or stuck run.
fn run(home: &Path) -> (Value, Value) {
    let started_ms = crate::unix_now() * 1000.0;
    let started = Instant::now();
    let runs = root(home).join("cache-watch").join("runs");
    let output = runs.join(format!("{}.json", started_ms as u64));
    let result = (|| -> Result<(Option<i32>, bool)> {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&runs)?;
        let file = |path: &Path| std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(path);
        let mut child = super::crm_ops::command(home, &["cache-watch"])
            .stdin(Stdio::null()).stdout(file(&output)?).stderr(file(&output.with_extension("log"))?).process_group(0).spawn()?;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok((status.code(), false));
            }
            if started.elapsed() >= RUN_TIMEOUT {
                unsafe { libc::kill(-(child.id() as i32), libc::SIGTERM) };
                let _ = child.wait();
                return Ok((None, true));
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    })();
    prune(&runs);
    let receipt = super::quota_watch::receipt(&std::fs::read_to_string(&output).unwrap_or_default());
    let (exit, timed_out, spawn_error) = match result {
        Ok((exit, timed_out)) => (exit, timed_out, None),
        Err(error) => (None, false, Some(format!("{error:#}"))),
    };
    (summarize(started_ms, started.elapsed().as_secs(), exit, timed_out, spawn_error, &output, &receipt), receipt)
}

pub(super) fn summarize(started_ms: f64, seconds: u64, exit: Option<i32>, timed_out: bool, spawn_error: Option<String>,
    output: &Path, receipt: &Value) -> Value {
    let ok = exit == Some(0) && receipt["ok"] == true;
    let error = spawn_error.or_else(|| timed_out.then(|| "timed out".to_string()))
        .or_else(|| (!ok).then(|| receipt["error"].as_str().unwrap_or("no receipt").to_string()));
    let rates: Vec<Value> = receipt["groups"].as_array().into_iter().flatten()
        .map(|g| json!({"group": format!("{}@{}", g["agent"].as_str().unwrap_or(""), g["model"].as_str().unwrap_or("")),
            "status": g["status"], "hit_rate": g["hitRate"], "calls": g["calls"]})).collect();
    json!({"at_ms": crate::unix_now() * 1000.0, "started_at_ms": started_ms, "seconds": seconds, "exit": exit, "ok": ok,
        "error": error, "output": output, "calls": receipt["calls"], "signature": receipt["signature"],
        "zero": receipt["zero"].as_u64().unwrap_or(0), "groups": rates})
}

/// Keeps the newest KEEP_RUN_FILES receipts and logs; retention.rs expires older days as well.
fn prune(runs: &Path) {
    let Ok(entries) = std::fs::read_dir(runs) else { return };
    let mut files: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    files.sort();
    let excess = files.len().saturating_sub(KEEP_RUN_FILES * 2);
    for path in &files[..excess] {
        let _ = std::fs::remove_file(path);
    }
}

fn evidence(home: &Path) -> Value {
    let state = root(home);
    json!({"cache_watch": state.join("cache-watch.json"), "runs": state.join("cache-watch/runs"),
        "probe": PathBuf::from(CRM).join("rust/crates/mars-ops/src/cache_watch"),
        "cache_skill": "/path/to/workspace/.claude/skills/cache-optimization/SKILL.md",
        "watchdog_log": home.join("Library/Logs/cpu-watchdog.log")})
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
pub(super) fn prompt(home: &Path, id: &str, signature: &str) -> String {
    format!("cpu-watchdog incident {id}: on canary.example.com these Mars CRM model groups read no prompt cache in their newest eligible calls: {signature}. {header} \
        Incident: {s}/incidents/{id}.json (zero groups with sample calls, thresholds, exemptions, previous cache sessions); state root {s}. \
        For each group decide with evidence whether the zero is expected (then record its exemption) or a defect (then find the root cause and fix it under mars-crm), \
        publish the report page and give its link, write {s}/analysis/{id}/report.md in Chinese and report.json, then run cpu-watchdog monitor-ack {id}.",
        header = crate::dispatch::route_header("cache-zero"), s = root(home).display())
}

#[cfg(test)]
#[path = "cache_watch_tests.rs"]
mod tests;
