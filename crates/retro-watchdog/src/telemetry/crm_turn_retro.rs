//! CRM turn retro (founder, 2026-11-11): after every THRESHOLD new Mars CRM production chat turns,
//! open one session that finds the wasted consumption in them (model steps the answer did not need,
//! failed and repeated tool calls, tokens resent per extra step), fixes the top causes in the backend
//! and ships them to production, so Web and iOS users gain without a client release.
//! `conversation_retro.rs` counts developer agent sessions; this route counts product chat turns.
//!
//! Every SAMPLE_SECS a worker counts the user chat turns stored past `baseline_ms` (`fetch.rs`,
//! read-only `mars canary sql`; the demo account and reminder turns are excluded). At THRESHOLD it
//! reads the window (at most MAX_WINDOW_MS back), `analyze.rs` classifies every step before the first
//! text, and one `crm_turn_count_reached` incident opens a session on route `crm-turn-retro`; the
//! baseline then moves to the window's last turn. None opens while the last is unacknowledged and at
//! most MAX_DISPATCH_PER_DAY a day, so a backlog joins the next window. A fresh install starts at its
//! install time. `cpu-watchdog crm-turn-stats` prints the same analysis on demand.
//!
//! State: `crm-turn-retro.json`; each incident's turn rows: `crm-turn-retro/<id>.json`.
mod analyze;
mod cache;
mod cost;
pub(super) mod fetch;
mod prices;
mod probes;
pub(super) mod costly;
mod validate;
pub(super) use validate::validate_ack;

use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::{path::Path, sync::mpsc::{self, Receiver, TryRecvError}};

pub(super) const SAMPLE_SECS: f64 = 900.0;
pub(super) const THRESHOLD: usize = 100;
const MAX_DISPATCH_PER_DAY: usize = 4;
const RETRY_SECS: f64 = 600.0;
const DAY_MS: f64 = 86_400_000.0;
const MAX_WINDOW_MS: f64 = 72.0 * 3_600_000.0;
/// Waste classes a retro must judge: the top ones by wait.
const REQUIRED_CLASSES: usize = 3;
/// Cache causes a retro must judge while the hit rate is under `cache::TARGET`: every cause holding at least
/// MATERIAL_LOSS_SHARE of the lost tokens, at least MIN and at most MAX of them.
const MIN_CACHE_CAUSES: usize = 3;
const MAX_CACHE_CAUSES: usize = 6;
const MATERIAL_LOSS_SHARE: f64 = 0.05;
/// Cost lines a retro must judge.
const REQUIRED_COST_LINES: usize = 3;

/// One worker result: the count past the baseline, the newest turn's time, the rows skipped as invalid jsonb
/// and, at the threshold, the window's rows.
type Sample = Result<(usize, f64, usize, Option<Vec<Value>>), String>;

#[derive(Default)]
pub(super) struct CrmTurnRetro {
    loaded: bool,
    last_sample: f64,
    pub(super) baseline_ms: f64,
    window_count: usize,
    /// Telemetry rows of the window left out because their content is not valid jsonb (`fetch::VALID`).
    skipped: usize,
    error: Option<String>,
    pub(super) dispatches: Vec<Value>,
    worker: Option<Receiver<Sample>>,
}

