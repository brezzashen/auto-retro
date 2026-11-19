//! Release queue watch (founder, 2026-10-25): scripts move the release queue, agents judge only
//! what scripts cannot settle — the watchdog pattern applied to `deliver`.
//!
//! 1. Every SAMPLE_SECS the release board (`~/.local/state/mars-deliver/releases/`) becomes one
//!    snapshot, `~/.local/state/mars-deliver/queue.json` (`snapshot.rs`): per environment each
//!    unfinished run's position, phase, wait, source commit, the agents queued beside it and the
//!    paths it shares with them. `deliver` prints its run's `lines[<id>]` to the waiting agent
//!    whenever it changes, so every publishing agent sees the queue live.
//! 2. `triggers.rs` reads the snapshot for collisions (T9), priority inversion (T8: a Q1 waiting on a
//!    holder pid that `blocker.rs` resolves to a younger Q2 whose holder started after the Q1 began waiting), abandoned or
//!    silent runs (T4), long waits (T11), repeated failures (T5), Docker restarts under beta
//!    runs (T1), runs into a frozen environment (T10) and contaminated canary candidates (T6: a
//!    `deliver canary` refused by `deliver candidate-check`, or a running one carrying a commit whose
//!    owner recorded a failed acceptance). A freeze (`freeze.json`, written by the
//!    release coordinator) admits only its allowed commits; no trigger but T4 abandoned acts on those. A new signature opens one incident and one judging agent through the shared
//!    analysis queue: one open at a time, DAILY_JUDGES per 24 h, a signature again only after
//!    REPEAT_SECS. The agent decides within the release-queue skill's authority, tells the
//!    affected agents in their Orca tabs, publishes a report and acknowledges the incident.
//!
//! State: `release-watch.json` (health, dispatches, seen signatures); every wait that ends is one line
//! of `mars-deliver/queue-waits.jsonl`, the measured queue time the release retro reads.
mod blocker;
mod deploys;
mod git;
pub(super) mod snapshot;
mod triggers;
#[cfg(test)]
mod tests;

use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{collections::{BTreeMap, HashMap}, path::{Path, PathBuf}};

pub(super) const SAMPLE_SECS: f64 = 30.0;
const DAILY_JUDGES: usize = 6;
const REPEAT_SECS: f64 = 6.0 * 3600.0;
const DAY_SECS: f64 = 86_400.0;
const SKILL: &str = "/path/to/workspace/.claude/skills/release-queue";

#[derive(Default)]
pub(super) struct ReleaseWatch {
    loaded: bool,
    last_sample: f64,
    waiting_since: HashMap<String, f64>,
    seen: BTreeMap<String, f64>,
    dispatches: Vec<Value>,
    docker_seen_ms: f64,
    git: git::RepoGit,
}

fn deliver_dir(home: &Path) -> PathBuf {
    home.join(".local/state/mars-deliver")
}

