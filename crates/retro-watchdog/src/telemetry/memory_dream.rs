//! Memory dream watch (founder, 2026-11-07, after Anthropic's "The components of agent memory"):
//! the shared agent memory store `~/.agent-memory` (override `AGENT_MEMORY_DIR`, a local Git
//! repository) decays between sessions: links to merged notes break, cited files move, the same
//! lesson is written twice, project facts go stale, and agents keep asking memory for facts no
//! note or indexed corpus holds.
//!
//! 1. Once a day, in Singapore time (UTC+8, no daylight saving; never US time), the dream slot
//!    opens: SLOTS 06:00 for WINDOW_MIN (founder 2026-11-18: once a day; 06:00 and 18:00 before).
//! 2. Inside a slot window that has not dreamed yet, open one `memory_dream_due` incident and one
//!    session on route `memory-dream` that consolidates the store offline: wrong facts out, recall
//!    gaps recorded as notes, hub notes for ongoing automation refreshed, recall measured before and
//!    after (`agent-memory eval`).
//! 3. While the previous dream is unacknowledged the slot waits (state `blocked`); a slot whose
//!    window ends first is skipped. A failed dispatch is retried after RETRY_SECS inside the window.
//! 4. Since 2026-11-10 (incident `contract: 2`) the dream audits the sessions of its `window` against the
//!    notes and skills they were given (`sessions.rs` picks them, `audit.rs` gates the answers).
//! 5. The store commits since the last acknowledged dream (`memory: dream` commits excluded) are
//!    counted for the prompt; they no longer gate the dream.
//!
//! State: `memory-dream.json` (base commit, pending count, dispatches, blocked slot).
mod audit;
pub(super) mod sessions;
mod transcript;
pub(super) mod validate;

use crate::monitor::{root, write_json};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub(super) const SAMPLE_SECS: f64 = 600.0;
/// Dream slots, minutes after Singapore midnight (founder, 2026-11-18: once a day at 06:00; the
/// 18:00 evening slot of 2026-11-07 was dropped).
pub(super) const SLOTS: [(&str, f64); 1] = [("morning", 6.0 * 60.0)];
pub(super) const WINDOW_MIN: f64 = 60.0;
const RETRY_SECS: f64 = 600.0;
/// Commits the dream session itself makes do not count as new store changes.
const DREAM_SUBJECT: &str = "^memory: dream";
/// Sessions a dream audits: since the previous dream, a day on the first run, never more than two.
const WINDOW_MS: (f64, f64) = (86_400_000.0, 172_800_000.0);

#[derive(Default)]
pub(super) struct MemoryDream {
    loaded: bool,
    last_sample: f64,
    base: String,
    pending: u64,
    dispatches: Vec<Value>,
    blocked: Option<Value>,
}

