//! One performance trigger (founder, 2026-10-27). CPU surge (`surge.rs`), WindowServer render
//! stall (`telemetry/render_probe.rs`), WindowServer and Docker VM hot (`telemetry/desktop.rs`),
//! the 90%/300 s CPU monitor (`monitor.rs`) and every forced recovery that is not closed by script
//! (`recovery_retro.rs`: Docker crash, headless page, daemon, Storybook) and host memory exhaustion
//! (`telemetry/memory_guard.rs`, founder 2026-11-04; capped like the others) are conditions of one
//! trigger. They feed one `perf` session that lays out the host's bottlenecks across all of them,
//! instead of a surge, a lag and several recovery sessions for the same load.
//!
//! A condition arriving while the performance session is unacknowledged (and younger than
//! OPEN_MAX_MS) joins it. Otherwise an opening condition queues a new session unless one started
//! within MIN_GAP_MS or MAX_PER_DAY started in a rolling 24 h; join-only conditions never open one.
//! Deferred episodes are covered by the next session.
//!
//! CPU surge is the emergency exception (founder, 2026-11-03): the responder (`surge/act.rs`) has
//! already checked and healed abnormal processes, and its episode then opens a review session with
//! no MIN_GAP_MS and no MAX_PER_DAY. Surge sessions also do not count toward the other conditions'
//! gap and daily cap, which keep 4 per rolling 24 h. Sessions go through the shared admission
//! queue (`dispatch::start_session`), so they start after the episode. `validate` makes the
//! session account for every episode it covers before `monitor-ack`.
use crate::monitor::{root, write_json};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

const STATE: &str = "wake-perf.json";
const DAY_MS: f64 = 86_400_000.0;
/// An unacknowledged session older than this no longer absorbs new episodes.
const OPEN_MAX_MS: f64 = 12.0 * 3_600_000.0;
/// Episodes after a session closes wait for the next one instead of opening another at once.
const MIN_GAP_MS: f64 = 2.0 * 3_600_000.0;
/// Daily cap for every opening condition except CpuSurge.
pub(crate) const MAX_PER_DAY: usize = 4;
const KEEP_EPISODES: usize = 400;
const TITLE: &str = "看门狗：性能瓶颈整体分析";

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Condition { CpuSurge, CpuHot, RenderStall, WindowServerHot, DockerVmHot, Recovery, MemoryExhausted }

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Outcome { Queued, Joined, Deferred, Closed }

impl Condition {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Condition::CpuSurge => "cpu_surge", Condition::CpuHot => "cpu_hot_90",
            Condition::RenderStall => "windowserver_render_stall", Condition::WindowServerHot => "windowserver_hot",
            Condition::DockerVmHot => "docker_vm_hot", Condition::Recovery => "forced_recovery",
            Condition::MemoryExhausted => "memory_exhausted",
        }
    }
    /// The 90% monitor trails every surge and Docker VM load is analyzed with the host's; neither
    /// opens a session alone.
    fn opens(self) -> bool { !matches!(self, Condition::CpuHot | Condition::DockerVmHot) }
    /// The CPU surge is an emergency gate: after its self-heal it always gets a review session.
    fn capped(self) -> bool { self != Condition::CpuSurge }
}

impl Outcome {
    fn name(self) -> &'static str { match self { Outcome::Queued => "queued", Outcome::Joined => "joined", Outcome::Deferred => "deferred", Outcome::Closed => "closed" } }
}

/// The state, seeded once from the 2026-10-24 per-kind files so their sessions keep counting.
fn load(home: &Path) -> Value {
    let valid = |s: &Value| s["sessions"].is_array() && s["episodes"].is_array();
    if let Some(state) = crate::telemetry::read_state(&root(home).join(STATE)).filter(valid) { return state; }
    let mut sessions: Vec<Value> = ["surge", "lag"].iter()
        .filter_map(|kind| crate::telemetry::read_state(&root(home).join(format!("wake-{kind}.json"))).filter(valid))
        .flat_map(|s| s["sessions"].as_array().cloned().unwrap_or_default())
        .map(|mut s| { s["legacy"] = json!(true); s }).collect();
    sessions.sort_by(|a, b| a["at_ms"].as_f64().unwrap_or(0.0).total_cmp(&b["at_ms"].as_f64().unwrap_or(0.0)));
    json!({"sessions": sessions, "episodes": []})
}