impl ReleaseWatch {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded { self.load(home, now); }
        if now - self.last_sample < SAMPLE_SECS { return Ok(()); }
        self.last_sample = now;
        let board = snapshot::board(&deliver_dir(home).join("releases"));
        let busy = board.iter().any(|e| matches!(snapshot::live_state(e), "running" | "abandoned"));
        let tabs = if busy { git::orca_tabs() } else { Default::default() };
        let images = super::read(&deliver_dir(home).join("image-map.json")).unwrap_or(Value::Null);
        // Written by the release coordinator: per environment the reason and the only allowed source commits.
        let freeze = super::read(&deliver_dir(home).join("freeze.json")).unwrap_or(Value::Null);
        let before = self.waiting_since.clone();
        let mut queue = snapshot::build(&board, &self.git, &tabs, &images, &freeze, now * 1000.0, &mut self.waiting_since);
        if blocker::wanted(&queue) { blocker::resolve(&mut queue, &board, &blocker::processes(now * 1000.0)); }
        record_waits(&deliver_dir(home).join("queue-waits.jsonl"), &board, &before, &self.waiting_since, now * 1000.0);
        write_json(&deliver_dir(home).join("queue.json"), &queue)?;
        let docker = super::read(&root(home).join("docker-watch.json"))
            .map(|d| d["actions_ms"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect::<Vec<_>>()).unwrap_or_default();
        let found = triggers::detect(&queue, &board, &self.git, &docker, self.docker_seen_ms, now * 1000.0);
        self.docker_seen_ms = docker.iter().copied().fold(self.docker_seen_ms, f64::max);
        let open = self.open_incident(home);
        let fresh: Vec<&triggers::Trigger> = found.iter()
            .filter(|t| self.seen.get(&t.signature).is_none_or(|at| now - at >= REPEAT_SECS)).collect();
        let today = self.dispatches.iter().filter(|d| d["started"] == true && d["at_s"].as_f64().is_some_and(|at| now - at < DAY_SECS)).count();
        if open.is_none() && today < DAILY_JUDGES {
            if let Some(first) = fresh.first() {
                let related: Vec<&triggers::Trigger> = fresh.iter().copied().filter(|t| t.affected.iter().any(|a| first.affected.contains(a))).collect();
                self.judge(home, now, &queue, &board, &tabs, &related);
            }
        }
        self.seen.retain(|_, at| now - *at < DAY_SECS);
        self.dispatches.retain(|d| d["at_s"].as_f64().is_some_and(|at| now - at < 7.0 * DAY_SECS));
        write_json(&root(home).join("release-watch.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "agents": queue["agents"], "active": queue["lines"].as_object().map_or(0, |l| l.len()),
            "triggers": found.iter().map(|t| json!({"code": t.code, "signature": t.signature, "summary": t.summary})).collect::<Vec<_>>(),
            "open_incident": self.open_incident(home), "seen": self.seen, "dispatches": self.dispatches, "docker_seen_ms": self.docker_seen_ms,
            "queue_file": deliver_dir(home).join("queue.json"),
            "threshold": {"sample_s": SAMPLE_SECS, "daily_judges": DAILY_JUDGES, "repeat_s": REPEAT_SECS,
                "wait_q1_s": triggers::WAIT_Q1_S, "wait_q2_s": triggers::WAIT_Q2_S, "stall_s": triggers::STALL_S}}))
    }

    fn load(&mut self, home: &Path, now: f64) {
        self.loaded = true;
        let Some(state) = super::read(&root(home).join("release-watch.json")) else {
            // A fresh install must not judge every Docker restart of the past day.
            self.docker_seen_ms = now * 1000.0;
            return;
        };
        self.seen = serde_json::from_value(state["seen"].clone()).unwrap_or_default();
        self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
        self.docker_seen_ms = state["docker_seen_ms"].as_f64().unwrap_or(now * 1000.0);
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.iter().rev().filter(|d| d["started"] == true).filter_map(|d| d["incident"].as_str())
            .find(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists()).map(str::to_string)
    }

    fn judge(&mut self, home: &Path, now: f64, queue: &Value, board: &[Value], tabs: &snapshot::Tabs, related: &[&triggers::Trigger]) {
        let ids: Vec<String> = related.iter().flat_map(|t| t.affected.clone()).fold(Vec::new(), |mut v, id| { if !v.contains(&id) { v.push(id) } v });
        let mut affected = triggers::affected(queue, board, &ids);
        // Finished runs come from the board without a resolved tab; queued ones already carry it.
        for release in affected.iter_mut().filter(|r| r["creator"]["tab"].is_null()) {
            let tab = release["creator"]["orcaTab"].as_str().and_then(|t| tabs.get(t)).cloned().unwrap_or(Value::Null);
            release["creator"]["tab"] = tab;
        }
        let signatures: Vec<&str> = related.iter().map(|t| t.signature.as_str()).collect();
        let result = super::diagnostics::incident(home, &format!("release-queue:{}", signatures.join("|")), json!({
            "trigger": "release_queue", "codes": related.iter().map(|t| t.code).collect::<Vec<_>>(),
            "signals": related.iter().map(|t| json!({"code": t.code, "signature": t.signature, "summary": t.summary})).collect::<Vec<_>>(),
            "affected": affected, "queue_file": deliver_dir(home).join("queue.json"), "queue_at_detection": queue,
        })).and_then(|id| {
            let started = if cfg!(test) { json!({"mode": "test"}) } else { crate::dispatch::start_session(home, &id,
                &format!("发布队列：{}", related[0].code), &prompt(home, &id, related))? };
            Ok((id, started))
        });
        for t in related { self.seen.insert(t.signature.clone(), now); }
        self.dispatches.push(match result {
            Ok((id, agent)) => {
                if !cfg!(test) { crate::dispatch::notify("发布队列：开判断", &related[0].summary); }
                json!({"incident": id, "at_s": now, "started": true, "agent": agent, "signatures": signatures})
            }
            Err(error) => json!({"at_s": now, "started": false, "error": format!("{error:#}"), "signatures": signatures}),
        });
    }
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
fn prompt(home: &Path, id: &str, related: &[&triggers::Trigger]) -> String {
    let s = root(home);
    let s = s.display();
    let signals: Vec<String> = related.iter().map(|t| format!("{} {}", t.code, t.summary)).collect();
    format!("Release-queue incident {id}: {signals}. You are its judging agent. Read {SKILL}/SKILL.md and {SKILL}/references/escalation.md (rows {codes}) and decide only within the skill's Authority; nothing else is yours to change. \
        Evidence: {s}/incidents/{id}.json holds the signals, the affected runs (purpose, queue, creator session; creator.tab is its Orca tab title and terminal handle) and the queue at detection; the live queue is {queue} (refreshed every 30 s) and `bun run deliver status` in Mars_CRM. Reproduce the signal once before deciding. \
        Tell each affected agent your decision and its next action in its own tab (`~/.local/bin/session-send <handle> \"[Release queue judge {id}] ...\"`, which retries refused input and prints a delivered receipt); that message is the feedback the queue owes them. A decision beyond authority becomes a question for the user in the report, and the incident stays unacknowledged. {recall} \
        Publish an encrypted report (report skill, group infra, slug release-queue-{code}-<YYYYMMDD-HHMM>), write {s}/analysis/{id}/report.md in Chinese and report.json with trigger, verdict, decision, affected [{{id, owner, next_action, told}}], report_url and verification.passed, then run cpu-watchdog monitor-ack {id}.",
        signals = signals.join("; "), codes = related.iter().map(|t| t.code).collect::<Vec<_>>().join(", "), code = related[0].code.to_lowercase(),
        queue = deliver_dir(home).join("queue.json").display(), recall = crate::log_recall::instruction())
}

/// Appends one line per wait that ended this sample (the run left `waiting`, finished or died), so
/// the release retro reads measured queue time instead of guessing it from untimed log lines.
fn record_waits(path: &Path, board: &[Value], before: &HashMap<String, f64>, after: &HashMap<String, f64>, now_ms: f64) {
    let mut text = String::new();
    for (id, since) in before.iter().filter(|(id, _)| !after.contains_key(*id)) {
        let run = board.iter().find(|r| r["id"].as_str() == Some(id.as_str()));
        let line = run.and_then(|r| r["progress"]["line"].as_str()).unwrap_or_default();
        text.push_str(&json!({"id": id, "target": run.map(|r| r["target"].clone()),
            "env": run.and_then(|r| r["target"].as_str()).map(snapshot::environment), "queue": run.map(|r| r["queue"].clone()),
            "since_ms": since, "ended_ms": now_ms, "waited_s": ((now_ms - since) / 1000.0).round(),
            "last_line": line.chars().take(160).collect::<String>()}).to_string());
        text.push('\n');
    }
    if text.is_empty() { return; }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = std::io::Write::write_all(&mut file, text.as_bytes());
    }
}
