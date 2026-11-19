//! Docker watch (user request, 2026-10-24): when Docker Desktop or its Kubernetes dies, keep a
//! quick cause analysis, rebuild it, and keep beta's Redis replicas loadable afterwards.
//!
//! 1. Every SAMPLE_SECS a worker probes the engine (`docker version`) and the API server
//!    (`kubectl get --raw /readyz`), each within heal::PROBE_TIMEOUT.
//! 2. ENGINE_FAILS engine failures, or KUBE_FAILS API failures with a live engine, in a row start
//!    one recovery, at most once per COOLDOWN_SECS and DAILY_ACTIONS per 24 h. `heal::recover`
//!    saves and classifies the evidence first. A copy of the VM disk, a fresh start or a pause
//!    (`cpu-watchdog docker-pause <minutes> <reason>`) skips the round; a missing or full data
//!    volume holds the restart and asks for analysis, because restarting then would split or
//!    corrupt the VM disk (the 2026-10-24 external-disk migration killed three relaunches). A
//!    backend `tmutil addexclusion` stuck on the data volume holds it too: the start waits on a
//!    system access prompt that only the user can answer (`heal::is_blocked_exclusion`).
//! 3. Recovery is `docker desktop start` for a dead backend, else `docker desktop restart`, then a
//!    wait for both probes. It never resets the cluster. While the beta tunnel agent is installed
//!    the runtime is remote (`remote`): probes address that host, and recovery never starts this
//!    Mac's Docker Desktop but stops it when it runs, since it takes the tunnel's ports.
//! 4. For REPAIR_WINDOW_SECS after the engine comes back (any restart, ours or not), each round
//!    repairs a beta Redis replica that crash-loops on a truncated AOF (`heal::repair_redis`;
//!    seen 2026-10-24 14:42 after an unclean stop). A primary is never touched.
//! 5. Each recorded recovery or repair enters `recovery_retro`, which queues the analysis session.
//! 6. After each new VM start `restarts` attributes every start of the last 14 days (who stopped
//!    Docker, who started it, downtime, waiting sessions, cause-tree node) to `docker-restarts.json`;
//!    a start by another actor during an agent's maintenance stop is a collision and notifies.
//!
//! Records go to `docker-heals/<utc-day-ms>.ndjson`; state to `docker-watch.json`.
mod heal;
mod power;
mod reboot;
mod remote;
mod restarts;

/// The SSH alias of the remote beta host (the tunnel agent's `--alias`), else `None`.
pub(super) fn remote_host(home: &Path) -> Option<String> {
    remote::host(home)
}

/// A planned self-heal reboot of the remote host (pause, reboot, Wake-on-LAN, record), for callers
/// outside docker_watch; `None` when the 24 h reboot cap holds it.
pub(super) fn self_heal_reboot(home: &Path, host: &str, reason: &str, kind: &str) -> Option<Result<Value>> {
    reboot::cap_allows(home).then(|| reboot::planned(home, host, reason, kind))
}

/// A PowerShell script as one ssh command for the remote Windows host.
pub(super) fn powershell(script: &str) -> String {
    remote::powershell(script)
}

use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
    sync::mpsc::{self, Receiver, TryRecvError},
};

pub(super) const SAMPLE_SECS: f64 = 30.0;
pub(super) const ENGINE_FAILS: u32 = 4;
/// Kubernetes needs about two minutes after an engine start (2026-10-24 14:40 → 14:42).
pub(super) const KUBE_FAILS: u32 = 10;
pub(super) const COOLDOWN_SECS: f64 = 900.0;
pub(super) const DAILY_ACTIONS: usize = 6;
pub(super) const REPAIR_WINDOW_SECS: f64 = 900.0;
const DAY_SECS: f64 = 86_400.0;
const PAUSE: &str = "docker-watch-pause.json";
const LEASE: &str = "Mars_agent/.claude/locks/deploy-local-k8s.lock/holder";

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Trigger { Engine, Kube }

pub(super) struct Probe { pub ok: bool, pub ms: u64, pub error: Option<String> }

