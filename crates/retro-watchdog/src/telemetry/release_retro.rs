//! Release retro watch (founder, 2026-10-27): after every THRESHOLD finished releases on the
//! release board — beta, beta-web, Canary, iOS and Mac together, since one queue, one build gate
//! and one Docker VM serve them all — open one session that reviews how the queue performed.
//!
//! Every SAMPLE_SECS the board's finished runs (succeeded, failed, a detached upload that ended, or
//! abandoned) are counted past `baseline_ms`. At THRESHOLD the newest THRESHOLD runs form one window
//! (older ones past a backlog are skipped and counted in `window.skipped`);
//! `stats.rs` measures it with the evidence around it: per-target outcomes and durations, the
//! `cpu-build-gate` waits each log prints, the queue waits release_watch measured
//! (`mars-deliver/queue-waits.jsonl`), gated builds, TestFlight publishes on and off the board,
//! Docker restarts, the judges release_watch opened (script-only rate), and the release commands
//! agents typed in that time with their instruction errors (`commands.rs`, founder 2026-10-28). Failed
//! background outputs are kept every hour (`outputs.rs`), so a recomputed window matches its incident. That opens one
//! `release_count_reached` incident and one session on route `release-retro`; the baseline then
//! moves to the window's last run. None opens while the last is unacknowledged and at most
//! MAX_DISPATCH_PER_DAY a day. A fresh install starts at its install time.
//! `cpu-watchdog release-stats [--last N] [--since <time>]` prints the same statistics on demand.
//!
//! State: `release-retro.json`.
pub(crate) mod classify;
pub(crate) mod commands;
pub(crate) mod outputs;
pub(crate) mod stats;

use super::release_watch::snapshot;
use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub(super) const SAMPLE_SECS: f64 = 300.0;
pub(super) const THRESHOLD: usize = 100;
const MAX_DISPATCH_PER_DAY: usize = 3;
const RETRY_SECS: f64 = 600.0;
const DAY_MS: f64 = 86_400_000.0;
/// Log bytes read per run; gate and queue lines repeat, so the head and tail carry them.
const LOG_CAP: u64 = 4 << 20;

#[derive(Default)]
pub(super) struct ReleaseRetro {
    loaded: bool,
    last_sample: f64,
    last_warm: f64,
    baseline_ms: f64,
    dispatches: Vec<Value>,
}

fn deliver(home: &Path) -> PathBuf { home.join(".local/state/mars-deliver") }

/// When a run joins a window: its end, or for a detached TestFlight publish the later time hygiene
/// closed it, since the board reports it `detached` until then. Retro 81781503530293676883: an iOS
/// run finished 09:57:39, closed 10:22:56; the window cut at 10:09 skipped it and the next window
/// starts after its end, so no window counted it.
fn counted_ms(run: &Value) -> Option<f64> {
    let end = stats::end_ms(run)?;
    let closed = run["backgroundPid"].as_u64().and_then(|_| stats::ms(run["closed"]["at"].as_str()));
    Some(closed.map_or(end, |c| c.max(end)))
}

/// Finished runs oldest first, each with its outcome and the time it joins a window.
pub(crate) fn finished(home: &Path) -> Vec<(Value, &'static str, f64)> {
    let mut out: Vec<(Value, &'static str, f64)> = snapshot::board(&deliver(home).join("releases")).into_iter().filter_map(|run| {
        // A detached publish whose uploader exited counts once hygiene has judged and closed it.
        if run["state"] == "detached" && run["closed"]["at"].is_null() { return None; }
        let outcome = match snapshot::live_state(&run) {
            "running" | "detached" => return None,
            "abandoned" => "abandoned",
            // `deliver hygiene` closes an abandoned run as failed and records why in `closed`.
            "failed" if run["closed"]["outcome"] == "abandoned" => "abandoned",
            "failed" => "failed",
            _ => "succeeded",
        };
        let end = counted_ms(&run)?;
        Some((run, outcome, end))
    }).collect();
    out.sort_by(|a, b| a.2.total_cmp(&b.2));
    out
}