impl CrmTurnRetro {
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
            let (from, to, open) = ((now * 1000.0 - MAX_WINDOW_MS).max(self.baseline_ms), now * 1000.0, self.may_open(home, now * 1000.0));
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::Builder::new().name("watchdog-crm-turns".into()).spawn(move || {
                let _ = sender.send(sample(from, to, open));
            })?;
            self.worker = Some(receiver);
        }
        if changed { self.write(home, now) } else { Ok(()) }
    }

    fn load(&mut self, home: &Path, now: f64) {
        self.loaded = true;
        match super::read(&root(home).join("crm-turn-retro.json")) {
            Some(state) => {
                self.baseline_ms = state["baseline_ms"].as_f64().unwrap_or(now * 1000.0);
                self.window_count = state["window_count"].as_u64().unwrap_or(0) as usize;
                self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
            }
            // A fresh install must not review the whole stored history at once.
            None => self.baseline_ms = now * 1000.0,
        }
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.iter().rev().find(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    /// Whether a window reaching the threshold may open a session at `at` (ms).
    pub(super) fn may_open(&self, home: &Path, at: f64) -> bool {
        let failed_recently = self.dispatches.last().is_some_and(|d| d["started"] != true && at - d["at_ms"].as_f64().unwrap_or(0.0) < RETRY_SECS * 1000.0);
        let today = self.dispatches.iter().filter(|d| d["started"] == true && d["at_ms"].as_f64().is_some_and(|t| at - t < DAY_MS)).count();
        self.open_incident(home).is_none() && !failed_recently && today < MAX_DISPATCH_PER_DAY
    }

    pub(super) fn absorb(&mut self, home: &Path, at: f64, sample: Sample) -> Result<()> {
        match sample {
            Err(error) => { crate::log(&format!("crm turn retro: {error}")); self.error = Some(error); }
            Ok((count, _, skipped, rows)) => {
                self.error = None;
                self.window_count = count;
                if skipped > 0 && skipped != self.skipped { crate::log(&format!("crm turn retro: skipped {skipped} row(s) whose content is not valid jsonb")); }
                self.skipped = skipped;
                if let Some(rows) = rows.filter(|_| count >= THRESHOLD && self.may_open(home, at)) { self.escalate(home, at, &rows)?; }
            }
        }
        Ok(())
    }

    pub(super) fn escalate(&mut self, home: &Path, at: f64, rows: &[Value]) -> Result<Option<String>> {
        let turns = analyze::turns(rows);
        if turns.len() < THRESHOLD { return Ok(None); }
        let stats = analyze::summarize(&turns, &analyze::prior_turns(rows));
        let classes: Vec<Value> = stats["ranked"].as_array().into_iter().flatten()
            .filter(|c| c["waste_candidate"] == true && c["wait_ms"].as_f64().unwrap_or(0.0) > 0.0).take(REQUIRED_CLASSES).map(|c| c["class"].clone()).collect();
        let last = turns.last().map_or(at, |t| t.at_ms);
        let summary = json!({"turns": turns.len(), "first_text_p50_ms": stats["first_text"]["p50_ms"], "steps_mean": stats["steps"]["mean"],
            "silent_step_share": stats["first_text"]["silent_step_share"], "classes": classes.clone(), "credits": stats["credits"]["total"],
            "cache_hit": stats["cache"]["hit"], "cache_ceiling": stats["cache"]["ceiling"], "list_usd": stats["cost"]["usd"]});
        let review = review(&stats, classes, &settled_cache_causes(home, &self.dispatches));
        let previous: Vec<Value> = self.dispatches.iter().rev().filter(|d| d["started"] == true).take(5)
            .map(|d| json!({"incident": d["incident"], "summary": d["summary"],
                "report": root(home).join("analysis").join(d["incident"].as_str().unwrap_or("")).join("report.json")})).collect();
        let id = super::diagnostics::incident(home, &format!("crm_turn_count_reached:{}", self.baseline_ms), json!({
            "trigger": "crm_turn_count_reached", "turns": turns.len(), "stats": stats, "previous": previous, "probes": probes::load(home),
            "requirements": {"crm_turn_review": review},
            "window": {"from_ms": turns.first().map(|t| t.at_ms), "to_ms": last, "baseline_ms": self.baseline_ms, "skipped_invalid_jsonb": self.skipped}}))?;
        let saved = root(home).join("crm-turn-retro").join(format!("{id}.json"));
        std::fs::create_dir_all(saved.parent().context("rows directory")?)?;
        write_json(&saved, &json!({"incident": id, "rows": rows}))?;
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, &format!("看门狗：CRM 对话复盘 {} 轮", turns.len()), &prompt(home, &id, &summary))
        };
        let started = result.is_ok();
        self.dispatches.push(json!({"incident": id, "at_ms": at, "started": started, "summary": summary,
            "agent": result.as_ref().ok(), "error": result.as_ref().err().map(|e| format!("{e:#}"))}));
        if !started { return Ok(None); }
        self.baseline_ms = last;
        self.window_count = 0;
        if !cfg!(test) {
            crate::dispatch::notify("看门狗：CRM 对话复盘", &format!("{} 轮生产对话，首字 p50 {} ms，无文字步骤占首字时间 {}，主模型缓存命中 {}，已开复盘会话",
                turns.len(), summary["first_text_p50_ms"], summary["silent_step_share"], summary["cache_hit"]));
        }
        Ok(Some(id))
    }

    fn write(&self, home: &Path, now: f64) -> Result<()> {
        write_json(&root(home).join("crm-turn-retro.json"), &json!({"at_ms": now * 1000.0, "error": self.error,
            "running": self.worker.is_some(), "baseline_ms": self.baseline_ms, "window_count": self.window_count, "skipped_invalid_jsonb": self.skipped,
            "open_incident": self.open_incident(home), "dispatches": self.dispatches,
            "threshold": {"turns": THRESHOLD, "sample_s": SAMPLE_SECS, "max_window_h": MAX_WINDOW_MS / 3_600_000.0, "max_dispatch_per_day": MAX_DISPATCH_PER_DAY}}))
    }
}