pub(super) fn store(home: &Path) -> PathBuf {
    std::env::var_os("AGENT_MEMORY_DIR").map(PathBuf::from).unwrap_or_else(|| home.join(".agent-memory"))
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output()?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Store commits after `base` up to HEAD, dream commits excluded.
fn count_since(dir: &Path, base: &str) -> Result<u64> {
    let range = format!("{base}..HEAD");
    Ok(git(dir, &["rev-list", "--count", "--invert-grep", "--grep", DREAM_SUBJECT, &range])?.parse().unwrap_or(0))
}

/// The slot whose window holds `at_ms`: (`<singapore day number>:<slot>`, slot name).
pub(super) fn slot_at(at_ms: f64) -> Option<(String, &'static str)> {
    let (day, minutes) = super::quota_watch::singapore(at_ms);
    SLOTS.iter().find(|(_, start)| minutes >= *start && minutes < start + WINDOW_MIN).map(|(name, _)| (format!("{day}:{name}"), *name))
}

impl MemoryDream {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        let dir = store(home);
        if !dir.join(".git").exists() {
            return Ok(());
        }
        if !self.loaded {
            self.load(home, &dir)?;
        }
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS {
            return Ok(());
        }
        self.last_sample = now;
        self.close_acked(home, &dir)?;
        self.pending = match count_since(&dir, &self.base) {
            Ok(n) => n,
            // History rewritten under the recorded base: start a fresh window at HEAD.
            Err(_) => {
                self.base = git(&dir, &["rev-parse", "HEAD"])?;
                0
            }
        };
        let open = self.escalate(home, &dir, now * 1000.0)?;
        self.write(home, now, open)
    }

    fn load(&mut self, home: &Path, dir: &Path) -> Result<()> {
        self.loaded = true;
        if let Some(state) = super::read(&root(home).join("memory-dream.json")) {
            if let Some(base) = state["base"].as_str() {
                self.base = base.to_string();
                self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
                return Ok(());
            }
        }
        self.base = git(dir, &["rev-parse", "HEAD"])?;
        Ok(())
    }

    /// Once the last dream is acknowledged, its own store commits (merges and rewrites made with
    /// `agent-memory write`, whatever their subject) are behind it: the count restarts at HEAD.
    fn close_acked(&mut self, home: &Path, dir: &Path) -> Result<()> {
        let Some(last) = self.dispatches.iter_mut().rev().find(|d| d["started"] == true).filter(|d| d["closed_head"].is_null()) else { return Ok(()) };
        let id = last["incident"].as_str().unwrap_or_default().to_string();
        if !root(home).join("incidents").join(format!("{id}.ack")).exists() {
            return Ok(());
        }
        let head = git(dir, &["rev-parse", "HEAD"])?;
        last["closed_head"] = json!(head);
        self.base = head;
        Ok(())
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.iter().rev().find(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    fn escalate(&mut self, home: &Path, dir: &Path, at: f64) -> Result<Option<String>> {
        let open = self.open_incident(home);
        let Some((slot, name)) = slot_at(at) else {
            self.blocked = None;
            return Ok(open);
        };
        let tries: Vec<&Value> = self.dispatches.iter().filter(|d| d["slot"] == slot.as_str()).collect();
        if tries.iter().any(|d| d["started"] == true)
            || tries.last().is_some_and(|d| at - d["at_ms"].as_f64().unwrap_or(0.0) < RETRY_SECS * 1000.0) {
            return Ok(open);
        }
        if let Some(id) = &open {
            self.blocked = Some(json!({"slot": slot, "by": id, "at_ms": at}));
            return Ok(open);
        }
        self.blocked = None;
        let head = git(dir, &["rev-parse", "HEAD"])?;
        let previous: Vec<Value> = self.dispatches.iter().rev().filter(|d| d["started"] == true).take(5)
            .map(|d| json!({"incident": d["incident"], "slot": d["slot"], "commits": d["commits"],
                "report": root(home).join("analysis").join(d["incident"].as_str().unwrap_or("")).join("report.json")})).collect();
        let since_ms = self.dispatches.iter().rev().find(|d| d["started"] == true).and_then(|d| d["at_ms"].as_f64());
        let id = super::diagnostics::incident(home, &format!("memory_dream_due:{slot}"), json!({
            "trigger": "memory_dream_due", "contract": 2, "window": {"from_ms": since_ms.unwrap_or(at - WINDOW_MS.0).max(at - WINDOW_MS.1), "to_ms": at}, "slot": name, "slot_key": slot, "timezone": "Asia/Singapore",
            "commits": self.pending, "store": dir, "base": self.base, "head": head, "since_ms": since_ms, "previous": previous,
        }))?;
        let label = if name == "morning" { "早间" } else { "傍晚" };
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, &format!("看门狗：记忆做梦（{label}）"), &prompt(home, &id, name, self.pending, dir))
        };
        self.dispatches.push(match &result {
            Ok(agent) => json!({"incident": id, "slot": slot, "commits": self.pending, "base": self.base, "head": head, "at_ms": at, "started": true, "agent": agent}),
            Err(error) => json!({"incident": id, "slot": slot, "commits": self.pending, "at_ms": at, "started": false, "error": format!("{error:#}")}),
        });
        if result.is_err() {
            return Ok(None);
        }
        super::conversation_retro::notify("看门狗：记忆做梦", &format!("新加坡时间{label}做梦已开会话：删错误记忆、记召回盲区、更新自动化主题页"));
        Ok(Some(id))
    }

    fn write(&self, home: &Path, now: f64, open: Option<String>) -> Result<()> {
        let slots: Vec<Value> = SLOTS.iter().map(|(name, start)| json!({"slot": name, "start_sgt": format!("{:02}:{:02}", (*start / 60.0) as u32, (*start % 60.0) as u32), "window_min": WINDOW_MIN})).collect();
        write_json(&root(home).join("memory-dream.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "base": self.base, "pending": self.pending, "open_incident": open, "blocked": self.blocked, "dispatches": self.dispatches,
            "schedule": {"timezone": "Asia/Singapore", "slots": slots, "sample_s": SAMPLE_SECS}}))
    }
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
pub(super) fn prompt(home: &Path, id: &str, slot: &str, commits: u64, dir: &Path) -> String {
    format!("cpu-watchdog incident {id}: scheduled {slot} memory dream (Singapore time) of the shared agent memory store {d}; {commits} store commits since the last dream. {header} \
        Incident: {s}/incidents/{id}.json; state root {s}; analysis dir {s}/analysis/{id}. \
        This dream audits what the window's sessions did against the notes and skills they were given (reference memory-dream.md). Measure recall (agent-memory eval) and scan (agent-memory dream) before; run cpu-watchdog dream-sessions {id} and audit every group in analysis/{id}/dream-sessions.json required[] from its cited transcript lines: what the session lacked, where it got stuck, what repeated across sessions, what the note or skill does not say; for a lesson: group whether the earlier lesson helped and where a helpful one is promoted, and rewrite or retire in this dream a lesson that is wrong, ignored or obsolete (retired lessons go in retired[]); for a best:, fast: or praise: group what the rare good run did differently (best: compares it with a typical and the costliest run of the same recurring task, fast: a release that took far less time than the usual one of its target) and whether every run can do that. \
        Write each lesson as a note (a wrong note calibrated, a skill gotcha memo, a recall gap, an instruction gap, or a one-off better path kept as an outlier) with the real request that should recall it, and measure those requests (eval-lessons.json); then review recall misses, refresh the automation hub notes and do the store hygiene; measure and scan again, \
        publish the encrypted report page and give its link, write {s}/analysis/{id}/report.md in Chinese and report.json, then run cpu-watchdog monitor-ack {id}.",
        header = crate::dispatch::route_header("memory-dream"), s = root(home).display(), d = dir.display())
}

#[cfg(test)]
#[path = "memory_dream/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "memory_dream/audit_tests.rs"]
mod audit_tests;
