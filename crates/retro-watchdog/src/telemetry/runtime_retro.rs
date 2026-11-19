//! Runtime retro watch (user request, 2026-10-23): after every THRESHOLD_HOURS of model time or
//! THRESHOLD_CALLS model calls, whichever comes first (merged 2026-10-25), open one session that
//! finds where agent time goes, scripts repeated work and removes avoidable waits.
//!
//! Model time is summed over every Claude account (lead sessions and subagents) and both Codex
//! accounts (`parse.rs` defines the spans); a model call is one turn there (a model input answered
//! by output, so each tool round trip counts once). A thread reads only what each
//! transcript gained since the last scan, every SAMPLE_SECS, and also tallies tool wait by tool,
//! turns and output tokens, so the incident carries the window's time breakdown. Reaching either
//! threshold opens one `model_runtime_reached` incident (`reached` names which) and one session on
//! route `runtime-retro` through `dispatch::start_session`; the window, and with it both counters,
//! then restarts. None opens while the last is unacknowledged; at most MAX_DISPATCH_PER_DAY a day.
//!
//! State: `runtime-retro.json` (totals, dispatches) and `runtime-retro-cursors.json` (file offsets).
mod label;
pub(super) mod parse;
pub(super) mod replay;

use crate::monitor::{root, write_json};
use anyhow::Result;
use parse::{Cursor, Tally};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::mpsc::{self, Receiver, TryRecvError},
};

pub(super) const SAMPLE_SECS: f64 = 600.0;
pub(super) const THRESHOLD_HOURS: f64 = 100.0;
/// About 13 wall hours at the 2026-10-25 rate (3,090 calls an hour), the same order as
/// THRESHOLD_HOURS, so either counter can come first.
pub(super) const THRESHOLD_CALLS: f64 = 40_000.0;
const MAX_DISPATCH_PER_DAY: usize = 1;
/// A session that failed to start is retried after this long.
const RETRY_SECS: f64 = 600.0;
const DAY_MS: f64 = 86_400_000.0;

pub(super) struct Scan { at: f64, files: BTreeMap<String, Cursor>, tally: Tally }

#[derive(Default)]
pub(super) struct RuntimeRetro {
    loaded: bool,
    last_sample: f64,
    last_scan_ms: f64,
    window_from: f64,
    files: BTreeMap<String, Cursor>,
    tally: Tally,
    dispatches: Vec<Value>,
    worker: Option<Receiver<Scan>>,
}

impl RuntimeRetro {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded {
            self.load(home, now);
        }
        let mut open = self.open_incident(home);
        let mut changed = false;
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(scan) => { self.worker = None; open = self.absorb(home, scan)?; changed = true; }
                // A panicked scan loses its cursors; files then restart at their ends.
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.worker.is_none() && (self.last_sample == 0.0 || now < self.last_sample || now - self.last_sample >= SAMPLE_SECS) {
            changed = true;
            self.last_sample = now;
            let (home, files, from, last) = (home.to_path_buf(), std::mem::take(&mut self.files), self.window_from, self.last_scan_ms);
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::Builder::new().name("watchdog-runtime".into()).spawn(move || {
                let at = crate::unix_now() * 1000.0;
                let (files, tally) = parse::scan(&home, files, from, last, at);
                let _ = sender.send(Scan { at, files, tally });
            })?;
            self.worker = Some(receiver);
        }
        if changed { self.write(home, now, open)?; }
        Ok(())
    }

    fn load(&mut self, home: &Path, now: f64) {
        self.loaded = true;
        self.window_from = now * 1000.0;
        if let Some(state) = super::read(&root(home).join("runtime-retro.json")) {
            self.window_from = state["window_from_ms"].as_f64().unwrap_or(self.window_from);
            self.last_scan_ms = state["last_scan_ms"].as_f64().unwrap_or(0.0);
            self.tally = serde_json::from_value(state["tally"].clone()).unwrap_or_default();
            self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
        }
        if let Some(files) = super::read(&root(home).join("runtime-retro-cursors.json")) {
            self.files = serde_json::from_value(files).unwrap_or_default();
        }
    }

    pub(super) fn absorb(&mut self, home: &Path, scan: Scan) -> Result<Option<String>> {
        self.files = scan.files;
        self.tally.merge(scan.tally);
        self.last_scan_ms = scan.at;
        write_json(&root(home).join("runtime-retro-cursors.json"), &self.files)?;
        self.escalate(home, scan.at)
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.last().filter(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    /// Opens one incident and session once the window holds THRESHOLD_HOURS or THRESHOLD_CALLS;
    /// returns the open incident id.
    fn escalate(&mut self, home: &Path, at: f64) -> Result<Option<String>> {
        let open = self.open_incident(home);
        let last = self.dispatches.last();
        let failed_recently = last.is_some_and(|d| d["started"] != true && at - d["at_ms"].as_f64().unwrap_or(0.0) < RETRY_SECS * 1000.0);
        let today = self.dispatches.iter().filter(|d| d["started"] == true && d["at_ms"].as_f64().is_some_and(|t| at - t < DAY_MS)).count();
        let (hours, calls) = (self.tally.model_hours(), self.tally.model_calls());
        let reached = if hours >= THRESHOLD_HOURS { "model_hours" } else if calls >= THRESHOLD_CALLS { "model_calls" } else { return Ok(open) };
        if open.is_some() || failed_recently || today >= MAX_DISPATCH_PER_DAY {
            return Ok(open);
        }
        let previous: Vec<Value> = self.dispatches.iter().rev().filter(|d| d["started"] == true).take(5)
            .map(|d| json!({"incident": d["incident"], "report": root(home).join("analysis").join(d["incident"].as_str().unwrap_or("")).join("report.json")})).collect();
        let id = super::diagnostics::incident(home, &format!("model_runtime_reached:{}", self.window_from), json!({
            "trigger": "model_runtime_reached", "reached": reached, "model_hours": round(hours), "threshold_hours": THRESHOLD_HOURS,
            "model_calls": calls, "threshold_calls": THRESHOLD_CALLS,
            "window": {"from_ms": self.window_from, "to_ms": at}, "breakdown": self.breakdown(),
            "transcripts": self.top_transcripts(40), "previous": previous,
            "requirements": {"followups": 1, "repeats": 1, "clarity": 1},
        }))?;
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, &format!("看门狗：{}复盘", reached_text(reached)), &prompt(home, &id, hours, calls, reached))
        };
        self.dispatches.push(match &result {
            Ok(agent) => json!({"incident": id, "reached": reached, "model_hours": round(hours), "model_calls": calls, "at_ms": at, "started": true, "agent": agent}),
            Err(error) => json!({"incident": id, "reached": reached, "model_hours": round(hours), "model_calls": calls, "at_ms": at, "started": false, "error": format!("{error:#}")}),
        });
        if result.is_err() {
            return Ok(None);
        }
        self.window_from = at;
        self.tally = Tally::default();
        for cursor in self.files.values_mut() { cursor.model_ms = 0.0; }
        super::conversation_retro::notify("看门狗：模型工作量复盘", &format!("{}（{:.0} 小时、{calls:.0} 次调用），已开会话分析时间去向", reached_text(reached), hours));
        Ok(Some(id))
    }

    /// Hours per `alias side family`, the top tool waits, turns and output tokens of the window.
    pub(super) fn breakdown(&self) -> Value {
        breakdown_of(&self.tally, 30)
    }
    fn top_transcripts(&self, n: usize) -> Vec<Value> {
        let mut files: Vec<(&String, &Cursor)> = self.files.iter().filter(|(_, c)| c.model_ms > 0.0).collect();
        files.sort_by(|a, b| b.1.model_ms.total_cmp(&a.1.model_ms));
        files.into_iter().take(n).map(|(path, c)| json!({"path": path, "source": c.alias, "subagent": c.sub, "model_hours": round(c.model_ms / 3_600_000.0)})).collect()
    }

    fn write(&self, home: &Path, now: f64, open: Option<String>) -> Result<()> {
        write_json(&root(home).join("runtime-retro.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "scan_running": self.worker.is_some(), "window_from_ms": self.window_from, "last_scan_ms": self.last_scan_ms,
            "model_hours": round(self.tally.model_hours()), "model_calls": self.tally.model_calls(), "tracked_files": self.files.len(), "tally": self.tally,
            "open_incident": open, "dispatches": self.dispatches,
            "threshold": {"model_hours": THRESHOLD_HOURS, "model_calls": THRESHOLD_CALLS, "sample_s": SAMPLE_SECS, "max_dispatch_per_day": MAX_DISPATCH_PER_DAY, "idle_cap_s": parse::CAP_MS / 1000.0}}))
    }
}

