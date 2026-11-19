//! Stops the bleeding before any analysis, as the user asked on 2026-10-21 (restart first,
//! analyze the cause afterwards): a ControlCenter over its port limit gets SIGTERM (what
//! `killall ControlCenter` sends); launchd respawns it within a second. The restart confirms the
//! pid and start time still match the counted process, waits for the new instance, counts its
//! ports, and appends the outcome to `controlcenter-restarts/<utc-day-ms>.ndjson` (latest in
//! `controlcenter-restart.json`). At most one restart per MIN_GAP_SECS, so a fast re-leak cannot
//! loop the menu bar.
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{io::Write, os::unix::fs::{DirBuilderExt, OpenOptionsExt}, path::Path, time::Duration};

const MIN_GAP_SECS: f64 = 600.0;
const RESPAWN_WAIT: Duration = Duration::from_secs(15);
/// Lets the new instance load its menu bar modules before its ports are counted.
const SETTLE: Duration = Duration::from_secs(3);
const DAY_MS: f64 = 86_400_000.0;

pub(super) fn restart(home: &Path, now: f64, pid: i32, start: i64, ports: u64, incident: &str) -> Result<Value> {
    let latest = root(home).join("controlcenter-restart.json");
    let last = super::read(&latest).and_then(|v| v["at_ms"].as_f64());
    let mut record = json!({"at_ms": now * 1000.0, "incident": incident, "pid": pid, "process_start": start, "ports": ports});
    if last.is_some_and(|at| now * 1000.0 - at < MIN_GAP_SECS * 1000.0) {
        record["outcome"] = json!("skipped: restarted less than 600 s ago");
        return Ok(record);
    }
    let outcome = if cfg!(test) { Ok((pid + 1, Some(1_000))) } else { terminate(pid, start) };
    match outcome {
        Ok((new_pid, new_ports)) => {
            record["outcome"] = json!("restarted");
            record["new_pid"] = json!(new_pid);
            record["new_ports"] = json!(new_ports);
        }
        Err(error) => record["outcome"] = json!(format!("failed: {error:#}")),
    }
    write_json(&latest, &record)?;
    let directory = root(home).join("controlcenter-restarts");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let day = ((now * 1000.0) / DAY_MS).floor() * DAY_MS;
    std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(directory.join(format!("{}.ndjson", day as u64)))?
        .write_all(format!("{record}\n").as_bytes())?;
    if let Err(error) = crate::recovery_retro::record(home, "controlcenter", &record) {
        crate::log(&format!("controlcenter retro: {error:#}"));
    }
    Ok(record)
}

/// SIGTERM to the exact counted instance, then the respawned pid and its port count.
fn terminate(pid: i32, start: i64) -> Result<(i32, Option<u64>)> {
    let (_, processes) = crate::procs::snapshot(crate::unix_now)?;
    anyhow::ensure!(is_counted(&processes, pid, start), "ControlCenter {pid} already exited or was replaced");
    // SAFETY: signals the verified ControlCenter instance only.
    anyhow::ensure!(unsafe { libc::kill(pid, libc::SIGTERM) } == 0, "SIGTERM failed: {}", std::io::Error::last_os_error());
    let waited = std::time::Instant::now();
    while waited.elapsed() < RESPAWN_WAIT {
        std::thread::sleep(Duration::from_millis(500));
        let (_, processes) = crate::procs::snapshot(crate::unix_now)?;
        if let Some(new) = processes.iter().find(|p| p.name == "ControlCenter" && p.pid != pid) {
            std::thread::sleep(SETTLE);
            return Ok((new.pid, super::port_census::controlcenter_ports().map(|(_, ports)| ports)));
        }
    }
    anyhow::bail!("launchd did not respawn ControlCenter within 15 s")
}

/// Whether the counted instance is still running. Start is now − etime in whole seconds, so two
/// snapshots of one process differ by a second or two; an exact match failed two restarts
/// (pid 14733 on 2026-10-22, pid 65109 on 2026-10-24) and left the leak to a manual killall.
fn is_counted(processes: &[crate::procs::Proc], pid: i32, start: i64) -> bool {
    processes.iter().any(|p| p.pid == pid && p.name == "ControlCenter" && crate::procs::is_same_process(start, Some(p.start)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restarts_once_per_gap_and_logs_each_outcome() {
        let home = super::super::test_home("cc-restart");
        let first = restart(&home, 1000.0, 548, 10, 68_705, "8a").unwrap();
        assert_eq!(first["outcome"], "restarted");
        assert_eq!(first["new_ports"], 1_000);
        let soon = restart(&home, 1300.0, 549, 1000, 5_000, "8b").unwrap();
        assert!(soon["outcome"].as_str().unwrap().starts_with("skipped"));
        let later = restart(&home, 1700.0, 549, 1000, 5_000, "8c").unwrap();
        assert_eq!(later["outcome"], "restarted");
        let log = std::fs::read_dir(root(&home).join("controlcenter-restarts")).unwrap().next().unwrap().unwrap().path();
        assert_eq!(std::fs::read_to_string(log).unwrap().lines().count(), 2);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn counted_instance_tolerates_start_rounding() {
        let process = |pid, start, name: &str| crate::procs::Proc {
            pid, ppid: 1, uid: 501, start, cpu_secs: 0.0, rss_kib: 0, name: name.into(), args: String::new(),
        };
        // Census recorded 1792806004; later snapshots of the same pid read 1792806005-06.
        assert!(is_counted(&[process(65109, 1_792_806_006, "ControlCenter")], 65109, 1_792_806_004));
        assert!(!is_counted(&[process(65109, 1_792_806_300, "ControlCenter")], 65109, 1_792_806_004), "pid reused");
        assert!(!is_counted(&[process(65109, 1_792_806_004, "Finder")], 65109, 1_792_806_004), "other process");
        assert!(!is_counted(&[], 65109, 1_792_806_004), "exited");
    }
}
