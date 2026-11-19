use super::*;

fn daemon(name: &str, pid: i32, start: i64, cpu_secs: f64) -> Proc {
    Proc { pid, ppid: 1, uid: 0, start, cpu_secs, rss_kib: 10_000, name: name.into(), args: format!("/usr/libexec/{name}") }
}

/// Feeds one sample every SAMPLE_SECS for `secs`, burning `rate` cores; returns the end time.
fn run(heal: &mut DaemonHeal, home: &Path, from: f64, secs: f64, rate: f64, pid: i32) -> f64 {
    let mut now = from;
    while now <= from + secs {
        heal.observe(home, now, &[daemon("PerfPowerServices", pid, 100, (now - from) * rate),
            daemon("syspolicyd", 259, 100, 0.0)]).unwrap();
        now += SAMPLE_SECS;
    }
    now
}

fn wait_worker(heal: &mut DaemonHeal) -> Value {
    let record = heal.worker.take().unwrap().recv().unwrap();
    record
}

#[test]
fn restarts_a_spinning_daemon_once_per_cooldown() {
    let home = crate::telemetry::test_home("daemon-heal");
    let mut heal = DaemonHeal::default();
    let end = run(&mut heal, &home, 1000.0, WINDOW_SECS, 1.4, 91618);
    let record = wait_worker(&mut heal);
    assert_eq!(record["name"], "PerfPowerServices");
    assert_eq!(record["outcome"], "restarted");
    assert!((record["usage_one_core"].as_f64().unwrap() - 1.4).abs() < 0.01);
    // Hot again on the new instance, but inside the cooldown: no second restart.
    run(&mut heal, &home, end, WINDOW_SECS + SAMPLE_SECS, 1.4, 79298);
    assert!(heal.worker.is_none());
    let state = super::super::read(&root(&home).join("daemon-heal.json")).unwrap();
    assert_eq!(state["restarted"]["PerfPowerServices"], 1000.0 + WINDOW_SECS);
    assert!(state["restarted"]["syspolicyd"].is_null());
    let log = std::fs::read_dir(root(&home).join("daemon-restarts")).unwrap().next().unwrap().unwrap().path();
    assert_eq!(std::fs::read_to_string(log).unwrap().lines().count(), 1);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn leaves_short_bursts_and_idle_daemons_alone() {
    let home = crate::telemetry::test_home("daemon-heal-idle");
    let mut heal = DaemonHeal::default();
    // Below the floor for the whole window.
    run(&mut heal, &home, 1000.0, WINDOW_SECS + SAMPLE_SECS, 0.3, 91618);
    assert!(heal.worker.is_none());
    // A fresh instance is not judged until it has been watched for the full window.
    run(&mut heal, &home, 5000.0, WINDOW_SECS - SAMPLE_SECS, 2.0, 91619);
    assert!(heal.worker.is_none());
    std::fs::remove_dir_all(home).unwrap();
}

/// Clock and cumulative CPU of one simulated syspolicyd instance, carried across phases.
struct Feed {
    now: f64,
    cpu: f64,
    pid: i32,
    start: i64,
}

impl Feed {
    /// Samples every SAMPLE_SECS for `secs`, burning `rate` cores, optionally with cargo alive.
    fn run(&mut self, heal: &mut DaemonHeal, home: &Path, secs: f64, rate: f64, build: bool) {
        let until = self.now + secs;
        while self.now <= until {
            let mut processes = vec![daemon("syspolicyd", self.pid, self.start, self.cpu)];
            if build {
                processes.push(Proc { pid: 4242, ppid: 1, uid: 501, start: 100, cpu_secs: 0.0, rss_kib: 1, name: "cargo".into(), args: "cargo test".into() });
            }
            heal.observe(home, self.now, &processes).unwrap();
            self.now += SAMPLE_SECS;
            self.cpu += SAMPLE_SECS * rate;
        }
    }
}

fn held(home: &Path) -> Value {
    super::super::read(&root(home).join("daemon-heal.json")).unwrap()["targets"]["syspolicyd"]["held"].clone()
}

#[test]
fn holds_syspolicyd_while_a_build_runs_inside_the_window() {
    let home = crate::telemetry::test_home("daemon-heal-build");
    let mut heal = DaemonHeal::default();
    let mut feed = Feed { now: 1000.0, cpu: 0.0, pid: 62496, start: 100 };
    feed.run(&mut heal, &home, WINDOW_SECS + SAMPLE_SECS, 0.8, true);
    assert!(heal.worker.is_none());
    assert_eq!(held(&home), "build active within window");
    assert!(!root(&home).join("daemon-restarts").exists());
    // The build stops; the hold lasts until no build has been seen for a whole window.
    feed.run(&mut heal, &home, WINDOW_SECS - 3.0 * SAMPLE_SECS, 0.8, false);
    assert!(heal.worker.is_none());
    assert_eq!(held(&home), "build active within window");
    feed.run(&mut heal, &home, 3.0 * SAMPLE_SECS, 0.8, false);
    assert_eq!(latest_restart(&mut heal, &home)["outcome"], "restarted");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn holds_syspolicyd_after_a_restart_that_never_went_quiet() {
    let home = crate::telemetry::test_home("daemon-heal-ineffective");
    let mut heal = DaemonHeal::default();
    let mut feed = Feed { now: 1000.0, cpu: 0.0, pid: 62496, start: 100 };
    feed.run(&mut heal, &home, WINDOW_SECS, 0.8, false);
    let first = wait_worker(&mut heal);
    assert_eq!(first["pid"], 62496);
    // The successor stays hot through the cooldown: held, not restarted again.
    let restarted_at = first["at_ms"].as_f64().unwrap() / 1000.0;
    let mut feed = Feed { now: feed.now, cpu: 0.0, pid: 72965, start: restarted_at as i64 + 1 };
    feed.run(&mut heal, &home, COOLDOWN_SECS + SAMPLE_SECS, 0.8, false);
    assert!(heal.worker.is_none());
    assert_eq!(held(&home), "previous restart left no quiet window");
    // One quiet window re-arms it; the next spin is restarted.
    feed.run(&mut heal, &home, WINDOW_SECS + SAMPLE_SECS, 0.1, false);
    assert!(heal.worker.is_none());
    assert!(held(&home).is_null());
    feed.run(&mut heal, &home, WINDOW_SECS + SAMPLE_SECS, 0.9, false);
    assert_eq!(latest_restart(&mut heal, &home)["pid"], 72965);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn builds_do_not_hold_perf_power_services() {
    let home = crate::telemetry::test_home("daemon-heal-ppd-build");
    let mut heal = DaemonHeal::default();
    let mut now = 1000.0;
    while now <= 1000.0 + WINDOW_SECS {
        heal.observe(&home, now, &[daemon("PerfPowerServices", 91618, 100, (now - 1000.0) * 1.4),
            Proc { pid: 4242, ppid: 1, uid: 501, start: 100, cpu_secs: 0.0, rss_kib: 1, name: "rustc".into(), args: "rustc".into() }]).unwrap();
        now += SAMPLE_SECS;
    }
    assert_eq!(wait_worker(&mut heal)["name"], "PerfPowerServices");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn retries_a_single_ineffective_restart_after_the_cooldown() {
    let home = crate::telemetry::test_home("daemon-heal-pps-retry");
    let log = root(&home).join("daemon-restarts");
    std::fs::create_dir_all(&log).unwrap();
    // 2026-11-04 17:28 SGT shape: 77382 -> 59268 settled at 0.79 core; retried at 19:40 (0.11).
    let record = json!({"at_ms": 1_000_000.0, "name": "PerfPowerServices", "outcome": "restarted",
        "pid": 77382, "new_pid": 59268, "floor": 0.5, "usage_one_core": 0.87, "new_usage_one_core": 0.79});
    std::fs::write(log.join("0.ndjson"), format!("{record}\n")).unwrap();
    let mut heal = DaemonHeal::default();
    let start = 1000.0 + PERF_POWER_COOLDOWN_SECS;
    let mut now = start;
    while heal.worker.is_none() && now <= start + WINDOW_SECS + 2.0 * SAMPLE_SECS {
        heal.observe(&home, now, &[daemon("PerfPowerServices", 59268, 1001, (now - start) * 0.9)]).unwrap();
        now += SAMPLE_SECS;
    }
    assert_eq!(wait_worker(&mut heal)["pid"], 59268);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn holds_perf_power_services_for_24_h_after_two_ineffective_restarts_in_a_row() {
    let home = crate::telemetry::test_home("daemon-heal-pps-ineffective");
    let log = root(&home).join("daemon-restarts");
    std::fs::create_dir_all(&log).unwrap();
    // 59268's predecessor restart at t=-6200 s settled hot, and its retry at t=1000 s left 59268 at 0.79 core.
    let first = json!({"at_ms": -6_200_000.0, "name": "PerfPowerServices", "outcome": "restarted",
        "pid": 77000, "new_pid": 77382, "floor": 0.5, "usage_one_core": 0.9, "new_usage_one_core": 0.8});
    let record = json!({"at_ms": 1_000_000.0, "name": "PerfPowerServices", "outcome": "restarted",
        "pid": 77382, "new_pid": 59268, "floor": 0.5, "usage_one_core": 0.87, "new_usage_one_core": 0.79});
    std::fs::write(log.join("0.ndjson"), format!("{first}\n{record}\n")).unwrap();
    let mut heal = DaemonHeal::default();
    // One continuous 0.9-core feed of the successor from past the 2 h cooldown.
    let start = 1000.0 + PERF_POWER_COOLDOWN_SECS;
    let mut now = start;
    let mut feed = |heal: &mut DaemonHeal, until: f64| {
        while now <= until {
            heal.observe(&home, now, &[daemon("PerfPowerServices", 59268, 1001, (now - start) * 0.9)]).unwrap();
            now += SAMPLE_SECS;
            if heal.worker.is_some() { break; }
        }
    };
    feed(&mut heal, start + WINDOW_SECS + SAMPLE_SECS);
    assert!(heal.worker.is_none());
    let state = super::super::read(&root(&home).join("daemon-heal.json")).unwrap();
    assert_eq!(state["targets"]["PerfPowerServices"]["held"], INEFFECTIVE_HOLD);
    // Held until 24 h after the ineffective restart, then restarted again.
    feed(&mut heal, 1000.0 + INEFFECTIVE_HOLD_SECS - SAMPLE_SECS);
    assert!(heal.worker.is_none());
    feed(&mut heal, 1000.0 + INEFFECTIVE_HOLD_SECS + 2.0 * SAMPLE_SECS);
    assert_eq!(wait_worker(&mut heal)["pid"], 59268);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_later_effective_restart_lifts_the_ineffective_hold() {
    let home = crate::telemetry::test_home("daemon-heal-pps-lifted");
    let log = root(&home).join("daemon-restarts");
    std::fs::create_dir_all(&log).unwrap();
    let rec = |at_ms: f64, pid: i64, new_pid: i64, usage: f64| json!({"at_ms": at_ms, "name": "PerfPowerServices",
        "outcome": "restarted", "pid": pid, "new_pid": new_pid, "floor": 0.5, "new_usage_one_core": usage});
    let first = rec(0.0, 10, 11, 0.8);
    let hot = rec(1_000_000.0, 11, 12, 0.79);
    // One ineffective restart: retried, not held.
    std::fs::write(log.join("0.ndjson"), format!("{hot}\n")).unwrap();
    assert!(ineffective_restarts(&home, 2000.0).is_empty());
    // Two in a row on the same lineage: held from the second.
    std::fs::write(log.join("0.ndjson"), format!("{first}\n{hot}\n")).unwrap();
    assert_eq!(ineffective_restarts(&home, 2000.0).get("PerfPowerServices"), Some(&1000.0));
    // 2026-11-04 shape: an effective retry (0.11) lifts it.
    let ok = rec(8_900_000.0, 12, 13, 0.11);
    std::fs::write(log.join("0.ndjson"), format!("{first}\n{hot}\n{ok}\n")).unwrap();
    assert!(ineffective_restarts(&home, 9000.0).is_empty());
    // Older than 24 h: no hold either.
    std::fs::write(log.join("0.ndjson"), format!("{first}\n{hot}\n")).unwrap();
    assert!(ineffective_restarts(&home, 1000.0 + INEFFECTIVE_HOLD_SECS + 1.0).is_empty());
    // 2026-11-08 shape: the second restart hit an instance launchd started on its own (54596 after
    // 90874 -> 87196), so it is a first strike and gets its retry.
    let fresh = rec(1_000_000.0, 54596, 41867, 0.72);
    std::fs::write(log.join("0.ndjson"), format!("{first}\n{fresh}\n")).unwrap();
    assert!(ineffective_restarts(&home, 2000.0).is_empty());
    // Two ineffective restarts more than 24 h apart are not in a row.
    let late = rec(1_000_000.0 + INEFFECTIVE_HOLD_SECS * 1000.0 + 1000.0, 11, 12, 0.79);
    std::fs::write(log.join("0.ndjson"), format!("{first}\n{late}\n")).unwrap();
    assert!(ineffective_restarts(&home, 1000.0 + INEFFECTIVE_HOLD_SECS + 2000.0).is_empty());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_later_syspolicyd_instance_is_not_held_as_the_restart_successor() {
    // Restart at t=1000 s; the respawned pid starts 9 s later (11-08 04:47:02 -> 04:47:11).
    let respawned = Track { start: 1009, ..Track::default() };
    assert_eq!(hold_reason("syspolicyd", &respawned, Some(&1000.0), None, 9000.0, f64::MAX),
        Some("previous restart left no quiet window"));
    // A boot-time instance two days later (pid 273, 2026-11-05) is judged on its own window.
    let rebooted = Track { start: 1000 + 2 * 86_400, ..Track::default() };
    assert_eq!(hold_reason("syspolicyd", &rebooted, Some(&1000.0), None, 300_000.0, f64::MAX), None);
    // Builds still hold it.
    assert_eq!(hold_reason("syspolicyd", &rebooted, Some(&1000.0), None, 300_000.0, 0.0),
        Some("build active within window"));
}

#[test]
fn an_effective_restart_does_not_hold_and_syspolicyd_ignores_the_ineffective_hold() {
    let effective = json!({"outcome": "restarted", "floor": 0.5, "new_usage_one_core": 0.13});
    let hot = json!({"outcome": "restarted", "floor": 0.5, "new_usage_one_core": 0.5});
    assert!(!ineffective_restart(&effective) && ineffective_restart(&hot));
    assert!(!ineffective_restart(&json!({"outcome": "restarted", "floor": 0.5, "new_usage_one_core": null})));
    let track = Track { quiet: true, start: 101, ..Track::default() };
    assert_eq!(hold_reason("syspolicyd", &track, None, Some(&100.0), 200.0, f64::MAX), None);
    assert_eq!(hold_reason("PerfPowerServices", &track, None, Some(&100.0), 200.0, 0.0), Some(INEFFECTIVE_HOLD));
    assert_eq!(hold_reason("PerfPowerServices", &track, None, Some(&100.0), 100.0 + INEFFECTIVE_HOLD_SECS, 0.0), None);
}

#[test]
fn the_ineffective_hold_covers_only_the_restart_successor() {
    // 2026-11-07: restart at 10:14 (t=0) settled at 0.82 core; launchd replaced the successor
    // with pid 54596 at 16:53 (t=23,981 s), which spun from 21:40 under the by-name hold.
    let successor = Track { pid: 87196, start: 9, ..Track::default() };
    let replaced = Track { pid: 54596, start: 23_981, ..Track::default() };
    assert_eq!(hold_reason("PerfPowerServices", &successor, Some(&0.0), Some(&0.0), 41_000.0, f64::MAX),
        Some(INEFFECTIVE_HOLD));
    assert_eq!(hold_reason("PerfPowerServices", &replaced, Some(&0.0), Some(&0.0), 41_000.0, f64::MAX), None);
}

/// The newest restart record, whether or not a later observe already drained the worker.
fn latest_restart(heal: &mut DaemonHeal, home: &Path) -> Value {
    if let Some(worker) = heal.worker.take() {
        let _ = worker.recv();
    }
    super::super::read(&root(home).join("daemon-restart.json")).unwrap()
}

#[test]
fn perf_power_services_cools_down_for_two_hours_and_syspolicyd_for_six() {
    assert_eq!(cooldown_secs("PerfPowerServices"), 7_200.0);
    assert_eq!(cooldown_secs("syspolicyd"), COOLDOWN_SECS);
    let home = crate::telemetry::test_home("daemon-heal-pps-cooldown");
    let mut heal = DaemonHeal::default();
    let end = run(&mut heal, &home, 1000.0, WINDOW_SECS, 1.4, 91618);
    let first = wait_worker(&mut heal);
    assert_eq!(first["pid"], 91618);
    // The successor spins at once; it is restarted again only after 2 h, not 6 h.
    let restarted_at = first["at_ms"].as_f64().unwrap() / 1000.0;
    run(&mut heal, &home, end, PERF_POWER_COOLDOWN_SECS + 2.0 * SAMPLE_SECS, 1.4, 79298);
    let second = latest_restart(&mut heal, &home);
    assert_eq!(second["pid"], 79298);
    let gap = second["at_ms"].as_f64().unwrap() / 1000.0 - restarted_at;
    assert!((PERF_POWER_COOLDOWN_SECS..PERF_POWER_COOLDOWN_SECS + 2.0 * SAMPLE_SECS).contains(&gap), "gap {gap}");
    let state = super::super::read(&root(&home).join("daemon-heal.json")).unwrap();
    assert_eq!(state["cooldown_s"]["PerfPowerServices"], 7_200.0);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn holds_syspolicyd_while_a_revocation_sweep_advances() {
    let home = crate::telemetry::test_home("daemon-heal-sweep");
    let mut heal = DaemonHeal::default();
    // 2026-11-13 09:25 SGT shape: 0.51 core, no build, 730 re-checks in the last 10 minutes.
    heal.sweep.fake = Some(730);
    let mut feed = Feed { now: 1000.0, cpu: 0.0, pid: 85121, start: 100 };
    feed.run(&mut heal, &home, WINDOW_SECS + 20.0 * SAMPLE_SECS, 0.51, false);
    assert!(heal.worker.is_none());
    assert_eq!(held(&home), super::super::gatekeeper_sweep::HOLD);
    assert!(!root(&home).join("daemon-restarts").exists());
    // The sweep is over but the daemon still spins: the next probe lifts the hold.
    heal.sweep.fake = Some(0);
    feed.run(&mut heal, &home, 20.0 * SAMPLE_SECS, 0.51, false);
    assert_eq!(latest_restart(&mut heal, &home)["pid"], 85121);
    std::fs::remove_dir_all(home).unwrap();
}

/// fseventsd (founder decision 2026-11-15, N8.4.1.1): 1.15 cores over the window is restarted with
/// its 0.8 floor on the record; 0.6 core, above the others' floor but below its own, is left alone.
#[test]
fn restarts_a_saturated_fseventsd_at_its_own_floor() {
    let home = crate::telemetry::test_home("daemon-heal-fseventsd");
    let mut heal = DaemonHeal::default();
    let feed = |heal: &mut DaemonHeal, from: f64, rate: f64, pid: i32| {
        let mut now = from;
        while now <= from + WINDOW_SECS + SAMPLE_SECS {
            heal.observe(&home, now, &[daemon("fseventsd", pid, 100, (now - from) * rate)]).unwrap();
            now += SAMPLE_SECS;
        }
    };
    feed(&mut heal, 1000.0, 0.6, 109);
    assert!(heal.worker.is_none(), "0.6 core is below the fseventsd floor");
    feed(&mut heal, 10_000.0, 1.15, 110);
    let record = wait_worker(&mut heal);
    assert_eq!(record["name"], "fseventsd");
    assert_eq!(record["floor"], 0.8);
    assert_eq!(record["outcome"], "restarted");
    assert!(record["sample"].is_null(), "tests never run sudo sample");
    std::fs::remove_dir_all(home).unwrap();
}
