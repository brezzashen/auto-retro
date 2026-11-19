//! Storybook hang watch (user request, 2026-10-22): heal each hang, and hand repeated hangs to a
//! session that finds and fixes the cause.
//!
//! 1. A wedged story page is closed by `headless_heal.rs`, which frees its renderer at once
//!    (measured: a page spinning one core exits within 1 s of `/json/close`).
//! 2. The shared CRM Storybook server itself is probed every SAMPLE_SECS (`/index.json`, answered
//!    within SERVER_TIMEOUT). After SERVER_FAILS failed probes in a row it is force-restarted:
//!    `scripts/startup/manage_storybook_catalogs.sh stop crm`, then a kickstart of its own
//!    LaunchAgent, at most once per RESTART_COOLDOWN_SECS.
//! 3. Every page/server recovery enters `recovery_retro` and the shared durable analysis queue.
//!    The rolling event window is display-only; it cannot discard a delayed review request.
//!
//! Server restarts go to `storybook-heals/<utc-day-ms>.ndjson`; state to `storybook-watch.json`.
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

pub(super) const SAMPLE_SECS: f64 = 30.0;
const SERVER: &str = "http://127.0.0.1:6006/index.json";
const SERVER_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const SERVER_FAILS: u32 = 4;
pub(super) const RESTART_COOLDOWN_SECS: f64 = 900.0;
pub(super) const REPEAT_WINDOW_SECS: f64 = 1800.0;
const DAY_MS: f64 = 86_400_000.0;
const REPO: &str = "/path/to/workspace";
const MANAGER: &str = "scripts/startup/manage_storybook_catalogs.sh";
/// The PATH of the `local.autoretro.storybook-catalogs` LaunchAgent that starts the server at login.
const MANAGER_PATH: &str = "/home/user/.hermes/node/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
const STORYBOOK_JOB: &str = "local.autoretro.storybook-catalogs";
/// How long a restart waits for the started server to pass the manager's check.
const START_WAIT: Duration = Duration::from_secs(120);
/// cpu-build-gate's busy rule (policy.rs THRESHOLD, WINDOW_MS): a whole-host sample at or above
/// HOST_BUSY_PERCENT within the last HOST_BUSY_WINDOW_MS. Probes time out under that load and a cold
/// start misses START_WAIT, so a due restart waits until the host leaves it.
const HOST_BUSY_PERCENT: f64 = 95.0;
const HOST_BUSY_WINDOW_MS: f64 = 30_000.0;
const CPU_HISTORY: &str = ".local/state/mars-cpu-build-gate/history.json";
const ORIGINS: [&str; 2] = ["http://localhost:6006/", "http://127.0.0.1:6006/"];

struct Round { ok: bool, ms: u64, error: Option<String>, restart: Option<Value> }

#[derive(Default)]
pub(super) struct StorybookWatch {
    loaded: bool,
    last_sample: f64,
    failures: u32,
    last_restart: f64,
    last_server: Value,
    worker: Option<Receiver<Round>>,
}

impl StorybookWatch {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded {
            self.load(home);
        }
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
        if self.worker.is_none() && !cfg!(test) {
            let mut restart = restart_due(self.failures, self.last_restart, now);
            if restart && super::read(&home.join(CPU_HISTORY)).is_some_and(|h| host_busy(&h, now * 1000.0)) {
                crate::log("storybook watch: restart due but host CPU busy; deferred");
                restart = false;
            }
            let home = home.to_path_buf();
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::Builder::new().name("watchdog-storybook".into()).spawn(move || {
                let _ = sender.send(probe_and_recover(restart, probe_server, || restart_server(&home)));
            })?;
            self.worker = Some(receiver);
        }
        let events = window_events(home, now);
        self.write(home, now, &events)
    }

    fn load(&mut self, home: &Path) {
        self.loaded = true;
        if let Some(state) = super::read(&root(home).join("storybook-watch.json")) {
            self.last_restart = state["last_restart_ms"].as_f64().unwrap_or(0.0) / 1000.0;
        }
    }

    fn absorb(&mut self, home: &Path, now: f64, round: Round) {
        if let Some(record) = round.restart {
            crate::log(&format!("storybook watch: {record}"));
            self.last_restart = now;
            self.failures = 0;
            if let Err(error) = append(home, now, &record) { crate::log(&format!("storybook heal log: {error:#}")); }
            if let Err(error) = crate::recovery_retro::record(home, "storybook", &record) { crate::log(&format!("storybook retro: {error:#}")); }
            notify("看门狗：Storybook 卡死", &format!("Storybook 6006 连续 {SERVER_FAILS} 次无响应，已强制重启：{}", record["outcome"]));
        }
        self.failures = if round.ok { 0 } else { self.failures + 1 };
        self.last_server = json!({"ok": round.ok, "ms": round.ms, "error": round.error, "failures": self.failures});
    }

    fn write(&self, home: &Path, now: f64, events: &[Value]) -> Result<()> {
        write_json(&root(home).join("storybook-watch.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "server": self.last_server, "probe_running": self.worker.is_some(),
            "last_restart_ms": (self.last_restart > 0.0).then_some(self.last_restart * 1000.0),
            "window": {"events": events.len(), "stories": stories(events)},
            "analysis_route": "recovery-retro", "analysis_queue": "analysis-queue.json",
            "threshold": {"server_fails": SERVER_FAILS, "server_timeout_s": SERVER_TIMEOUT.as_secs(),
                "restart_cooldown_s": RESTART_COOLDOWN_SECS,
                "repeat_window_s": REPEAT_WINDOW_SECS, "review_every_recovery": true}}))
    }
}

