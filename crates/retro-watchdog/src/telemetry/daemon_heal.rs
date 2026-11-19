//! Self-heal for Apple root daemons that spin without end (user request, 2026-10-22). That
//! morning PerfPowerServices had held 1.2-1.7 cores for 22 h and syspolicyd 0.6-0.8 core for 13 h
//! while desktop frames slowed; `sudo kill -TERM` on both and launchd's respawn brought them back
//! to idle. Every SAMPLE_SECS this records each target's cumulative CPU time; when one instance
//! averages at least its floor over WINDOW_SECS, one worker thread re-checks its pid and start
//! time, sends SIGTERM through the Keychain askpass, waits for launchd to respawn it, and appends
//! the outcome to `daemon-restarts/<utc-day-ms>.ndjson` (latest in `daemon-restart.json`). A
//! daemon is restarted at most once per its cooldown (`cooldown_secs`). An EXEC_DRIVEN daemon is held instead
//! (state `held`, one log line, no restart and no recovery review) while a build ran inside the
//! window, while its previous restart never produced a quiet window (user request, 2026-10-25),
//! or while a Gatekeeper revocation sweep advances (`gatekeeper_sweep`, cause-tree N7.2.2).
//! For any other daemon, an ineffective restart (successor settled at or above the floor) is
//! retried once after the cooldown; the successor of a second one in a row is held for
//! INEFFECTIVE_HOLD_SECS (`ineffective_restarts`; founder decision B 2026-11-08, perf retro
//! 86403027128652638928, cause-tree N7.1.5). The first one closes by script (N7.1.3).
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

/// Executable name and the sustained fraction of one core that counts as spinning. Idle, all
/// stay near zero. fseventsd (founder decision 2026-11-15, cause-tree N8.4.1.1): its notify thread
/// holds a core once its path table stops being reclaimed, and its footprint then grows to 40+ GiB;
/// 649 ten-minute windows before the 11-13 onset held none at 0.8 core, 224 of 228 after it did.
pub(super) const TARGETS: &[(&str, f64)] = &[("PerfPowerServices", 0.5), ("syspolicyd", 0.5), ("fseventsd", 0.8)];
/// Daemons whose load follows new executables: syspolicyd runs Gatekeeper scans and online
/// notarization lookups for every unsigned build product, and parallel cargo test runs held it
/// above the floor for whole windows; a restart only dipped until the next binary launched
/// (recovery retros 86548466387911415232, 815703498935595536002).
const EXEC_DRIVEN: &[&str] = &["syspolicyd"];
/// Processes that write or launch fresh unsigned executables. Idle resident build services
/// (XCBBuildService) are left out so an open Xcode does not hold restarts forever.
const BUILD_TOOLS: &[&str] = &["cargo", "rustc", "xcodebuild", "swift-frontend", "swiftc", "clang", "ld"];
pub(super) const WINDOW_SECS: f64 = 1_800.0;
const SAMPLE_SECS: f64 = 30.0;
const COOLDOWN_SECS: f64 = 21_600.0;
/// PerfPowerServices re-heats about 1.5 h after an effective restart and then spins for the rest
/// of a 6 h cooldown (about 0.8 core, cause-tree N7.1.1); 2 h caps that tail (founder, 2026-11-01).
const PERF_POWER_COOLDOWN_SECS: f64 = 7_200.0;
const RESPAWN_WAIT: Duration = Duration::from_secs(15);
const SETTLE_WAIT: Duration = Duration::from_secs(30);
/// Extra snapshots when the settle snapshot misses a successor that is still alive (N7.1.2).
const SETTLE_RETRIES: usize = 3;
/// How long a second ineffective restart in a row holds the daemon (founder, 2026-11-04/11-08).
const INEFFECTIVE_HOLD_SECS: f64 = 86_400.0;
const INEFFECTIVE_HOLD: &str = "two restarts in a row ineffective; held 24 h";

fn cooldown_secs(name: &str) -> f64 {
    if name == "PerfPowerServices" { PERF_POWER_COOLDOWN_SECS } else { COOLDOWN_SECS }
}
const DAY_MS: f64 = 86_400_000.0;

#[derive(Default)]
struct Track {
    pid: i32,
    start: i64,
    samples: VecDeque<(f64, f64)>,
    /// A full window below the floor has been seen on this instance.
    quiet: bool,
}

