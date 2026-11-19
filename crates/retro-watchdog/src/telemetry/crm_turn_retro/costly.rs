//! Costly-turn watch (founder, 2026-11-16, quoted: `当用户在生产环境使用积分单轮次超过 20 分时，需要复盘一下原因，确认是正常调用还是异常情况，以及是否存在缓存失效的情况`):
//! every ended production chat turn that settled more than THRESHOLD_CREDITS is profiled, and a session judges
//! each one normal or abnormal and whether its prompt cache was invalidated.
//!
//! Every SAMPLE_SECS a worker reads the costly turn ids stored past `cursor_ms` (`fetch::costly_sql`, read-only
//! `mars canary sql --role=agent`; the canary track and example.com share the database). For new ones it reads
//! the window with each thread's previous turn and `profile.rs` writes the facts and a pre-verdict per turn. The
//! profiles wait in `pending`; one `crm_costly_turn_found` incident on route `crm-costly-turn` takes all of them
//! when no earlier one is unacknowledged, at most MAX_DISPATCH_PER_DAY a day. The session judges the
//! REQUIRED_TURNS costliest turns one by one and every signature group (`profile::groups`) for the rest.
//! Read-only: abnormal causes are handed to `crm-turn-retro`, which owns the fixes. A fresh install starts
//! SEED_MS back. `cpu-watchdog crm-costly-turns` prints the profiles on demand.
//!
//! State: `crm-costly-turn.json`; each incident's profiles: `crm-costly-turn/<id>.json`.
pub(in crate::telemetry) mod profile;
mod slow;
mod validate;
pub(in crate::telemetry) use validate::validate_ack;

use super::fetch;
use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::{collections::HashSet, path::Path, sync::mpsc::{self, Receiver, TryRecvError}};

/// Settled credits (enforced mode, every call of the turn) above which a turn is reviewed.
pub(in crate::telemetry) const THRESHOLD_CREDITS: f64 = 20.0;
pub(in crate::telemetry) const SAMPLE_SECS: f64 = 300.0;
const MAX_DISPATCH_PER_DAY: usize = 4;
/// Turns judged one by one (the costliest); the rest are judged by signature group.
const REQUIRED_TURNS: usize = 8;
/// Profiles kept waiting at most; older ones are dropped and counted.
const MAX_PENDING: usize = 400;
const RETRY_SECS: f64 = 600.0;
const DAY_MS: f64 = 86_400_000.0;
const SEED_MS: f64 = 6.0 * 3_600_000.0;
/// A turn row is stored when the turn ends; reads reach this far behind the cursor so a long turn is still seen.
const OVERLAP_MS: f64 = 3_600_000.0;
const SEEN_MS: f64 = 3.0 * DAY_MS;

type Sample = Result<(f64, Vec<String>, Vec<Value>), String>;

#[derive(Default)]
pub(in crate::telemetry) struct CrmCostlyTurn {
    loaded: bool,
    last_sample: f64,
    pub(super) cursor_ms: f64,
    pub(super) seen: Map<String, Value>,
    pub(super) pending: Vec<Value>,
    dropped: usize,
    pub(super) dispatches: Vec<Value>,
    error: Option<String>,
    worker: Option<Receiver<Sample>>,
}

