//! Canary retro watch (founder, 2026-10-28): after every THRESHOLD finished Canary runs on the
//! release board, open one session that reviews those releases against the release-time targets
//! (about 5-10 min, hard limit 20 min; founder, 2026-10-31) and answers three questions: did a release hit an error, was a
//! cache not deployed correctly, did the queue misbehave. A normal release is a rolling update of
//! the changed images only, so each run past the hard limit must name its cause.
//!
//! Every SAMPLE_SECS the board's finished `canary` runs are counted past `baseline_ms`. At THRESHOLD
//! the newest THRESHOLD form one window (older ones past a backlog are skipped and counted); `analyze.rs` reads each run's receipt and log, splits its
//! time into critical-path segments and runs the checks (errors, cache, queue, and since 2026-11-02 ci). That opens one
//! `canary_release_count_reached` incident and one session on route `canary-retro`; the baseline
//! then moves to the window's last run. None opens while the last is unacknowledged and at most
//! MAX_DISPATCH_PER_DAY a day. A fresh install starts at its install time.
//! `cpu-watchdog canary-stats [--last N]` prints the same analysis on demand. A single run past
//! the hard limit does not wait for a window: `over_hard.rs` opens its own urgent session.
//!
//! State: `canary-retro.json`.
pub(crate) mod analyze;
mod ci;
mod over_hard;
mod retry;
mod validate;
pub(super) use validate::validate_ack;

use super::release_retro::{finished, jsonl, log_text};
use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::Path;

pub(super) const SAMPLE_SECS: f64 = 300.0;
pub(super) const THRESHOLD: usize = 10;
const MAX_DISPATCH_PER_DAY: usize = 4;
const RETRY_SECS: f64 = 600.0;
const DAY_MS: f64 = 86_400_000.0;

#[derive(Default)]
pub(super) struct CanaryRetro {
    loaded: bool,
    last_sample: f64,
    baseline_ms: f64,
    dispatches: Vec<Value>,
    over_hard: over_hard::OverHard,
}

fn canary_runs(home: &Path) -> Vec<(Value, &'static str, f64)> {
    finished(home).into_iter().filter(|r| r.0["target"] == "canary").collect()
}