#[derive(Default)]
pub(super) struct DaemonHeal {
    last_sample: f64,
    /// Unix seconds a BUILD_TOOLS process was last seen.
    last_build: f64,
    tracks: HashMap<&'static str, Track>,
    /// Current hold reason by name; a log line is written only when it changes.
    held: HashMap<&'static str, &'static str>,
    /// Unix seconds of the latest restart attempt by name; loaded from the state file once.
    restarted: Option<HashMap<String, f64>>,
    /// Unix seconds of the second of two ineffective restarts in a row by name (reloaded after each restart).
    ineffective: Option<HashMap<String, f64>>,
    sweep: super::gatekeeper_sweep::Sweep,
    worker: Option<Receiver<Value>>,
}

impl DaemonHeal {
    pub(super) fn observe(&mut self, home: &Path, now: f64, processes: &[Proc]) -> Result<()> {
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(record) => {
                    self.worker = None;
                    // restart() appended the record to the log; the newest two restarts decide.
                    self.ineffective = None;
                    crate::log(&format!("daemon heal: {record}"));
                }
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS {
            return Ok(());
        }
        self.last_sample = now;
        let state_path = root(home).join("daemon-heal.json");
        let restarted = self.restarted.get_or_insert_with(|| super::read(&state_path)
            .and_then(|state| serde_json::from_value(state["restarted"].clone()).ok()).unwrap_or_default());
        let ineffective = self.ineffective.get_or_insert_with(|| ineffective_restarts(home, now));
        let mut usage = serde_json::Map::new();
        let mut due = None;
        if processes.iter().any(|p| BUILD_TOOLS.contains(&p.name.as_str())) {
            self.last_build = now;
        }
        let build_age = now - self.last_build;
        for &(name, floor) in TARGETS {
            let Some(p) = processes.iter().find(|p| p.name == name && p.uid == 0) else {
                self.tracks.remove(name);
                continue;
            };
            let track = self.tracks.entry(name).or_default();
            if track.pid != p.pid || (track.start - p.start).abs() > 2 {
                *track = Track { pid: p.pid, start: p.start, ..Track::default() };
            }
            track.samples.push_back((now, p.cpu_secs));
            while track.samples.len() > 1 && track.samples.get(1).is_some_and(|(at, _)| now - at >= WINDOW_SECS) {
                track.samples.pop_front();
            }
            let used = spin(track, now);
            track.quiet |= used.is_some_and(|u| u < floor);
            let cooled = restarted.get(name).is_none_or(|at| now - at >= cooldown_secs(name));
            let hot = cooled && used.is_some_and(|u| u >= floor);
            let hold = if hot { hold_reason(name, track, restarted.get(name), ineffective.get(name), now, build_age)
                .or_else(|| (EXEC_DRIVEN.contains(&name) && self.sweep.active(home, now)).then_some(super::gatekeeper_sweep::HOLD)) } else { None };
            usage.insert(name.into(), json!({"pid": p.pid, "usage_one_core": used, "floor": floor, "held": hold}));
            match hold {
                Some(reason) if self.held.insert(name, reason) != Some(reason) =>
                    crate::log(&format!("daemon heal: hold {name} {} at {used:?} core: {reason}", p.pid)),
                Some(_) => {}
                None => { self.held.remove(name); }
            }
            if due.is_none() && self.worker.is_none() && hot && hold.is_none() {
                due = Some((p.clone(), used.unwrap_or_default()));
            }
        }
        if let Some((p, used)) = due {
            restarted.insert(p.name.clone(), now);
            self.tracks.remove(p.name.as_str());
            let (sender, receiver) = mpsc::sync_channel(1);
            let home = home.to_path_buf();
            std::thread::Builder::new().name("watchdog-daemon-heal".into()).spawn(move || {
                let record = restart(&home, now, &p, used);
                let _ = sender.send(record);
            })?;
            self.worker = Some(receiver);
        }
        write_json(&state_path, &json!({"at_ms": now * 1000.0, "error": null, "window_s": WINDOW_SECS,
            "cooldown_s": TARGETS.iter().map(|&(n, _)| (n.to_string(), json!(cooldown_secs(n)))).collect::<serde_json::Map<_, _>>(),
            "targets": usage, "restarted": restarted,
            "restart_running": self.worker.is_some()}))
    }
}

/// Average fraction of one core over the newest span of at least WINDOW_SECS; None until the
/// same instance has been watched that long.
fn spin(track: &Track, now: f64) -> Option<f64> {
    let (at, cpu) = track.samples.iter().rev().find(|(at, _)| now - at >= WINDOW_SECS)?;
    let (_, latest) = track.samples.back()?;
    Some((latest - cpu).max(0.0) / (now - at))
}

