//! Release hard-limit watch (founder, 2026-10-31): one feature's release on a K8S cluster (`beta`
//! and `beta-web` on the local cluster, `canary` on GKE) takes 5-10 min and never more than
//! HARD_S (20 min). A run past HARD_S is fixed at once, not at the next 10-release retro.
//!
//! Every SAMPLE_SECS the board's TARGETS runs are timed: a live run from its start to now, a
//! finished one from start to end (only when it ended after `baseline_ms`, so a fresh install does
//! not replay history). Runs past HARD_S that no incident named yet open one
//! `release_over_hard_limit` incident and an urgent session on route `canary-retro`, queued ahead
//! of every other analysis. One is open at a time: runs that pass the limit meanwhile wait and
//! open the next incident together once it is acknowledged. At most MAX_PER_DAY a day.
//!
//! State: `release-over-hard.json`.
use super::analyze::{HARD_S, TARGET_S};
use crate::monitor::{root, write_json};
use crate::telemetry::{release_retro::stats, release_watch::snapshot};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

pub(super) const SAMPLE_SECS: f64 = 60.0;
pub(super) const TARGETS: [&str; 4] = ["beta", "beta-web", "beta-crm", "canary"];
const MAX_PER_DAY: usize = 8;
const DAY_MS: f64 = 86_400_000.0;
const KEEP_SEEN: usize = 200;
const KEEP_DISPATCHES: usize = 50;

#[derive(Default)]
pub(super) struct OverHard {
    pub(super) loaded: bool,
    pub(super) last_sample: f64,
    pub(super) baseline_ms: f64,
    pub(super) seen: Vec<String>,
    pub(super) dispatches: Vec<Value>,
}

/// TARGETS runs past HARD_S at `now_ms`, as (run, live state, seconds). A finished run counts only
/// when it ended after `since_ms`.
pub(super) fn over(board: &[Value], now_ms: f64, since_ms: f64) -> Vec<(Value, &'static str, f64)> {
    board.iter().filter(|r| r["target"].as_str().is_some_and(|t| TARGETS.contains(&t))).filter_map(|r| {
        let start = stats::ms(r["startedAt"].as_str())?;
        let state = snapshot::live_state(r);
        let end = match state { "running" | "detached" => now_ms, _ => stats::end_ms(r).filter(|e| *e > since_ms)? };
        let seconds = (end - start) / 1000.0;
        (seconds > HARD_S).then(|| (r.clone(), state, seconds))
    }).collect()
}

