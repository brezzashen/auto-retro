use super::{history::History, launchd::{self, Job}, rules::{self, Action}, Surge};
use crate::{policy::Policy, procs::Proc};
use std::collections::HashMap;

fn proc(pid: i32, ppid: i32, name: &str, args: &str) -> Proc {
    Proc { pid, ppid, uid: 501, start: 1_000, cpu_secs: 0.0, rss_kib: 1024, name: name.into(), args: args.into() }
}

/// Records `processes` every 30 s for `seconds`, each burning its `cpu_secs` value per second.
fn history_of(processes: &[Proc], seconds: u32) -> (History, Vec<Proc>) {
    let mut history = History::default();
    let mut current = processes.to_vec();
    for step in 0..=seconds / 30 {
        let now = 10_000.0 + f64::from(step * 30);
        current = processes.iter().map(|p| Proc { cpu_secs: p.cpu_secs * f64::from(step * 30), ..p.clone() }).collect();
        history.record(now, &current);
    }
    (history, current)
}

fn decide(processes: &[Proc], seconds: u32, jobs: &[Job], killed: &HashMap<String, f64>) -> Vec<rules::Decision> {
    let (history, current) = history_of(processes, seconds);
    let policy = Policy::from_env();
    rules::decide(&rules::Inputs { now: 10_000.0 + f64::from(seconds / 30 * 30), processes: &current, history: &history,
        policy: &policy, jobs, killed, own_pid: 1 })
}

#[test]
fn fires_once_after_two_minutes_above_ninety_five_with_a_cooldown() {
    let mut surge = Surge::default();
    let mut fired = Vec::new();
    for i in 0..21 {
        let start = f64::from(i) * 5_500.0;
        fired.extend(surge.ingest(start, start + 5_500.0, 96.0));
    }
    assert!(fired.is_empty(), "115.5 s above 95% is not yet two minutes");
    fired.extend(surge.ingest(115_500.0, 121_000.0, 96.0));
    assert_eq!(fired, vec![0.0], "fires when 120 s are covered, once per episode");
    assert!(surge.ingest(121_000.0, 126_500.0, 95.0).is_none(), "exactly 95% is not above it");
    assert!(surge.since.is_none());
    for i in 0..23 {
        let start = 126_500.0 + f64::from(i) * 5_500.0;
        assert!(surge.ingest(start, start + 5_500.0, 99.0).is_none(), "inside the ten-minute cooldown");
    }
    surge.reset();
    let late = 900_000.0;
    let fired: Vec<f64> = (0..23).filter_map(|i| surge.ingest(late + f64::from(i) * 5_500.0, late + f64::from(i + 1) * 5_500.0, 97.0)).collect();
    assert_eq!(fired, vec![late]);
}

#[test]
fn usage_needs_the_full_window_and_a_stable_identity() {
    let node = Proc { cpu_secs: 1.0, ..proc(42, 7, "node", "node spin.js") };
    let (history, current) = history_of(&[node], 300);
    let usage = history.usage(&current[0], 10_300.0, 300.0).unwrap();
    assert!((usage - 1.0).abs() < 1e-9);
    assert!(history.usage(&current[0], 10_300.0, 600.0).is_none());
    let reused = Proc { start: 5_000, ..current[0].clone() };
    assert!(history.usage(&reused, 10_300.0, 60.0).is_none());
}

#[test]
fn runaway_tools_die_but_agents_builds_and_short_bursts_do_not() {
    let runaway = Proc { cpu_secs: 1.0, ..proc(42, 7, "node", "node /tmp/spin.js") };
    let agent = Proc { cpu_secs: 1.0, ..proc(43, 7, "node", "node /opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js") };
    let gate = proc(50, 1, "bun", "bun /Users/x/.local/state/mars-cpu-build-gate/cli.ts run");
    let compiler = Proc { cpu_secs: 1.0, ..proc(51, 50, "swift-frontend", "swift-frontend -c") };
    let decisions = decide(&[runaway.clone(), agent, gate, compiler], 300, &[], &HashMap::new());
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    assert_eq!(decisions[0].action, Action::Terminate { pid: 42, start: 1_000 });
    assert!(decide(&[runaway], 270, &[], &HashMap::new()).is_empty(), "under the five-minute window");
}