/// Hours per `alias side family`, the `top` tool waits, turns and output tokens of a tally
/// (the incident's `breakdown`; `runtime-breakdown` replays a past window into the same shape).
pub(super) fn breakdown_of(tally: &Tally, top: usize) -> Value {
    let hours = |m: &BTreeMap<String, f64>| m.iter().map(|(k, v)| (k.clone(), json!(round(v / 3_600_000.0)))).collect::<serde_json::Map<_, _>>();
    let mut tools: Vec<(&String, &f64)> = tally.tool_ms.iter().collect();
    tools.sort_by(|a, b| b.1.total_cmp(a.1));
    let tools: Vec<Value> = tools.into_iter().take(top).map(|(label, ms)| {
        let calls = tally.tool_calls.get(label).copied().unwrap_or(0.0);
        json!({"tool": label, "hours": round(ms / 3_600_000.0), "calls": calls, "avg_s": round(ms / 1000.0 / calls.max(1.0))})
    }).collect();
    json!({"model_hours": hours(&tally.model_ms), "tool_wait_hours_total": round(tally.tool_ms.values().sum::<f64>() / 3_600_000.0 + 0.0),
        "tool_wait_top": tools, "turns": tally.turns, "output_tokens": tally.output_tokens})
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn reached_text(reached: &str) -> String {
    if reached == "model_calls" { format!("模型调用满 {THRESHOLD_CALLS:.0} 次") } else { format!("模型运行满 {THRESHOLD_HOURS:.0} 小时") }
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
pub(super) fn prompt(home: &Path, id: &str, hours: f64, calls: f64, reached: &str) -> String {
    format!("cpu-watchdog incident {id}: agents ran {hours:.0} hours of model time in {calls:.0} model calls; the {reached} threshold \
        ({THRESHOLD_HOURS} hours or {THRESHOLD_CALLS} calls, whichever comes first) was reached. {header} \
        Incident: {s}/incidents/{id}.json (window, model hours, calls and tokens by account and model, tool wait by tool, busiest transcripts, previous runtime retros); state root {s}. \
        {recall} {followup} {repeats} {clarity} Find where the time and calls went, script repeated call sequences and remove the largest avoidable waits with measured changes, publish the report page and give its link, \
        write {s}/analysis/{id}/report.md in Chinese and report.json, then run cpu-watchdog monitor-ack {id}.",
        header = crate::dispatch::route_header("runtime-retro"), recall = crate::retro_recall::instruction(id), followup = super::retro_followup::instruction(id), repeats = super::retro_repeats::instruction(id), clarity = super::retro_clarity::instruction(id), s = root(home).display())
}

#[cfg(test)]
#[path = "runtime_retro/tests.rs"]
mod tests;
