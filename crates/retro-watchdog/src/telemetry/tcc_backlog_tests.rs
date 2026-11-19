use super::*;

fn proc(pid: i32, ppid: i32, uid: u32, name: &str, args: &str) -> Proc {
    Proc { pid, ppid, uid, start: 100, cpu_secs: 0.0, rss_kib: 1000, name: name.into(), args: args.into() }
}
const SIM_TCCD: &str = "/private/var/run/com.apple.security.cryptexd/mnt/com.apple.iPhoneOS.SimulatorRuntime-v24.2/Library/Developer/CoreSimulator/Profiles/Runtimes/iOS 27.2.simruntime/Contents/Resources/RuntimeRoot/System/Library/PrivateFrameworks/TCC.framework/Support/tccd";

#[test]
fn only_our_launchd_parented_compilers_are_orphans() {
    let procs = vec![
        proc(1, 1, 501, "swift-frontend", "swift-frontend -frontend -c"),
        proc(2, 900, 501, "swift-frontend", "swift-frontend -frontend -c"),
        proc(3, 1, 0, "clang", "clang"),
        proc(4, 1, 501, "node", "node server.js"),
        proc(5, 1, 501, "ld", "ld -o MarsCRM"),
    ];
    let found: Vec<i32> = orphaned_compilers(&procs, 501).iter().map(|p| p.pid).collect();
    assert_eq!(found, vec![1, 5]);
}

#[test]
fn restart_targets_skip_simulator_tccd() {
    let procs = vec![
        proc(655, 1, 0, "sandboxd", "/usr/libexec/sandboxd"),
        proc(661, 1, 0, "tccd", "/System/Library/PrivateFrameworks/TCC.framework/Support/tccd system"),
        proc(1028, 1, 501, "tccd", "/System/Library/PrivateFrameworks/TCC.framework/Support/tccd"),
        proc(7843, 2, 501, "tccd", SIM_TCCD),
        proc(9, 1, 0, "syspolicyd", "/usr/libexec/syspolicyd"),
    ];
    let found: Vec<i32> = restart_targets(&procs, 501).iter().map(|p| p.pid).collect();
    assert_eq!(found, vec![655, 661, 1028]);
    assert_eq!(system_tccd(&procs).map(|p| p.pid), Some(661));
}

#[test]
fn usage_needs_a_full_window_of_one_instance() {
    let mut samples = VecDeque::new();
    let mut tccd = proc(661, 1, 0, "tccd", "tccd system");
    assert_eq!(record_sample(&mut samples, 1000.0, &tccd), None);
    tccd.cpu_secs = 100.0;
    assert_eq!(record_sample(&mut samples, 1200.0, &tccd), None);
    tccd.cpu_secs = 300.0;
    let usage = record_sample(&mut samples, 1400.0, &tccd).unwrap();
    assert!((usage - 0.75).abs() < 1e-9, "300 cpu s over 400 s, got {usage}");
    let respawned = Proc { pid: 24366, start: 1400, cpu_secs: 1.0, ..tccd.clone() };
    assert_eq!(record_sample(&mut samples, 1460.0, &respawned), None, "a new instance starts a new window");
}

#[test]
fn old_samples_are_pruned_but_one_full_window_is_kept() {
    let mut samples = VecDeque::new();
    let mut tccd = proc(661, 1, 0, "tccd", "tccd system");
    for step in 0..30 {
        tccd.cpu_secs = step as f64 * 6.0;
        record_sample(&mut samples, step as f64 * 60.0, &tccd);
    }
    assert!(samples.len() <= 7, "kept {}", samples.len());
    let usage = record_sample(&mut samples, 1800.0, &Proc { cpu_secs: 180.0, ..tccd }).unwrap();
    assert!((usage - 0.1).abs() < 1e-9, "6 cpu s per 60 s, got {usage}");
}

#[test]
fn restart_is_due_only_when_hot_outside_cooldown_and_hold() {
    assert!(due(Some(0.9), 10_000.0, 0.0, 0.0));
    assert!(!due(Some(0.2), 10_000.0, 0.0, 0.0));
    assert!(!due(None, 10_000.0, 0.0, 0.0));
    assert!(!due(Some(0.9), 10_000.0, 9_000.0, 0.0), "inside the cooldown");
    assert!(!due(Some(0.9), 10_000.0, 0.0, 20_000.0), "held after an ineffective restart");
}

#[test]
fn a_hot_successor_marks_the_restart_ineffective() {
    assert!(ineffective(&json!({"outcome": "restarted", "new_usage_one_core": 0.8})));
    assert!(!ineffective(&json!({"outcome": "restarted", "new_usage_one_core": 0.02})));
    assert!(!ineffective(&json!({"outcome": "failed: sudo", "new_usage_one_core": null})));
}
