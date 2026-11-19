use super::*;
use super::daily::DAILY_SECS;

fn run(at_s: f64, before: f64, after: f64) -> Value {
    json!({"at_ms": at_s * 1000.0, "before_gib": before, "after_gib": after, "exit": 1, "summary": null})
}

#[test]
fn s0_runs_only_below_the_floor_and_after_its_cooldown() {
    let mut guard = DiskGuard::default();
    let now = 20_000.0 * 86_400.0;
    assert!(!guard.s0_due(now, Some(MIN_FREE_GIB + 0.1), None, false));
    assert!(!guard.s0_due(now, None, None, false), "no measurement, no cleanup");
    assert!(guard.s0_due(now, Some(MIN_FREE_GIB - 0.1), None, false));
    assert!(!guard.s0_due(now, Some(50.0), None, true), "an open session owns the cleanup");
    guard.runs.push(run(now - S0_COOLDOWN_SECS + 1.0, MIN_FREE_GIB - 10.0, MIN_FREE_GIB + 20.0));
    assert!(!guard.s0_due(now, Some(50.0), None, false));
    assert!(guard.s0_due(now + 1.0, Some(50.0), None, false));
    // A run that left the disk short waits the longer cooldown.
    guard.runs.push(run(now, 90.0, 95.0));
    assert!(!guard.s0_due(now + S0_COOLDOWN_SECS, Some(50.0), None, false));
    assert!(guard.s0_due(now + S0_SHORT_COOLDOWN_SECS, Some(50.0), None, false));
}