impl CrmCostlyTurn {
    pub(in crate::telemetry) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
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
            let seen: HashSet<String> = self.seen.keys().cloned().collect();
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::Builder::new().name("watchdog-crm-costly".into()).spawn(move || {
                let _ = sender.send(sample(from, to, &seen));
            })?;
            self.worker = Some(receiver);
        }
        if !self.pending.is_empty() && self.may_open(home, now * 1000.0) { changed = true; self.escalate(home, now * 1000.0)?; }
        if changed { self.write(home, now) } else { Ok(()) }
    }

    fn load(&mut self, home: &Path, now: f64) {
        self.loaded = true;
        match super::super::read(&root(home).join("crm-costly-turn.json")) {
            Some(state) => {
                self.cursor_ms = state["cursor_ms"].as_f64().unwrap_or(now * 1000.0);
                self.seen = state["seen"].as_object().cloned().unwrap_or_default();
                self.pending = state["pending"].as_array().cloned().unwrap_or_default();
                self.dropped = state["dropped"].as_u64().unwrap_or(0) as usize;
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

    /// Marks the sample's turn ids seen, queues their profiles and opens a session when allowed.
    pub(super) fn absorb(&mut self, home: &Path, at: f64, sample: Sample) -> Result<()> {
        let (to, ids, profiles) = match sample {
            Err(error) => { crate::log(&format!("crm costly turn: {error}")); self.error = Some(error); return Ok(()); }
            Ok(sample) => sample,
        };
        self.error = None;
        for id in ids { self.seen.entry(id).or_insert(json!(at)); }
        self.seen.retain(|_, first| first.as_f64().is_some_and(|t| at - t < SEEN_MS));
        self.cursor_ms = self.cursor_ms.max(to);
        let mut queued: HashSet<String> = self.pending.iter().filter_map(|p| p["key"].as_str().map(str::to_string)).collect();
        self.pending.extend(profiles.into_iter().filter(|p| p["key"].as_str().is_some_and(|k| queued.insert(k.to_string()))));
        if self.pending.len() > MAX_PENDING {
            let over = self.pending.len() - MAX_PENDING;
            self.pending.drain(..over);
            self.dropped += over;
        }
        if !self.pending.is_empty() && self.may_open(home, at) { self.escalate(home, at)?; }
        Ok(())
    }

    pub(super) fn escalate(&mut self, home: &Path, at: f64) -> Result<Option<String>> {
        let turns = self.pending.clone();
        let review = review(&turns);
        let summary = summary(&turns);
        let keys: Vec<&str> = turns.iter().filter_map(|t| t["key"].as_str()).collect();
        let id = super::super::diagnostics::incident(home, &format!("crm_costly_turn_found:{}", keys.join(",")), json!({
            "trigger": "crm_costly_turn_found", "threshold_credits": THRESHOLD_CREDITS, "summary": summary, "groups": review["groups_detail"],
            "turns": turns, "dropped_before": self.dropped,
            "previous": self.dispatches.iter().rev().filter(|d| d["started"] == true).take(5).map(|d| json!({"incident": d["incident"], "summary": d["summary"],
                "report": root(home).join("analysis").join(d["incident"].as_str().unwrap_or("")).join("report.json")})).collect::<Vec<_>>(),
            "slow_accounts": review["slow_accounts"],
            "requirements": {"crm_costly_review": {"turns": review["turns"], "groups": review["groups"], "threshold_credits": THRESHOLD_CREDITS,
                "slow_accounts": review["slow_accounts"].as_array().map(|a| a.iter().map(|x| x["account"].clone()).collect::<Vec<_>>()), "slow_first_text_ms": slow::SLOW_FIRST_TEXT_MS}}}))?;
        let saved = root(home).join("crm-costly-turn").join(format!("{id}.json"));
        std::fs::create_dir_all(saved.parent().context("profiles directory")?)?;
        write_json(&saved, &json!({"incident": id, "turns": turns}))?;
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, &format!("看门狗：高积分或慢首字轮次复盘 {} 轮", turns.len()), &prompt(home, &id, &summary))
        };
        let started = result.is_ok();
        self.dispatches.push(json!({"incident": id, "at_ms": at, "started": started, "summary": summary,
            "agent": result.as_ref().ok(), "error": result.as_ref().err().map(|e| format!("{e:#}"))}));
        if !started { return Ok(None); }
        self.pending.clear();
        self.dropped = 0;
        if !cfg!(test) {
            crate::dispatch::notify("看门狗：高积分或慢首字轮次", &format!("{} 轮待复盘（最高 {} 积分），疑似异常 {} 轮，缓存失效 {} 轮，首字超过 {} 秒 {} 轮，已开复盘会话",
                turns.len(), summary["max_credits"], summary["suspect"], summary["cache_invalidated"], slow::SLOW_FIRST_TEXT_MS / 1000.0, summary["slow"]));
        }
        Ok(Some(id))
    }

    fn write(&self, home: &Path, now: f64) -> Result<()> {
        write_json(&root(home).join("crm-costly-turn.json"), &json!({"at_ms": now * 1000.0, "error": self.error,
            "running": self.worker.is_some(), "cursor_ms": self.cursor_ms, "pending": self.pending, "pending_count": self.pending.len(),
            "dropped": self.dropped, "seen": self.seen, "open_incident": self.open_incident(home), "dispatches": self.dispatches,
            "threshold": {"credits": THRESHOLD_CREDITS, "sample_s": SAMPLE_SECS, "max_dispatch_per_day": MAX_DISPATCH_PER_DAY,
                "required_turns": REQUIRED_TURNS, "slow_first_text_ms": slow::SLOW_FIRST_TEXT_MS, "overlap_s": OVERLAP_MS / 1000.0, "cache_loss_share": profile::CACHE_LOSS_SHARE}}))
    }
}

/// What the session must judge: the REQUIRED_TURNS costliest turns and every signature group.
pub(super) fn review(turns: &[Value]) -> Value {
    let mut ranked: Vec<&Value> = turns.iter().collect();
    ranked.sort_by(|a, b| b["credits"].as_f64().unwrap_or(0.0).total_cmp(&a["credits"].as_f64().unwrap_or(0.0)));
    let costly: Vec<Value> = turns.iter().filter(|t| t["credits"].as_f64().unwrap_or(0.0) > THRESHOLD_CREDITS).cloned().collect();
    let groups = profile::groups(&costly);
    json!({"turns": ranked.iter().filter(|t| t["credits"].as_f64().unwrap_or(0.0) > THRESHOLD_CREDITS).take(REQUIRED_TURNS).map(|t| t["key"].clone()).collect::<Vec<_>>(),
        "groups": groups.iter().map(|g| g["signature"].clone()).collect::<Vec<_>>(), "groups_detail": groups, "slow_accounts": slow::accounts(turns)})
}