struct Round { engine: Probe, kube: Probe, recovery: Option<Value>, repairs: Vec<Value>, collisions: Vec<Value>, stops_seen: Vec<Value> }

#[derive(Default)]
pub(super) struct DockerWatch {
    loaded: bool,
    last_sample: f64,
    engine_fails: u32,
    kube_fails: u32,
    actions: Vec<f64>,
    engine_up: Option<bool>,
    repair_until: f64,
    repaired: Vec<(String, f64)>,
    /// Pids of stop commands already recorded as `docker_stop_seen`.
    seen_stops: Vec<i64>,
    last: Value,
    worker: Option<Receiver<Round>>,
}

impl DockerWatch {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded { self.load(home); }
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(round) => { self.worker = None; self.absorb(home, now, round); }
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS {
            return Ok(());
        }
        self.last_sample = now;
        let paused = paused(home, now);
        let trigger = due(self.engine_fails, self.kube_fails, &self.actions, now);
        if self.worker.is_none() && !cfg!(test) {
            let trigger = if paused.is_some() { None } else { trigger };
            let repair = now < self.repair_until;
            let skip: Vec<String> = self.repaired.iter().filter(|(_, at)| now - at < REPAIR_WINDOW_SECS).map(|(p, _)| p.clone()).collect();
            let seen = self.seen_stops.clone();
            let home = home.to_path_buf();
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::Builder::new().name("watchdog-docker".into()).spawn(move || {
                let _ = sender.send(run_round(&home, trigger, repair, &skip, &seen));
            })?;
            self.worker = Some(receiver);
        }
        self.write(home, now, paused)
    }

    fn load(&mut self, home: &Path) {
        self.loaded = true;
        if let Some(state) = super::read(&root(home).join("docker-watch.json")) {
            self.actions = state["actions_ms"].as_array().into_iter().flatten()
                .filter_map(Value::as_f64).map(|ms| ms / 1000.0).collect();
        }
    }

    fn absorb(&mut self, home: &Path, now: f64, round: Round) {
        if self.engine_up == Some(false) && round.engine.ok {
            self.repair_until = now + REPAIR_WINDOW_SECS;
        }
        self.engine_up = Some(round.engine.ok);
        if let Some(record) = round.recovery {
            let outcome = record["outcome"].as_str().unwrap_or_default().to_string();
            if !outcome.starts_with("skipped") {
                self.actions.push(now);
                self.engine_fails = 0;
                self.kube_fails = 0;
                if outcome == "recovered" { self.repair_until = now + REPAIR_WINDOW_SECS; }
                keep(home, now, "docker", &record);
                let hint = if record["cause"] == "volume_access_blocked" { "；请处理外置盘访问授权弹窗" } else { "" };
                notify("看门狗：Docker 已死", &format!("原因 {}，处理结果 {outcome}{hint}", record["cause"]));
            }
        }
        for record in round.repairs {
            if let Some(pod) = record["pod"].as_str() { self.repaired.push((pod.to_string(), now)); }
            keep(home, now, "docker-redis", &record);
            notify("看门狗：Redis AOF 修复", &format!("{} {}", record["pod"], record["outcome"]));
        }
        for row in round.stops_seen {
            if let Some(pid) = row["pid"].as_i64() { self.seen_stops.push(pid); }
            crate::log(&format!("docker stop seen: {row}"));
            if let Err(error) = append(home, now, &row) { crate::log(&format!("docker heal log: {error:#}")); }
        }
        let excess = self.seen_stops.len().saturating_sub(64);
        self.seen_stops.drain(..excess);
        for row in round.collisions {
            crate::log(&format!("docker restart collision: {row}"));
            notify("看门狗：Docker 维护被打断", &format!("{} 停了 Docker，{} 又把它启动了；停 Docker 前先运行 cpu-watchdog docker-pause",
                row["stopped_by"]["session"].as_str().unwrap_or("?"), row["started_by"]["session"].as_str().or(row["started_by"]["actor"].as_str()).unwrap_or("?")));
        }
        self.repaired.retain(|(_, at)| now - at < REPAIR_WINDOW_SECS);
        self.actions.retain(|at| now - at < DAY_SECS);
        self.engine_fails = if round.engine.ok { 0 } else { self.engine_fails + 1 };
        self.kube_fails = if round.kube.ok { 0 } else { self.kube_fails + 1 };
        let probe = |p: &Probe| json!({"ok": p.ok, "ms": p.ms, "error": p.error});
        self.last = json!({"engine": probe(&round.engine), "kube": probe(&round.kube)});
    }

    fn write(&self, home: &Path, now: f64, paused: Option<Value>) -> Result<()> {
        let capped = self.actions.len() >= DAILY_ACTIONS;
        write_json(&root(home).join("docker-watch.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "probes": self.last, "engine_fails": self.engine_fails, "kube_fails": self.kube_fails,
            "probe_running": self.worker.is_some(), "paused": paused, "daily_cap_reached": capped,
            "actions_ms": self.actions.iter().map(|at| at * 1000.0).collect::<Vec<_>>(),
            "repair_until_ms": (self.repair_until > now).then_some(self.repair_until * 1000.0),
            "analysis_route": "recovery-retro", "analysis_queue": "analysis-queue.json",
            "threshold": {"engine_fails": ENGINE_FAILS, "kube_fails": KUBE_FAILS, "sample_s": SAMPLE_SECS,
                "cooldown_s": COOLDOWN_SECS, "daily_actions": DAILY_ACTIONS, "repair_window_s": REPAIR_WINDOW_SECS}}))
    }
}

