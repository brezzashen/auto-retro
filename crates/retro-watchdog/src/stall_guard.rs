//! Liveness of the resident loop. The loop is one thread: a call that never returns (an unanswered
//! privacy prompt inside `open`, a child that never exits) stops every sample, incident and heal
//! while the process stays alive, so launchd's KeepAlive never restarts it. On 2026-11-11 16:04 SGT
//! the loop went silent for 21.6 h until an unrelated install replaced the process, and no stack of
//! the stuck thread survived. One guard thread watches the loop's beat. When the beat has not moved
//! for the limit it samples this process, writes `stall-records/<ms>.json` beside the sample, and
//! exits so launchd starts a fresh process; that process reports the record as a forced recovery
//! (`recovery_retro.rs`). `cpu-watchdog stall-selftest` proves the path on this host.
//!
//! The record must explain itself: the installed binary is stripped, so its sample holds addresses,
//! not names. Every telemetry route marks a `step` before it runs and the record lists the child
//! processes with their age. The 2026-11-11 stall would read step "search-retro" with a
//! `script-guard classify` child: the loop was in write(2) to that child, both 64 KiB pipes full,
//! after a rewrite of the search log made one window of the whole log (reproduced 2026-11-12 with
//! the binary of that day). `cpu-watchdog stall-drill` (`drill.rs`) blocks the resident loop the same
//! way on request and checks the restart, so the whole path can be proven on the live daemon.
use crate::monitor::{root, write_json};
use anyhow::{bail, Context, Result};
use serde_json::json;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Seconds without a beat before the loop counts as stalled: 40 missed 15 s ticks.
pub(crate) const STALL_SECS: u64 = 600;
const CHECK: Duration = Duration::from_secs(15);
const SAMPLE_SECS: &str = "3";
const SAMPLE_WAIT: Duration = Duration::from_secs(30);
const CHILDREN_WAIT: Duration = Duration::from_secs(10);
const COMMAND_CHARS: usize = 300;
const PS_ARGUMENTS: [&str; 2] = ["-axo", "pid=,ppid=,etime=,command="];
/// sysexits EX_TEMPFAIL; launchd restarts a KeepAlive job after any exit.
const EXIT_CODE: i32 = 75;
const RECORDS: &str = "stall-records";
/// Rewritten by the guard thread while the loop beats; read by the cpu-build-gate collector.
const BEAT_FILE: &str = "loop-beat.json";

static BEATS: AtomicU64 = AtomicU64::new(0);
static STAGE: AtomicU8 = AtomicU8::new(0);
/// The telemetry route or loop part that runs now; empty between them.
static STEP: Mutex<&'static str> = Mutex::new("");

pub(crate) mod drill;

/// The part of one loop pass that is running.
#[derive(Clone, Copy)]
pub(crate) enum Stage {
    Rotate = 1,
    Tick = 2,
    Sweep = 3,
    Sleep = 4,
}

fn stage_name(code: u8) -> &'static str {
    match code {
        1 => "rotate",
        2 => "tick",
        3 => "sweep",
        4 => "sleep",
        _ => "start",
    }
}

/// Marks progress of the resident loop.
pub(crate) fn beat(stage: Stage) {
    STAGE.store(stage as u8, Ordering::Relaxed);
    mark("");
}

/// Names the route about to run inside the current stage. It counts as a beat, so the limit is the
/// longest silence of one route, not of a whole pass.
pub(crate) fn step(name: &'static str) {
    mark(name);
}

fn mark(name: &'static str) {
    *STEP.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = name;
    BEATS.fetch_add(1, Ordering::Relaxed);
}

fn current_step() -> &'static str {
    *STEP.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How long the beat count has stood still. `Instant` does not advance while the host sleeps, so a
/// night of sleep is not a stall.
struct Watch {
    seen: u64,
    since: Instant,
}

impl Watch {
    fn stalled(&mut self, beats: u64, now: Instant, limit: Duration) -> Option<Duration> {
        if beats != self.seen {
            self.seen = beats;
            self.since = now;
            return None;
        }
        let idle = now.duration_since(self.since);
        (idle >= limit).then_some(idle)
    }
}