/// Why a hot, cooled daemon is left running. EXEC_DRIVEN: a build inside the window means the
/// load is real work, and a successor of the last restart that was never quiet shows the
/// restart did not help (the caller adds the revocation sweep). Others: the successor of the second
/// ineffective restart in a row, within INEFFECTIVE_HOLD_SECS of it (`ineffective_at`).
fn hold_reason(name: &str, track: &Track, restarted_at: Option<&f64>, ineffective_at: Option<&f64>, now: f64, build_age: f64) -> Option<&'static str> {
    if !EXEC_DRIVEN.contains(&name) {
        // Only the hot successor is held: on 2026-11-07 launchd replaced it with pid 54596, which spun
        // 1.2-1.85 cores for 7 h under a by-name hold (perf retro 812511861658399771312).
        return (ineffective_at.is_some_and(|at| now - at < INEFFECTIVE_HOLD_SECS) && is_successor(track, ineffective_at))
            .then_some(INEFFECTIVE_HOLD);
    }
    if build_age < WINDOW_SECS {
        return Some("build active within window");
    }
    (is_successor(track, restarted_at) && !track.quiet).then_some("previous restart left no quiet window")
}

/// The instance launchd spawned in answer to the restart, i.e. started during its respawn and
/// settle wait. A later instance (a reboot, an idle exit and relaunch) is not judged by that
/// restart: on 2026-11-07 19:30 SGT boot-time syspolicyd 273 (started 11-05) was held as the
/// "successor" of the 11-03 restart after a watchdog restart cleared its in-memory quiet flag
/// (perf retro 812511861658399771312).
fn is_successor(track: &Track, restarted_at: Option<&f64>) -> bool {
    let spawn_wait = (RESPAWN_WAIT + SETTLE_WAIT).as_secs_f64();
    restarted_at.is_some_and(|at| (at - 5.0..=at + spawn_wait).contains(&(track.start as f64)))
}

/// A respawned successor that settled at or above its floor: the restart did not help.
pub(crate) fn ineffective_restart(record: &Value) -> bool {
    let floor = record["floor"].as_f64().unwrap_or(0.5);
    record["outcome"] == "restarted" && record["new_usage_one_core"].as_f64().is_some_and(|u| u >= floor)
}

/// Daemons whose newest restart (within INEFFECTIVE_HOLD_SECS) was the second ineffective one in a
/// row: the previous restart was ineffective, at most INEFFECTIVE_HOLD_SECS earlier, and left the
/// pid the newest one restarted. Value: the newest one's time. Both earlier retries of a hot
/// successor cooled it (10-30 0.65 -> 0.17, 11-04 0.79 -> 0.11 core); an instance launchd started
/// on its own (11-08 07:01: pid 54596) starts a fresh count; an effective restart lifts the hold.
fn ineffective_restarts(home: &Path, now: f64) -> HashMap<String, f64> {
    // (time, ineffective, restarted pid, successor pid)
    let mut restarts: HashMap<String, Vec<(f64, bool, i64, i64)>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir(root(home).join("daemon-restarts")) else { return HashMap::new() };
    let oldest_day = now * 1000.0 - 2.0 * INEFFECTIVE_HOLD_SECS * 1000.0 - DAY_MS;
    for path in entries.flatten().map(|e| e.path()) {
        let day = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<f64>().ok());
        if day.is_none_or(|d| d < oldest_day) { continue; }
        for record in std::fs::read_to_string(&path).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
            let (Some(name), Some(at)) = (record["name"].as_str(), record["at_ms"].as_f64().map(|ms| ms / 1000.0)) else { continue };
            if record["outcome"] == "restarted" && at <= now {
                let (pid, new_pid) = (record["pid"].as_i64().unwrap_or(-1), record["new_pid"].as_i64().unwrap_or(-2));
                restarts.entry(name.into()).or_default().push((at, ineffective_restart(&record), pid, new_pid));
            }
        }
    }
    restarts.into_iter().filter_map(|(name, mut list)| {
        list.sort_by(|a, b| b.0.total_cmp(&a.0));
        match list.as_slice() {
            [(at, true, pid, _), (prev, true, _, successor), ..]
                if now - at < INEFFECTIVE_HOLD_SECS && at - prev <= INEFFECTIVE_HOLD_SECS && pid == successor => Some((name, *at)),
            _ => None,
        }
    }).collect()
}