/// The recovery this round may start, from the failures before it and the recent actions.
pub(super) fn due(engine_fails: u32, kube_fails: u32, actions: &[f64], now: f64) -> Option<Trigger> {
    let recent = actions.iter().filter(|at| now - **at < DAY_SECS).count();
    let cooled = actions.iter().all(|at| now - at >= COOLDOWN_SECS);
    if recent >= DAILY_ACTIONS || !cooled { return None; }
    if engine_fails >= ENGINE_FAILS { return Some(Trigger::Engine); }
    (kube_fails >= KUBE_FAILS).then_some(Trigger::Kube)
}

fn run_round(home: &Path, trigger: Option<Trigger>, repair: bool, skip: &[String], seen: &[i64]) -> Round {
    let stops_seen = if cfg!(test) { Vec::new() } else { restarts::running_stops(seen, crate::unix_now() * 1000.0) };
    let remote = remote::host(home);
    let probe_engine = || match &remote { Some(host) => remote::probe_engine(home, host), None => heal::probe_engine(home) };
    let (mut engine, mut kube) = (probe_engine(), heal::probe_kube(home));
    let failing = match trigger {
        Some(Trigger::Engine) => !engine.ok,
        Some(Trigger::Kube) => engine.ok && !kube.ok,
        None => false,
    };
    // Attributed before the restart adds its own VM start; recovery_retro closes a known node by script.
    let attribution = (failing && remote.is_none() && !cfg!(test)).then(|| restarts::current(home, crate::unix_now() * 1000.0));
    // A pause written after this round was spawned still wins (2026-11-03 16:25:23.490: the pause
    // landed 0.42 s before this round ran the remote start task during a founder-approved restart).
    let recovery = failing.then(|| match (skipped_if_paused(home, crate::unix_now()), &remote) {
        (Some(skipped), _) => skipped,
        (None, Some(host)) => remote::recover(home, host),
        (None, None) => heal::recover(home, engine.ok),
    }).map(|mut record| {
        if let Some(a) = &attribution {
            record["attribution"] = json!({"node": a["node"], "stopped_by": a["stopped_by"], "stop_ms": a["stop_ms"]});
        }
        record
    });
    if recovery.is_some() { (engine, kube) = (probe_engine(), heal::probe_kube(home)); }
    let recovered = recovery.as_ref().is_some_and(|r| r["outcome"] == "recovered");
    let repairs = if (repair || recovered) && kube.ok { heal::repair_redis(home, skip) } else { Vec::new() };
    let collisions = if cfg!(test) { Vec::new() } else { restarts::refresh(home, crate::unix_now() * 1000.0) };
    Round { engine, kube, recovery, repairs, collisions, stops_seen }
}

