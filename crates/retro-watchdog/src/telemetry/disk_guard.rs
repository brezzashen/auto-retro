//! Disk guard (user request, 2026-10-23): keep MIN_FREE_GIB free without waiting for a build to hit
//! the storage guard.
//!
//! 1. Every SAMPLE_SECS the free space of VOLUMES is read with statvfs; the smallest one counts. The
//!    Docker VM's own disk is read too, inside a running container (2026-10-25: it filled to 0 B while the
//!    host volumes had room, and image builds failed with `no space left on device`).
//! 2. Below MIN_FREE_GIB, or the Docker VM below DOCKER_VM_MIN_FREE_GIB, the S0 script cleanup runs on its own thread: `mars release recover-disk`
//!    in Mars_CRM (`mars-ops` `release/disk_recovery.rs`), at most once per S0_COOLDOWN_SECS and never
//!    while a disk session is open. Each run is appended to `disk-guard/<utc-day-ms>.ndjson`; its
//!    JSON output and stderr are kept as `disk-guard/<ms>-s0.{json,log}`; a run that skipped snapshots
//!    for a running backup waits for an idle Time Machine and prunes once (`disk_guard_tm_retry.rs`); it also runs daily (`disk_guard_daily.rs`).
//! 3. Still below MIN_FREE_GIB after S0: one `disk_space_low` incident and one session on route
//!    `disk-low`, which analyzes duplicate content, extends the cleanup and lists what the user must
//!    confirm. No new incident opens while one is unacknowledged, nor within ACK_COOLDOWN_SECS of its ack
//!    (`disk_guard_ack.rs`); at most MAX_DISPATCH_PER_DAY a day.
//!    State goes to `disk-guard.json`. An S0 run that starts below the floor also lists the largest directories (`disk_guard_census.rs`).
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    io::Write,
    os::unix::{fs::{DirBuilderExt, OpenOptionsExt}, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

pub(super) const SAMPLE_SECS: f64 = 300.0;
/// 200 GiB (founder, 2026-11-09; was 278, the 30% internal-SSD target). Must equal tm-scheduler's
/// `build_min_free_gib`: S0 (`recover-disk`) judges "recovered" by that floor, so a higher value here
/// opened disk-low sessions after an S0 that stopped early as healthy (incident 814646763617667739095).
pub(super) const MIN_FREE_GIB: f64 = 200.0;
pub(super) const S0_COOLDOWN_SECS: f64 = 3600.0;
/// After a run that left the disk short, the next one waits longer: S0 has nothing new to remove
/// until the session or the user frees space.
pub(super) const S0_SHORT_COOLDOWN_SECS: f64 = 6.0 * 3600.0;
pub(super) const MAX_DISPATCH_PER_DAY: usize = 1;
/// The internal Data volume and the external build volume (Docker data, DerivedData, Rust targets;
/// it replaced the internal `DockerVM` volume on 2026-10-24).
const VOLUMES: [&str; 2] = ["/System/Volumes/Data", "/Volumes/MarsBuild"];
/// `DOCKER_VM_MIN_FREE_GIB` in `scripts/release/dockerVmDisk.ts`, which decides when S0 prunes the VM.
pub(super) const DOCKER_VM_MIN_FREE_GIB: f64 = 12.0;
/// A session that failed to start is retried after this long.
const RETRY_SECS: f64 = 600.0;
/// Image pruning holds the local-k8s lease and can wait for it; a stuck run is killed after this.
const S0_TIMEOUT: Duration = Duration::from_secs(1800);
const DAY_MS: f64 = 86_400_000.0;
pub(super) const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const KEEP_RUNS: usize = 20;
const CRM: &str = "/path/to/workspace/Mars_CRM";
const MARS: &str = "/home/user/.local/bin/mars";
/// `recover-disk` calls cargo (`cpu-build-gate cargo-run -p mars-ops`), tm-scheduler, tmutil, docker, kubectl
/// and kind; without rustup on PATH every S0 run exited 127 on 2026-10-26.
pub(super) const S0_PATH: &str = super::MARS_SCRIPT_PATH;

#[derive(Default)]
pub(super) struct DiskGuard {
    loaded: bool, last_sample: f64,
    last_volumes: Value,
    last_vm: Option<f64>,
    pub(super) since: f64,
    /// One record per S0 run, newest last: `{at_ms, seconds, exit, before_gib, after_gib, summary, ...}`.
    pub(super) runs: Vec<Value>,
    /// `{incident, s0_at_ms, at_ms, started, agent|error}` for every dispatched session.
    pub(super) dispatches: Vec<Value>,
    worker: Option<Receiver<Value>>,
}

impl DiskGuard {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded {
            self.load(home);
        }
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(run) => { self.worker = None; self.absorb(home, now, run); }
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS {
            return Ok(());
        }
        if self.since == 0.0 { self.since = now; }
        self.last_sample = now;
        self.last_volumes = volumes();
        self.last_vm = if cfg!(test) { None } else { docker_vm::free_gib(home) };
        let free = lowest(&self.last_volumes);
        let open = self.open_incident(home);
        if self.s0_due(now, free, self.last_vm, open.is_some()) && !cfg!(test) {
            let home = home.to_path_buf();
            let (before, vm_before) = (self.last_volumes.clone(), self.last_vm);
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::Builder::new().name("watchdog-disk-s0".into()).spawn(move || {
                let _ = sender.send(run_s0(&home, before, vm_before));
            })?;
            self.worker = Some(receiver);
        }
        let incident = self.escalate(home, now, free)?;
        self.write(home, now, free, incident)
    }

    fn load(&mut self, home: &Path) {
        self.loaded = true;
        if let Some(state) = super::read(&root(home).join("disk-guard.json")) {
            self.runs = state["runs"].as_array().cloned().unwrap_or_default();
            self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
        }
    }

    pub(super) fn absorb(&mut self, home: &Path, now: f64, run: Value) {
        crate::log(&format!("disk guard S0: {run}"));
        if let Err(error) = append(home, now, &run) { crate::log(&format!("disk guard log: {error:#}")); }
        let (before, after) = (run["before_gib"].as_f64().unwrap_or(0.0), run["after_gib"].as_f64().unwrap_or(0.0));
        if !cfg!(test) {
            let next = if after < MIN_FREE_GIB { "仍不足，交给模型做重复内容分析和扩展清理" } else { "已恢复" };
            let vm = match (run["vm_before_gib"].as_f64(), run["vm_after_gib"].as_f64()) {
                (Some(b), Some(a)) => format!("；Docker 虚拟机 {b:.1} → {a:.1} GiB"),
                _ => String::new(),
            };
            super::conversation_retro::notify("看门狗：磁盘空间不足", &format!("剩余 {before:.0} GiB，S0 脚本清理后 {after:.0} GiB，{next}{vm}"));
        }
        self.runs.push(run);
        let excess = self.runs.len().saturating_sub(KEEP_RUNS);
        self.runs.drain(..excess);
    }

    /// `vm` is the Docker VM's free GiB; unknown (no engine, no running container) never triggers.
    pub(super) fn s0_due(&self, now: f64, free: Option<f64>, vm: Option<f64>, open: bool) -> bool {
        let last = self.runs.last();
        let at = last.and_then(|r| r["at_ms"].as_f64()).unwrap_or(0.0);
        // The Docker VM fills and drains with image builds, so a run that left only the VM short keeps the
        // normal cooldown; the longer one is for a short host, which nothing frees until someone acts.
        let short = last.and_then(|r| r["after_gib"].as_f64()).is_some_and(|f| f < MIN_FREE_GIB);
        let cooldown = if short { S0_SHORT_COOLDOWN_SECS } else { S0_COOLDOWN_SECS };
        let needed = free.is_some_and(|f| f < MIN_FREE_GIB) || vm.is_some_and(|f| f < DOCKER_VM_MIN_FREE_GIB) || daily::due(at, self.since, now, free);
        needed && !open && self.worker.is_none() && now * 1000.0 - at >= cooldown * 1000.0
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.last().filter(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    /// Opens one incident and session when the last S0 run left the disk short; returns the open incident id.
    pub(super) fn escalate(&mut self, home: &Path, now: f64, free: Option<f64>) -> Result<Option<String>> {
        let open = self.open_incident(home);
        let last = self.dispatches.last();
        let failed_recently = last.is_some_and(|d| d["started"] != true && now * 1000.0 - d["at_ms"].as_f64().unwrap_or(0.0) < RETRY_SECS * 1000.0);
        let today = self.dispatches.iter().filter(|d| d["started"] == true && d["at_ms"].as_f64().is_some_and(|at| now * 1000.0 - at < DAY_MS)).count();
        // Only an unseen S0 run, never while a newer S0 is in flight (814479291214934651419), only while still short.
        let seen = self.dispatches.iter().filter_map(|d| d["s0_at_ms"].as_f64()).fold(0.0, f64::max);
        let Some(run) = self.runs.last().filter(|r| r["at_ms"].as_f64().is_some_and(|at| at > seen)) else { return Ok(open) };
        let host = run["after_gib"].as_f64().is_some_and(|f| f < MIN_FREE_GIB) && free.is_some_and(|f| f < MIN_FREE_GIB);
        let vm = run["vm_after_gib"].as_f64().is_some_and(|f| f < DOCKER_VM_MIN_FREE_GIB) && self.last_vm.is_some_and(|f| f < DOCKER_VM_MIN_FREE_GIB);
        let short = host || vm;
        if !short || self.worker.is_some() || open.is_some() || failed_recently || today >= MAX_DISPATCH_PER_DAY || ack::cooling(&root(home).join("incidents"), &self.dispatches, now) {
            return Ok(open);
        }
        let previous: Vec<Value> = self.dispatches.iter().rev().filter(|d| d["started"] == true).take(5)
            .map(|d| json!({"incident": d["incident"], "report": root(home).join("analysis").join(d["incident"].as_str().unwrap_or("")).join("report.json")})).collect();
        let run = run.clone();
        let id = super::diagnostics::incident(home, &format!("disk_space_low:{}", run["at_ms"]), json!({
            "trigger": "disk_space_low", "threshold_gib": MIN_FREE_GIB, "free_gib": free.map(round),
            "docker_vm": {"free_gib": self.last_vm.map(round), "threshold_gib": DOCKER_VM_MIN_FREE_GIB, "short": vm},
            "volumes": self.last_volumes, "s0": run, "previous": previous, "evidence": evidence(home),
        }))?;
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, "看门狗：磁盘不足扩展清理", &prompt(home, &id, free.unwrap_or(0.0), self.last_vm.filter(|_| vm)))
        };
        let record = |status: Value| {
            let mut record = json!({"incident": id, "s0_at_ms": run["at_ms"], "at_ms": now * 1000.0});
            record.as_object_mut().unwrap().extend(status.as_object().cloned().unwrap_or_default());
            record
        };
        self.dispatches.push(match &result {
            Ok(agent) => record(json!({"started": true, "agent": agent})),
            Err(error) => record(json!({"started": false, "s0_at_ms": null, "error": format!("{error:#}")})),
        });
        if result.is_ok() && !cfg!(test) {
            let what = if vm && !host {
                format!("Docker 虚拟机仍低于 {DOCKER_VM_MIN_FREE_GIB:.0} GiB")
            } else {
                format!("S0 脚本清理后仍低于 {MIN_FREE_GIB:.0} GiB")
            };
            super::conversation_retro::notify("看门狗：磁盘不足", &format!("{what}，已开会话做重复内容分析和扩展清理"));
        }
        Ok(result.ok().map(|_| id))
    }

    fn write(&self, home: &Path, now: f64, free: Option<f64>, incident: Option<String>) -> Result<()> {
        write_json(&root(home).join("disk-guard.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "free_gib": free.map(round), "below": free.is_some_and(|f| f < MIN_FREE_GIB), "volumes": self.last_volumes,
            "docker_vm": {"free_gib": self.last_vm.map(round), "below": self.last_vm.is_some_and(|f| f < DOCKER_VM_MIN_FREE_GIB)},
            "s0_running": self.worker.is_some(), "open_incident": incident, "runs": self.runs, "dispatches": self.dispatches,
            "threshold": {"min_free_gib": MIN_FREE_GIB, "docker_vm_min_free_gib": DOCKER_VM_MIN_FREE_GIB, "sample_s": SAMPLE_SECS, "s0_cooldown_s": S0_COOLDOWN_SECS, "s0_short_cooldown_s": S0_SHORT_COOLDOWN_SECS,
                "s0_timeout_s": S0_TIMEOUT.as_secs(), "max_dispatch_per_day": MAX_DISPATCH_PER_DAY}}))
    }
}

