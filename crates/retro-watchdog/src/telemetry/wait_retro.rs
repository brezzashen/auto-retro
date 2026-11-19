//! Agent wait retro (founder, 2026-10-28): once one agent session has sat blocked on tool calls
//! for THRESHOLD_MIN minutes, open one session that reviews why it waited: inputs filled in wrong
//! and the rebuilds they cost, queues that stuck, held resources or stayed silent, and repeated
//! scripts that one script call could replace.
//!
//! A thread reads what each Claude (lead and subagent) and Codex transcript gained since the last
//! scan every SAMPLE_SECS and sums its blocked spans (`parse.rs`). A session crossing the threshold
//! is flagged; flagged sessions wait for the next retro. One `agent_wait_reached` incident covers
//! every flagged session with its longest waits and wait classes, plus the queue-side evidence of
//! their window (`queues.rs`), and opens one session on route `wait-retro`. Each covered session
//! then starts counting from zero, so it is reviewed again after another THRESHOLD_MIN. None opens
//! while the last is unacknowledged and at most MAX_DISPATCH_PER_DAY a day. A fresh install
//! starts at its install time. `cpu-watchdog wait-stats [--hours N]` prints the same per-session
//! figures from the transcripts of the last N hours.
//!
//! State: `wait-retro.json` (flagged sessions, leaders, dispatches) and `wait-retro-cursors.json`.
pub(super) mod parse;
pub(super) mod queues;
mod validate;
pub(super) use validate::validate_ack;

use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use parse::Cursor;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::mpsc::{self, Receiver, TryRecvError},
};

pub(super) const SAMPLE_SECS: f64 = 600.0;
pub(super) const THRESHOLD_MIN: f64 = 100.0;
const MAX_DISPATCH_PER_DAY: usize = 2;
/// Sessions with their top waits in the incident; the rest are listed by totals only.
const DETAILED: usize = 12;
const RETRY_SECS: f64 = 600.0;
const DAY_MS: f64 = 86_400_000.0;

pub(super) struct Scan { at: f64, files: BTreeMap<String, Cursor> }

#[derive(Default)]
pub(super) struct WaitRetro {
    loaded: bool,
    last_sample: f64,
    last_scan_ms: f64,
    window_from: f64,
    files: BTreeMap<String, Cursor>,
    dispatches: Vec<Value>,
    worker: Option<Receiver<Scan>>,
    load_error: Option<String>,
}

fn minutes(ms: f64) -> f64 { (ms / 6_000.0).round() / 10.0 }

/// One session across account profiles: `limit-resume` copies a transcript into the next account's
/// profile, so `team2/projects/P/S.jsonl` and `team3/projects/P/S.jsonl` repeat the same calls
/// (wait retro 86936479242594809468 found 22b4e43f counted three times, 8fc597c0 twice).
fn session_key(path: &str) -> &str {
    ["/projects/", "/sessions/"].iter().find_map(|m| path.find(m).map(|i| &path[i + m.len()..])).unwrap_or(path)
}

/// The copy written last of each session, longest waits first.
pub(super) fn distinct(files: &BTreeMap<String, Cursor>) -> Vec<(&String, &Cursor)> {
    let mut latest: BTreeMap<&str, (&String, &Cursor)> = BTreeMap::new();
    for (path, c) in files {
        let slot = latest.entry(session_key(path)).or_insert((path, c));
        if c.mtime_ms > slot.1.mtime_ms { *slot = (path, c); }
    }
    let mut out: Vec<_> = latest.into_values().collect();
    out.sort_by(|a, b| b.1.wait_ms.total_cmp(&a.1.wait_ms));
    out
}

/// One session's totals; `detail` adds its longest waits.
pub(super) fn row(path: &str, c: &Cursor, detail: bool) -> Value {
    let classes: BTreeMap<&String, f64> = c.classes.iter().map(|(k, v)| (k, minutes(*v))).collect();
    let mut out = json!({"path": path, "source": c.alias, "subagent": c.sub, "cwd": c.cwd, "wait_min": minutes(c.wait_ms), "waits": c.waits,
        "first_ms": c.first_ms, "last_ms": c.last_ms, "classes_min": classes});
    if detail { out["top"] = json!(c.top); }
    out
}

