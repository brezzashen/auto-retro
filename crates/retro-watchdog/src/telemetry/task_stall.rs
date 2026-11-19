//! Task stall watch (founder, 2026-10-29): a single agent task open for TASK_MIN minutes is checked
//! for an unexpected stall, queue and information problems first; stop the bleeding, then open one
//! session that optimizes and reviews. Founder, 2026-11-02: a task (Orca tab) open TASK_MIN+
//! minutes that has been waiting BLOCK_MIN / SILENT_MIN (10) minutes gets an analysis model at once,
//! ahead of ordinary watchdog analyses; a task that is not waiting, or waits on the user, passes.
//!
//! Every SAMPLE_SECS a thread reads what each Claude (lead and subagent) and Codex transcript gained
//! (`parse.rs`) and heals job-bus orphans (`heal.rs`). A task open TASK_MIN or more stalls when
//! - `blocked_call`: a tool call has waited BLOCK_MIN without a result (questions to the user and
//!   subagent dispatches excepted); `waits_on` names the job-bus job or release run it waits on,
//!   `ended: true` when that already finished (an information problem: the end never reached it),
//!   or the session's sim-lease queue ticket and the device holder (`simq.rs`), plus any open
//!   render-queue exclusive window that holds new builds (`exclusive.rs`);
//! - `silent`: no call is pending and the transcript has not moved for SILENT_MIN;
//! - `repeat_failure`: one command signature came back as an error REPEAT_FAILS times (a wrong
//!   input retried instead of fixed).
//!
//! New stalled tasks open one `task_stalled` incident with the queue evidence of the wait retro and
//! the heals since the last one, and one urgent session on route `task-stall` right away, also while
//! an earlier one is still unacknowledged (at most MAX_DISPATCH_PER_DAY a day, the runaway guard).
//! Each task start is reported once. Transcripts not
//! written for DROP_MS are dropped. `cpu-watchdog task-stall [--minutes N]` prints the open tasks.
//!
//! State: `task-stall.json` (open tasks, stalls, heals, dispatches) and `task-stall-cursors.json`.
mod codex_live;
mod exclusive;
mod heal;
mod rows;
mod signature;
mod simq;
pub(super) mod parse;
mod validate;
pub(super) use validate::validate_ack;

use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use parse::TaskCursor;
use rows::open_rows;
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, sync::mpsc::{self, Receiver, TryRecvError}};

pub(super) const SAMPLE_SECS: f64 = 120.0;
pub(super) const TASK_MIN: f64 = 30.0;
pub(super) const BLOCK_MIN: f64 = 10.0;
pub(super) const SILENT_MIN: f64 = 10.0;
pub(super) const REPEAT_FAILS: u64 = 3;
const MAX_DISPATCH_PER_DAY: usize = 12;
pub(super) const DROP_MS: f64 = 4.0 * 3_600_000.0;
const HEALS_KEPT: usize = 50;
const DAY_MS: f64 = 86_400_000.0;

pub(super) struct Scan { at: f64, files: BTreeMap<String, TaskCursor>, heals: Vec<Value>, rows: Vec<Value> }

#[derive(Default)]
pub(super) struct TaskStall {
    loaded: bool,
    last_sample: f64,
    files: BTreeMap<String, TaskCursor>,
    heals: Vec<Value>,
    open_tasks: Vec<Value>,
    dispatches: Vec<Value>,
    worker: Option<Receiver<Scan>>,
}


/// Reads every transcript written since its cursor; a file first seen is read whole when written
/// within DROP_MS, so a task already running at install is seen with its real start.
pub(super) fn scan(home: &Path, mut files: BTreeMap<String, TaskCursor>, now: f64) -> BTreeMap<String, TaskCursor> {
    super::wait_retro::parse::each(home, &mut |alias, codex, path, _, modified, len| {
        if now - modified >= DROP_MS { return; }
        let key = path.to_string_lossy().into_owned();
        let sub = path.components().any(|c| c.as_os_str() == "subagents");
        let cursor = files.entry(key).or_insert_with(|| TaskCursor::new(alias, codex, sub));
        cursor.mtime_ms = modified;
        let mut offset = cursor.offset;
        super::runtime_retro::parse::read_lines(path, len, &mut offset, &mut |value| cursor.feed(value));
        cursor.offset = offset;
    });
    files.retain(|_, c| now - c.mtime_ms < DROP_MS);
    files
}