fn acked(home: &Path, id: &str) -> bool { root(home).join("incidents").join(format!("{id}.ack")).exists() }

/// Records episode `id` and decides it: join the open session, queue a new one, or defer it to the
/// next. The same id is decided once; a repeated call returns the recorded outcome.
pub(crate) fn request(home: &Path, now_ms: f64, condition: Condition, id: &str, detail: &str) -> Result<Outcome> {
    let mut state = load(home);
    if let Some(seen) = state["episodes"].as_array().unwrap().iter().find(|e| e["incident"] == id) {
        return Ok(match seen["outcome"].as_str() { Some("queued") => Outcome::Queued, Some("joined") => Outcome::Joined, Some("closed") => Outcome::Closed, _ => Outcome::Deferred });
    }
    // Self-heals already ran in their own modules; compare the window with the merged cause list.
    let (found, closing) = crate::cause_tree::perf::compare(home, id, now_ms).unwrap_or_else(|error| {
        crate::log(&format!("perf attribution {id}: {error:#}"));
        (Value::Null, None)
    });
    let cause = json!({"explained_by": found["explained_by"], "dominant": found["dominant"], "share": found["classes"][0]["share"], "heal_failed": found["heal_failed"]});
    if let Some(node) = closing {
        crate::cause_tree::perf::close(home, id, condition.name(), &found, &node, now_ms)?;
        return record(home, state, now_ms, condition, id, detail, Outcome::Closed, Value::Null, json!("known cause closed by script"), cause);
    }
    let sessions = state["sessions"].as_array().unwrap();
    let started = |s: &Value| s["at_ms"].as_f64().unwrap_or(0.0);
    let open = sessions.iter().rev().filter(|s| now_ms - started(s) < OPEN_MAX_MS)
        .filter_map(|s| s["incident"].as_str()).find(|s| !acked(home, s)).map(str::to_string);
    // Gap and cap count only sessions opened by capped conditions; surge sessions are exempt.
    let capped: Vec<&Value> = sessions.iter().filter(|s| s["condition"] != Condition::CpuSurge.name()).collect();
    let today = capped.iter().filter(|s| now_ms - started(s) < DAY_MS).count();
    let last = capped.last().map(|s| (s["incident"].clone(), started(s)));
    let (outcome, session, reason) = if let Some(open) = open {
        (Outcome::Joined, json!(open), Value::Null)
    } else if !condition.opens() {
        (Outcome::Deferred, Value::Null, json!(format!("{} joins a performance session but never opens one", condition.name())))
    } else if condition.capped() && today >= MAX_PER_DAY {
        (Outcome::Deferred, Value::Null, json!(format!("{today} sessions in 24 h reached the limit of {MAX_PER_DAY}")))
    } else if let Some((previous, at)) = last.filter(|(_, at)| condition.capped() && now_ms - at < MIN_GAP_MS) {
        (Outcome::Deferred, Value::Null, json!(format!("session {} started {:.0} min ago; the next one covers this episode",
            previous.as_str().unwrap_or("?"), (now_ms - at) / 60_000.0)))
    } else {
        let agent = if cfg!(test) { json!({"mode": "test"}) } else { crate::dispatch::start_led_session(home, id, TITLE, &prompt(home, condition, id, detail), crate::dispatch::LEAD_FABLE)? };
        state["sessions"].as_array_mut().unwrap().push(json!({"incident": id, "condition": condition.name(), "at_ms": now_ms, "agent": agent}));
        // Deferred episodes of the last 24 h wait for this session; older ones are stale evidence.
        for episode in state["episodes"].as_array_mut().unwrap().iter_mut()
            .filter(|e| e["outcome"] == "deferred" && e["session"].is_null() && e["at_ms"].as_f64().is_some_and(|at| now_ms - at < DAY_MS)) {
            episode["session"] = json!(id);
        }
        (Outcome::Queued, json!(id), Value::Null)
    };
    let queued = outcome == Outcome::Queued;
    let outcome = record(home, state, now_ms, condition, id, detail, outcome, session, reason, cause)?;
    if queued && !cfg!(test) {
        crate::dispatch::notify(TITLE, &format!("{detail}；已排队一个性能分析会话，CPU 回落后启动，期间的 CPU、WindowServer 与恢复事件都并入它"));
    }
    Ok(outcome)
}