#[test]
fn a_short_s0_run_opens_one_session_until_it_acknowledges() {
    let home = crate::telemetry::test_home("disk-guard");
    let now = 20_000.0 * 86_400.0;
    let mut guard = DiskGuard::default();
    // Nothing ran yet, or S0 recovered: no session.
    assert_eq!(guard.escalate(&home, now, Some(90.0)).unwrap(), None);
    guard.absorb(&home, now, run(now, MIN_FREE_GIB - 10.0, MIN_FREE_GIB + 10.0));
    assert_eq!(guard.escalate(&home, now, Some(MIN_FREE_GIB + 10.0)).unwrap(), None);
    // Short after S0 but already above the floor now: no session.
    guard.absorb(&home, now + 10.0, run(now + 10.0, 90.0, 95.0));
    assert_eq!(guard.escalate(&home, now + 10.0, Some(MIN_FREE_GIB + 1.0)).unwrap(), None);
    let id = guard.escalate(&home, now + 20.0, Some(95.0)).unwrap().expect("incident opened");
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents").join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(incident["trigger"], "disk_space_low");
    assert_eq!(incident["s0"]["after_gib"], 95.0);
    // While it is open the same incident is returned and nothing new starts.
    assert_eq!(guard.escalate(&home, now + 30.0, Some(95.0)).unwrap(), Some(id.clone()));
    assert_eq!(guard.dispatches.len(), 1);
    // After the ack the run the session saw does not reopen; a new short run waits for the daily cap.
    write_json(&root(&home).join("incidents").join(format!("{id}.ack")), &json!({})).unwrap();
    assert_eq!(guard.escalate(&home, now + 40.0, Some(95.0)).unwrap(), None);
    guard.absorb(&home, now + 50.0, run(now + 50.0, 95.0, 96.0));
    assert_eq!(guard.escalate(&home, now + 60.0, Some(96.0)).unwrap(), None, "daily cap of {MAX_DISPATCH_PER_DAY}");
    let next = guard.escalate(&home, now + 86_400.0 + 30.0, Some(96.0)).unwrap().expect("next day");
    assert_ne!(next, id);
    // Every S0 run is kept in the day file.
    let day = ((now * 1000.0) / DAY_MS).floor() * DAY_MS;
    let lines = std::fs::read_to_string(root(&home).join("disk-guard").join(format!("{}.ndjson", day as u64))).unwrap();
    assert_eq!(lines.lines().count(), 3);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn summarizes_recover_disk_output_and_picks_the_smallest_volume() {
    let report = json!({"recovered": false, "totalReclaimedItems": 3,
        "steps": [{"step": "s0:tm-snapshots", "reclaimed": 1, "details": ["a"]}, {"step": "s0:k8s-images(keep=5)", "reclaimed": 2, "details": ["b", "c"]}]});
    assert_eq!(summarize(&report), json!({"recovered": false, "reclaimed_items": 3,
        "steps": [{"step": "s0:tm-snapshots", "reclaimed": 1}, {"step": "s0:k8s-images(keep=5)", "reclaimed": 2}],
        "docker_vm": {"before_gib": null, "after_gib": null}}));
    assert_eq!(summarize(&Value::Null), Value::Null);
    assert_eq!(lowest(&json!([{"path": "/a", "free_gib": 120.0}, {"path": "/b", "free_gib": 99.5}])), Some(99.5));
    assert_eq!(lowest(&json!([])), None);
    assert!(free_gib("/").is_some_and(|free| free > 0.0));
}

#[test]
fn the_prompt_names_a_route_that_exists() {
    let prompt = prompt(Path::new("/h"), "8123", 93.4, None);
    assert!(prompt.contains("route disk-low") && prompt.contains("monitor-ack 8123") && prompt.contains("93.4 GiB") && !prompt.contains('\n'));
    let skill = PathBuf::from(std::env::var("HOME").unwrap()).join(".codex/skills/watchdog");
    if skill.exists() {
        assert!(skill.join("references/disk-low.md").exists());
        assert!(std::fs::read_to_string(skill.join("SKILL.md")).unwrap().contains("`disk-low`"));
    }
}

#[test]
fn a_short_docker_vm_runs_s0_even_when_the_host_has_room() {
    let mut guard = DiskGuard::default();
    let now = 20_000.0 * 86_400.0;
    let roomy = MIN_FREE_GIB + 140.0;
    assert!(!guard.s0_due(now, Some(roomy), Some(DOCKER_VM_MIN_FREE_GIB + 0.1), false));
    assert!(!guard.s0_due(now, Some(roomy), None, false), "no engine or container: unknown, no cleanup");
    assert!(guard.s0_due(now, Some(roomy), Some(4.7), false));
    // Builds fill and drain the VM, so a run that left only the VM short keeps the normal cooldown.
    let mut short = run(now, roomy, roomy);
    short["vm_after_gib"] = json!(8.0);
    guard.runs.push(short);
    assert!(!guard.s0_due(now + S0_COOLDOWN_SECS - 1.0, Some(roomy), Some(8.0), false));
    assert!(guard.s0_due(now + S0_COOLDOWN_SECS, Some(roomy), Some(8.0), false));
}

#[test]
fn a_docker_vm_still_short_after_s0_opens_a_session_naming_the_vm() {
    let home = crate::telemetry::test_home("disk-guard-vm");
    let now = 20_000.0 * 86_400.0;
    let mut guard = DiskGuard::default();
    let roomy = MIN_FREE_GIB + 140.0;
    let mut short = run(now, roomy, roomy);
    short["vm_before_gib"] = json!(9.7);
    short["vm_after_gib"] = json!(9.7);
    guard.absorb(&home, now, short);
    guard.last_vm = Some(13.0);
    assert_eq!(guard.escalate(&home, now + 1.0, Some(roomy)).unwrap(), None, "the VM recovered since");
    guard.last_vm = Some(9.7);
    let id = guard.escalate(&home, now + 2.0, Some(roomy)).unwrap().expect("incident opened");
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents").join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(incident["docker_vm"]["short"], true);
    assert!(prompt(&home, &id, roomy, Some(9.7)).contains("Docker VM"));
}

#[test]
fn the_s0_report_is_read_after_the_deploy_lease_notices() {
    let text = "✅ deploy lease (local-k8s) acquired\nRelease it when the deploy finishes:\n{\n  \"recovered\": true,\n  \"totalReclaimedItems\": 1,\n  \"steps\": [],\n  \"dockerVm\": {\"before\": {\"freeGib\": 4.7}, \"after\": {\"freeGib\": 14.4}}\n}\n";
    let summary = summarize(&report_json(text));
    assert_eq!(summary["recovered"], true);
    assert_eq!(summary["docker_vm"]["after_gib"], 14.4);
    assert_eq!(report_json("not json"), Value::Null);
}

#[test]
fn docker_vm_free_space_comes_from_the_df_data_line() {
    let df = "Filesystem     1024-blocks      Used Available Capacity Mounted on\noverlay         287205236 267495064   4932728      99% /\n";
    let free = super::docker_vm::parse_df(df).unwrap();
    assert!((free - 4.704).abs() < 0.01, "{free}");
    assert_eq!(super::docker_vm::parse_df("Error: no such container"), None);
}

#[test]
fn a_run_that_skipped_snapshots_for_a_running_backup_is_retried() {
    // Shape of disk-guard/1793153046987-s0.json (2026-10-28 10:04): the lease notice, then the report.
    let text = "✅ deploy lease (local-k8s) acquired\n{\"recovered\": false, \"steps\": [\
        {\"step\": \"s0:tm-snapshots\", \"reclaimed\": 1, \"details\": [\"Error: snapshot cleanup requires an idle Time Machine\"]},\
        {\"step\": \"s0:release-outputs(keep=3)\", \"reclaimed\": 0, \"details\": []}]}";
    let report = report_json(text);
    assert!(tm_retry::skipped_for_backup(&report));
    let mut recovered = report.clone();
    recovered["recovered"] = json!(true);
    assert!(!tm_retry::skipped_for_backup(&recovered), "a recovered run needs no retry");
    let pruned = report_json("{\"recovered\": false, \"steps\": [{\"step\": \"s0:tm-snapshots\", \"details\": [\"local-snapshot:2026-10-28-022344\"]}]}");
    assert!(!tm_retry::skipped_for_backup(&pruned), "snapshots were pruned; nothing to retry");
    assert!(!tm_retry::skipped_for_backup(&Value::Null));
    // In tests the retry never runs commands and keeps the measured volumes.
    let after = json!([{"path": "/System/Volumes/Data", "free_gib": 90.0}]);
    assert_eq!(tm_retry::run(&report, after.clone()), (after, Value::Null));
}

#[test]
fn s0_runs_once_a_day_even_with_room_everywhere() {
    let roomy = MIN_FREE_GIB + 140.0;
    let now = 20_000.0 * 86_400.0;
    let mut guard = DiskGuard { since: now, ..DiskGuard::default() };
    assert!(!guard.s0_due(now + DAILY_SECS - 1.0, Some(roomy), Some(500.0), false), "within a day of start");
    assert!(guard.s0_due(now + DAILY_SECS, Some(roomy), Some(500.0), false), "a day after start, no run yet");
    assert!(!guard.s0_due(now + DAILY_SECS, None, None, false), "unmeasured host never triggers");
    guard.runs.push(run(now + DAILY_SECS, roomy, roomy));
    assert!(!guard.s0_due(now + 2.0 * DAILY_SECS - 1.0, Some(roomy), None, false), "a day after the last run");
    assert!(guard.s0_due(now + 2.0 * DAILY_SECS, Some(roomy), None, false));
    assert!(!guard.s0_due(now + 2.0 * DAILY_SECS, Some(roomy), None, true), "an open session owns the cleanup");
}

#[test]
fn no_session_opens_while_a_fresh_s0_run_is_in_flight() {
    let home = crate::telemetry::test_home("disk-guard-in-flight");
    let now = 20_000.0 * 86_400.0;
    let mut guard = DiskGuard::default();
    // An earlier short run nobody saw yet, and a new S0 started this tick.
    guard.absorb(&home, now - 36_000.0, run(now - 36_000.0, 250.0, 258.0));
    let (_sender, receiver) = std::sync::mpsc::sync_channel::<Value>(1);
    guard.worker = Some(receiver);
    assert_eq!(guard.escalate(&home, now, Some(MIN_FREE_GIB - 0.1)).unwrap(), None, "the running S0 decides first");
    assert!(guard.dispatches.is_empty());
    // Once the fresh run lands short, the session sees that run.
    guard.worker = None;
    guard.absorb(&home, now + 60.0, run(now + 60.0, MIN_FREE_GIB - 0.1, MIN_FREE_GIB - 5.0));
    let id = guard.escalate(&home, now + 61.0, Some(MIN_FREE_GIB - 5.0)).unwrap().expect("incident opened");
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents").join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(incident["s0"]["at_ms"], (now + 60.0) * 1000.0);
    std::fs::remove_dir_all(home).unwrap();
}