impl TaskStall {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded { self.load(home); }
        let mut changed = false;
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(scan) => { self.worker = None; self.absorb(home, scan)?; changed = true; }
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.worker.is_none() && (self.last_sample == 0.0 || now < self.last_sample || now - self.last_sample >= SAMPLE_SECS) {
            (changed, self.last_sample) = (true, now);
            let (home, files) = (home.to_path_buf(), std::mem::take(&mut self.files));
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::Builder::new().name("watchdog-task-stall".into()).spawn(move || {
                let at = crate::unix_now() * 1000.0;
                let heals = heal::heal_bus(&home, at);
                let files = scan(&home, files, at);
                let rows = open_rows(&home, &files, at, TASK_MIN);
                let _ = sender.send(Scan { at, files, heals, rows });
            })?;
            self.worker = Some(receiver);
        }
        if changed { self.write(home, now)?; }
        Ok(())
    }

    fn load(&mut self, home: &Path) {
        self.loaded = true;
        if let Some(state) = super::read(&root(home).join("task-stall.json")) {
            self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
            self.heals = state["heals"].as_array().cloned().unwrap_or_default();
        }
        if let Some(files) = super::read(&root(home).join("task-stall-cursors.json")) {
            self.files = serde_json::from_value(files).unwrap_or_default();
        }
    }

    pub(super) fn absorb(&mut self, home: &Path, scan: Scan) -> Result<()> {
        self.files = scan.files;
        self.heals.extend(scan.heals);
        let excess = self.heals.len().saturating_sub(HEALS_KEPT);
        self.heals.drain(..excess);
        self.open_tasks = scan.rows;
        self.escalate(home, scan.at)?;
        write_json(&root(home).join("task-stall-cursors.json"), &self.files)
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.last().filter(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    fn escalate(&mut self, home: &Path, at: f64) -> Result<Option<String>> {
        let fresh: Vec<Value> = self.open_tasks.iter().filter(|r| rows::escalating(r))
            .filter(|r| self.files.get(r["path"].as_str().unwrap_or("")).is_some_and(|c| r["report_ms"].as_f64() != Some(c.reported_ms))).cloned().collect();
        let today = self.dispatches.iter().filter(|d| d["started"] == true && d["at_ms"].as_f64().is_some_and(|t| at - t < DAY_MS)).count();
        if fresh.is_empty() || today >= MAX_DISPATCH_PER_DAY { return Ok(None); }
        let since = self.dispatches.iter().rev().find(|d| d["started"] == true).and_then(|d| d["at_ms"].as_f64()).unwrap_or(0.0);
        let heals: Vec<&Value> = self.heals.iter().filter(|h| h["at_ms"].as_f64().unwrap_or(0.0) > since).collect();
        let from = fresh.iter().filter_map(|r| r["task_start_ms"].as_f64()).fold(at, f64::min);
        let id = super::diagnostics::incident(home, &format!("task_stalled:{}:{}", fresh[0]["path"].as_str().unwrap_or(""), fresh[0]["task_start_ms"]), json!({
            "trigger": "task_stalled", "threshold": {"task_min": TASK_MIN, "block_min": BLOCK_MIN, "silent_min": SILENT_MIN, "repeat_fails": REPEAT_FAILS},
            "tasks": fresh, "heals": heals, "queues": super::wait_retro::queues::evidence(home, from, at, at),
            "window": {"from_ms": from, "to_ms": at}, "requirements": {"task_review": 1},
        }))?;
        let summary = json!({"tasks": fresh.len(), "longest_min": fresh[0]["open_min"], "heals": heals.len()});
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_urgent_session(home, &id, &format!("看门狗：任务卡顿 {TASK_MIN:.0} 分钟"), &prompt(home, &id, &summary))
        };
        let started = result.is_ok();
        self.dispatches.push(json!({"incident": id, "at_ms": at, "started": started, "summary": summary,
            "agent": result.as_ref().ok(), "error": result.as_ref().err().map(|e| format!("{e:#}"))}));
        if !started { return Ok(None); }
        for r in &fresh { if let Some(c) = self.files.get_mut(r["path"].as_str().unwrap_or("")) { c.reported_ms = r["report_ms"].as_f64().unwrap_or(c.task_ms); } }
        super::conversation_retro::notify("看门狗：任务卡顿", &format!("{} 个任务超过 {TASK_MIN:.0} 分钟且有卡顿迹象，已止血 {} 项并开分析会话", fresh.len(), heals.len()));
        Ok(Some(id))
    }

    fn write(&self, home: &Path, now: f64) -> Result<()> {
        write_json(&root(home).join("task-stall.json"), &json!({"at_ms": now * 1000.0, "error": null, "scan_running": self.worker.is_some(),
            "tracked_files": self.files.len(), "open_tasks": self.open_tasks, "heals": self.heals, "open_incident": self.open_incident(home),
            "dispatches": self.dispatches, "threshold": {"task_min": TASK_MIN, "block_min": BLOCK_MIN, "silent_min": SILENT_MIN,
                "repeat_fails": REPEAT_FAILS, "orphan_min": heal::ORPHAN_MIN, "sample_s": SAMPLE_SECS, "max_dispatch_per_day": MAX_DISPATCH_PER_DAY}}))
    }
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
fn prompt(home: &Path, id: &str, summary: &Value) -> String {
    format!("cpu-watchdog incident {id}: agent tasks open {TASK_MIN:.0}+ minutes show stall signs; {summary}. {header} \
        Incident: {s}/incidents/{id}.json (tasks with blocked calls and what they wait on, silent sessions, repeated failing inputs; job-bus orphans already healed; queue evidence). \
        `cpu-watchdog task-stall` prints the open tasks now. Judge each task first: a wait that is normal (necessary long work still progressing) passes untouched with verdict necessary. \
        A stuck one: release what holds it through its owner, nudge the task with session-send so it continues, confirm it moved on, then fix the causes and review: \
        queue problems, information problems (inputs, missing or late events), and what recurs. Publish the report page and give its link, write {s}/analysis/{id}/report.md in Chinese and report.json, then run cpu-watchdog monitor-ack {id}.",
        header = crate::dispatch::route_header("task-stall"), s = root(home).display())
}

/// `task-stall [--minutes N]`: open tasks of at least N minutes (default TASK_MIN) with their
/// stalls, and the job-bus orphans a heal would close (nothing is emitted).
pub(crate) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let mut min = TASK_MIN;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--minutes" => min = it.next().context("--minutes needs a number")?.parse().context("--minutes needs a number")?,
            other => anyhow::bail!("usage: cpu-watchdog task-stall [--minutes N] (unknown {other})"),
        }
    }
    let now = crate::unix_now() * 1000.0;
    let files = scan(home, BTreeMap::new(), now);
    let rows = open_rows(home, &files, now, min);
    let state = home.join(".local/state");
    let orphans: Vec<Value> = heal::jobs(&state.join("job-bus/events.jsonl")).into_iter().filter_map(|(key, s)| {
        let job = key.split_once('|').map_or(key.clone(), |(_, j)| j.to_string());
        heal::verdict(&s, &job, &state.join("mars-deliver/releases"), now).map(|(event, reason)| json!({"topic": s.topic, "job": job, "last_event": s.event, "would_emit": event, "reason": reason}))
    }).collect();
    let stalled = rows.iter().filter(|r| r["stalls"].as_array().is_some_and(|s| !s.is_empty())).count();
    println!("{}", serde_json::to_string_pretty(&json!({"minutes": min, "tracked_files": files.len(), "open_tasks": rows.len(), "stalled": stalled,
        "tasks": rows, "bus_orphans": orphans}))?);
    Ok(())
}

#[cfg(test)]
#[path = "task_stall/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "task_stall/tests_file_tools.rs"]
mod tests_file_tools;
