//! Which processes and launchd jobs a surge may end. Mirrors the performance skill's
//! `references/process-cleanup.md`: the resident runaway rule (S0) plus S1 rows 1-3, each with
//! its sampled hot-duration evidence. The protected list wins over every row: agent CLIs, Orca,
//! MCP gateways, builds and shared services are never chosen; rows 4-5 need a session-ownership
//! judgement and stay with the AI.
use super::{history::History, launchd::Job};
use crate::{policy::Policy, procs::Proc};
use serde::Serialize;
use std::collections::HashMap;

const DIAGNOSTICS: &[&str] = &["spindump", "ReportCrash", "sysdiagnose", "tailspind"];
const USER_AGENTS: &[&str] = &["fileproviderd", "cloudd", "bird", "photoanalysisd", "mediaanalysisd", "mdworker_shared"];
const OWN_LABELS: &[&str] = &["local.autoretro.", "local.autoretro.", "local.autoretro."];
/// Jobs that hold builds, login state, remote access or shared tooling.
const PROTECTED_LABELS: &[&str] = &["cpu-watchdog", "cpu-build-gate", "chrome-debug", "codex-mcp", "mcp-gateway",
    "orca", "frp", "gateway", "screen-sharing", "storybook", "ssh", "local-search"];
/// Agent sessions and their tools run as plain `node`/`bun`; their own command line exempts them.
const PROTECTED_ARGS: &[&str] = &["claude-code", "@anthropic-ai", "bin/claude", "bin/codex", "Codex.app", "Orca.app", "mcp", "llama-server",
    // GPU renders (Blender and Cinema 4D): `render-queue` ends a stalled render itself and reclaims its slot.
    "render-queue", "Blender.app/Contents/MacOS/Blender", "Commandline.app/Contents/MacOS/Commandline"];
/// A job whose tree runs one of these is building: SIGTERM throws away admitted compile work and a
/// scheduled job relaunches into the same build (surges 1793030422369, 1793091289483, 1793151211388).
const BUILD_PROCESSES: &[&str] = &["cpu-build-gate", "cargo", "rustc", "xcodebuild", "swift-frontend", "swiftc", "clang", "ld"];
const GIB_KIB: u64 = 1024 * 1024;
const DAY_SECS: f64 = 86_400.0;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// SIGTERM, then SIGKILL after the grace while pid and start time still match.
    Terminate { pid: i32, start: i64 },
    SudoKillall { name: String },
    Killall { name: String },
    /// `launchctl kickstart -k`: a KeepAlive job restarts at once.
    Kickstart { label: String },
    /// `launchctl kill SIGTERM`: ends a scheduled job's current run; its schedule stays.
    StopJob { label: String },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Decision {
    pub rule: &'static str,
    pub target: String,
    pub usage_one_core: Option<f64>,
    pub rss_kib: u64,
    pub evidence_window_s: f64,
    pub action: Action,
}

pub struct Inputs<'a> {
    pub now: f64,
    pub processes: &'a [Proc],
    pub history: &'a History,
    pub policy: &'a Policy,
    pub jobs: &'a [Job],
    /// Unix seconds of earlier row-2 `killall`s by process name.
    pub killed: &'a HashMap<String, f64>,
    pub own_pid: i32,
}

pub fn decide(input: &Inputs) -> Vec<Decision> {
    let by_pid: HashMap<i32, &Proc> = input.processes.iter().map(|p| (p.pid, p)).collect();
    let usage = |p: &Proc, window: f64| input.history.usage(p, input.now, window);
    let mut decisions = Vec::new();
    for p in input.processes {
        if p.pid == input.own_pid || PROTECTED_ARGS.iter().any(|marker| p.args.contains(marker)) {
            continue;
        }
        let window = input.policy.window_secs;
        if input.policy.eligible(p, &by_pid) && usage(p, window).is_some_and(|u| u >= input.policy.threshold) {
            decisions.push(decision("s0_runaway_tool", p, usage(p, window), window,
                Action::Terminate { pid: p.pid, start: p.start }));
        }
    }
    let mut named = |rule, names: &[&str], window: f64, floor: f64, own_only: bool, action: fn(String) -> Action| {
        for name in names {
            if decisions.iter().any(|d| d.target == *name) {
                continue;
            }
            let hot = input.processes.iter()
                .filter(|p| p.name == *name && (!own_only || p.uid == input.policy.uid))
                .find(|p| usage(p, window).is_some_and(|u| u >= floor));
            if let Some(p) = hot {
                decisions.push(Decision { target: name.to_string(), ..decision(rule, p, usage(p, window), window, action(name.to_string())) });
            }
        }
    };
    named("s1_row1_stuck_diagnostics", DIAGNOSTICS, 1_800.0, 0.3, false, |name| Action::SudoKillall { name });
    let recently: Vec<&str> = USER_AGENTS.iter().copied()
        .filter(|name| input.killed.get(*name).is_some_and(|at| input.now - at < DAY_SECS)).collect();
    // Row 2 kills once; an agent hot again after its kill is doing real work and is left alone.
    let agents: Vec<&str> = USER_AGENTS.iter().copied().filter(|name| !recently.contains(name)).collect();
    named("s1_row2_user_agent", &agents, 3_600.0, 0.5, true, |name| Action::Killall { name });
    for job in input.jobs.iter().filter(|job| own_job(&job.label)) {
        let Some(root) = job.pid.and_then(|pid| by_pid.get(&pid)) else { continue };
        let tree: Vec<&Proc> = input.processes.iter().filter(|p| descends(p, root.pid, &by_pid)).collect();
        if tree.iter().any(|p| PROTECTED_ARGS.iter().any(|marker| p.args.contains(marker))
            || BUILD_PROCESSES.contains(&p.name.as_str())) {
            continue;
        }
        let cpu = usage(root, 600.0).map(|own| own + tree.iter().filter(|p| p.pid != root.pid).filter_map(|p| usage(p, 600.0)).sum::<f64>());
        let rss: u64 = tree.iter().map(|p| p.rss_kib).sum();
        if cpu.is_some_and(|u| u >= 0.5) || rss >= GIB_KIB {
            let label = job.label.clone();
            let action = if job.keepalive { Action::Kickstart { label } } else { Action::StopJob { label } };
            decisions.push(Decision { target: job.label.clone(), rss_kib: rss, ..decision("s1_row3_launchd_job", root, cpu, 600.0, action) });
        }
    }
    decisions
}

pub fn own_job(label: &str) -> bool {
    OWN_LABELS.iter().any(|prefix| label.starts_with(prefix)) && !PROTECTED_LABELS.iter().any(|p| label.contains(p))
}

fn decision(rule: &'static str, p: &Proc, usage: Option<f64>, window: f64, action: Action) -> Decision {
    Decision { rule, target: format!("{} (pid {})", p.name, p.pid), usage_one_core: usage, rss_kib: p.rss_kib,
        evidence_window_s: window, action }
}

/// True for `root` itself and every process whose ppid chain reaches it.
fn descends(p: &Proc, root: i32, by_pid: &HashMap<i32, &Proc>) -> bool {
    let mut current = Some(p);
    for _ in 0..64 {
        let Some(node) = current else { return false };
        if node.pid == root {
            return true;
        }
        if node.ppid <= 1 || node.ppid == node.pid {
            return false;
        }
        current = by_pid.get(&node.ppid).copied();
    }
    false
}
