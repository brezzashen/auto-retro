//! Risk retro (founder, 2026-11-14: "用户做出风险行为，被警告，立刻分析和复盘是不是真实风险，还是误判"):
//! every time Mars CRM warns, reviews or suspends a non-exempt production account for risk behaviour,
//! one session judges at once whether the risk is real or the detector misfired.
//!
//! Every SAMPLE_SECS a worker reads the events recorded past `cursor_ms` (`fetch.rs`, read-only
//! `mars canary sql --role=crm`): the provider-block warning (3rd and 4th blocked turn in 24 h), the
//! cancellation abuse warning, risk reviews (pending or suspended, every behaviour) and chat or account
//! suspensions (`crm.billing_abuse_flag`). A new event notifies at once and opens one
//! `risk_warning_found` incident on route `risk-retro` through the urgent queue. Events found while a
//! session is unacknowledged wait in `pending` and open the next one as soon as it is acknowledged, so
//! none is dropped; at most MAX_DISPATCH_PER_DAY a day. A fresh install starts SEED_MS back so the
//! open backlog is reviewed once. `cpu-watchdog risk-events` prints the events on demand.
//!
//! State: `risk-retro.json`; each incident's events: `risk-retro/<id>.json`.
pub(super) mod fetch;
mod validate;
pub(super) use validate::validate_ack;

use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{path::Path, sync::mpsc::{self, Receiver, TryRecvError}};

pub(super) const SAMPLE_SECS: f64 = 120.0;
const MAX_DISPATCH_PER_DAY: usize = 12;
const RETRY_SECS: f64 = 600.0;
const DAY_MS: f64 = 86_400_000.0;
/// A fresh install reviews the last 30 days once (risk signals are purged after 30 days).
const SEED_MS: f64 = 30.0 * DAY_MS;
/// Each read reaches this far behind the cursor, so a row committed late is still seen; keys dedupe it.
const OVERLAP_MS: f64 = 600_000.0;
/// Seen keys are kept this long (beyond any overlap or seed window).
const SEEN_MS: f64 = 35.0 * DAY_MS;

type Sample = Result<(f64, Vec<Value>), String>;

#[derive(Default)]
pub(super) struct RiskRetro {
    loaded: bool,
    last_sample: f64,
    pub(super) cursor_ms: f64,
    /// Event key → first seen (ms).
    pub(super) seen: serde_json::Map<String, Value>,
    pub(super) pending: Vec<Value>,
    pub(super) dispatches: Vec<Value>,
    error: Option<String>,
    worker: Option<Receiver<Sample>>,
}