/// Starts the guard thread: after `limit` without a beat it records the stall under `directory` and exits.
/// While the beat moves it rewrites `beat_file`, the sign of life the cpu-build-gate collector reads
/// (`crates/cpu-build-gate/src/watchdog_beat.rs`): a file that stops changing means this guard did not
/// act, because the whole process is stopped, the guard died or the job is not running.
pub(crate) fn start(directory: PathBuf, limit: Duration, beat_file: Option<PathBuf>) -> Result<()> {
    std::thread::Builder::new().name("watchdog-stall-guard".into()).spawn(move || {
        let mut watch = Watch { seen: BEATS.load(Ordering::Relaxed), since: Instant::now() };
        let mut written = None;
        loop {
            let beats = BEATS.load(Ordering::Relaxed);
            if let Some(path) = beat_file.as_ref().filter(|_| written != Some(beats)) {
                let _ = write_json(path, &json!({"at_ms": crate::unix_now() * 1000.0, "pid": std::process::id(), "beats": beats}));
                written = Some(beats);
            }
            std::thread::sleep(CHECK.min(limit));
            let Some(idle) = watch.stalled(BEATS.load(Ordering::Relaxed), Instant::now(), limit) else { continue };
            let (stage, step) = (stage_name(STAGE.load(Ordering::Relaxed)), current_step());
            let record = capture(&directory, idle, limit, stage, step, crate::unix_now() * 1000.0);
            // stderr, not `crate::log`: the stuck thread may hold the stdout lock.
            eprintln!("{} stall guard: loop silent {} s in stage {stage} step {}; record {}; exiting for a launchd restart",
                crate::local_time(crate::unix_now() as i64), idle.as_secs(), if step.is_empty() { "-" } else { step }, record.display());
            drill::end();
            // SAFETY: _exit only ends the process. `exit` would run handlers that can wait on the stuck thread.
            unsafe { libc::_exit(EXIT_CODE) }
        }
    }).context("spawn stall guard")?;
    Ok(())
}

/// Samples this process and writes the stall record; returns the record path.
fn capture(directory: &Path, idle: Duration, limit: Duration, stage: &str, step: &str, now_ms: f64) -> PathBuf {
    let pid = std::process::id();
    let sample_path = directory.join(format!("{now_ms:.0}.sample.txt"));
    let made = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(directory);
    let children = children(pid, &directory.join(format!("{now_ms:.0}.ps.tmp")));
    let sampled = made.map_err(anyhow::Error::from).and_then(|()| sample(pid, &sample_path));
    let record = json!({
        "at_ms": now_ms, "outcome": "restarted", "name": "cpu-watchdog", "pid": pid, "stage": stage,
        "step": step, "drill": drill::active(),
        "children": children.as_ref().ok(), "children_error": children.as_ref().err().map(|error| format!("{error:#}")),
        "stalled_secs": idle.as_secs(), "limit_secs": limit.as_secs(),
        "sample": sampled.as_ref().ok().map(|()| sample_path.display().to_string()),
        "sample_error": sampled.err().map(|error| format!("{error:#}")),
    });
    let path = directory.join(format!("{now_ms:.0}.json"));
    if let Err(error) = write_json(&path, &record) {
        eprintln!("stall guard: record {}: {error:#}", path.display());
    }
    path
}

/// `/usr/bin/sample` of `pid` into `output`, bounded by SAMPLE_WAIT so the guard itself cannot hang.
fn sample(pid: u32, output: &Path) -> Result<()> {
    if cfg!(test) {
        bail!("not sampled in tests");
    }
    let mut command = Command::new("/usr/bin/sample");
    command.args([pid.to_string().as_str(), SAMPLE_SECS, "-file"]).arg(output).stdout(Stdio::null());
    bounded(command, "sample", SAMPLE_WAIT)
}

/// Runs `command` to its end or kills it after `wait`. Its output goes to a file or nowhere, never
/// to a pipe this thread would have to drain.
fn bounded(mut command: Command, name: &str, wait: Duration) -> Result<()> {
    let mut child = command.stdin(Stdio::null()).stderr(Stdio::null()).spawn().with_context(|| format!("spawn {name}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait().with_context(|| format!("wait for {name}"))? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => bail!("{name} exited with {status}"),
            None if started.elapsed() < wait => std::thread::sleep(Duration::from_millis(200)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("{name} did not finish within {} s", wait.as_secs());
            }
        }
    }
}

