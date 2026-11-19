//! Stage S0 process cleanup (`process-cleanup` skill): closes what is provably abandoned and lists
//! the rest for the agent to decide.
//!
//! Closed without asking: orphaned static file servers (`python -m http.server`,
//! `bun -e Bun.serve(`) re-parented to launchd whose working directory is an agent scratchpad of a
//! session whose transcript was found and idle for IDLE_HOURS (a session with no transcript is
//! left alone), and orphaned fixture `mars-crm-server` processes of killed `bun test` runs
//! ([`crm_fixture`]). Simulators beyond the residents are not handled here:
//! `local.autoretro.sim-lease-trim` runs `sim-lease trim` every two minutes.
//!
//! Reported for the agent (S1): every process averaging HOT_PCT of a core over a short sample.

mod crm_fixture;

use crate::procs::{self, Proc};
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime};

pub struct Sweep {
    pub uid: u32,
    pub home: PathBuf,
    pub idle_hours: f64,
    pub dry_run: bool,
    /// Seconds between the two snapshots of the hot-process report; 0 skips the report.
    pub sample_secs: f64,
    pub hot_pct: f64,
}

#[derive(Serialize)]
pub struct Closed {
    pub pid: i32,
    pub name: String,
    pub reason: String,
    pub args: String,
}

#[derive(Serialize)]
pub struct Hot {
    pub pid: i32,
    pub uid: u32,
    pub ppid: i32,
    pub usage_pct: f64,
    pub elapsed_secs: i64,
    pub args: String,
}

#[derive(Serialize)]
pub struct Report {
    pub dry_run: bool,
    pub closed: Vec<Closed>,
    /// Busy processes for the agent to judge with the skill's S1 rules, busiest first.
    pub hot: Vec<Hot>,
}

impl Sweep {
    pub fn run(&self, clock: impl Fn() -> f64 + Copy) -> Result<Report> {
        let (now, first) = procs::snapshot(clock)?;
        let mut closed = Vec::new();
        let mut abandoned = self.abandoned_servers(&first);
        abandoned.extend(self.orphaned_crm_fixtures(&first, now as i64));
        for (proc, reason) in abandoned {
            if !self.dry_run && !terminate(proc) {
                continue;
            }
            closed.push(Closed { pid: proc.pid, name: proc.name.clone(), reason, args: proc.args.clone() });
        }
        let hot = if self.sample_secs > 0.0 { self.hot(now, &first, clock)? } else { Vec::new() };
        Ok(Report { dry_run: self.dry_run, closed, hot })
    }

    fn abandoned_servers<'a>(&self, procs: &'a [Proc]) -> Vec<(&'a Proc, String)> {
        let servers: Vec<&Proc> = procs
            .iter()
            .filter(|proc| proc.uid == self.uid && proc.ppid == 1)
            .filter(|proc| proc.args.contains(" -m http.server") || proc.args.contains("Bun.serve("))
            .collect();
        let cwds = working_directories(&servers.iter().map(|proc| proc.pid).collect::<Vec<_>>());
        servers
            .into_iter()
            .filter_map(|proc| {
                let place = cwds.get(&proc.pid).map(String::as_str).unwrap_or(&proc.args);
                let session = scratchpad_session(place)?;
                // An unknown idle time is not idle: no transcript means no kill.
                let idle = self.session_idle_hours(&session)?;
                (idle >= self.idle_hours)
                    .then(|| (proc, format!("orphaned file server of session {session} (idle {idle:.1} h)")))
            })
            .collect()
    }

    fn orphaned_crm_fixtures<'a>(&self, procs: &'a [Proc], now: i64) -> Vec<(&'a Proc, String)> {
        let candidates: Vec<&Proc> = procs.iter().filter(|proc| proc.uid == self.uid && proc.ppid == 1 && proc.name == "mars-crm-server").collect();
        let cwds = working_directories(&candidates.iter().map(|proc| proc.pid).collect::<Vec<_>>());
        candidates
            .into_iter()
            .filter(|proc| crm_fixture::is_orphaned(proc, self.uid, cwds.get(&proc.pid).map(String::as_str), now))
            .map(|proc| (proc, format!("orphaned fixture mars-crm-server ({} s old)", now - proc.start)))
            .collect()
    }

    /// Hours since the session's newest transcript write, across every Claude profile.
    fn session_idle_hours(&self, session: &str) -> Option<f64> {
        let file = format!("{session}.jsonl");
        let mut roots = vec![self.home.join(".claude/projects")];
        if let Ok(profiles) = std::fs::read_dir(self.home.join(".claude-profiles")) {
            roots.extend(profiles.flatten().map(|profile| profile.path().join("projects")));
        }
        roots
            .iter()
            .filter_map(|root| std::fs::read_dir(root).ok())
            .flat_map(|projects| projects.flatten())
            .filter_map(|project| std::fs::metadata(project.path().join(&file)).ok()?.modified().ok())
            .max()
            .map(|modified| SystemTime::now().duration_since(modified).unwrap_or_default().as_secs_f64() / 3600.0)
    }

    fn hot(&self, before_time: f64, before: &[Proc], clock: impl Fn() -> f64) -> Result<Vec<Hot>> {
        std::thread::sleep(Duration::from_secs_f64(self.sample_secs));
        let (now, after) = procs::snapshot(clock)?;
        let earlier: HashMap<i32, &Proc> = before.iter().map(|proc| (proc.pid, proc)).collect();
        let mut hot: Vec<Hot> = after
            .iter()
            .filter_map(|proc| {
                let previous = earlier.get(&proc.pid).filter(|previous| (previous.start - proc.start).abs() <= 2)?;
                let usage_pct = (proc.cpu_secs - previous.cpu_secs) / (now - before_time) * 100.0;
                (usage_pct >= self.hot_pct).then(|| Hot {
                    pid: proc.pid,
                    uid: proc.uid,
                    ppid: proc.ppid,
                    usage_pct: (usage_pct * 10.0).round() / 10.0,
                    elapsed_secs: now as i64 - proc.start,
                    args: proc.args.chars().take(240).collect(),
                })
            })
            .collect();
        hot.sort_by(|left, right| right.usage_pct.total_cmp(&left.usage_pct));
        Ok(hot)
    }
}