impl RiskRetro {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded { self.load(home, now); }
        let mut changed = false;
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(sample) => { self.worker = None; changed = true; self.absorb(home, now * 1000.0, sample)?; }
                Err(TryRecvError::Disconnected) => { self.worker = None; changed = true; }
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.worker.is_none() && !cfg!(test) && !(self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS) {
            self.last_sample = now;
            changed = true;
            let (from, to) = (self.cursor_ms - OVERLAP_MS, now * 1000.0);
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::Builder::new().name("watchdog-risk-retro".into()).spawn(move || {
                let _ = sender.send(fetch::events(from, to).map(|rows| (to, rows)).map_err(|e| format!("{e:#}")));
            })?;
            self.worker = Some(receiver);
        }
        // A session acknowledged since the last sample frees the slot for the waiting events at once.
        if !self.pending.is_empty() && self.may_open(home, now * 1000.0) { changed = true; self.escalate(home, now * 1000.0)?; }
        if changed { self.write(home, now) } else { Ok(()) }
    }

    fn load(&mut self, home: &Path, now: f64) {
        self.loaded = true;
        match super::read(&root(home).join("risk-retro.json")) {
            Some(state) => {
                self.cursor_ms = state["cursor_ms"].as_f64().unwrap_or(now * 1000.0);
                self.seen = state["seen"].as_object().cloned().unwrap_or_default();
                self.pending = state["pending"].as_array().cloned().unwrap_or_default();
                self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
            }
            None => self.cursor_ms = now * 1000.0 - SEED_MS,
        }
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.iter().rev().find(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    pub(super) fn may_open(&self, home: &Path, at: f64) -> bool {
        let failed_recently = self.dispatches.last().is_some_and(|d| d["started"] != true && at - d["at_ms"].as_f64().unwrap_or(0.0) < RETRY_SECS * 1000.0);
        let today = self.dispatches.iter().filter(|d| d["started"] == true && d["at_ms"].as_f64().is_some_and(|t| at - t < DAY_MS)).count();
        self.open_incident(home).is_none() && !failed_recently && today < MAX_DISPATCH_PER_DAY
    }

    /// Adds the sample's unseen events to `pending`, notifies them and opens a session when allowed.
    pub(super) fn absorb(&mut self, home: &Path, at: f64, sample: Sample) -> Result<()> {
        let (to, rows) = match sample {
            Err(error) => { crate::log(&format!("risk retro: {error}")); self.error = Some(error); return Ok(()); }
            Ok(sample) => sample,
        };
        self.error = None;
        let fresh: Vec<Value> = rows.into_iter().filter(|r| r["key"].as_str().is_some_and(|k| !self.seen.contains_key(k))).collect();
        for event in &fresh { self.seen.insert(event["key"].as_str().unwrap_or_default().to_string(), json!(at)); }
        self.seen.retain(|_, first| first.as_f64().is_some_and(|t| at - t < SEEN_MS));
        self.cursor_ms = self.cursor_ms.max(to);
        if !fresh.is_empty() && !cfg!(test) {
            let open = self.open_incident(home);
            crate::dispatch::notify("看门狗：用户风险警告", &format!("{}{}", describe(&fresh),
                open.map_or(String::from("，立即开复盘会话"), |id| format!("，排在复盘 {id} 之后"))));
        }
        self.pending.extend(fresh);
        if !self.pending.is_empty() && self.may_open(home, at) { self.escalate(home, at)?; }
        Ok(())
    }

    pub(super) fn escalate(&mut self, home: &Path, at: f64) -> Result<Option<String>> {
        let events = self.pending.clone();
        let keys: Vec<Value> = events.iter().map(|e| e["key"].clone()).collect();
        let accounts = { let mut a: Vec<&str> = events.iter().filter_map(|e| e["user_id"].as_str()).collect(); a.sort(); a.dedup(); a.len() };
        let summary = json!({"events": events.len(), "accounts": accounts, "kinds": kinds(&events)});
        let id = super::diagnostics::incident(home, &format!("risk_warning_found:{}", keys.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")), json!({
            "trigger": "risk_warning_found", "summary": summary, "events": events,
            "previous": self.dispatches.iter().rev().filter(|d| d["started"] == true).take(5).map(|d| json!({"incident": d["incident"], "summary": d["summary"]})).collect::<Vec<_>>(),
            "requirements": {"risk_review": {"events": keys}}}))?;
        let saved = root(home).join("risk-retro").join(format!("{id}.json"));
        std::fs::create_dir_all(saved.parent().context("events directory")?)?;
        write_json(&saved, &json!({"incident": id, "events": events}))?;
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_urgent_session(home, &id, &format!("看门狗：风险警告复盘 {} 个账号", accounts), &prompt(home, &id, &summary))
        };
        let started = result.is_ok();
        self.dispatches.push(json!({"incident": id, "at_ms": at, "started": started, "summary": summary,
            "agent": result.as_ref().ok(), "error": result.as_ref().err().map(|e| format!("{e:#}"))}));
        if !started { return Ok(None); }
        self.pending.clear();
        Ok(Some(id))
    }

    fn write(&self, home: &Path, now: f64) -> Result<()> {
        write_json(&root(home).join("risk-retro.json"), &json!({"at_ms": now * 1000.0, "error": self.error,
            "running": self.worker.is_some(), "cursor_ms": self.cursor_ms, "pending": self.pending, "seen": self.seen,
            "open_incident": self.open_incident(home), "dispatches": self.dispatches,
            "threshold": {"sample_s": SAMPLE_SECS, "max_dispatch_per_day": MAX_DISPATCH_PER_DAY, "overlap_s": OVERLAP_MS / 1000.0}}))
    }
}