fn restart(home: &Path, now: f64, p: &Proc, used: f64) -> Value {
    let floor = TARGETS.iter().find(|&&(n, _)| n == p.name).map_or(0.5, |&(_, f)| f);
    let mut record = json!({"at_ms": now * 1000.0, "name": p.name, "pid": p.pid, "process_start": p.start,
        "usage_one_core": used, "floor": floor, "window_s": WINDOW_SECS});
    let keep = !cfg!(test) && evidence::EVIDENCE_FIRST.contains(&p.name.as_str());
    if keep {
        record["footprint"] = evidence::footprint(home, p.pid);
        record["sample"] = evidence::sample(home, now, &p.name, p.pid);
    }
    let outcome = if cfg!(test) { Ok((Some(p.pid + 1), Some(0.01))) } else { terminate(p) };
    match outcome {
        Ok((new_pid, new_usage)) => {
            record["outcome"] = json!(if new_pid.is_some() { "restarted" } else { "stopped; launchd starts it on demand" });
            record["new_pid"] = json!(new_pid);
            record["new_usage_one_core"] = json!(new_usage);
            if let Some(pid) = new_pid.filter(|_| keep) {
                record["new_footprint"] = evidence::footprint(home, pid);
                // Clients opened before the restart get no events: restart Storybook, name the rest.
                record["rewatch"] = evidence::rewatch(now, &record["sample"]);
                crate::dispatch::notify("看门狗：fseventsd 已重启", &evidence::summary(&record["rewatch"]));
            }
        }
        Err(error) => record["outcome"] = json!(format!("failed: {error:#}")),
    }
    if let Err(error) = append(home, now, &record) {
        record["log_error"] = json!(format!("{error:#}"));
    }
    if let Err(error) = crate::recovery_retro::record(home, "daemon", &record) {
        record["retro_error"] = json!(format!("{error:#}"));
    }
    record
}

fn append(home: &Path, now: f64, record: &Value) -> Result<()> {
    write_json(&root(home).join("daemon-restart.json"), record)?;
    let directory = root(home).join("daemon-restarts");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let day = ((now * 1000.0) / DAY_MS).floor() * DAY_MS;
    std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(directory.join(format!("{}.ndjson", day as u64)))?
        .write_all(format!("{record}\n").as_bytes())?;
    Ok(())
}

/// SIGTERM through sudo to the verified root instance, then the respawned pid and its settled CPU usage.
fn terminate(p: &Proc) -> Result<(Option<i32>, Option<f64>)> {
    anyhow::ensure!(crate::procs::still_same_process(p.pid, p.start, crate::unix_now),
        "{} {} already exited or was replaced", p.name, p.pid);
    let output = Command::new("/usr/bin/sudo").args(["-A", "/bin/kill", "-TERM", &p.pid.to_string()])
        .env("SUDO_ASKPASS", format!("{}/.local/bin/sudo-askpass", std::env::var("HOME")?))
        .stdin(Stdio::null()).output()?;
    anyhow::ensure!(output.status.success(), "sudo kill {} {}", output.status, String::from_utf8_lossy(&output.stderr).trim());
    let waited = std::time::Instant::now();
    while waited.elapsed() < RESPAWN_WAIT {
        std::thread::sleep(Duration::from_millis(500));
        let (t0, processes) = crate::procs::snapshot(crate::unix_now)?;
        if let Some(new) = processes.iter().find(|n| n.name == p.name && n.uid == 0 && n.pid != p.pid).cloned() {
            std::thread::sleep(SETTLE_WAIT);
            // 10-28 23:55 and 10-29 11:56 one settle snapshot missed a successor that lived on for
            // hours, which left the usage null and blocked N7.1 script closure; look again briefly.
            let mut found = None;
            for attempt in 0..=SETTLE_RETRIES {
                if attempt > 0 { std::thread::sleep(Duration::from_secs(1)); }
                let (t1, settled) = crate::procs::snapshot(crate::unix_now)?;
                if let Some(cur) = settled.into_iter().find(|n| n.name == p.name && n.uid == 0 && n.pid != p.pid) {
                    found = Some((t1, cur));
                    break;
                }
            }
            let Some((t1, cur)) = found else { return Ok((Some(new.pid), None)) };
            let base = if cur.pid == new.pid && (cur.start - new.start).abs() <= 2 { new.cpu_secs } else { 0.0 };
            let usage = (t1 > t0).then(|| (cur.cpu_secs - base).max(0.0) / (t1 - t0));
            return Ok((Some(cur.pid), usage));
        }
    }
    anyhow::ensure!(!crate::procs::still_same_process(p.pid, p.start, crate::unix_now),
        "{} {} still running 15 s after SIGTERM", p.name, p.pid);
    Ok((None, None))
}

#[path = "daemon_heal_evidence.rs"]
mod evidence;

#[cfg(test)]
#[path = "daemon_heal_tests.rs"]
mod tests;
