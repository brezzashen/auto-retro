//! TCC backlog heal (user request 2026-11-05, memory `tcc-sandboxd-backlog-git-timeout`).
//!
//! Every first open of a file on an external volume (/Volumes/MarsWork, /Volumes/MarsBuild) goes
//! kernel -> sandboxd -> tccd. On 2026-11-05 two swift-frontend left by a dead Mac Catalyst
//! archive build (re-parented to launchd for 79 min) retried `open()` on /Volumes about 22 times a
//! second each. The system tccd held about one core, sandboxd fell minutes behind (it answered
//! requests of processes that had already exited: `Failed csops_audittoken() ... No such
//! process`), and each blocked syscall timed out after 5 s, restarted and queued again. Orca's git
//! waited 5 s per `access(".git/config")` until Orca reported "Worktree scan failed: git timed out";
//! live builds were denied too. Waiting did not drain it; SIGKILL of sandboxd and both tccd did
//! (launchd respawns them; SIGTERM is ignored and `launchctl kickstart` is refused under SIP).
//!
//! Two steps, every CHECK_SECS:
//! 1. Close our compiler processes found re-parented to launchd on two checks (same start).
//! 2. When the system tccd averaged FLOOR of one core over WINDOW_SECS (idle it stays near 1 %,
//!    3.5 % during parallel builds), SIGKILL sandboxd, the system tccd and this user's tccd through
//!    the Keychain askpass, at most once per COOLDOWN_SECS; a restart whose successor stayed hot
//!    holds the heal for HOLD_SECS. Each action goes to `tcc-backlog/<utc-day-ms>.ndjson`, the
//!    newest state to `tcc-backlog.json`.
use super::esbuild_orphans::confirm;
use crate::{monitor::{root, write_json}, procs::Proc};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::Duration,
};

const CHECK_SECS: f64 = 60.0;
pub(super) const WINDOW_SECS: f64 = 300.0;
pub(super) const FLOOR: f64 = 0.5;
const COOLDOWN_SECS: f64 = 3_600.0;
const HOLD_SECS: f64 = 86_400.0;
const SETTLE_WAIT: Duration = Duration::from_secs(30);
const DAY_MS: f64 = 86_400_000.0;
/// Compiler processes that only ever run under a build driver; launchd as parent means abandoned.
const COMPILERS: &[&str] = &["swift-frontend", "swift-driver", "swiftc", "clang", "ld"];
const USER_TCCD_PREFIX: &str = "/System/Library/PrivateFrameworks/TCC.framework/";

#[derive(Default)]
pub(super) struct TccBacklog {
    last_check: f64,
    orphans: HashMap<i32, i64>,
    /// (at, pid, start, cumulative cpu seconds) of the system tccd.
    samples: VecDeque<(f64, i32, i64, f64)>,
    last_restart: Option<f64>,
    held_until: f64,
    worker: Option<Receiver<Value>>,
}

impl TccBacklog {
    pub(super) fn observe(&mut self, home: &Path, now: f64, processes: &[Proc]) -> Result<()> {
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(record) => {
                    self.worker = None;
                    if ineffective(&record) {
                        self.held_until = now + HOLD_SECS;
                    }
                    crate::log(&format!("tcc backlog: {record}"));
                    finish(home, now, &record)?;
                }
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.last_check > 0.0 && now >= self.last_check && now - self.last_check < CHECK_SECS {
            return Ok(());
        }
        self.last_check = now;
        let uid = unsafe { libc::getuid() };
        let (confirmed, seen) = confirm(&self.orphans, orphaned_compilers(processes, uid));
        self.orphans = seen;
        if !confirmed.is_empty() {
            for proc in &confirmed {
                let record = json!({"at_ms": now * 1000.0, "action": "close_orphan_compiler", "pid": proc.pid,
                    "start": proc.start, "name": proc.name, "args": proc.args.chars().take(240).collect::<String>()});
                crate::log(&format!("tcc backlog: closing {record}"));
                append(home, now, &record)?;
            }
            std::thread::Builder::new().name("watchdog-tcc-orphans".into()).spawn(move || {
                for proc in &confirmed {
                    crate::sweep::terminate(proc);
                }
            })?;
        }
        let Some(tccd) = system_tccd(processes) else { return Ok(()) };
        let usage = record_sample(&mut self.samples, now, tccd);
        let last_restart = *self.last_restart.get_or_insert_with(|| super::read(&root(home).join("tcc-backlog.json"))
            .and_then(|state| state["last_restart_at_ms"].as_f64()).map_or(0.0, |ms| ms / 1000.0));
        if self.worker.is_none() && due(usage, now, last_restart, self.held_until) {
            self.last_restart = Some(now);
            self.samples.clear();
            let (sender, receiver) = mpsc::channel();
            let targets = restart_targets(processes, uid);
            let used = usage.unwrap_or_default();
            std::thread::Builder::new().name("watchdog-tcc-restart".into())
                .spawn(move || { let _ = sender.send(restart(now, targets, used)); })?;
            self.worker = Some(receiver);
        }
        Ok(())
    }
}

/// Our compiler processes whose parent is launchd.
pub(super) fn orphaned_compilers(processes: &[Proc], uid: u32) -> Vec<Proc> {
    processes.iter().filter(|p| p.uid == uid && p.ppid == 1 && COMPILERS.contains(&p.name.as_str())).cloned().collect()
}

