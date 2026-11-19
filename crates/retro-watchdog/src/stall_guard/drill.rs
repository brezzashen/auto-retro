//! `cpu-watchdog stall-drill`: a real stall of the resident loop on request, to prove the guard on
//! the live daemon after a change. The CLI writes `stall-drill.json`; the loop takes it at the start
//! of its next tick and blocks the way it did on 2026-11-11: write(2) into the full stdin pipe of a
//! child that never reads. The guard must then sample, write a record with `drill: true` and exit
//! 75; launchd starts a new process, which marks the record reported without a forced recovery.
//! The CLI waits for that marker and prints the record. The host has no watchdog for the length of
//! the limit (STALL_SECS), so run it only to accept a guard change.
//!
//! `stall-drill --stop` proves the outer check instead: the loop stops its whole process (SIGSTOP),
//! so this guard cannot act, and the cpu-build-gate collector must find the beat file unchanged for
//! EXTERNAL_SECS, write the record and restart the job (`crates/cpu-build-gate/src/watchdog_beat.rs`).
//! `stall-drill-active.json` tells the collector the stop is a drill; a CLI that gives up continues
//! the stopped process.
use crate::monitor::{root, write_json};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

const REQUEST: &str = "stall-drill.json";
/// Names the process a `--stop` drill stopped, so the collector marks its record as a drill.
pub(super) const ACTIVE_FILE: &str = "stall-drill-active.json";
/// The collector's limit (`watchdog_beat.rs` LIMIT_SECS); the CLI waits this long plus RESTART_WAIT.
const EXTERNAL_SECS: u64 = 900;
/// A request the loop did not take within this time is dropped: a watchdog that starts later must
/// not stall on a forgotten file.
const REQUEST_FRESH_MS: f64 = 120_000.0;
/// The child ends by itself after this long if nothing kills it.
const CHILD_SECS: &str = "3600";
/// Time allowed after the limit for the sample, the exit, launchd's restart and the marker.
const RESTART_WAIT: Duration = Duration::from_secs(180);

static ACTIVE: AtomicBool = AtomicBool::new(false);
static CHILD: AtomicI32 = AtomicI32::new(0);
static ENDING: AtomicBool = AtomicBool::new(false);

/// True while the loop is blocked by a drill; the record carries it.
pub(super) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Ends the drill's child before the guard exits, so no orphan is left behind.
pub(super) fn end() {
    let child = CHILD.load(Ordering::Relaxed);
    ENDING.store(true, Ordering::Relaxed);
    if child > 0 {
        // SAFETY: signals the pid this process spawned for the drill.
        unsafe { libc::kill(child, libc::SIGKILL) };
    }
}

/// Whether a request written at `requested_ms` is still to be honoured at `now_ms`.
fn fresh(requested_ms: f64, now_ms: f64) -> bool {
    requested_ms > 0.0 && now_ms >= requested_ms && now_ms - requested_ms <= REQUEST_FRESH_MS
}

/// Called by the resident loop once per pass: takes a pending request and, if it is fresh, never returns.
pub(crate) fn take(home: &Path) {
    let path = root(home).join(REQUEST);
    let Some(request) = crate::telemetry::read_state(&path) else { return };
    let _ = std::fs::remove_file(&path);
    if !fresh(request["requested_at_ms"].as_f64().unwrap_or(0.0), crate::unix_now() * 1000.0) {
        crate::log("stall drill: dropped a request older than 120 s");
        return;
    }
    if request["mode"] == "stop" {
        return stop(home);
    }
    crate::log("stall drill: blocking the loop in write(2) on a full pipe; the stall guard must restart this process");
    ACTIVE.store(true, Ordering::Relaxed);
    super::step("stall-drill");
    let error = block();
    while ENDING.load(Ordering::Relaxed) {
        std::thread::park(); // The guard ended the child and is about to exit.
    }
    crate::log(&format!("stall drill: the pipe did not block ({error:#}); parking the loop instead"));
    loop {
        std::thread::park();
    }
}

/// Stops the whole process, guard thread included; returns only if something continues it.
fn stop(home: &Path) {
    let active = root(home).join(ACTIVE_FILE);
    let _ = write_json(&active, &json!({"pid": std::process::id(), "at_ms": crate::unix_now() * 1000.0, "mode": "stop"}));
    crate::log("stall drill: stopping the whole process (SIGSTOP); the cpu-build-gate collector must restart it");
    // A guard that wakes first after a continue sees a long silence; its record is then a drill too.
    ACTIVE.store(true, Ordering::Relaxed);
    // SAFETY: raise only signals this process.
    unsafe { libc::raise(libc::SIGSTOP) };
    super::step("stall-drill-continued");
    ACTIVE.store(false, Ordering::Relaxed);
    let _ = std::fs::remove_file(&active);
    crate::log("stall drill: continued without a restart");
}

/// Writes into the stdin of a child that never reads it; returns only if the write fails.
fn block() -> anyhow::Error {
    let mut command = Command::new("/bin/sleep");
    command.arg(CHILD_SECS);
    match fill(command, &CHILD) {
        Ok(()) => anyhow::anyhow!("the child exited"),
        Err(error) => error,
    }
}

/// Spawns `command` with a piped stdin, stores its pid in `pid` and writes until the write fails.
fn fill(mut command: Command, pid: &AtomicI32) -> Result<()> {
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().context("spawn the drill child")?;
    pid.store(child.id() as i32, Ordering::Relaxed);
    let mut stdin = child.stdin.take().context("no stdin")?;
    let chunk = [b'\n'; 65_536];
    loop {
        if let Err(error) = stdin.write_all(&chunk) {
            let _ = child.wait();
            return Err(error).context("write to the drill child");
        }
    }
}