pub(super) fn restart_due(failures: u32, last_restart: f64, now: f64) -> bool {
    failures >= SERVER_FAILS && now - last_restart >= RESTART_COOLDOWN_SECS
}

pub(super) fn host_busy(history: &Value, now_ms: f64) -> bool {
    history["samples"].as_array().is_some_and(|samples| samples.iter().any(|s| {
        s["end"].as_f64().is_some_and(|end| end <= now_ms + 1000.0 && now_ms - end <= HOST_BUSY_WINDOW_MS)
            && s["percent"].as_f64().is_some_and(|p| p >= HOST_BUSY_PERCENT)
    }))
}

fn probe_and_recover(due: bool, mut probe: impl FnMut() -> (bool, u64, Option<String>),
    recover: impl FnOnce() -> Value) -> Round {
    let (ok, ms, error) = probe();
    // The failed probe's own error tells an index error (`status code 500`, server alive) from a
    // server that does not answer (`timed out`), which the hung capture alone cannot.
    let restart = (due && !ok).then(|| {
        let mut record = recover();
        record["probe"] = json!({"ms": ms, "error": &error});
        record
    });
    let (ok, ms, error) = if restart.is_some() { probe() } else { (ok, ms, error) };
    Round { ok, ms, error, restart }
}

/// Storybook heal events inside the window: pages `headless_heal` closed and server restarts.
pub(super) fn window_events(home: &Path, now: f64) -> Vec<Value> {
    let since = (now - REPEAT_WINDOW_SECS) * 1000.0;
    let day = |ms: f64| (ms / DAY_MS).floor() * DAY_MS;
    let mut events = Vec::new();
    for (directory, kind) in [("headless-heals", "page_closed"), ("storybook-heals", "server_restart")] {
        let mut days = vec![day(since), day(now * 1000.0)];
        days.dedup();
        for start in days {
            let path = root(home).join(directory).join(format!("{}.ndjson", start as u64));
            for line in std::fs::read_to_string(path).unwrap_or_default().lines() {
                let Ok(record) = serde_json::from_str::<Value>(line) else { continue };
                let at = record["at_ms"].as_f64().unwrap_or(0.0);
                let storybook = kind == "server_restart" || record["url"].as_str().is_some_and(|url| ORIGINS.iter().any(|o| url.starts_with(o)));
                if at >= since && storybook {
                    events.push(json!({"kind": kind, "at_ms": at, "url": record["url"], "outcome": record["outcome"]}));
                }
            }
        }
    }
    events
}