/// Appends the decided episode and saves the state.
#[allow(clippy::too_many_arguments)]
fn record(home: &Path, mut state: Value, now_ms: f64, condition: Condition, id: &str, detail: &str, outcome: Outcome,
    session: Value, reason: Value, cause: Value) -> Result<Outcome> {
    let episodes = state["episodes"].as_array_mut().unwrap();
    episodes.push(json!({"incident": id, "condition": condition.name(), "at_ms": now_ms, "detail": detail,
        "outcome": outcome.name(), "session": session, "reason": reason, "cause": cause}));
    let excess = episodes.len().saturating_sub(KEEP_EPISODES);
    episodes.drain(..excess);
    state["at_ms"] = json!(now_ms);
    state["limits"] = json!({"max_sessions_per_day": MAX_PER_DAY, "min_gap_ms": MIN_GAP_MS, "open_session_max_ms": OPEN_MAX_MS,
        "uncapped_conditions": [Condition::CpuSurge.name()]});
    write_json(&root(home).join(STATE), &state)?;
    Ok(outcome)
}

/// Requests a session for `id` and acknowledges a joined or deferred episode when `close` allows;
/// returns the outcome. Callers that must keep a failed recovery open pass `close = false`.
pub(crate) fn trigger(home: &Path, now_ms: f64, condition: Condition, id: &str, detail: &str, close: bool, extra: Value) -> Result<Outcome> {
    let outcome = request(home, now_ms, condition, id, detail)?;
    if outcome != Outcome::Queued && close && !acked(home, id) {
        let mut record = json!({"see": STATE});
        record.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
        ack(home, id, outcome, record)?;
    }
    Ok(outcome)
}

/// Acknowledges an incident a session will not analyze on its own, naming why.
pub(crate) fn ack(home: &Path, id: &str, outcome: Outcome, extra: Value) -> Result<()> {
    if outcome == Outcome::Closed { return Ok(()); } // The script closure wrote its own ack.
    let mut record = json!({"acknowledged_at_ms": crate::unix_now() * 1000.0, "kind": format!("wake_{}", outcome.name())});
    record.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
    write_json(&root(home).join("incidents").join(format!("{id}.ack")), &record)
}