/// The active pause: `cpu-watchdog docker-pause` before deliberate Docker maintenance, or a held
/// `local-k8s` deploy lease (`deploy_lease.sh acquire --resource=local-k8s`, file
/// `holder|acquired_s|ttl_s|pid|session`), which the 2026-10-24 disk cutover held while the
/// watchdog and another session restarted Docker under it.
pub(super) fn paused(home: &Path, now: f64) -> Option<Value> {
    if let Some(pause) = super::read(&root(home).join(PAUSE)).filter(|p| p["until_ms"].as_f64().is_some_and(|u| u > now * 1000.0)) {
        return Some(pause);
    }
    let line = std::fs::read_to_string(home.join(LEASE)).ok()?;
    let fields: Vec<&str> = line.trim().split('|').collect();
    let (at, ttl) = (fields.get(1)?.parse::<f64>().ok()?, fields.get(2)?.parse::<f64>().ok()?);
    (at + ttl > now).then(|| json!({"at_ms": at * 1000.0, "until_ms": (at + ttl) * 1000.0,
        "reason": format!("local-k8s lease held by {}", fields[0]), "session": fields.get(4)}))
}

/// The record of a recovery that an active pause skips; `absorb` ignores `skipped` outcomes.
pub(super) fn skipped_if_paused(home: &Path, now: f64) -> Option<Value> {
    paused(home, now).map(|pause| json!({"at_ms": now * 1000.0, "kind": "docker_recovery", "outcome": "skipped: paused", "pause": pause}))
}

/// `docker-restarts [days]`: every VM start of the window with its attribution.
pub(super) fn restarts_cli(home: &Path, args: Vec<String>) -> Result<()> {
    restarts::cli(home, args)
}

/// `docker-pause <minutes> [reason ...]`; `docker-pause 0` resumes.
pub(super) fn pause_cli(home: &Path, args: Vec<String>) -> Result<()> {
    let minutes: f64 = args.first().and_then(|m| m.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("usage: cpu-watchdog docker-pause <minutes> [reason ...]"))?;
    anyhow::ensure!((0.0..=1440.0).contains(&minutes), "minutes must be between 0 and 1440");
    let pause = hold(home, crate::unix_now(), minutes * 60.0, &args[1..].join(" "))?;
    println!("{}", serde_json::to_string_pretty(&pause)?);
    Ok(())
}

/// Write the pause for `seconds` from `now` (0 releases it) and log a `docker_pause` row.
fn hold(home: &Path, now: f64, seconds: f64, reason: &str) -> Result<Value> {
    let pause = json!({"at_ms": now * 1000.0, "until_ms": (now + seconds) * 1000.0, "reason": reason});
    write_json(&root(home).join(PAUSE), &pause)?;
    append(home, now, &json!({"kind": "docker_pause", "at_ms": pause["at_ms"], "until_ms": pause["until_ms"], "reason": pause["reason"]}))?;
    Ok(pause)
}

/// `remote-reboot [--dry-run] <reason ...>`: a planned reboot of the remote beta host (`reboot`).
pub(super) fn remote_reboot_cli(home: &Path, args: Vec<String>) -> Result<()> {
    reboot::planned_cli(home, args)
}

fn keep(home: &Path, now: f64, kind: &str, record: &Value) {
    crate::log(&format!("docker watch: {record}"));
    if let Err(error) = append(home, now, record) { crate::log(&format!("docker heal log: {error:#}")); }
    if let Err(error) = crate::recovery_retro::record(home, kind, record) { crate::log(&format!("docker retro: {error:#}")); }
}

fn append(home: &Path, now: f64, record: &Value) -> Result<()> {
    let directory = root(home).join("docker-heals");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let day = (now / DAY_SECS).floor() * DAY_SECS * 1000.0;
    std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(directory.join(format!("{}.ndjson", day as u64)))?
        .write_all(format!("{record}\n").as_bytes())?;
    Ok(())
}

fn notify(title: &str, body: &str) {
    if cfg!(test) { return; }
    crate::dispatch::notify(title, body);
}

#[cfg(test)]
#[path = "docker_watch/tests.rs"]
mod tests;