/// `stall-drill [--no-wait]`: requests the drill, then waits for the guard's record and the new
/// process's marker; exits non-zero when either is missing in time.
pub(crate) fn cli(home: &Path, arguments: Vec<String>) -> Result<()> {
    let (wait, external) = match arguments.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => (true, false),
        ["--no-wait"] => (false, false),
        ["--stop"] => (true, true),
        _ => bail!("usage: cpu-watchdog stall-drill [--no-wait | --stop]\nBlocks the resident loop until the stall guard restarts it ({} s without a watchdog).\n--stop stops the whole process instead; the cpu-build-gate collector must restart it ({EXTERNAL_SECS} s).", super::STALL_SECS),
    };
    let requested_ms = crate::unix_now() * 1000.0;
    let request = root(home).join(REQUEST);
    write_json(&request, &json!({"requested_at_ms": requested_ms, "by_pid": std::process::id(), "mode": if external { "stop" } else { "block" }}))?;
    let seconds = if external { EXTERNAL_SECS } else { super::STALL_SECS };
    eprintln!("stall-drill: requested; the resident loop {} at its next tick and the {} acts after {seconds} s",
        if external { "stops its process" } else { "blocks" }, if external { "cpu-build-gate collector" } else { "guard" });
    if !wait {
        return Ok(());
    }
    let started = Instant::now();
    let limit = Duration::from_secs(seconds) + RESTART_WAIT;
    let records = root(home).join(super::RECORDS);
    loop {
        if let Some((path, record)) = drill_record(&records, requested_ms).filter(|(_, record)| (record["by"] == "cpu-build-gate") == external) {
            if path.with_extension("reported").exists() {
                println!("{}", serde_json::to_string_pretty(&json!({"ok": true, "waited_secs": started.elapsed().as_secs(),
                    "record_path": path, "record": record, "restarted": true}))?);
                return Ok(());
            }
        }
        if started.elapsed() >= limit {
            let _ = std::fs::remove_file(&request);
            resume(home);
            bail!("stall-drill: no reported drill record under {} within {} s; is the resident loop running this version?", records.display(), limit.as_secs());
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// Continues a process a `--stop` drill left stopped, so a missing collector never leaves the host unwatched.
fn resume(home: &Path) {
    let active = root(home).join(ACTIVE_FILE);
    let Some(pid) = crate::telemetry::read_state(&active).and_then(|state| state["pid"].as_i64()).and_then(|pid| i32::try_from(pid).ok()) else { return };
    if pid > 0 {
        // SAFETY: continues the pid the drill recorded as stopped.
        unsafe { libc::kill(pid, libc::SIGCONT) };
        eprintln!("stall-drill: continued the stopped process {pid}");
    }
}

/// The newest drill record written at or after `since_ms`.
fn drill_record(records: &Path, since_ms: f64) -> Option<(std::path::PathBuf, Value)> {
    std::fs::read_dir(records).ok()?.flatten().map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .filter_map(|path| crate::telemetry::read_state(&path).map(|record| (path, record)))
        .filter(|(_, record)| record["drill"] == true && record["at_ms"].as_f64().is_some_and(|at| at >= since_ms))
        .max_by(|left, right| left.1["at_ms"].as_f64().unwrap_or(0.0).total_cmp(&right.1["at_ms"].as_f64().unwrap_or(0.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_honoured_only_while_fresh() {
        assert!(fresh(1_000_000.0, 1_000_000.0));
        assert!(fresh(1_000_000.0, 1_120_000.0));
        assert!(!fresh(1_000_000.0, 1_120_001.0), "a watchdog that starts later ignores it");
        assert!(!fresh(1_000_000.0, 999_000.0), "a request from the future is not trusted");
        assert!(!fresh(0.0, 1_000.0));
    }

    /// The 2026-11-11 stall: a write into the stdin of a child that does not read blocks once the
    /// 64 KiB pipe is full, and ends only when the child does.
    #[test]
    fn a_write_into_a_full_pipe_blocks_until_the_child_ends() {
        static PID: AtomicI32 = AtomicI32::new(0);
        let writer = std::thread::spawn(|| {
            let mut command = Command::new("/bin/sleep");
            command.arg("2");
            fill(command, &PID)
        });
        std::thread::sleep(Duration::from_millis(900));
        assert!(!writer.is_finished(), "the writer is blocked while the child lives");
        assert!(PID.load(Ordering::Relaxed) > 0);
        let result = writer.join().unwrap();
        assert!(result.is_err(), "the write fails once the child is gone");
    }

    #[test]
    fn the_cli_reads_the_newest_drill_record_after_the_request() {
        let records = std::env::temp_dir().join(format!("cpu-watchdog-drill-records-{}", std::process::id()));
        std::fs::create_dir_all(&records).unwrap();
        std::fs::write(records.join("100.json"), br#"{"at_ms":100.0,"drill":true}"#).unwrap();
        std::fs::write(records.join("300.json"), br#"{"at_ms":300.0,"drill":false}"#).unwrap();
        std::fs::write(records.join("400.json"), br#"{"at_ms":400.0,"drill":true}"#).unwrap();
        assert_eq!(drill_record(&records, 200.0).unwrap().0.file_name().unwrap(), "400.json");
        assert!(drill_record(&records, 500.0).is_none());
        std::fs::remove_dir_all(&records).unwrap();
    }
}
