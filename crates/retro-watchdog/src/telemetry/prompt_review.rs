//! Weekly prompt review (founder request, 2026-11-09: a watchdog that reviews and optimizes the
//! system prompt and the tool and skill prompts once a week, removing redundancy and repetition):
//! the Mars CRM main agent's system prompt, tool descriptions and schemas and bundled skill prompts
//! are measured every week and deduplicated when they grow or copy a rule.
//!
//! 1. Every SAMPLE_SECS the schedule is read in Singapore time (UTC+8). The run is due from Monday
//!    WEEKLY_AT_MIN until one succeeds that week; a failed run is retried after RETRY_SECS, at most
//!    MAX_ATTEMPTS a week. One run at a time, on its own thread, killed after RUN_TIMEOUT.
//! 2. A run is `prompt-review run` (crate ~/automation/crates/prompt-review): capture of origin/mastra
//!    in its own worktree, analyzer comparison with the previous week's capture, the semantic duplicate
//!    pass, the encrypted Chinese report page; its stdout ends with one JSON receipt.
//! 3. A receipt with `escalate` (a threshold in prompt-review `metrics.rs` crossed) opens one
//!    `prompt_review_due` incident and one session on route `prompt-review`, unless the previous one is
//!    still unacknowledged (state `blocked`; the page still publishes). Every run notifies its page.
//!
//! State: `prompt-review.json`; each run's stdout and log in `prompt-review/runs/`.
pub(super) mod validate;

use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    os::unix::{fs::{DirBuilderExt, OpenOptionsExt}, process::CommandExt},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

pub(super) const SAMPLE_SECS: f64 = 300.0;
/// Monday 10:30 Singapore time, after the 09:30 weekly quota report.
pub(super) const WEEKLY_AT_MIN: f64 = 10.0 * 60.0 + 30.0;
pub(super) const RETRY_SECS: f64 = 3600.0;
pub(super) const MAX_ATTEMPTS: usize = 3;
const RUN_TIMEOUT: Duration = Duration::from_secs(2400);
const KEEP_RUNS: usize = 60;
const BIN: &str = "/home/user/.local/bin/prompt-review";

/// Singapore day number of the Monday that starts the week holding `day`.
pub(super) fn week_of(day: i64) -> i64 {
    day - (day + 3).rem_euclid(7)
}

#[derive(Default)]
pub(super) struct PromptReview {
    loaded: bool,
    last_sample: f64,
    /// One record per run, newest last: `{week, at_ms, seconds, exit, ok, receipt, error}`.
    pub(super) runs: Vec<Value>,
    dispatches: Vec<Value>,
    blocked: Option<Value>,
    worker: Option<Receiver<Value>>,
}

impl PromptReview {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded {
            self.loaded = true;
            if let Some(state) = super::read(&root(home).join("prompt-review.json")) {
                self.runs = state["runs"].as_array().cloned().unwrap_or_default();
                self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
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
        if self.worker.is_none() && !cfg!(test) && self.due(now * 1000.0).is_some() {
            let home_path = home.to_path_buf();
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::Builder::new().name("watchdog-prompt-review".into()).spawn(move || {
                let _ = sender.send(run(&home_path));
            })?;
            self.worker = Some(receiver);
        }
        self.write(home, now)
    }

    /// The week to run now, if any.
    pub(super) fn due(&self, now_ms: f64) -> Option<i64> {
        let (day, minutes) = super::quota_watch::singapore(now_ms);
        let week = week_of(day);
        if day == week && minutes < WEEKLY_AT_MIN {
            return None;
        }
        let tries: Vec<&Value> = self.runs.iter().filter(|r| r["week"].as_i64() == Some(week)).collect();
        let last = tries.iter().filter_map(|r| r["at_ms"].as_f64()).fold(0.0, f64::max);
        (!tries.iter().any(|r| r["ok"] == true) && tries.len() < MAX_ATTEMPTS && now_ms - last >= RETRY_SECS * 1000.0).then_some(week)
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.iter().rev().find(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    pub(super) fn absorb(&mut self, home: &Path, run: Value) -> Result<()> {
        crate::log(&format!("prompt review: ok={} escalate={} url={}", run["ok"], run["receipt"]["escalate"], run["receipt"]["url"]));
        let receipt = run["receipt"].clone();
        let week = run["week"].as_i64().unwrap_or(0);
        let final_failure = run["ok"] != true && self.runs.iter().filter(|r| r["week"].as_i64() == Some(week)).count() + 1 >= MAX_ATTEMPTS;
        self.runs.push(run.clone());
        let excess = self.runs.len().saturating_sub(KEEP_RUNS);
        self.runs.drain(..excess);
        if run["ok"] != true {
            if final_failure {
                super::conversation_retro::notify("看门狗：提示词周复盘失败", run["error"].as_str().unwrap_or("见 prompt-review.json"));
            }
            return Ok(());
        }
        let url = receipt["url"].as_str().unwrap_or("");
        if receipt["escalate"] != true {
            super::conversation_retro::notify("看门狗：提示词周复盘", &format!("未超过阈值，不开会话。{url}"));
            return Ok(());
        }
        if let Some(id) = self.open_incident(home) {
            self.blocked = Some(json!({"week": week, "by": id, "at_ms": run["at_ms"], "url": url}));
            super::conversation_retro::notify("看门狗：提示词周复盘", &format!("超过阈值，但上次会话 {id} 未确认，本周不开新会话。{url}"));
            return Ok(());
        }
        self.blocked = None;
        let new_groups: Vec<Value> = receipt["groups"].as_array().into_iter().flatten()
            .filter(|g| g["new_standing_waste"].as_u64().unwrap_or(0) > 0).cloned().collect();
        let id = super::diagnostics::incident(home, &format!("prompt_review_due:{week}"), json!({
            "trigger": "prompt_review_due", "week": week, "timezone": "Asia/Singapore", "report_url": url,
            "run_dir": receipt["run_dir"], "sha": receipt["sha"], "base_sha": receipt["base_sha"], "base_source": receipt["base_source"],
            "reasons": receipt["reasons"], "metrics": receipt["metrics"], "base_metrics": receipt["base_metrics"],
            "gate_failures": receipt["gate_failures"], "new_groups": new_groups,
        }))?;
        let reasons = receipt["reasons"].as_array().into_iter().flatten().filter_map(|r| r["text"].as_str()).collect::<Vec<_>>().join("; ");
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, "看门狗：提示词周复盘", &prompt(home, &id, &reasons, &receipt))
        };
        self.dispatches.push(match &result {
            Ok(agent) => json!({"incident": id, "week": week, "at_ms": run["at_ms"], "started": true, "agent": agent}),
            Err(error) => json!({"incident": id, "week": week, "at_ms": run["at_ms"], "started": false, "error": format!("{error:#}")}),
        });
        let opened = if result.is_ok() { "已开去重会话" } else { "会话未能打开，见 prompt-review.json" };
        super::conversation_retro::notify("看门狗：提示词周复盘", &format!("{reasons}。{opened}。{url}"));
        Ok(())
    }