pub(super) fn summary(turns: &[Value]) -> Value {
    let credits: Vec<f64> = turns.iter().filter_map(|t| t["credits"].as_f64()).collect();
    let count = |f: &dyn Fn(&Value) -> bool| turns.iter().filter(|t| f(t)).count();
    let threads: HashSet<&str> = turns.iter().filter_map(|t| t["thread"].as_str()).collect();
    json!({"turns": turns.len(), "threads": threads.len(), "credits": super::analyze::round(credits.iter().sum(), 1),
        "max_credits": credits.iter().cloned().fold(0.0, f64::max), "suspect": count(&|t| t["signals"].as_array().is_some_and(|s| !s.is_empty())),
        "cache_invalidated": count(&|t| t["cache"]["invalidated"] == true), "slow": count(&|t| t["slow"].is_object())})
}

fn sample(from: f64, to: f64, seen: &HashSet<String>) -> Sample {
    let rows = fetch::query_as("agent", &fetch::costly_sql(from, to, THRESHOLD_CREDITS, slow::SLOW_FIRST_TEXT_MS)).map_err(|e| format!("{e:#}"))?;
    let fresh: Vec<(String, f64)> = rows.iter().filter_map(|r| Some((r["turn"].as_str()?.to_string(), fetch::number(&r["at_ms"])?)))
        .filter(|(id, _)| !seen.contains(id)).collect();
    let Some(earliest) = fresh.iter().map(|(_, at)| *at).reduce(f64::min) else { return Ok((to, Vec::new(), Vec::new())) };
    let window = fetch::window(earliest - 1.0, to).map_err(|e| format!("{e:#}"))?;
    let ids: Vec<String> = fresh.into_iter().map(|(id, _)| id).collect();
    let keep: HashSet<String> = ids.iter().cloned().collect();
    Ok((to, ids, profile::profiles(&window, &keep)))
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
pub(super) fn prompt(home: &Path, id: &str, summary: &Value) -> String {
    format!("cpu-watchdog incident {id}: Mars CRM production chat turns settled more than {THRESHOLD_CREDITS} credits each; {summary}. {header} \
        Incident: {s}/incidents/{id}.json (turns[]: per-turn credits by role/agent/model, steps, tokens, token-priced credits and residual, main-step cache chain with cause and lost tokens, silent waste classes, tool failures, signals and signature; groups[]; requirements.crm_costly_review); profiles also in {s}/crm-costly-turn/{id}.json; `cpu-watchdog crm-costly-turns --incident {id}` prints them. \
        For each required turn and each required signature group decide normal, abnormal or inconclusive and whether the prompt cache was invalidated (cause from cache-optimization's causeCatalog), from the turn's own calls read-only on canary (mars-locate <thread> --show context, mars canary sql --role=agent, round-optimization's analyze-rounds.ts), never from the pre-verdict alone. \
        For each account in requirements.crm_costly_review.slow_accounts (its turns waited {slow} s or more for the first text; turns[].slow splits that wait into provider first-chunk waits, generation and host time with a pre-cause) decide the cause of the person's waits in report.json slow[] {{account, cause: provider_wait|serial_rounds|slow_generation|tool_wait|mixed, evidence, reason, action: crm-turn-retro, needs_founder or none with why}} and a 首字等待 section in report.md. \
        Read-only on production: no code, prompt or data change in this session; hand each abnormal cause with a backend fix to the crm-turn-retro route as its handoff and mark model, cache-policy or price questions needs_founder. \
        Publish the encrypted Chinese report page and give its link, write {s}/analysis/{id}/report.md in Chinese and report.json, then run cpu-watchdog monitor-ack {id}.",
        header = crate::dispatch::route_header("crm-costly-turn"), s = root(home).display(), slow = slow::SLOW_FIRST_TEXT_MS / 1000.0)
}

const USAGE: &str = "usage: cpu-watchdog crm-costly-turns [--hours N | --incident <id>]";

/// `crm-costly-turns`: the costly turns of the last N hours (default 24) profiled now, or an incident's saved profiles.
pub(in crate::telemetry) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let mut hours = 24.0;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--hours" => hours = it.next().context(USAGE)?.parse().context("--hours needs a number")?,
            "--incident" => {
                let path = root(home).join("crm-costly-turn").join(format!("{}.json", it.next().context(USAGE)?));
                let saved: Value = serde_json::from_slice(&std::fs::read(&path).with_context(|| format!("read {}", path.display()))?)?;
                println!("{}", serde_json::to_string_pretty(&saved)?);
                return Ok(());
            }
            other => anyhow::bail!("{USAGE} (unknown {other})"),
        }
    }
    let now = crate::unix_now() * 1000.0;
    let (_, _, turns) = sample(now - hours * 3_600_000.0, now, &HashSet::new()).map_err(anyhow::Error::msg)?;
    let review = review(&turns);
    println!("{}", serde_json::to_string_pretty(&json!({"hours": hours, "threshold_credits": THRESHOLD_CREDITS, "summary": summary(&turns),
        "groups": review["groups_detail"], "turns": turns}))?);
    Ok(())
}

#[cfg(test)]
#[path = "costly/tests.rs"]
mod tests;