/// The analysis of `runs` (oldest first).
pub(crate) fn measure(home: &Path, runs: &[(Value, &'static str, f64)]) -> Value {
    let live = analyze::live_tags(&home.join(".local/state/mars-crm/canary-releases"));
    let rows: Vec<Value> = runs.iter().map(|(run, outcome, _)| {
        let log = log_text(run["log"].as_str());
        let receipt = analyze::receipt_path(&log).and_then(|p| super::read(Path::new(&p))).unwrap_or(Value::Null);
        let started = super::release_retro::stats::ms(receipt["startedAt"].as_str()).unwrap_or(f64::INFINITY);
        let before = live.iter().rev().find(|(at, _)| *at < started).map_or(Value::Null, |l| l.1.clone());
        analyze::run_row(run, outcome, &log, &receipt, &before)
    }).collect();
    let from = runs.iter().filter_map(|(r, _, _)| super::release_retro::stats::ms(r["startedAt"].as_str())).fold(f64::INFINITY, f64::min);
    let to = runs.last().map_or(0.0, |r| r.2);
    let judges = super::read(&root(home).join("release-watch.json")).map_or(Vec::new(), |s| s["dispatches"].as_array().cloned().unwrap_or_default());
    let waits = jsonl(&home.join(".local/state/mars-deliver/queue-waits.jsonl"));
    let mut out = analyze::summarize(&rows, &judges, &waits, from, to);
    out["ci_runner"] = ci::runner(home, from, to);
    out["runs"] = json!(rows);
    out
}

impl CanaryRetro {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        self.over_hard.observe(home, now);
        if !self.loaded { self.load(home, now); }
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS { return Ok(()); }
        self.last_sample = now;
        let fresh: Vec<_> = canary_runs(home).into_iter().filter(|r| r.2 > self.baseline_ms).collect();
        let open = self.escalate(home, now * 1000.0, &fresh)?;
        write_json(&root(home).join("canary-retro.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "baseline_ms": self.baseline_ms, "window_count": fresh.len(), "open_incident": open, "dispatches": self.dispatches,
            "threshold": {"releases": THRESHOLD, "target_s": analyze::TARGET_S, "hard_s": analyze::HARD_S,
                "sample_s": SAMPLE_SECS, "max_dispatch_per_day": MAX_DISPATCH_PER_DAY}}))
    }

    fn load(&mut self, home: &Path, now: f64) {
        self.loaded = true;
        match super::read(&root(home).join("canary-retro.json")) {
            Some(state) => {
                self.baseline_ms = state["baseline_ms"].as_f64().unwrap_or(now * 1000.0);
                self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
            }
            // A fresh install must not review the board's whole Canary history at once.
            None => self.baseline_ms = now * 1000.0,
        }
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.last().filter(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    fn escalate(&mut self, home: &Path, at: f64, fresh: &[(Value, &'static str, f64)]) -> Result<Option<String>> {
        let open = self.open_incident(home);
        let failed_recently = self.dispatches.last().is_some_and(|d| d["started"] != true && at - d["at_ms"].as_f64().unwrap_or(0.0) < RETRY_SECS * 1000.0);
        let today = self.dispatches.iter().filter(|d| d["started"] == true && d["at_ms"].as_f64().is_some_and(|t| at - t < DAY_MS)).count();
        if fresh.len() < THRESHOLD || open.is_some() || failed_recently || today >= MAX_DISPATCH_PER_DAY { return Ok(open); }
        // After a backlog (a retro left unacknowledged, the daily cap) review the newest THRESHOLD
        // runs and skip the older ones: their causes are history by now, and every one past the
        // hard limit already had its own over_hard session. Reviewing the oldest first reopened
        // 3-day-old windows whose fixes had shipped (incident 814538033793746427555, 111 behind).
        let skipped = fresh.len() - THRESHOLD;
        let window = &fresh[skipped..];
        let measured = measure(home, window);
        let previous: Vec<Value> = self.dispatches.iter().rev().filter(|d| d["started"] == true).take(5)
            .map(|d| json!({"incident": d["incident"], "summary": d["summary"]})).collect();
        let checks = &measured["checks"];
        let summary = json!({"p50_s": measured["seconds"]["succeeded"]["p50"], "max_s": measured["seconds"]["all"]["max"],
            "verdicts": measured["verdicts"], "over_hard_causes": measured["over_hard_causes"], "cache_hit_rate": measured["cache_hit_rate"],
            "error_runs": checks["errors"]["runs"], "cache_runs": checks["cache"]["runs"], "queue_runs": checks["queue"]["runs"],
            "ci_runs": checks["ci"]["runs"], "ci_s": measured["ci_cost"]["total_s"]});
        let over_hard: Vec<Value> = measured["over_hard"].as_array().into_iter().flatten().map(|r| r["id"].clone()).collect();
        let id = super::diagnostics::incident(home, &format!("canary_release_count_reached:{}", self.baseline_ms), json!({
            "trigger": "canary_release_count_reached", "releases": THRESHOLD, "stats": measured, "previous": previous,
            "requirements": {"canary_review": {"target_s": analyze::TARGET_S, "hard_s": analyze::HARD_S, "over_hard": over_hard}},
            "window": {"from_ms": measured["window"]["from_ms"], "to_ms": window[THRESHOLD - 1].2, "baseline_ms": self.baseline_ms, "skipped": skipped}}))?;
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, &format!("看门狗：Canary 复盘 {THRESHOLD} 次"), &prompt(home, &id, &summary))
        };
        let started = result.is_ok();
        self.dispatches.push(json!({"incident": id, "at_ms": at, "started": started, "summary": summary,
            "agent": result.as_ref().ok(), "error": result.as_ref().err().map(|e| format!("{e:#}"))}));
        if !started { return Ok(None); }
        self.baseline_ms = window[THRESHOLD - 1].2;
        if !cfg!(test) {
            crate::dispatch::notify("看门狗：Canary 复盘", &format!("{THRESHOLD} 次 Canary 发布，p50 {} s，超 {:.0} 分钟 {} 次，可前移 CI {} s，已开复盘会话",
                summary["p50_s"], analyze::HARD_S / 60.0, summary["verdicts"]["over_hard"], summary["ci_s"]));
        }
        Ok(Some(id))
    }
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
fn prompt(home: &Path, id: &str, summary: &Value) -> String {
    format!("cpu-watchdog incident {id}: {THRESHOLD} Canary releases finished; {summary}. {header} \
        Incident: {s}/incidents/{id}.json (stats: per-run receipts split into start, prepare, rollout_wait, rollout and finish segments; preflight, Cloud Build queue and run, rust-build; \
        verdict against the {target:.0} s target and the {hard:.0} s hard limit; checks.errors, checks.cache, checks.queue, checks.ci and ci_cost (release seconds a pre-release check could save), ci_runner (crm-ci verdicts on mastra heads in the window: red, flaky = false positive); previous Canary retros); `cpu-watchdog canary-stats --last {THRESHOLD}` recomputes it. \
        Verify the figures against the raw receipts and logs; for every run over {hard:.0} s find the cause (cache, build, upload, queue, preflight, rollout, error) with evidence; \
        answer the four checks (errors, cache not deployed correctly, queue problems, and ci: from checks.ci and ci_cost, what a check ahead of the release could have caught or taken off the release path: deliver refusing before it builds, the touched suites at push time, reusing a commit's green result, a CI lane; take the cheapest that works, never one that adds more release time than it saves, and state its release_cost_s) and fix each evidenced defect under its owning skill so a release is a rolling update of the changed images in about 5-10 minutes; \
        {publish}",
        header = crate::dispatch::route_header("canary-retro"), publish = super::retro_series::instruction("canary", id, &root(home)), s = root(home).display(), target = analyze::TARGET_S, hard = analyze::HARD_S)
}

/// `canary-stats [--last N]`: the retro's analysis of the newest finished Canary runs.
pub(crate) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let mut last = THRESHOLD;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--last" => last = it.next().context("--last needs a count")?.parse().context("--last needs a count")?,
            other => anyhow::bail!("usage: cpu-watchdog canary-stats [--last N] (unknown {other})"),
        }
    }
    let all = canary_runs(home);
    let skip = all.len().saturating_sub(last);
    println!("{}", serde_json::to_string_pretty(&measure(home, &all[skip..]))?);
    Ok(())
}

#[cfg(test)]
#[path = "canary_retro/tests.rs"]
mod tests;
#[cfg(test)]
#[path = "canary_retro/tests_ride.rs"]
mod tests_ride;
#[cfg(test)]
#[path = "canary_retro/tests_retry.rs"]
mod tests_retry;