    fn write(&self, home: &Path, now: f64) -> Result<()> {
        write_json(&root(home).join("prompt-review.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "running": self.worker.is_some(), "open_incident": self.open_incident(home), "blocked": self.blocked,
            "runs": self.runs, "dispatches": self.dispatches,
            "schedule": {"timezone": "Asia/Singapore", "weekday": "Monday", "at_min": WEEKLY_AT_MIN, "retry_s": RETRY_SECS,
                "max_attempts": MAX_ATTEMPTS, "timeout_s": RUN_TIMEOUT.as_secs(), "sample_s": SAMPLE_SECS}}))
    }
}

/// Runs `prompt-review run` once and returns its record; never panics on a failed or stuck run.
fn run(home: &Path) -> Value {
    let started_ms = crate::unix_now() * 1000.0;
    let started = Instant::now();
    let runs = root(home).join("prompt-review").join("runs");
    let output = runs.join(format!("{}.json", started_ms as u64));
    let result = (|| -> Result<(Option<i32>, bool)> {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&runs)?;
        let file = |path: &Path| std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(path);
        let mut child = Command::new(BIN).arg("run").env("PATH", super::MARS_SCRIPT_PATH).stdin(Stdio::null())
            .stdout(file(&output)?).stderr(file(&output.with_extension("log"))?).process_group(0).spawn()?;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok((status.code(), false));
            }
            if started.elapsed() >= RUN_TIMEOUT {
                unsafe { libc::kill(-(child.id() as i32), libc::SIGTERM) };
                let _ = child.wait();
                return Ok((None, true));
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    })();
    let receipt = super::quota_watch::receipt(&std::fs::read_to_string(&output).unwrap_or_default());
    let (exit, timed_out, spawn_error) = match result {
        Ok((exit, timed_out)) => (exit, timed_out, None),
        Err(error) => (None, false, Some(format!("{error:#}"))),
    };
    let ok = exit == Some(0) && receipt["ok"] == true;
    let error = spawn_error.or_else(|| timed_out.then(|| "timed out".to_string()))
        .or_else(|| (!ok).then(|| receipt["error"].as_str().unwrap_or("no receipt").to_string()));
    json!({"week": week_of(super::quota_watch::singapore(started_ms).0), "at_ms": crate::unix_now() * 1000.0, "started_at_ms": started_ms,
        "seconds": started.elapsed().as_secs(), "exit": exit, "ok": ok, "error": error, "output": output, "receipt": receipt})
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
pub(super) fn prompt(home: &Path, id: &str, reasons: &str, receipt: &Value) -> String {
    format!("cpu-watchdog incident {id}: the weekly review of the Mars CRM main agent's prompts (system prompt, tool descriptions and schemas, bundled skill prompts and their catalog) crossed its threshold: {reasons}. {header} \
        Incident: {s}/incidents/{id}.json (new_groups lists the new duplicate groups); run directory {run} (report.md, semantic.json with every copy and its file:line, analysis.json, gate.json, body.json); weekly page {url}. \
        Work in the Mars_agent main checkout on branch mastra under the mars-crm and prompt-dedup skills. First run session-status 'prompt dedup' and leave every group whose files a live session is editing to it (verdict owned_elsewhere, owner = that session). \
        For each new group, and any other group worth removing, keep one authoritative copy (prompt-dedup authoring rule 6) and delete or point the others, then deliver through the normal Mars CRM flow (tests, prompt-dedup capture and analyze-prefix gate against origin/mastra, beta acceptance of the affected tool call); propose instead when a removal changes behavior. \
        Write {s}/analysis/{id}/report.md in Chinese and report.json with report_url, groups[] {{id, verdict fixed|proposed|kept|owned_elsewhere, reason, commit when fixed, owner when owned_elsewhere}} and after {{prefix, gate}} when anything was fixed, publish the encrypted report page, then run cpu-watchdog monitor-ack {id}.",
        header = crate::dispatch::route_header("prompt-review"), s = root(home).display(),
        run = receipt["run_dir"].as_str().unwrap_or("?"), url = receipt["url"].as_str().unwrap_or("?"))
}

#[cfg(test)]
#[path = "prompt_review/tests.rs"]
mod tests;