/// monitor-ack gate for a performance session: report.json lists every episode it covers, maps the
/// bottlenecks and carries the optimization review. Legacy surge/lag sessions keep their own gates.
pub(crate) fn validate(home: &Path, id: &str) -> Result<()> {
    let state = load(home);
    let own = |s: &&Value| s["incident"] == id && s["legacy"] != true;
    if !state["sessions"].as_array().unwrap().iter().any(|s| own(&s)) { return Ok(()); }
    let report: Value = serde_json::from_slice(&std::fs::read(root(home).join("analysis").join(id).join("report.json"))
        .context("report.json missing")?)?;
    let listed: Vec<&str> = report["episodes"].as_array().into_iter().flatten()
        .filter(|e| e["finding"].as_str().is_some_and(|f| !f.trim().is_empty())).filter_map(|e| e["incident"].as_str()).collect();
    let missing: Vec<&str> = state["episodes"].as_array().unwrap().iter()
        .filter(|e| e["session"] == id || e["incident"] == id).filter_map(|e| e["incident"].as_str())
        .filter(|e| !listed.contains(e)).collect();
    ensure!(missing.is_empty(), "report.json episodes[] needs {{incident, finding}} for every episode of {STATE} this session covers; missing {}", missing.join(", "));
    // One cause list for every condition: each episode names a node of the merged tree under its trigger.
    if let Ok(tree) = crate::cause_tree::load(home) {
        for episode in report["episodes"].as_array().into_iter().flatten() {
            let Some(incident) = episode["incident"].as_str() else { continue };
            let event = crate::telemetry::read_state(&root(home).join(format!("incidents/{incident}.json"))).unwrap_or(Value::Null);
            let Some(trigger) = crate::cause_tree::tree_trigger(&tree, &event) else { continue };
            let node = episode["node"].as_str().unwrap_or_default();
            ensure!(crate::cause_tree::node(&tree, node).is_some() && crate::cause_tree::under(&tree, node, &trigger),
                "episodes[] {incident}: node {node:?} must be a cause-tree.json node under {trigger}; add the node first when it is new");
        }
    }
    let map = report["bottleneck_map"].as_array().filter(|m| !m.is_empty())
        .context("report.json needs bottleneck_map[]: {resource, workloads[], evidence, action} across all covered episodes")?;
    for entry in map {
        ensure!(entry["resource"].as_str().is_some_and(|r| !r.trim().is_empty())
            && entry["workloads"].as_array().is_some_and(|w| !w.is_empty())
            && entry["evidence"].as_str().is_some_and(|e| !e.trim().is_empty())
            && entry["action"].as_str().is_some_and(|a| !a.trim().is_empty()),
            "every bottleneck_map entry needs resource, workloads[], evidence and action");
    }
    crate::recovery_retro::review(&report)
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
fn prompt(home: &Path, condition: Condition, id: &str, detail: &str) -> String {
    let s = root(home);
    let s = s.display();
    format!("cpu-watchdog performance incident {id} ({condition}): {detail}. {header} \
        Incident: {s}/incidents/{id}.json. Every episode this session covers is in {s}/{STATE} (session or incident {id}): CPU surges, the 90% monitor, WindowServer stalls and hot windows, Docker VM load, forced recoveries (Docker crash, headless page, daemon, Storybook) and memory exhaustion (memory-guard.json, memory-guard/ relief records); more may join while you work, so re-read it before writing the report. \
        Each episode's analysis/<episode>/perf-attribution.json already compares its window with the merged cause list (perf_class nodes of ~/.codex/skills/watchdog/references/cause-tree.json: class shares of host CPU, dominant class, explaining node) and lists the self-heals that ran first and their outcomes; start from it, work on what the known nodes and heals leave open, and add each new finding as a node. \
        Treat them as one load picture, not separate incidents: build the bottleneck layout of the whole window across CPU cores, memory, GPU/WindowServer, disk and the Docker VM (cpu-windows frames with by_name, cpu-build-gate status and slot-events.jsonl, render-probe, desktop.json, port-census, cpu-cap status, each episode's incident and responder report under analysis/<episode>/), then optimize the bottleneck that explains most episodes. \
        Build throughput comes first, measured by real build times, while the desktop must stay usable; fix only evidenced causes under their owning skill. \
        report.json: episodes[] {{incident, condition, finding, node}} for every covered episode, bottleneck_map[] {{resource, workloads, evidence, action}}, optimization_review with resource_contention, error_prevention, change_or_no_change_reason, validation and remaining_risk, cause_tree {{node, new_nodes or no_new_node_reason}} when monitor-ack asks for it, and verification.passed=true only after checks. \
        Write {s}/analysis/{id}/report.md in Chinese and report.json, publish the encrypted report page and give its link, then run cpu-watchdog monitor-ack {id}.",
        condition = condition.name(), header = crate::dispatch::route_header("perf"))
}

#[cfg(test)]
#[path = "wake_tests.rs"]
mod tests;