/// `{story id: closures}` of the window, most frequent first.
pub(super) fn stories(events: &[Value]) -> Value {
    let mut counts: Vec<(String, u64)> = Vec::new();
    for url in events.iter().filter_map(|e| e["url"].as_str()) {
        let story = url.split("id=").nth(1).and_then(|rest| rest.split(['&', '#']).next()).unwrap_or(url).to_string();
        match counts.iter_mut().find(|(s, _)| *s == story) {
            Some((_, n)) => *n += 1,
            None => counts.push((story, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    Value::Array(counts.into_iter().map(|(story, n)| json!({"story": story, "closed": n})).collect())
}

fn probe_server() -> (bool, u64, Option<String>) {
    let started = Instant::now();
    let result = ureq::AgentBuilder::new().timeout(SERVER_TIMEOUT).build().get(SERVER).call();
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(_) => (true, ms, None),
        Err(error) => (false, ms, Some(error.to_string())),
    }
}

/// Stop the CRM catalog through its manager, which signals only the process it recorded, then start
/// it through its own LaunchAgent (STORYBOOK_JOB, `AbandonProcessGroup`). A server started from
/// this thread would belong to the watchdog's job and die at the watchdog's next reload (seen
/// 2026-10-22 18:19). `start` truncates the server log, so the hung server's log is kept first, and
/// the hung process's state (`storybook_watch_hung.rs`) is captured before `stop` ends it.
fn restart_server(home: &Path) -> Value {
    let started_at_ms = crate::unix_now() * 1000.0;
    let started = Instant::now();
    let kept = keep_server_log(home).map_or_else(|error| json!(format!("failed: {error:#}")), |path| json!(path));
    let hung = hung::capture(Path::new(REPO), &root(home).join("storybook-heals"), started_at_ms as u64);
    let steps = cycle();
    let ok = steps.iter().all(|s| s["exit"] == 0);
    json!({"at_ms": crate::unix_now() * 1000.0, "started_at_ms": started_at_ms, "kind": "server_restart", "fails": SERVER_FAILS,
        "seconds": started.elapsed().as_secs(), "steps": steps, "server_log": kept, "hung": hung,
        "outcome": if ok { "restarted".to_string() } else { "failed".to_string() }})
}

/// Restarts the CRM catalog when its recorded server started before `before_s` (Unix seconds):
/// after an fseventsd restart its file watcher stays registered but gets no events, so hot reload
/// stops (perf session 1794671950116, 2026-11-15: pid 58874 until restarted as 9224). A catalog that
/// is not running, or was started after `before_s`, is left alone.
pub(crate) fn restart_if_older(before_s: f64) -> Value {
    let pid_file = PathBuf::from(REPO).join(".claude/locks/storybook-catalogs/crm.pid");
    let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|s| s.trim().parse::<i32>().ok()) else {
        return json!({"outcome": "skipped", "reason": "no crm.pid"});
    };
    let start = crate::procs::snapshot(crate::unix_now).ok()
        .and_then(|(_, procs)| procs.into_iter().find(|p| p.pid == pid).map(|p| p.start));
    match start {
        None => json!({"outcome": "skipped", "reason": format!("crm pid {pid} not running")}),
        Some(start) if start as f64 >= before_s => json!({"outcome": "skipped", "reason": format!("crm pid {pid} started after the restart")}),
        Some(_) => {
            let started = Instant::now();
            let steps = cycle();
            let ok = steps.iter().all(|s| s["exit"] == 0);
            json!({"outcome": if ok { "restarted" } else { "failed" }, "old_pid": pid,
                "new_pid": std::fs::read_to_string(&pid_file).ok().and_then(|s| s.trim().parse::<i32>().ok()),
                "seconds": started.elapsed().as_secs(), "steps": steps})
        }
    }
}

/// stop through the manager, kickstart STORYBOOK_JOB, then `check` until START_WAIT.
fn cycle() -> Vec<Value> {
    let manager = |action: &str| Command::new("/bin/bash").args([MANAGER, action, "crm"]).current_dir(REPO)
        .env("PATH", MANAGER_PATH).stdin(Stdio::null()).output();
    let step = |action: &str, out: std::io::Result<std::process::Output>| match out {
        Ok(out) => json!({"action": action, "exit": out.status.code(),
            "output": format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).chars().take(600).collect::<String>()}),
        Err(error) => json!({"action": action, "exit": null, "output": format!("{error:#}")}),
    };
    let started = Instant::now();
    let mut steps = vec![step("stop", manager("stop"))];
    let job = format!("gui/{}/{STORYBOOK_JOB}", unsafe { libc::getuid() });
    steps.push(step("kickstart", Command::new("/bin/launchctl").args(["kickstart", &job]).stdin(Stdio::null()).output()));
    let mut check = json!({"action": "check", "exit": null, "output": "not ready"});
    while started.elapsed() < START_WAIT {
        std::thread::sleep(Duration::from_secs(3));
        check = step("check", manager("check"));
        if check["exit"] == 0 { break; }
    }
    steps.push(check);
    steps
}

fn keep_server_log(home: &Path) -> Result<PathBuf> {
    let directory = root(home).join("storybook-heals");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let path = directory.join(format!("{}-crm.log", (crate::unix_now() * 1000.0) as u64));
    let log = std::fs::read(PathBuf::from(REPO).join(".claude/locks/storybook-catalogs/crm.log"))?;
    std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(&path)?.write_all(&log)?;
    Ok(path)
}

fn append(home: &Path, now: f64, record: &Value) -> Result<()> {
    let directory = root(home).join("storybook-heals");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let day = ((now * 1000.0) / DAY_MS).floor() * DAY_MS;
    std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(directory.join(format!("{}.ndjson", day as u64)))?
        .write_all(format!("{record}\n").as_bytes())?;
    Ok(())
}

fn notify(title: &str, body: &str) {
    if cfg!(test) {
        return;
    }
    crate::dispatch::notify(title, body);
}

#[path = "storybook_watch_hung.rs"]
mod hung;

#[cfg(test)]
#[path = "storybook_watch_tests.rs"]
mod tests;
