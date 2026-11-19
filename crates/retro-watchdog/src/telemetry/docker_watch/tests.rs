use super::heal::*;
use super::*;

#[test]
fn engine_failures_restart_before_kubernetes_failures_and_respect_cooldown_and_cap() {
    let now = 1_000_000.0;
    assert_eq!(due(ENGINE_FAILS - 1, 0, &[], now), None);
    assert_eq!(due(ENGINE_FAILS, KUBE_FAILS, &[], now), Some(Trigger::Engine));
    assert_eq!(due(0, KUBE_FAILS - 1, &[], now), None);
    assert_eq!(due(0, KUBE_FAILS, &[], now), Some(Trigger::Kube));
    assert_eq!(due(ENGINE_FAILS, 0, &[now - COOLDOWN_SECS + 1.0], now), None);
    assert_eq!(due(ENGINE_FAILS, 0, &[now - COOLDOWN_SECS], now), Some(Trigger::Engine));
    let full: Vec<f64> = (0..DAILY_ACTIONS).map(|i| now - COOLDOWN_SECS - 60.0 * i as f64).collect();
    assert_eq!(due(ENGINE_FAILS, 0, &full, now), None);
    let old: Vec<f64> = full.iter().map(|at| at - 86_400.0).collect();
    assert_eq!(due(ENGINE_FAILS, 0, &old, now), Some(Trigger::Engine));
}

#[test]
fn classification_holds_restarts_that_would_corrupt_or_split_the_vm_disk() {
    let base = json!({"copying": [], "blocked": [], "data_exists": true, "data_free_bytes": MIN_FREE_BYTES * 2, "backend_age_s": 3600});
    let with = |key: &str, value: Value| { let mut e = base.clone(); e[key] = value; e };
    assert_eq!(classify(&with("copying", json!(["1 01:00 /bin/cp -c Docker.raw /Volumes/X"])), false), "vm_disk_copy");
    assert_eq!(classify(&with("blocked", json!([" 8727       16:30 /usr/bin/tmutil addexclusion /Volumes/MarsBuild/DockerDesktop"])), false), "volume_access_blocked");
    assert_eq!(classify(&with("data_exists", json!(false)), false), "data_volume_missing");
    assert_eq!(classify(&with("data_free_bytes", json!(MIN_FREE_BYTES - 1)), false), "data_volume_full");
    assert_eq!(classify(&with("backend_age_s", json!(FRESH_START_SECS - 1)), false), "starting");
    assert_eq!(classify(&base, true), "kubernetes_down");
    assert_eq!(classify(&base, false), "engine_hung");
    assert_eq!(classify(&with("backend_age_s", Value::Null), false), "backend_dead");
}

#[test]
fn recognises_vm_disk_copies_and_parses_ps_and_df() {
    assert!(is_disk_copy("123 01:02:03 /bin/cp -c /Users/b/Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw /Volumes/MarsBuild/DockerDesktop/"));
    assert!(is_disk_copy("123 05:00 rsync -a /old/DockerDesktop /new"));
    assert!(!is_disk_copy("123 05:00 ugrep -G Docker.raw"));
    assert!(!is_disk_copy("123 05:00 /Applications/Docker.app/Contents/MacOS/com.docker.backend"));
    // The 2026-10-24 18:46 incident line, then a fresh exclusion and an unrelated tmutil call.
    assert!(is_blocked_exclusion(" 8727       16:30 /usr/bin/tmutil addexclusion /Volumes/MarsBuild/DockerDesktop"));
    assert!(!is_blocked_exclusion(" 8727       00:05 /usr/bin/tmutil addexclusion /Volumes/MarsBuild/DockerDesktop"));
    assert!(!is_blocked_exclusion(" 8727       16:30 /usr/bin/tmutil status DockerDesktop"));
    assert_eq!(etime_secs("05:07"), Some(307));
    assert_eq!(etime_secs("01:00:00"), Some(3600));
    assert_eq!(etime_secs("2-00:00:01"), Some(172_801));
    assert_eq!(etime_secs("x"), None);
    let df = "Filesystem 1024-blocks Used Available Capacity iused ifree %iused Mounted on\n/dev/disk5s1 1000 400 600 40% 1 2 0% /Volumes/MarsBuild\n";
    assert_eq!(df_free_bytes(df), Some(600 * 1024));
    let vz = "[t][com.docker.virtualization] Disk 0 (<HOME>/Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw) caching is enabled\n\
              [t][com.docker.virtualization] Disk 0 (/Volumes/MarsBuild/DockerDesktop/Docker.raw) caching is enabled\n";
    assert_eq!(disk_folder(vz, Path::new("/Users/b")).as_deref(), Some("/Volumes/MarsBuild/DockerDesktop"));
    assert_eq!(disk_folder(vz.lines().next().unwrap(), Path::new("/Users/b")).as_deref(),
        Some("/Users/b/Library/Containers/com.docker.docker/Data/vms/0/data"));
}