impl WaitRetro {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded { self.load(home, now); }
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
            std::thread::Builder::new().name("watchdog-wait".into()).spawn(move || {
                let at = crate::unix_now() * 1000.0;
                let files = parse::scan(&home, files, from, last, at);
                let _ = sender.send(Scan { at, files });
            })?;
            self.worker = Some(receiver);
        }
        if changed { self.write(home, now, open)?; }
        Ok(())
    }

    fn load(&mut self, home: &Path, now: f64) {
        self.loaded = true;
        self.window_from = now * 1000.0;
        if let Some(state) = super::read(&root(home).join("wait-retro.json")) {
            self.window_from = state["window_from_ms"].as_f64().unwrap_or(self.window_from);
            self.last_scan_ms = state["last_scan_ms"].as_f64().unwrap_or(0.0);
            self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
        }
        // Not super::read: its 4 MiB cap refused the 4.8 MB cursors at every restart, every transcript
        // since window_from was read again from 0 and reviewed sessions came back with their old
        // totals, re-opening acknowledged retros (wait retro 86145098471064008529).
        match std::fs::read(root(home).join("wait-retro-cursors.json")) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(files) => self.files = files,
                Err(e) => self.load_error = Some(format!("wait-retro-cursors.json: {e}")),
            },
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => self.load_error = Some(format!("wait-retro-cursors.json: {e}")),
            Err(_) => {}
        }
    }

    pub(super) fn absorb(&mut self, home: &Path, scan: Scan) -> Result<Option<String>> {
        self.files = scan.files;
        self.last_scan_ms = scan.at;
        for cursor in self.files.values_mut() {
            if cursor.wait_ms >= THRESHOLD_MIN * 60_000.0 { cursor.flagged = true; }
        }
        let open = self.escalate(home, scan.at);
        write_json(&root(home).join("wait-retro-cursors.json"), &self.files)?;
        open
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.last().filter(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    fn flagged(&self) -> Vec<(&String, &Cursor)> {
        distinct(&self.files).into_iter().filter(|(_, c)| c.flagged).collect()
    }

    fn escalate(&mut self, home: &Path, at: f64) -> Result<Option<String>> {
        let open = self.open_incident(home);
        let failed_recently = self.dispatches.last().is_some_and(|d| d["started"] != true && at - d["at_ms"].as_f64().unwrap_or(0.0) < RETRY_SECS * 1000.0);
        let today = self.dispatches.iter().filter(|d| d["started"] == true && d["at_ms"].as_f64().is_some_and(|t| at - t < DAY_MS)).count();
        let flagged = self.flagged();
        if flagged.is_empty() || open.is_some() || failed_recently || today >= MAX_DISPATCH_PER_DAY { return Ok(open); }
        let sessions: Vec<Value> = flagged.iter().enumerate().map(|(i, (path, c))| row(path, c, i < DETAILED)).collect();
        let from = flagged.iter().map(|(_, c)| c.first_ms).filter(|t| *t > 0.0).fold(at, f64::min);
        let waited: f64 = flagged.iter().map(|(_, c)| c.wait_ms).sum();
        let mut classes: BTreeMap<String, f64> = BTreeMap::new();
        for (_, c) in &flagged { for (k, v) in &c.classes { *classes.entry(k.clone()).or_default() += v; } }
        let classes: BTreeMap<String, f64> = classes.into_iter().map(|(k, v)| (k, minutes(v))).collect();
        let previous: Vec<Value> = self.dispatches.iter().rev().filter(|d| d["started"] == true).take(5)
            .map(|d| json!({"incident": d["incident"], "report": root(home).join("analysis").join(d["incident"].as_str().unwrap_or("")).join("report.json")})).collect();
        let id = super::diagnostics::incident(home, &format!("agent_wait_reached:{}:{}", flagged[0].0, flagged[0].1.first_ms), json!({
            "trigger": "agent_wait_reached", "threshold_min": THRESHOLD_MIN, "sessions_over": sessions.len(), "wait_min_total": minutes(waited),
            "classes_min": classes, "sessions": sessions, "queues": queues::evidence(home, from, at, at), "previous": previous,
            "window": {"from_ms": from, "to_ms": at}, "requirements": {"wait_review": 1},
        }))?;
        let covered: Vec<String> = flagged.iter().map(|(p, _)| session_key(p).to_string()).collect();
        // The same leader and first wait again is a review already done: count from zero, no session.
        if root(home).join("incidents").join(format!("{id}.ack")).exists() {
            for (path, c) in self.files.iter_mut() { if covered.iter().any(|k| k == session_key(path)) { c.reset(); } }
            return Ok(None);
        }
        let summary = json!({"sessions": flagged.len(), "longest_min": minutes(flagged[0].1.wait_ms), "classes_min": classes});
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, &format!("看门狗：等待复盘 {THRESHOLD_MIN:.0} 分钟"), &prompt(home, &id, &summary))
        };
        let started = result.is_ok();
        self.dispatches.push(json!({"incident": id, "at_ms": at, "started": started, "summary": summary,
            "agent": result.as_ref().ok(), "error": result.as_ref().err().map(|e| format!("{e:#}"))}));
        if !started { return Ok(None); }
        for (path, c) in self.files.iter_mut() { if covered.iter().any(|k| k == session_key(path)) { c.reset(); } }
        if !cfg!(test) {
            super::conversation_retro::notify("看门狗：Agent 等待复盘", &format!("{} 个会话等待超过 {THRESHOLD_MIN:.0} 分钟，最长 {} 分钟，已开复盘会话", summary["sessions"], summary["longest_min"]));
        }
        Ok(Some(id))
    }

    fn write(&self, home: &Path, now: f64, open: Option<String>) -> Result<()> {
        let leaders: Vec<(&String, &Cursor)> = distinct(&self.files).into_iter().filter(|(_, c)| c.wait_ms > 0.0).collect();
        let leaders: Vec<Value> = leaders.into_iter().take(10).map(|(p, c)| row(p, c, false)).collect();
        write_json(&root(home).join("wait-retro.json"), &json!({"at_ms": now * 1000.0, "error": self.load_error,
            "scan_running": self.worker.is_some(), "window_from_ms": self.window_from, "last_scan_ms": self.last_scan_ms,
            "tracked_files": self.files.len(), "flagged": self.flagged().len(), "leaders": leaders, "open_incident": open, "dispatches": self.dispatches,
            "threshold": {"wait_min": THRESHOLD_MIN, "sample_s": SAMPLE_SECS, "max_dispatch_per_day": MAX_DISPATCH_PER_DAY,
                "min_wait_s": parse::MIN_MS / 1000.0, "cap_wait_s": parse::CAP_MS / 1000.0}}))
    }
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
fn prompt(home: &Path, id: &str, summary: &Value) -> String {
    format!("cpu-watchdog incident {id}: agent sessions sat blocked on tool calls for {THRESHOLD_MIN:.0}+ minutes; {summary}. {header} \
        Incident: {s}/incidents/{id}.json (sessions with their longest waits and wait classes; queues: release queue waits, build gate admissions, job-bus admission waits and silent jobs, release judges, expired simulator leases and orphaned tickets; previous wait retros); \
        `cpu-watchdog wait-stats --hours 24` recomputes per-session waits. {recall} Review three things: inputs filled in wrong and what they cost (record each in the infrastructure skill's wait ledger), \
        queue mechanisms that stuck, kept resources or waited without progress notices, and repeated scripts one script could replace (implement them). \
        Publish the report page and give its link, write {s}/analysis/{id}/report.md in Chinese and report.json, then run cpu-watchdog monitor-ack {id}.",
        header = crate::dispatch::route_header("wait-retro"), recall = crate::retro_recall::instruction(id), s = root(home).display())
}