#[test]
fn system_rows_need_their_hour_or_half_hour_and_kill_agents_once() {
    let spotlight = Proc { cpu_secs: 0.6, ..proc(60, 1, "mdworker_shared", "/System/.../mdworker_shared") };
    let spindump = Proc { uid: 0, cpu_secs: 0.4, ..proc(61, 1, "spindump", "/usr/sbin/spindump") };
    let decisions = decide(&[spotlight.clone(), spindump.clone()], 3_600, &[], &HashMap::new());
    let actions: Vec<_> = decisions.iter().map(|d| d.action.clone()).collect();
    assert!(actions.contains(&Action::Killall { name: "mdworker_shared".into() }));
    assert!(actions.contains(&Action::SudoKillall { name: "spindump".into() }));
    let killed = HashMap::from([("mdworker_shared".to_string(), 12_000.0)]);
    assert_eq!(decide(&[spotlight.clone()], 3_600, &[], &killed).len(), 0, "killed within a day: real work");
    assert!(decide(&[spotlight], 1_800, &[], &HashMap::new()).is_empty(), "row 2 needs an hour");
}

#[test]
fn hot_own_jobs_restart_or_stop_and_protected_jobs_stay() {
    let job = |label: &str, pid: i32, keepalive: bool| Job { label: label.into(), pid: Some(pid), keepalive, ..Job::default() };
    let daemon = proc(70, 1, "token-dashboard", "/Users/x/.local/bin/token-dashboard");
    let worker = Proc { cpu_secs: 0.6, ..proc(71, 70, "bun", "bun worker.ts") };
    let scheduled = Proc { cpu_secs: 0.7, ..proc(72, 1, "tm-scheduler", "/Users/x/.local/bin/tm-scheduler") };
    let gate = Proc { cpu_secs: 0.9, ..proc(73, 1, "cpu-build-gate", "/Users/x/.local/bin/cpu-build-gate") };
    let jobs = [job("local.autoretro.token-dashboard", 70, true), job("local.autoretro.tm-scheduler", 72, false),
        job("local.autoretro.cpu-build-gate", 73, true), job("com.vendor.agent", 72, true)];
    let decisions = decide(&[daemon, worker, scheduled, gate], 600, &jobs, &HashMap::new());
    let actions: Vec<_> = decisions.iter().map(|d| d.action.clone()).collect();
    assert_eq!(actions, vec![Action::Kickstart { label: "local.autoretro.token-dashboard".into() },
        Action::StopJob { label: "local.autoretro.tm-scheduler".into() }]);
}

#[test]
fn a_job_running_a_build_is_left_alone() {
    // 2026-10-28 09:33: a 30 s StartInterval invite job held a 4 GiB gated `cargo build --release`.
    let shell = proc(90, 1, "zsh", "/bin/zsh -lc bun run canary invite");
    let cargo = Proc { rss_kib: 4 * 1024 * 1024, ..proc(91, 90, "cargo", "cargo build --release -p mars-ops") };
    let rustc = Proc { cpu_secs: 3.0, ..proc(92, 91, "rustc", "rustc --crate-name mars_ops") };
    let job = Job { label: "local.autoretro.mars-invite-5040".into(), pid: Some(90), keepalive: false, ..Job::default() };
    assert!(decide(&[shell, cargo, rustc], 600, &[job], &HashMap::new()).is_empty());
}

#[test]
fn launchctl_rows_parse_and_only_errors_or_crashes_fail() {
    let jobs = launchd::parse("PID\tStatus\tLabel\n-\t0\tlocal.autoretro.ok\n-\t2\tlocal.autoretro.coupon\n90848\t-15\tlocal.autoretro.mcp\n12\t-11\tcom.x.crash\n");
    let failing: Vec<_> = jobs.iter().filter(|job| job.failing()).map(|job| job.label.as_str()).collect();
    assert_eq!(failing, ["local.autoretro.coupon", "com.x.crash"]);
    assert_eq!(jobs[2].pid, Some(90848));
}

#[test]
fn resident_model_weights_and_active_indexing_are_not_memory_leaks() {
    for label in ["local.autoretro.local-search-embed", "local.autoretro.local-search-query",
        "local.autoretro.local-search-rerank", "local.autoretro.local-search-sync"] {
        let model = Proc { rss_kib: 8 * 1024 * 1024, cpu_secs: 2.0,
            ..proc(70, 1, "local-search", "/Users/x/.local/bin/local-search sync") };
        let job = Job { label: label.into(), pid: Some(70), keepalive: true, ..Job::default() };
        assert!(decide(&[model], 600, &[job], &HashMap::new()).is_empty(), "{label}");
    }
    // Protection follows a model worker even under a newly named job label.
    let model = Proc { rss_kib: 8 * 1024 * 1024, ..proc(80, 1, "llama-server", "llama-server") };
    let job = Job { label: "local.autoretro.other-model".into(), pid: Some(80), keepalive: true, ..Job::default() };
    assert!(decide(&[model], 0, &[job], &HashMap::new()).is_empty());
}
