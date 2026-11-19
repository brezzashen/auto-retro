//! Orphaned esbuild service reaper (perf incident 85819924763996845273, 2026-10-31).
//!
//! Vite and Storybook talk to `esbuild --service=<v> --ping` only through the stdio pipe of the
//! parent that spawned it, so a service re-parented to launchd has no consumer left. On 2026-10-31
//! two crashed CRM Storybook servers left six such services spinning 5-16 cores for nine hours
//! (07:58-16:51 SGT) until the surge responder killed them; the resident 90% policy only logged
//! "would terminate" because it runs dry. This closes them regardless of CPU: a service seen
//! orphaned on two checks CHECK_SECS apart (same pid and start time) gets TERM, then KILL.
//! Every closure is logged and appended to `esbuild-orphans/<utc-day-ms>.ndjson`.
use crate::monitor::root;
use crate::procs::Proc;
use anyhow::Result;
use serde_json::json;
use std::{
    collections::HashMap,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
};

pub(super) const CHECK_SECS: f64 = 120.0;
const DAY_MS: f64 = 86_400_000.0;

#[derive(Default)]
pub(super) struct EsbuildOrphans {
    last_check: f64,
    /// pid → start time of services found orphaned at the previous check.
    seen: HashMap<i32, i64>,
}

impl EsbuildOrphans {
    pub(super) fn observe(&mut self, home: &Path, now: f64, processes: &[Proc]) -> Result<()> {
        if self.last_check > 0.0 && now >= self.last_check && now - self.last_check < CHECK_SECS {
            return Ok(());
        }
        self.last_check = now;
        let uid = unsafe { libc::getuid() };
        let (confirmed, seen) = confirm(&self.seen, orphaned_services(processes, uid));
        self.seen = seen;
        if confirmed.is_empty() {
            return Ok(());
        }
        for proc in &confirmed {
            let record = json!({"at_ms": now * 1000.0, "pid": proc.pid, "start": proc.start,
                "rss_kib": proc.rss_kib, "cpu_secs": proc.cpu_secs, "args": proc.args.chars().take(240).collect::<String>(),
                "reason": "esbuild service re-parented to launchd (no consumer)"});
            crate::log(&format!("esbuild orphan: closing {record}"));
            append(home, now, &record)?;
        }
        std::thread::Builder::new().name("watchdog-esbuild-orphans".into()).spawn(move || {
            for proc in &confirmed {
                crate::sweep::terminate(proc);
            }
        })?;
        Ok(())
    }
}

/// Our own esbuild services whose parent is launchd.
pub(super) fn orphaned_services(processes: &[Proc], uid: u32) -> Vec<Proc> {
    processes.iter()
        .filter(|p| p.uid == uid && p.ppid == 1 && p.name == "esbuild")
        .filter(|p| p.args.contains("/node_modules/@esbuild/") && p.args.contains(" --service=") && p.args.contains(" --ping"))
        .cloned()
        .collect()
}

/// Splits the current orphans into those already seen with the same start time (to close) and
/// the set to remember for the next check.
pub(super) fn confirm(previous: &HashMap<i32, i64>, current: Vec<Proc>) -> (Vec<Proc>, HashMap<i32, i64>) {
    let seen = current.iter().map(|p| (p.pid, p.start)).collect();
    let confirmed = current.into_iter()
        .filter(|p| previous.get(&p.pid).is_some_and(|start| (start - p.start).abs() <= 2))
        .collect();
    (confirmed, seen)
}

fn append(home: &Path, now: f64, record: &serde_json::Value) -> Result<()> {
    let directory = root(home).join("esbuild-orphans");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let day = ((now * 1000.0) / DAY_MS).floor() * DAY_MS;
    std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(directory.join(format!("{}.ndjson", day as u64)))?
        .write_all(format!("{record}\n").as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: i32, ppid: i32, start: i64, args: &str) -> Proc {
        Proc { pid, ppid, uid: 501, start, cpu_secs: 0.0, rss_kib: 4000, name: "esbuild".into(), args: args.into() }
    }
    const SERVICE: &str = "/Volumes/MarsWork/Workspaces/Mars_agent/Mars_CRM/web/node_modules/@esbuild/darwin-arm64/bin/esbuild --service=0.28.2 --ping";

    #[test]
    fn only_launchd_parented_services_of_this_user_are_orphans() {
        let mut other_user = proc(4, 1, 10, SERVICE);
        other_user.uid = 0;
        let procs = vec![proc(1, 1, 10, SERVICE), proc(2, 57143, 10, SERVICE),
            proc(3, 1, 10, "/x/node_modules/@esbuild/darwin-arm64/bin/esbuild src/app.ts --bundle"), other_user];
        let found: Vec<i32> = orphaned_services(&procs, 501).iter().map(|p| p.pid).collect();
        assert_eq!(found, vec![1]);
    }

    #[test]
    fn closes_only_after_a_second_sighting_with_the_same_start() {
        let (first, seen) = confirm(&HashMap::new(), vec![proc(1, 1, 100, SERVICE), proc(2, 1, 100, SERVICE)]);
        assert!(first.is_empty());
        let (second, _) = confirm(&seen, vec![proc(1, 1, 101, SERVICE), proc(2, 1, 500, SERVICE)]);
        let closed: Vec<i32> = second.iter().map(|p| p.pid).collect();
        assert_eq!(closed, vec![1], "pid 2 was reused with a new start time");
    }
}