#[test]
fn keeps_real_backend_errors_and_drops_event_stream_eofs() {
    let lines: Vec<String> = [
        "[t][com.docker.backend.idle] event handler 1: event streamer: error: EOF",
        "[t][com.docker.backend] level=error msg=\"vm crashed\"",
        "[t][com.docker.backend] request: GET /v1/events",
        "[t][com.docker.backend] panic: runtime error",
    ].iter().map(|s| s.to_string()).collect();
    assert_eq!(error_lines(&lines, 30), vec![lines[1].clone(), lines[3].clone()]);
    assert_eq!(error_lines(&lines, 1), vec![lines[3].clone()]);
}

#[test]
fn finds_crash_looping_redis_replicas_and_ready_sentinels() {
    let pod = |name: &str, redis: Value, sentinel_ready: bool| json!({"metadata": {"name": name}, "status": {"containerStatuses": [
        {"name": "redis", "ready": false, "state": redis},
        {"name": "sentinel", "ready": sentinel_ready, "state": {"running": {}}}]}});
    let pods = json!({"items": [
        pod("mars-crm-redis-0", json!({"running": {}}), true),
        pod("mars-crm-redis-1", json!({"running": {}}), false),
        pod("mars-crm-redis-2", json!({"waiting": {"reason": "CrashLoopBackOff"}}), true)]});
    let (crashing, ready) = redis_pods(&pods);
    assert_eq!(crashing, vec!["mars-crm-redis-2"]);
    assert_eq!(ready, vec!["mars-crm-redis-0"]);
}

