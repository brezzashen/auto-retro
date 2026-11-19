//! What a waiting run waits on: the holder pid its progress line names, resolved to the board run
//! whose process tree contains that pid. T8 (`triggers.rs`) needs it to tell a Q2 that holds the
//! resource a Q1 waits for from a Q2 that merely runs beside it.
//!
//! Every queue-shaped wait `deliver` and `mars-ops` print names its holder by pid:
//! - `⏳ deploy lease (local-k8s) held by 'mars-crm-k8s-deploy-<pid>' …` (`deploy_lease.sh`; the
//!   holder name ends in the `mars-ops` pid, `k8s/rollout.rs`), `'canary rollout <sha> pid <pid>'`
//! - `[build-lease] waiting for k8s-build-crm-<pid> (pid <pid>) …`, `[ship-web] waiting for
//!   ship-web-verify-<pid> (pid <pid>) …`, `… prepared by pid <pid>; waiting …`, `queued for a canary
//!   rollout behind pid <pid> …`, `waiting for the TestFlight publish held by pid <pid>`
//!
//! The holder is a descendant of the run's `deliver` process (`creator.pid`, or `backgroundPid` of a
//! detached run), so walking parent pids from the holder reaches its run. A holder that is gone,
//! re-parented, or off the board (an operator's `mars-ops k8s build-crm`) stays unresolved: `run` null.
//! `started_ms` is the holder process's start (`ps etime`, 1 s resolution): the earliest it can have
//! taken the resource, absent when `ps` did not list it.
use regex::Regex;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::OnceLock};

/// Parent links walked from a holder before giving up; real chains are under ten deep.
const MAX_DEPTH: usize = 64;

/// One `ps` reading: `pid → ppid`, and `pid → start` in epoch ms.
#[derive(Default)]
pub(crate) struct Processes {
    pub(crate) parents: HashMap<i64, i64>,
    pub(crate) started_ms: HashMap<i64, f64>,
}

impl Processes {
    /// Parent links only (tests): no start times, so no blocker carries `started_ms`.
    #[cfg(test)]
    pub(crate) fn parents(parents: HashMap<i64, i64>) -> Self { Self { parents, started_ms: HashMap::new() } }
}

/// The pid of the process the line says this run waits on.
pub(crate) fn holder_pid(line: &str) -> Option<i64> {
    static PID: OnceLock<Regex> = OnceLock::new();
    static NAMED: OnceLock<Regex> = OnceLock::new();
    let pid = PID.get_or_init(|| Regex::new(r"\bpid (\d+)").expect("pid pattern"));
    let named = NAMED.get_or_init(|| Regex::new(r"held by '[^']*-(\d+)'").expect("holder pattern"));
    pid.captures(line).or_else(|| named.captures(line)).and_then(|c| c[1].parse().ok())
}

/// Every process from one `ps` call; empty when `ps` fails.
pub(crate) fn processes(now_ms: f64) -> Processes {
    let Ok(out) = std::process::Command::new("/bin/ps").args(["-A", "-o", "pid=,ppid=,etime="]).output() else { return Processes::default() };
    parse_ps(&String::from_utf8_lossy(&out.stdout), now_ms)
}

pub(crate) fn parse_ps(text: &str, now_ms: f64) -> Processes {
    let mut table = Processes::default();
    for line in text.lines() {
        let mut f = line.split_whitespace();
        let (Some(Ok(pid)), Some(Ok(ppid))) = (f.next().map(str::parse::<i64>), f.next().map(str::parse::<i64>)) else { continue };
        table.parents.insert(pid, ppid);
        if let Some(secs) = f.next().and_then(etime_secs) { table.started_ms.insert(pid, now_ms - secs * 1000.0); }
    }
    table
}

/// `ps` elapsed time `[[dd-]hh:]mm:ss` in seconds.
pub(crate) fn etime_secs(text: &str) -> Option<f64> {
    let (days, clock) = text.split_once('-').map_or((Some(0.0), text), |(d, c)| (d.parse().ok(), c));
    let parts: Vec<f64> = clock.split(':').map(str::parse).collect::<Result<_, _>>().ok()?;
    if !(2..=3).contains(&parts.len()) { return None; }
    Some(days? * 86_400.0 + parts.iter().fold(0.0, |acc, p| acc * 60.0 + p))
}

/// Whether any waiting run names a holder pid, so the caller loads the process table only then.
pub(crate) fn wanted(queue: &Value) -> bool {
    waiting(queue).any(|r| r["progress"].as_str().and_then(holder_pid).is_some())
}

fn waiting(queue: &Value) -> impl Iterator<Item = &Value> {
    queue["environments"].as_object().into_iter().flatten()
        .flat_map(|(_, e)| e["releases"].as_array().into_iter().flatten()).filter(|r| r["phase"] == "waiting")
}

/// Sets `blocker` (`{"pid", "run", "started_ms"?}`) on every waiting release of `queue` whose line
/// names a holder; `run` is the board run whose `deliver` process tree holds that pid, else null.
pub(crate) fn resolve(queue: &mut Value, board: &[Value], procs: &Processes) {
    let roots: HashMap<i64, String> = board.iter().filter(|e| matches!(e["state"].as_str(), Some("running" | "detached")))
        .flat_map(|e| [e["creator"]["pid"].as_i64(), e["backgroundPid"].as_i64()].into_iter().flatten()
            .filter(|p| *p > 1).map(|p| (p, e["id"].as_str().unwrap_or_default().to_string())))
        .collect();
    let Some(envs) = queue["environments"].as_object_mut() else { return };
    for release in envs.values_mut().flat_map(|e| e["releases"].as_array_mut().into_iter().flatten()) {
        if release["phase"] != "waiting" { continue; }
        let Some(pid) = release["progress"].as_str().and_then(holder_pid) else { continue };
        release["blocker"] = json!({"pid": pid, "run": owner(pid, &roots, &procs.parents)});
        if let Some(at) = procs.started_ms.get(&pid) { release["blocker"]["started_ms"] = json!(at.round()); }
    }
}

/// The run whose root process is `pid` or one of its ancestors.
fn owner(pid: i64, roots: &HashMap<i64, String>, parents: &HashMap<i64, i64>) -> Option<String> {
    let mut at = pid;
    for _ in 0..MAX_DEPTH {
        if let Some(run) = roots.get(&at) { return Some(run.clone()); }
        match parents.get(&at) { Some(&up) if up > 1 && up != at => at = up, _ => return None }
    }
    None
}