/// What the retro must judge: the ranked waste classes, the top recoverable cache causes while the main hit
/// rate is under `cache::TARGET` (every material one, so a large structural cause never crowds out the fixable
/// ones), and the costliest list-price lines. The target is never lowered (founder, 2026-11-12, keep 90% and
/// keep optimizing): a cause leaves the list only while its proposal waits on the founder (`needs_founder` in
/// the latest retro, `settled`); it stays in `stats.cache.ranked`.
pub(super) fn review(stats: &Value, classes: Vec<Value>, settled: &Map<String, Value>) -> Value {
    let cache = &stats["cache"];
    let causes: Vec<Value> = if cache["below_target"] == true {
        cache["ranked"].as_array().into_iter().flatten()
            .filter(|c| c["cause"].as_str().is_some_and(|c| !settled.contains_key(c))).enumerate()
            .take_while(|(i, c)| *i < MIN_CACHE_CAUSES || c["loss_share"].as_f64().unwrap_or(0.0) >= MATERIAL_LOSS_SHARE)
            .take(MAX_CACHE_CAUSES).map(|(_, c)| c["cause"].clone()).collect()
    } else { Vec::new() };
    let lines: Vec<Value> = stats["cost"]["lines"].as_array().into_iter().flatten().take(REQUIRED_COST_LINES).map(|l| l["line"].clone()).collect();
    json!({"classes": classes, "cache": {"target": cache["target"], "hit": cache["hit"], "ceiling": cache["ceiling"], "causes": causes, "settled": settled},
        "cost": {"lines": lines}})
}

/// Cache causes whose proposal waits on the founder: `needs_founder` in the latest retro with a report.
fn settled_cache_causes(home: &Path, dispatches: &[Value]) -> Map<String, Value> {
    let latest = dispatches.iter().rev().filter(|d| d["started"] == true).filter_map(|d| d["incident"].as_str())
        .find_map(|id| super::read(&root(home).join("analysis").join(id).join("report.json")).map(|r| (id.to_string(), r)));
    let Some((id, report)) = latest else { return Map::new() };
    report["cache"]["causes"].as_array().into_iter().flatten()
        .filter(|c| c["verdict"] == "needs_founder")
        .filter_map(|c| Some((c["cause"].as_str()?.to_string(), json!({"verdict": c["verdict"], "incident": id, "reason": c["reason"]}))))
        .collect()
}