#[test]
fn pause_file_holds_recovery_until_it_expires() {
    let home = crate::telemetry::test_home("docker-pause");
    assert!(paused(&home, 100.0).is_none());
    write_json(&root(&home).join(PAUSE), &json!({"at_ms": 0.0, "until_ms": 200_000.0, "reason": "disk migration"})).unwrap();
    assert_eq!(paused(&home, 100.0).unwrap()["reason"], "disk migration");
    assert!(paused(&home, 200.0).is_none());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_pause_written_after_the_round_spawned_skips_its_recovery() {
    let home = crate::telemetry::test_home("docker-pause-race");
    assert!(skipped_if_paused(&home, 1_793_694_323.0).is_none());
    // 2026-11-03: the pause landed at 16:25:23.490, the round ran its recovery 0.42 s later.
    write_json(&root(&home).join(PAUSE), &json!({"at_ms": 1_793_694_323_490.0, "until_ms": 1_793_694_923_490.0, "reason": "win Docker restart"})).unwrap();
    let skipped = skipped_if_paused(&home, 1_793_694_323.914).unwrap();
    assert_eq!(skipped["outcome"], "skipped: paused");
    assert_eq!(skipped["pause"]["reason"], "win Docker restart");
    let mut watch = DockerWatch { loaded: true, ..Default::default() };
    let probe = || Probe { ok: false, ms: 1, error: None };
    watch.absorb(&home, 1_793_694_324.0, Round { engine: probe(), kube: probe(), recovery: Some(skipped), repairs: vec![], collisions: vec![], stops_seen: vec![] });
    assert!(watch.actions.is_empty(), "a skipped round spends no restart budget");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_held_local_k8s_lease_pauses_recovery_until_its_ttl() {
    let home = crate::telemetry::test_home("docker-lease");
    let lease = home.join(LEASE);
    std::fs::create_dir_all(lease.parent().unwrap()).unwrap();
    std::fs::write(&lease, "marsbuild-cutover|1000|3600|42|ce1b11a8").unwrap();
    let pause = paused(&home, 2000.0).unwrap();
    assert_eq!(pause["reason"], "local-k8s lease held by marsbuild-cutover");
    assert_eq!(pause["session"], "ce1b11a8");
    assert!(paused(&home, 4600.0).is_none());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn absorb_counts_failures_records_actions_and_opens_the_repair_window_on_return() {
    let home = crate::telemetry::test_home("docker-absorb");
    let mut watch = DockerWatch { loaded: true, ..Default::default() };
    let probe = |ok: bool| Probe { ok, ms: 1, error: None };
    watch.absorb(&home, 100.0, Round { engine: probe(false), kube: probe(false), recovery: None, repairs: vec![], collisions: vec![], stops_seen: vec![] });
    assert_eq!((watch.engine_fails, watch.kube_fails), (1, 1));
    let skipped = json!({"outcome": "skipped: starting", "cause": "starting"});
    watch.absorb(&home, 130.0, Round { engine: probe(false), kube: probe(false), recovery: Some(skipped), repairs: vec![], collisions: vec![], stops_seen: vec![] });
    assert!(watch.actions.is_empty());
    assert_eq!(watch.engine_fails, 2);
    watch.absorb(&home, 160.0, Round { engine: probe(true), kube: probe(false), recovery: None, repairs: vec![], collisions: vec![], stops_seen: vec![] });
    assert_eq!(watch.repair_until, 160.0 + REPAIR_WINDOW_SECS);
    assert_eq!((watch.engine_fails, watch.kube_fails), (0, 3));
    std::fs::remove_dir_all(home).unwrap();
}

/// `cargo test -p cpu-watchdog docker_live -- --ignored --nocapture`: probes, evidence and
/// classification on this host, without any recovery action.
#[test]
#[ignore]
fn docker_live_probe_and_evidence() {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let (engine, kube) = (probe_engine(&home), probe_kube(&home));
    let evidence = super::heal::evidence_for_test(&home);
    println!("engine ok={} {}ms {:?}\nkube ok={} {}ms {:?}", engine.ok, engine.ms, engine.error, kube.ok, kube.ms, kube.error);
    println!("cause={}\n{}", classify(&evidence, engine.ok), serde_json::to_string_pretty(&evidence).unwrap());
}

#[test]
fn remote_mode_reads_the_tunnel_alias_and_never_restarts_local_docker() {
    let plist = "<array>\n    <string>/Users/b/.local/bin/offload</string>\n    <string>tunnel</string>\n    <string>--alias</string>\n    <string>win</string>\n  </array>";
    assert_eq!(super::remote::alias_from_plist(plist).as_deref(), Some("win"));
    assert_eq!(super::remote::alias_from_plist("<string>offload</string><string>tunnel</string>"), None);
    let cause = |tunnel: bool, engine: bool| super::remote::classify(&json!({"tunnel_listening": tunnel, "engine_ok": engine}));
    // 2026-10-28 07:53: the tunnel was down while the Windows engine served; the cure is the tunnel.
    assert_eq!(cause(false, true), "tunnel_down");
    assert_eq!(cause(true, false), "remote_engine_down");
    assert_eq!(cause(true, true), "remote_kubernetes_down");
    let plan = |local: bool, tunnel: bool, engine: bool| super::remote::plan(&json!({
        "local_desktop_running": local, "tunnel_listening": tunnel, "engine_ok": engine}));
    // 2026-10-28 11:36: the host rebooted and a Dock-opened local Desktop held 8035-8038.
    assert_eq!(plan(true, false, false), ["stop_local_desktop", "restore_desktop_linux", "start_remote_desktop", "kickstart_tunnel"]);
    assert_eq!(plan(false, false, true), ["kickstart_tunnel"]);
    assert_eq!(plan(false, true, false), ["start_remote_desktop"]);
    assert!(plan(false, true, true).is_empty());
}

#[test]
fn stale_published_ports_are_republished_only_when_the_cluster_answers_inside_the_node() {
    // 2026-11-17 13:36 SGT: the Windows host ran out of commit memory, its WSL VM restarted under
    // the surviving Desktop backend, and every published port answered EOF while the node was Ready.
    let plan = |engine: bool, tunnel: bool, inside: Value| super::remote::plan(&json!({
        "local_desktop_running": false, "host_reachable": true, "tunnel_listening": tunnel, "engine_ok": engine, "cluster_ready_inside": inside}));
    assert_eq!(plan(true, true, json!(true)), ["republish_ports"]);
    assert!(plan(true, true, json!(false)).is_empty());
    assert!(plan(true, true, Value::Null).is_empty());
    assert_eq!(plan(true, false, json!(true)), ["kickstart_tunnel"]);
    assert_eq!(plan(false, true, Value::Null), ["start_remote_desktop"]);
}

#[test]
fn an_unreachable_remote_host_is_its_own_cause_and_gets_no_start_task() {
    // 2026-11-02 05:33 SGT: a 0x7F bugcheck took the Windows host off the network; the tunnel was
    // down only because of it, and ssh schtasks could not reach the host.
    let down = json!({"host_reachable": false, "tunnel_listening": false, "engine_ok": false, "local_desktop_running": false});
    assert_eq!(super::remote::classify(&down), "remote_host_down");
    assert_eq!(super::remote::plan(&down), ["kickstart_tunnel"]);
    let up = json!({"host_reachable": true, "tunnel_listening": false, "engine_ok": false, "local_desktop_running": false});
    assert_eq!(super::remote::classify(&up), "tunnel_down");
    assert_eq!(super::remote::plan(&up), ["start_remote_desktop", "kickstart_tunnel"]);
}

#[test]
fn a_dark_host_after_a_clean_shutdown_is_a_hung_restart_not_a_crash() {
    // 2026-11-03 19:05:10 SGT: a planned reboot shut Windows down cleanly (Kernel-General 13) and
    // the host stayed dark until the power card reset it at 19:21; the newest Kernel-Power 41 was
    // the 2026-11-02 06:47 one, which the record used to show as this outage's bugcheck.
    let started = 1_793_704_806_125.0;
    let attr = |boot: Value| super::remote::host_down_attribution(Some(&boot), started)["node"].clone();
    assert_eq!(attr(json!({"unexpected_this_boot": false, "last_clean_shutdown_ms": 1_793_703_910_763.0})), "N4.13");
    assert_eq!(attr(json!({"unexpected_this_boot": true, "last_clean_shutdown_ms": 1_793_703_910_763.0})), "N4.11");
    // A clean shutdown hours before belongs to an earlier boot, not to this outage.
    assert_eq!(attr(json!({"unexpected_this_boot": false, "last_clean_shutdown_ms": started - 7_200_000.0})), "N4.11");
    // Unparsed evidence (an older script) or an unreachable host keeps the crash node.
    assert_eq!(attr(json!({"exit": 1, "output": "x"})), "N4.11");
    assert_eq!(super::remote::host_down_attribution(None, started)["node"], "N4.11");
}

#[test]
fn a_boot_inside_a_reachable_recovery_counts_as_this_outage() {
    // 2026-11-04: recovery started 12:58:19Z with the host reachable (tunnel_down); win crashed
    // and booted 13:01:37Z during the wait. An older boot (10:49:04Z) is not this outage.
    let since = 1_793_797_099_510.0 - 120_000.0;
    assert!(super::remote::booted_during(&json!({"boot_ms": 1_793_797_297_500.0}), since));
    assert!(!super::remote::booted_during(&json!({"boot_ms": 1_793_789_344_500.0}), since));
    assert!(!super::remote::booted_during(&json!({"exit": 1, "output": "x"}), since));
}

#[test]
#[ignore = "live: reads the remote beta host over SSH"]
fn live_remote_host_boot() {
    println!("{}", super::remote::host_boot("win"));
}