impl OverHard {
    pub(super) fn observe(&mut self, home: &Path, now: f64) {
        if let Err(error) = self.sample(home, now) {
            let _ = write_json(&root(home).join("release-over-hard.json"), &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}));
        }
    }

    fn sample(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded { self.load(home, now); }
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS { return Ok(()); }
        self.last_sample = now;
        let at = now * 1000.0;
        let board = snapshot::board(&home.join(".local/state/mars-deliver/releases"));
        let fresh: Vec<_> = over(&board, at, self.baseline_ms).into_iter()
            .filter(|(r, _, _)| !self.seen.iter().any(|s| r["id"] == s.as_str())).collect();
        let open = self.open_incident(home);
        let today = self.dispatches.iter().filter(|d| d["started"] == true && d["at_ms"].as_f64().is_some_and(|t| at - t < DAY_MS)).count();
        if !fresh.is_empty() && open.is_none() && today < MAX_PER_DAY { self.escalate(home, at, &fresh)?; }
        write_json(&root(home).join("release-over-hard.json"), &json!({"at_ms": at, "error": null, "baseline_ms": self.baseline_ms,
            "waiting": fresh.iter().filter(|(r, _, _)| !self.seen.iter().any(|s| r["id"] == s.as_str())).map(|(r, _, s)| json!({"id": r["id"], "s": s})).collect::<Vec<_>>(),
            "open_incident": self.open_incident(home), "seen": self.seen, "dispatches": self.dispatches,
            "threshold": {"targets": TARGETS, "target_s": TARGET_S, "hard_s": HARD_S, "sample_s": SAMPLE_SECS, "max_dispatch_per_day": MAX_PER_DAY}}))
    }

    fn load(&mut self, home: &Path, now: f64) {
        self.loaded = true;
        let state = crate::telemetry::read(&root(home).join("release-over-hard.json")).unwrap_or(Value::Null);
        // A fresh install must not open incidents for the board's whole history.
        self.baseline_ms = state["baseline_ms"].as_f64().unwrap_or(now * 1000.0);
        self.seen = state["seen"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.last().filter(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    fn escalate(&mut self, home: &Path, at: f64, fresh: &[(Value, &'static str, f64)]) -> Result<()> {
        let ids: Vec<String> = fresh.iter().filter_map(|(r, _, _)| r["id"].as_str().map(str::to_string)).collect();
        let runs: Vec<Value> = fresh.iter().map(|(r, state, s)| json!({"id": r["id"], "target": r["target"], "state": state, "seconds": s.round(),
            "queue": r["queue"], "purpose": r["purpose"], "source": r["source"], "log": r["log"], "creator": r["creator"],
            "failure": r["failure"], "progress": r["progress"]})).collect();
        // Finished Canary runs carry a receipt: the retro's per-segment analysis applies to them.
        let canary: Vec<(Value, &'static str, f64)> = fresh.iter().filter(|(r, state, _)| r["target"] == "canary" && !matches!(*state, "running" | "detached"))
            .filter_map(|(r, state, _)| Some((r.clone(), if *state == "finished" { "succeeded" } else { *state }, stats::end_ms(r)?))).collect();
        let stats = if canary.is_empty() { Value::Null } else { super::measure(home, &canary) };
        let id = crate::telemetry::diagnostics::incident(home, &format!("release_over_hard_limit:{}", ids.join(",")), json!({
            "trigger": "release_over_hard_limit", "runs": runs, "stats": stats,
            "requirements": {"canary_review": {"target_s": TARGET_S, "hard_s": HARD_S, "over_hard": ids}}}))?;
        let summary: Vec<String> = fresh.iter().map(|(r, state, s)| format!("{} {} {state} {:.0} min",
            r["target"].as_str().unwrap_or("?"), r["id"].as_str().unwrap_or("?"), s / 60.0)).collect();
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_urgent_session(home, &id, "看门狗：发布超 20 分钟", &prompt(home, &id, &summary.join("; ")))
        };
        let started = result.is_ok();
        self.dispatches.push(json!({"incident": id, "at_ms": at, "started": started, "runs": ids,
            "agent": result.as_ref().ok(), "error": result.as_ref().err().map(|e| format!("{e:#}"))}));
        let drop = self.dispatches.len().saturating_sub(KEEP_DISPATCHES);
        self.dispatches.drain(..drop);
        if !started { return Ok(()); }
        self.seen.extend(ids);
        let drop = self.seen.len().saturating_sub(KEEP_SEEN);
        self.seen.drain(..drop);
        if !cfg!(test) { crate::dispatch::notify("看门狗：发布超 20 分钟", &format!("{}，已开紧急修复会话", summary.join("；"))); }
        Ok(())
    }
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
fn prompt(home: &Path, id: &str, summary: &str) -> String {
    format!("cpu-watchdog incident {id}: release(s) past the {hard:.0} min hard limit: {summary}. {header} \
        Incident: {s}/incidents/{id}.json (runs with board state, log and creator; stats for finished Canary runs). \
        A K8S release (beta and beta-web on the local cluster, canary on GKE) should take {lo:.0}-{hi:.0} min and never over {hard:.0}; fix this now. \
        For a run still running, find where it is stuck (deliver status, its log, the release-queue skill) and unblock it within that skill's authority, telling its creator's Orca tab. \
        For every run find the cause (cache, build, upload, queue, preflight, rollout, error, other) with evidence from its log and receipt and fix the evidenced defect under its owning skill (mars-crm delivery, release-queue, Cloud Build cache); \
        answer the four checks (errors, cache not deployed correctly, queue problems, and ci: from checks.ci and ci_cost, what a check ahead of the release could have caught or taken off the release path: deliver refusing before it builds, the touched suites at push time, reusing a commit's green result, a CI lane; take the cheapest that works, never one that adds more release time than it saves, and state its release_cost_s); {publish}",
        hard = HARD_S / 60.0, lo = 5.0, hi = TARGET_S / 60.0, header = crate::dispatch::route_header("canary-retro"), publish = crate::telemetry::retro_series::instruction("canary", id, &root(home)), s = root(home).display())
}