fn sample(from: f64, to: f64, open: bool) -> Sample {
    let (count, newest, skipped) = fetch::count(from, to).map_err(|e| format!("{e:#}"))?;
    let rows = if count >= THRESHOLD && open { Some(fetch::window(from, to).map_err(|e| format!("{e:#}"))?) } else { None };
    Ok((count, newest, skipped, rows))
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
pub(super) fn prompt(home: &Path, id: &str, summary: &Value) -> String {
    format!("cpu-watchdog incident {id}: Mars CRM production stored {THRESHOLD}+ new chat turns; {summary}. {header} \
        Incident: {s}/incidents/{id}.json (stats: steps before first text, every silent step classified and ranked by user wait and credits, tool failures with error codes, silent tool sets, worst turns with thread ids; probes; previous retros); turn rows {s}/crm-turn-retro/{id}.json; `cpu-watchdog crm-turn-stats --incident {id}` recomputes it. \
        First run `cpu-watchdog crm-turn-stats --followup {id}` and judge every shipped probe. Then confirm the ranked waste classes on their worst threads with round-optimization's analyze-rounds.ts, \
        review the prompt cache (stats.cache: main-model weighted hit against the 90% target, its ceiling, loss by cause in list US$, worst threads) and while it is under target fix its top recoverable causes with cache-optimization's causeCatalog, \
        and the cost structure (stats.cost: list US$ by component and role/agent/model, settled credits, tool-fee residual, coming price bands) and cut its costliest avoidable lines; \
        fix the top one to three backend causes in the cheapest layer (tool result, crm route, processor, then prompt; all prompt edits in one deploy), never lowering a user-selected reasoning level and never needing a Web or iOS release, \
        measure before and after on beta and then Canary with no loss of answer quality, run mars canary promote after Canary acceptance (riding commits' owners checked) without asking, and register each fix as a probe in crm-turn-probes.json; \
        ask the founder only for a DESTRUCTIVE migration, data deletion or bulk write, Secret, IAM, Ingress or DNS change, and mark needs_founder (never ship) a new explicit cache, keep-warm ping, Observational Memory threshold, recall size, model view or model routing change. \
        Publish the encrypted Chinese report page and give its link, write {s}/analysis/{id}/report.md in Chinese and report.json, then run cpu-watchdog monitor-ack {id}.",
        header = crate::dispatch::route_header("crm-turn-retro"), s = root(home).display())
}

const USAGE: &str = "usage: cpu-watchdog crm-turn-stats [--hours N | --incident <id> | --turns <rows.json>] [--out <rows.json>] | --followup <incident>";

/// `crm-turn-stats`: the retro's analysis of the last N hours (default 24), of an incident's window or of saved rows.
pub(crate) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let (mut hours, mut source, mut out) = (24.0, None, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().cloned().with_context(|| format!("{name} needs a value; {USAGE}"));
        match arg.as_str() {
            "--hours" => hours = value("--hours")?.parse().context("--hours needs a number")?,
            "--turns" => source = Some(std::path::PathBuf::from(value("--turns")?)),
            "--incident" => source = Some(root(home).join("crm-turn-retro").join(format!("{}.json", value("--incident")?))),
            "--out" => out = Some(value("--out")?),
            "--followup" => return probes::cli(home, &value("--followup")?),
            other => anyhow::bail!("{USAGE} (unknown {other})"),
        }
    }
    let (rows, skipped) = match source {
        Some(path) => (fetch::saved(&path)?, None),
        None => {
            let (now, ms) = (crate::unix_now() * 1000.0, hours * 3_600_000.0);
            (fetch::window(now - ms, now)?, Some(fetch::count(now - ms, now)?.2))
        }
    };
    if let Some(out) = out { write_json(Path::new(&out), &json!({"rows": rows}))?; }
    let mut stats = analyze::summarize_rows(&rows);
    if let (Some(skipped), Some(map)) = (skipped, stats.as_object_mut()) { map.insert("skipped_invalid_jsonb".into(), json!(skipped)); }
    println!("{}", serde_json::to_string_pretty(&stats)?);
    Ok(())
}

#[cfg(test)]
#[path = "crm_turn_retro/tests.rs"]
mod tests;
#[cfg(test)]
#[path = "crm_turn_retro/tests_cost.rs"]
mod tests_cost;