pub(super) fn log_text(path: Option<&str>) -> String {
    let Some(Ok(file)) = path.map(std::fs::File::open) else { return String::new() };
    let mut bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut std::io::Read::take(file, LOG_CAP), &mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

pub(super) fn jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn within(at: Option<f64>, from: f64, to: f64) -> bool { at.is_some_and(|t| t >= from && t <= to) }

/// Statistics of `runs` (oldest first) with every outside source cut to their time range. The range
/// starts at the earliest run start but never before `floor`, the end of the run before the window,
/// so windows partition commands, builds and waits: retro 85869459281164395933 held a beta-web run
/// SIGTERMed on 10-27 and closed by hygiene 43 h later, which pulled 31 h of earlier windows back in.
pub(crate) fn measure(home: &Path, runs: &[(Value, &'static str, f64)], floor: f64) -> Value {
    let from = runs.iter().filter_map(|(r, _, _)| stats::ms(r["startedAt"].as_str())).fold(f64::INFINITY, f64::min).max(floor);
    let to = runs.last().map_or(0.0, |r| r.2);
    let rows: Vec<Value> = runs.iter().map(|(r, outcome, _)| stats::run_row(r, outcome, &log_text(r["log"].as_str()))).collect();
    let gate = home.join(".local/state/mars-cpu-build-gate");
    let state = |name: &str| super::read(&root(home).join(name)).unwrap_or(Value::Null);
    let board_ios_starts = snapshot::board(&deliver(home).join("releases")).iter()
        .filter(|r| matches!(r["target"].as_str(), Some("ios" | "mac"))).filter_map(|r| r["startedAt"].as_str().map(str::to_string)).collect();
    let evidence = stats::Evidence {
        slot_events: jsonl(&gate.join("slot-events.jsonl")).into_iter().filter(|e| within(e["at"].as_f64(), from, to)).collect(),
        docker_actions_ms: state("docker-watch.json")["actions_ms"].as_array().into_iter().flatten().filter_map(Value::as_f64)
            .filter(|t| within(Some(*t), from, to)).collect(),
        judges: state("release-watch.json")["dispatches"].as_array().into_iter().flatten()
            .filter(|d| within(d["at_s"].as_f64().map(|s| s * 1000.0), from, to)).cloned().collect(),
        testflight: jsonl(&home.join(".local/state/mars-testflight/performance.jsonl")).into_iter()
            .filter(|p| within(stats::ms(p["startedAt"].as_str()), from, to)).collect(),
        waits: jsonl(&deliver(home).join("queue-waits.jsonl")).into_iter().filter(|w| within(w["ended_ms"].as_f64(), from, to)).collect(),
        board_ios_starts,
    };
    let mut out = stats::summarize(&rows, &evidence, from, to);
    out["commands"] = commands::review(home, from, to);
    out["runs"] = json!(rows);
    out
}

impl ReleaseRetro {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded { self.load(home, now); }
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS { return Ok(()); }
        self.last_sample = now;
        let fresh: Vec<_> = finished(home).into_iter().filter(|r| r.2 > self.baseline_ms).collect();
        // Keep failed background outputs before /private/tmp purges them (`outputs.rs`).
        if now - self.last_warm >= outputs::WARM_SECS { self.last_warm = now; let _ = commands::review(home, self.baseline_ms, now * 1000.0); }
        let open = self.escalate(home, now * 1000.0, &fresh)?;
        write_json(&root(home).join("release-retro.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "baseline_ms": self.baseline_ms, "window_count": fresh.len(), "open_incident": open, "dispatches": self.dispatches,
            "threshold": {"releases": THRESHOLD, "sample_s": SAMPLE_SECS, "max_dispatch_per_day": MAX_DISPATCH_PER_DAY}}))
    }

    fn load(&mut self, home: &Path, now: f64) {
        self.loaded = true;
        match super::read(&root(home).join("release-retro.json")) {
            Some(state) => {
                self.baseline_ms = state["baseline_ms"].as_f64().unwrap_or(now * 1000.0);
                self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
            }
            // A fresh install must not review the board's whole history at once.
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
        // After a backlog review the newest THRESHOLD runs and skip the older ones, as canary_retro
        // does (founder, 2026-11-02, retro 85869459281164395933: 657 runs behind, its window was
        // 4 days old and other sessions had already fixed both of its defects).
        let skipped = fresh.len() - THRESHOLD;
        let window = &fresh[skipped..];
        let floor = skipped.checked_sub(1).map_or(self.baseline_ms, |i| fresh[i].2);
        let measured = measure(home, window, floor);
        let previous: Vec<Value> = self.dispatches.iter().rev().filter(|d| d["started"] == true).take(5)
            .map(|d| json!({"incident": d["incident"], "summary": d["summary"]})).collect();
        let summary = json!({"script_only_rate": measured["scripted"]["script_only_rate"], "judges": measured["scripted"]["judges"],
            "gate_wait_p90_s": measured["waits"]["gate"]["per_run_s"]["p90"], "instruction_errors": measured["commands"]["instruction_errors"], "flag_retries": measured["commands"]["flag_retries"]["n"], "failed": measured["failures"].as_object().map_or(0, |f| f.values().filter_map(|v| v["n"].as_u64()).sum::<u64>())});
        let id = super::diagnostics::incident(home, &format!("release_count_reached:{}", self.baseline_ms), json!({
            "trigger": "release_count_reached", "releases": THRESHOLD, "stats": measured, "previous": previous,
            "window": {"from_ms": measured["window"]["from_ms"], "to_ms": window[THRESHOLD - 1].2, "baseline_ms": self.baseline_ms, "skipped": skipped}}))?;
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, &format!("看门狗：发布复盘 {THRESHOLD} 次"), &prompt(home, &id, &summary, (floor + 1.0, window[THRESHOLD - 1].2)))
        };
        let started = result.is_ok();
        self.dispatches.push(json!({"incident": id, "at_ms": at, "started": started, "summary": summary,
            "agent": result.as_ref().ok(), "error": result.as_ref().err().map(|e| format!("{e:#}"))}));
        if !started { return Ok(None); }
        self.baseline_ms = window[THRESHOLD - 1].2;
        if !cfg!(test) {
            crate::dispatch::notify("看门狗：发布复盘", &format!("{THRESHOLD} 次发布，脚本闭环率 {}%，指令错误 {} 次，已开复盘会话", summary["script_only_rate"], summary["instruction_errors"]));
        }
        Ok(Some(id))
    }
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
fn prompt(home: &Path, id: &str, summary: &Value, range: (f64, f64)) -> String {
    format!("cpu-watchdog incident {id}: {THRESHOLD} releases finished on the release board (beta, beta-web, Canary, iOS, Mac); {summary}. {header} \
        Incident: {s}/incidents/{id}.json (stats: per-target outcomes and durations, gate and queue waits, failures by class, script-only rate, coverage, gated builds, TestFlight publishes, Docker restarts; previous release retros); \
        `cpu-watchdog release-stats --since {since} --until {until}` recomputes exactly this window. {recall} Verify the figures against raw logs, turn each measured wait, repeated failure class and coverage gap into a scripted fix under its owning skill with before/after numbers; \
        review stats.commands (release commands agents typed: instruction errors by class, flag retries, ungated or off-board lints) and make each repeated instruction error impossible: a deterministic command, a guard refusal before anything compiles, or the skill example that taught it, \
        publish the report page and give its link, write {s}/analysis/{id}/report.md in Chinese and report.json, then run cpu-watchdog monitor-ack {id}.",
        header = crate::dispatch::route_header("release-retro"), since = range.0 as u64, until = range.1 as u64, recall = crate::retro_recall::instruction(id), s = root(home).display())
}

/// `release-stats [--last N] [--since <time>] [--until <time>]`: the retro's statistics for the
/// newest finished runs, or for the runs that finished in a time range (ISO or epoch ms, inclusive).
pub(crate) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let (mut last, mut since, mut until) = (THRESHOLD, None, None);
    let time = |v: Option<&String>| v.and_then(|s| s.parse::<f64>().ok().or_else(|| stats::ms(Some(s))));
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--last" => last = it.next().context("--last needs a count")?.parse().context("--last needs a count")?,
            "--since" => since = Some(time(it.next()).context("--since needs an ISO time or epoch ms")?),
            "--until" => until = Some(time(it.next()).context("--until needs an ISO time or epoch ms")?),
            other => anyhow::bail!("usage: cpu-watchdog release-stats [--last N] [--since <time>] [--until <time>] (unknown {other})"),
        }
    }
    let all: Vec<_> = finished(home).into_iter().filter(|r| until.is_none_or(|t| r.2 <= t)).collect();
    let skip = match since { Some(t) => all.iter().take_while(|r| r.2 < t).count(), None => all.len().saturating_sub(last) };
    // The window's floor is the end of the run before it, as the watch's baseline is.
    let floor = skip.checked_sub(1).map_or(f64::NEG_INFINITY, |i| all[i].2);
    println!("{}", serde_json::to_string_pretty(&measure(home, &all[skip..], floor))?);
    Ok(())
}

#[cfg(test)]
#[path = "release_retro/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "release_retro/tests_network.rs"]
mod tests_network;