pub(super) fn system_tccd(processes: &[Proc]) -> Option<&Proc> {
    processes.iter().find(|p| p.uid == 0 && p.name == "tccd")
}

/// sandboxd, the system tccd and this user's tccd (not a simulator runtime's tccd).
pub(super) fn restart_targets(processes: &[Proc], uid: u32) -> Vec<Proc> {
    processes.iter().filter(|p| match p.name.as_str() {
        "sandboxd" => p.uid == 0,
        "tccd" => p.uid == 0 || (p.uid == uid && p.args.starts_with(USER_TCCD_PREFIX)),
        _ => false,
    }).cloned().collect()
}

/// Adds a sample and returns the average fraction of one core over the newest span of at least
/// WINDOW_SECS of the same instance; a new pid or start time resets the history.
pub(super) fn record_sample(samples: &mut VecDeque<(f64, i32, i64, f64)>, now: f64, tccd: &Proc) -> Option<f64> {
    if samples.back().is_some_and(|&(_, pid, start, _)| pid != tccd.pid || (start - tccd.start).abs() > 2) {
        samples.clear();
    }
    samples.push_back((now, tccd.pid, tccd.start, tccd.cpu_secs));
    while samples.len() > 2 && samples.get(1).is_some_and(|&(at, ..)| now - at >= WINDOW_SECS) {
        samples.pop_front();
    }
    let &(at, _, _, cpu) = samples.iter().rev().find(|&&(at, ..)| now - at >= WINDOW_SECS)?;
    Some((tccd.cpu_secs - cpu).max(0.0) / (now - at))
}

pub(super) fn due(usage: Option<f64>, now: f64, last_restart: f64, held_until: f64) -> bool {
    usage.is_some_and(|u| u >= FLOOR) && now - last_restart >= COOLDOWN_SECS && now >= held_until
}

pub(super) fn ineffective(record: &Value) -> bool {
    record["outcome"] == "restarted" && record["new_usage_one_core"].as_f64().is_some_and(|u| u >= FLOOR)
}

fn restart(now: f64, targets: Vec<Proc>, used: f64) -> Value {
    let mut record = json!({"at_ms": now * 1000.0, "action": "restart_tcc", "usage_one_core": used, "floor": FLOOR,
        "window_s": WINDOW_SECS, "killed": targets.iter().map(|p| json!({"name": p.name, "pid": p.pid, "uid": p.uid})).collect::<Vec<_>>()});
    match kill_and_settle(&targets) {
        Ok((new_pid, new_usage)) => {
            record["outcome"] = json!(if new_pid.is_some() { "restarted" } else { "stopped; launchd starts it on demand" });
            record["new_pid"] = json!(new_pid);
            record["new_usage_one_core"] = json!(new_usage);
        }
        Err(error) => record["outcome"] = json!(format!("failed: {error:#}")),
    }
    record
}

/// SIGKILL through sudo to the verified targets, then the successor system tccd and its usage.
fn kill_and_settle(targets: &[Proc]) -> Result<(Option<i32>, Option<f64>)> {
    let live: Vec<&Proc> = targets.iter().filter(|p| crate::procs::still_same_process(p.pid, p.start, crate::unix_now)).collect();
    anyhow::ensure!(live.iter().any(|p| p.uid == 0 && p.name == "tccd"), "system tccd already exited or was replaced");
    let mut command = Command::new("/usr/bin/sudo");
    command.args(["-A", "/bin/kill", "-KILL"]).args(live.iter().map(|p| p.pid.to_string()))
        .env("SUDO_ASKPASS", format!("{}/.local/bin/sudo-askpass", std::env::var("HOME")?)).stdin(Stdio::null());
    let output = command.output()?;
    anyhow::ensure!(output.status.success(), "sudo kill {} {}", output.status, String::from_utf8_lossy(&output.stderr).trim());
    std::thread::sleep(Duration::from_secs(5));
    let (t0, first) = crate::procs::snapshot(crate::unix_now)?;
    let Some(new) = system_tccd(&first).filter(|n| live.iter().all(|old| old.pid != n.pid)).cloned() else { return Ok((None, None)) };
    std::thread::sleep(SETTLE_WAIT);
    let (t1, settled) = crate::procs::snapshot(crate::unix_now)?;
    let usage = system_tccd(&settled).filter(|cur| cur.pid == new.pid)
        .and_then(|cur| (t1 > t0).then(|| (cur.cpu_secs - new.cpu_secs).max(0.0) / (t1 - t0)));
    Ok((Some(new.pid), usage))
}

fn finish(home: &Path, now: f64, record: &Value) -> Result<()> {
    write_json(&root(home).join("tcc-backlog.json"), &json!({"last_restart_at_ms": record["at_ms"], "last": record, "written_at_ms": now * 1000.0}))?;
    append(home, now, record)
}

fn append(home: &Path, now: f64, record: &Value) -> Result<()> {
    let directory = root(home).join("tcc-backlog");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let day = ((now * 1000.0) / DAY_MS).floor() * DAY_MS;
    std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(directory.join(format!("{}.ndjson", day as u64)))?
        .write_all(format!("{record}\n").as_bytes())?;
    Ok(())
}

#[cfg(test)]
#[path = "tcc_backlog_tests.rs"]
mod tests;