/// `wait-stats [--hours N]`: per-session waits from the transcripts of the last N hours (default 24).
pub(crate) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let mut hours = 24.0;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--hours" => hours = it.next().context("--hours needs a number")?.parse().context("--hours needs a number")?,
            other => anyhow::bail!("usage: cpu-watchdog wait-stats [--hours N] (unknown {other})"),
        }
    }
    let now = crate::unix_now() * 1000.0;
    let from = now - hours * 3_600_000.0;
    let files = parse::scan_since(home, from);
    let rows: Vec<(&String, &Cursor)> = distinct(&files).into_iter().filter(|(_, c)| c.wait_ms > 0.0).collect();
    let over = rows.iter().filter(|(_, c)| c.wait_ms >= THRESHOLD_MIN * 60_000.0).count();
    let total: f64 = rows.iter().map(|(_, c)| c.wait_ms).sum();
    println!("{}", serde_json::to_string_pretty(&json!({"hours": hours, "sessions_waiting": rows.len(), "sessions_over_threshold": over,
        "threshold_min": THRESHOLD_MIN, "wait_min_total": minutes(total),
        "sessions": rows.iter().take(20).enumerate().map(|(i, (p, c))| row(p, c, i < 5)).collect::<Vec<_>>(),
        "queues": queues::evidence(home, from, now, now)}))?);
    Ok(())
}

#[cfg(test)]
#[path = "wait_retro/tests.rs"]
mod tests;