/// The child processes of `pid` with their age: a loop blocked on a child names it here even when
/// the sample holds no symbols. `ps` writes to `scratch`, which is removed.
fn children(pid: u32, scratch: &Path) -> Result<Vec<serde_json::Value>> {
    if cfg!(test) {
        bail!("not listed in tests");
    }
    let file = std::fs::File::create(scratch).with_context(|| format!("create {}", scratch.display()))?;
    let mut command = Command::new("/bin/ps");
    command.args(PS_ARGUMENTS).stdout(file);
    let listed = bounded(command, "ps", CHILDREN_WAIT).and_then(|()| std::fs::read_to_string(scratch).context("read ps output"));
    let _ = std::fs::remove_file(scratch);
    Ok(child_rows(&listed?, pid))
}

/// Rows of `ps -axo pid=,ppid=,etime=,command=` whose parent is `pid`, without that `ps` itself.
fn child_rows(listing: &str, pid: u32) -> Vec<serde_json::Value> {
    listing.lines().filter_map(|line| {
        let mut fields = line.split_whitespace();
        let (child, parent, age) = (fields.next()?.parse::<u32>().ok()?, fields.next()?.parse::<u32>().ok()?, fields.next()?);
        let command: String = fields.collect::<Vec<_>>().join(" ").chars().take(COMMAND_CHARS).collect();
        (parent == pid && !command.ends_with(&PS_ARGUMENTS.join(" "))).then(|| json!({"pid": child, "etime": age, "command": command}))
    }).collect()
}

/// The resident loop's guard, with stall records under the watchdog state directory.
pub(crate) fn guard_resident(home: &Path) -> Result<()> {
    start(root(home).join(RECORDS), Duration::from_secs(STALL_SECS), Some(root(home).join(BEAT_FILE)))
}

/// Reports the stall records earlier processes left as forced recoveries, once each.
pub(crate) fn report_pending(home: &Path) {
    let Ok(entries) = std::fs::read_dir(root(home).join(RECORDS)) else { return };
    for path in entries.flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|e| e == "json")) {
        let marker = path.with_extension("reported");
        if marker.exists() {
            continue;
        }
        let Some(record) = crate::telemetry::read_state(&path) else { continue };
        if record["drill"] == true {
            // A requested drill proves the guard; it is not a recovery to review.
            let _ = std::fs::write(&marker, b"");
            let _ = std::fs::remove_file(root(home).join(drill::ACTIVE_FILE));
            crate::log(&format!("stall guard: drill record {} (no forced recovery)", path.display()));
            continue;
        }
        match crate::recovery_retro::record(home, "watchdog", &record) {
            Ok(()) => {
                let _ = std::fs::write(&marker, b"");
                crate::log(&format!("stall guard: reported {} as a forced recovery", path.display()));
            }
            Err(error) => crate::log(&format!("stall guard: report {}: {error:#}", path.display())),
        }
    }
}

/// `stall-selftest [limit secs]`: beats three times, then blocks this thread for good. The guard
/// must sample the process, write a record under the printed directory and exit 75. Records go to
/// a scratch directory, so no forced recovery is reported.
pub(crate) fn selftest(arguments: Vec<String>) -> Result<()> {
    let limit: u64 = match arguments.first() {
        Some(text) => text.parse().context("stall-selftest takes a limit in seconds")?,
        None => 5,
    };
    let directory = std::env::temp_dir().join(format!("cpu-watchdog-stall-selftest-{}", std::process::id()));
    start(directory.clone(), Duration::from_secs(limit), None)?;
    for _ in 0..3 {
        beat(Stage::Tick);
        std::thread::sleep(Duration::from_secs(1));
    }
    eprintln!("stall-selftest: loop thread blocked; expect exit {EXIT_CODE} after {limit} s and a record in {}", directory.display());
    loop {
        std::thread::park();
    }
}

#[cfg(test)]
#[path = "stall_guard/tests.rs"]
mod tests;