/// `[{path, free_gib}]` for each mounted volume of VOLUMES.
fn volumes() -> Value {
    Value::Array(VOLUMES.iter().filter_map(|path| free_gib(path).map(|free| json!({"path": path, "free_gib": round(free)}))).collect())
}

pub(super) fn lowest(volumes: &Value) -> Option<f64> {
    volumes.as_array()?.iter().filter_map(|v| v["free_gib"].as_f64()).reduce(f64::min)
}

fn free_gib(path: &str) -> Option<f64> {
    let path = std::ffi::CString::new(path).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    Some(stat.f_bavail as f64 * stat.f_frsize as f64 / GIB)
}

pub(super) fn round(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// Runs `recover-disk` once and returns its record; never panics on a failed or stuck run.
fn run_s0(home: &Path, before: Value, vm_before: Option<f64>) -> Value {
    let started_ms = crate::unix_now() * 1000.0;
    let started = Instant::now();
    let directory = root(home).join("disk-guard");
    let output = directory.join(format!("{}-s0.json", started_ms as u64));
    let census = census::start(home, &before);
    let result = (|| -> Result<(Option<i32>, bool)> {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
        let file = |path: &Path| std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(path);
        let mut child = Command::new(MARS).args(["release", "recover-disk"]).current_dir(CRM)
            .env("PATH", S0_PATH).stdin(Stdio::null()).stdout(file(&output)?).stderr(file(&output.with_extension("log"))?)
            .process_group(0).spawn()?;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok((status.code(), false));
            }
            if started.elapsed() >= S0_TIMEOUT {
                unsafe { libc::kill(-(child.id() as i32), libc::SIGTERM) };
                let _ = child.wait();
                return Ok((None, true));
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    })();
    let report = std::fs::read_to_string(&output).map(|text| report_json(&text)).unwrap_or(Value::Null);
    let (after, tm_retry) = tm_retry::run(&report, volumes());
    let vm_after = docker_vm::free_gib(home);
    let (exit, timed_out, error) = match result {
        Ok((exit, timed_out)) => (exit, timed_out, None),
        Err(error) => (None, false, Some(format!("{error:#}"))),
    };
    json!({"at_ms": crate::unix_now() * 1000.0, "started_at_ms": started_ms, "seconds": started.elapsed().as_secs(),
        "exit": exit, "timed_out": timed_out, "error": error, "output": output,
        "before": before.clone(), "after": after.clone(), "before_gib": lowest(&before), "after_gib": lowest(&after),
        "vm_before_gib": vm_before.map(round), "vm_after_gib": vm_after.map(round),
        "summary": summarize(&report), "tm_retry": tm_retry, "census": census.finish()})
}

fn evidence(home: &Path) -> Value {
    let state = root(home);
    json!({"disk_guard": state.join("disk-guard.json"), "s0_runs": state.join("disk-guard"),
        "cleanup_catalog": home.join(".codex/skills/performance/references/disk-cleanup.md"),
        "s0_script": PathBuf::from(CRM).join("rust/crates/mars-ops/src/release/disk_recovery.rs"),
        "watchdog_log": home.join("Library/Logs/cpu-watchdog.log")})
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
pub(super) fn prompt(home: &Path, id: &str, free: f64, vm: Option<f64>) -> String {
    let short = match vm {
        Some(vm) => format!("the Docker VM's own disk has only {vm:.1} GiB free (floor {DOCKER_VM_MIN_FREE_GIB:.0} GiB; host {free:.1} GiB)"),
        None => format!("only {free:.1} GiB free (floor {MIN_FREE_GIB:.0} GiB)"),
    };
    format!("cpu-watchdog incident {id}: {short} after the S0 script cleanup. {header} \
        Incident: {s}/incidents/{id}.json (volumes, S0 run and its output, previous disk sessions); state root {s}. \
        Run the S1/S2 cleanup, analyze duplicate content, extend the cleanup to unneeded regenerable content, \
        list everything you could not confirm for the user, publish the report page and give its link, \
        write {s}/analysis/{id}/report.md in Chinese and report.json, then run cpu-watchdog monitor-ack {id}.",
        header = crate::dispatch::route_header("disk-low"), s = root(home).display())
}

fn append(home: &Path, now: f64, record: &Value) -> Result<()> {
    let directory = root(home).join("disk-guard");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let day = ((now * 1000.0) / DAY_MS).floor() * DAY_MS;
    std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(directory.join(format!("{}.ndjson", day as u64)))?
        .write_all(format!("{record}\n").as_bytes())?;
    Ok(())
}

#[path = "disk_guard_docker_vm.rs"] mod docker_vm; #[path = "disk_guard_census.rs"] mod census;
#[path = "disk_guard_daily.rs"] mod daily; #[path = "disk_guard_ack.rs"] mod ack;
#[path = "disk_guard_report.rs"] mod report; #[path = "disk_guard_tm_retry.rs"] mod tm_retry;
pub(super) use report::{report_json, summarize};

#[cfg(test)] #[path = "disk_guard_tests.rs"] mod tests;