/// The session UUID of an agent scratchpad path: `/…/claude-<uid>/<project>/<session>/scratchpad/…`.
pub fn scratchpad_session(path: &str) -> Option<String> {
    let segments: Vec<&str> = path.split('/').collect();
    let scratch = segments.iter().position(|segment| *segment == "scratchpad")?;
    let under_agent_tmp = segments[..scratch].iter().any(|segment| segment.starts_with("claude-"));
    let session = segments.get(scratch.checked_sub(1)?)?;
    (under_agent_tmp && is_uuid(session)).then(|| session.to_string())
}

fn is_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
}

fn working_directories(pids: &[i32]) -> HashMap<i32, String> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let list = pids.iter().map(i32::to_string).collect::<Vec<_>>().join(",");
    let Ok(output) = Command::new("/usr/sbin/lsof").args(["-a", "-p", &list, "-d", "cwd", "-Fpn"]).output() else {
        return HashMap::new();
    };
    let mut cwds = HashMap::new();
    let mut pid = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse().ok();
        } else if let (Some(value), Some(current)) = (line.strip_prefix('n'), pid) {
            cwds.insert(current, value.to_string());
        }
    }
    cwds
}

/// Returns false, sending nothing, when the pid was reused or exited since the snapshot.
pub(crate) fn terminate(proc: &Proc) -> bool {
    if !procs::still_same_process(proc.pid, proc.start, crate::unix_now) {
        return false;
    }
    // SAFETY: kill only reads its arguments; a stale pid yields ESRCH.
    unsafe { libc::kill(proc.pid, libc::SIGTERM) };
    std::thread::sleep(Duration::from_secs(3));
    if procs::still_same_process(proc.pid, proc.start, crate::unix_now) {
        // SAFETY: as above; the start time was just re-checked against the pid.
        unsafe { libc::kill(proc.pid, libc::SIGKILL) };
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratchpad_sessions_are_recognised_only_under_agent_tmp() {
        let path = "/private/tmp/claude-501/-home-user-workspace/2e6d6dd4-c503-4b38-96e0-7d9f9bbaecda/scratchpad/webcat";
        assert_eq!(scratchpad_session(path).as_deref(), Some("2e6d6dd4-c503-4b38-96e0-7d9f9bbaecda"));
        assert_eq!(scratchpad_session("/Users/x/2e6d6dd4-c503-4b38-96e0-7d9f9bbaecda/scratchpad"), None);
        assert_eq!(scratchpad_session("/private/tmp/claude-501/p/not-a-session/scratchpad"), None);
        assert_eq!(scratchpad_session("/home/user/site"), None);
    }

    #[test]
    fn served_path_in_bun_args_names_the_session() {
        let args = "bun -e Bun.serve({fetch(r){return new Response(Bun.file('/private/tmp/claude-501/-Users-b/b76c6e4e-e744-4266-8e99-162039c6e3e7/scratchpad/blur'+p))}})";
        assert_eq!(scratchpad_session(args).as_deref(), Some("b76c6e4e-e744-4266-8e99-162039c6e3e7"));
    }
}
