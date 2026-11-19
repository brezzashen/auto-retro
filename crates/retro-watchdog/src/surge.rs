//! CPU surge responder (user request, 2026-10-21): when every whole-host sample exceeds
//! THRESHOLD_PERCENT for WINDOW_MS, one worker thread looks for abnormal processes, abnormal
//! launchd jobs and recent errors, terminates what the process-cleanup rules allow
//! (`surge/rules.rs`), checks ControlCenter (`surge/control_center.rs`), then writes
//! `incidents/<id>.json`, `analysis/<id>/report.{json,md}` and posts a macOS notification.
//! One activation per hot episode, at most one per COOLDOWN_MS, never two workers at once.
//! The resident five-minute policy keeps its own DRY_RUN setting; only this responder signals.
mod act;
mod control_center;
mod history;
mod launchd;
mod report;
mod rules;

use crate::{monitor::root, procs::Proc};
use anyhow::Result;
use serde_json::{json, Value};
use std::{path::Path, sync::mpsc::{self, Receiver, TryRecvError}};

pub const THRESHOLD_PERCENT: f64 = 95.0;
/// User request 2026-10-25: two continuous minutes (was 60 s), so a one-minute build burst no longer wakes a session.
pub const WINDOW_MS: f64 = 120_000.0;
const COOLDOWN_MS: f64 = 600_000.0;
pub(crate) const TRIGGER: &str = "whole_host_cpu_continuously_gt_95_for_120s";

#[derive(Default)]
pub struct Surge {
    since: Option<f64>,
    latched: bool,
    last_fire: f64,
    skipped: Option<String>,
    history: history::History,
    worker: Option<Receiver<()>>,
}

impl Surge {
    pub fn reset(&mut self) {
        self.since = None;
        self.latched = false;
    }

    pub fn record(&mut self, now: f64, processes: &[Proc]) {
        self.history.record(now, processes);
    }

    /// Feeds one validated whole-host sample (ms; the monitor already resets on gaps). Returns
    /// the episode start when the episode first reaches WINDOW_MS outside the cooldown.
    pub fn ingest(&mut self, start: f64, end: f64, percent: f64) -> Option<f64> {
        if percent <= THRESHOLD_PERCENT {
            self.reset();
            return None;
        }
        let since = *self.since.get_or_insert(start);
        if self.latched || end - since < WINDOW_MS {
            return None;
        }
        self.latched = true;
        if self.last_fire > 0.0 && end - self.last_fire < COOLDOWN_MS {
            self.skipped = Some(format!("episode at {end:.0} inside the {COOLDOWN_MS:.0} ms cooldown"));
            return None;
        }
        self.last_fire = end;
        Some(since)
    }

    /// Starts the responder thread for an episode `since..end` (ms); `frames` are the monitor's
    /// recent per-process CPU frames.
    pub fn fire(&mut self, home: &Path, since: f64, end: f64, processes: &[Proc], frames: Value) -> Result<()> {
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {
                    self.skipped = Some(format!("episode at {end:.0} while the previous responder still ran"));
                    return Ok(());
                }
            }
        }
        let id = incident_id(home, end);
        let (home, processes, history) = (home.to_path_buf(), processes.to_vec(), self.history.clone());
        let (done, receiver) = mpsc::channel();
        std::thread::Builder::new().name("watchdog-cpu-surge".into()).spawn(move || {
            let episode = act::Episode { id, trigger: TRIGGER, execute: true, since_ms: since, end_ms: end, frames };
            if let Err(error) = act::respond(&home, &episode, &processes, &history) {
                crate::log(&format!("cpu surge {} failed: {error:#}", episode.id));
            }
            let _ = done.send(());
        })?;
        self.worker = Some(receiver);
        Ok(())
    }

    pub fn state(&self) -> Value {
        json!({"threshold_percent_whole_host": THRESHOLD_PERCENT, "required_continuous_ms": WINDOW_MS,
            "hot_since_ms": self.since, "latched": self.latched, "last_fire_ms": self.last_fire,
            "responder_running": self.worker.is_some(), "last_skipped": self.skipped})
    }
}

/// `cpu-watchdog surge-now`: the same checks synchronously, without the CPU condition. Decisions
/// are reported but never executed; ControlCenter's own port-limit restart still applies.
pub fn run_now(home: &Path) -> Result<()> {
    let (now, processes) = crate::procs::snapshot(crate::unix_now)?;
    let id = incident_id(home, now * 1000.0);
    let episode = act::Episode { id: id.clone(), trigger: "manual_surge_now", execute: false, since_ms: now * 1000.0, end_ms: now * 1000.0, frames: Value::Null };
    act::respond(home, &episode, &processes, &history::History::default())?;
    println!("{}", root(home).join("analysis").join(id).join("report.md").display());
    Ok(())
}

/// Numeric like the 90% monitor's ids; bumped when that monitor already used the same end.
fn incident_id(home: &Path, end: f64) -> String {
    let mut id = end.round() as u64;
    while root(home).join("incidents").join(format!("{id}.json")).exists() {
        id += 1;
    }
    id.to_string()
}

#[cfg(test)]
#[path = "surge/tests.rs"]
mod tests;