/// Event count per kind (a risk review also by behaviour and status).
pub(super) fn kinds(events: &[Value]) -> Value {
    let mut out = serde_json::Map::new();
    for e in events {
        let mut kind = e["kind"].as_str().unwrap_or("unknown").to_string();
        if kind == "risk_review" { kind = format!("risk_review:{}:{}", e["detail"]["behaviour"].as_str().unwrap_or("?"), e["detail"]["status"].as_str().unwrap_or("?")); }
        if kind == "abuse_flag" { kind = format!("abuse_flag:{}:{}", e["detail"]["reason"].as_str().unwrap_or("?"), e["detail"]["status"].as_str().unwrap_or("?")); }
        let n = out.get(&kind).and_then(Value::as_u64).unwrap_or(0) + 1;
        out.insert(kind, json!(n));
    }
    Value::Object(out)
}

fn describe(events: &[Value]) -> String {
    let kinds = kinds(events);
    let parts: Vec<String> = kinds.as_object().into_iter().flatten().map(|(k, n)| format!("{k}×{n}")).collect();
    format!("{} 条新事件：{}", events.len(), parts.join("，"))
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
pub(super) fn prompt(home: &Path, id: &str, summary: &Value) -> String {
    format!("cpu-watchdog incident {id}: Mars CRM production warned, reviewed or suspended a non-exempt account for risk behaviour; {summary}. {header} \
        Incident: {s}/incidents/{id}.json (events[]: kind, key, time, user id, masked email, detail with behaviour, confidence, status, window features, masked excerpt, block or cancellation counts); events also in {s}/risk-retro/{id}.json. \
        For every event decide real_risk, false_positive or inconclusive from evidence: the account's own turns and requests (mars canary abuse show, mars canary sql read-only, the mars-crm conversation diagnosis), the detector's source and rules with file:line, and whether a normal user would do the same. \
        Read-only on production: never lift, confirm, dismiss, suspend, write user data or ship a rule change; put each such action in the report as needs_founder with the exact command. \
        Run mars-alert-watch status first and reuse a live alert agent's findings for the same suspension. \
        Publish the encrypted Chinese report page and give its link, write {s}/analysis/{id}/report.md in Chinese and report.json, then run cpu-watchdog monitor-ack {id}.",
        header = crate::dispatch::route_header("risk-retro"), s = root(home).display())
}

const USAGE: &str = "usage: cpu-watchdog risk-events [--hours N | --incident <id>]";

/// `risk-events`: the risk events of the last N hours (default 24) read now, or an incident's saved events.
pub(crate) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let mut hours = 24.0;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--hours" => hours = it.next().context(USAGE)?.parse().context("--hours needs a number")?,
            "--incident" => {
                let path = root(home).join("risk-retro").join(format!("{}.json", it.next().context(USAGE)?));
                let saved: Value = serde_json::from_slice(&std::fs::read(&path).with_context(|| format!("read {}", path.display()))?)?;
                println!("{}", serde_json::to_string_pretty(&saved)?);
                return Ok(());
            }
            other => anyhow::bail!("{USAGE} (unknown {other})"),
        }
    }
    let now = crate::unix_now() * 1000.0;
    let events = fetch::events(now - hours * 3_600_000.0, now)?;
    println!("{}", serde_json::to_string_pretty(&json!({"hours": hours, "kinds": kinds(&events), "events": events}))?);
    Ok(())
}

#[cfg(test)]
#[path = "risk_retro/tests.rs"]
mod tests;
