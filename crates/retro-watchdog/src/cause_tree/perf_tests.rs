use super::*;

fn tree(script_close: bool) -> Value {
    json!({"nodes": [
        {"id": "root", "parent": null, "status": "open"},
        {"id": "N8", "parent": "root", "status": "open", "triggers": ["whole_host_cpu_continuously_gt_95_for_120s", "windowserver_render_stall"]},
        {"id": "N8.1", "parent": "N8", "status": "closed", "probe": PROBE, "match": ["com.apple.Virtualization.VirtualMachine"],
         "close_share": 0.4, "script_close": script_close, "question": "Does the Docker VM dominate?", "answer": "Image builds in the VM."},
        {"id": "N8.2", "parent": "N8", "status": "open", "probe": PROBE, "match": ["rustc", "swift-frontend"], "close_share": 0.4},
    ]})
}

fn home(name: &str, script_close: bool) -> std::path::PathBuf {
    let home = crate::telemetry::test_home(name);
    let refs = home.join(".codex/skills/watchdog/references");
    std::fs::create_dir_all(&refs).unwrap();
    std::fs::write(refs.join("cause-tree.json"), serde_json::to_vec(&tree(script_close)).unwrap()).unwrap();
    std::fs::create_dir_all(root(&home).join("cpu-windows")).unwrap();
    std::fs::create_dir_all(root(&home).join("incidents")).unwrap();
    let frame = |at: f64, vm: f64, rustc: f64| json!({"at_ms": at, "cpu": {"total_cpu_percent_one_core": vm + rustc + 100.0,
        "by_name": [{"name": "com.apple.Virtualization.VirtualMachine", "cpu_percent_one_core": vm}, {"name": "rustc", "cpu_percent_one_core": rustc}]}});
    write_json(&root(&home).join("cpu-windows/600000.json"), &json!({"frames": [frame(700_000.0, 1400.0, 500.0), frame(730_000.0, 1600.0, 300.0), frame(5_000_000.0, 0.0, 9000.0)]})).unwrap();
    std::fs::create_dir_all(root(&home).join("daemon-restarts")).unwrap();
    std::fs::write(root(&home).join("daemon-restarts/0.ndjson"),
        format!("{}\n{}\n", json!({"at_ms": 650_000.0, "outcome": "restarted", "name": "PerfPowerServices"}), json!({"at_ms": -5_000_000.0, "outcome": "failed: old"}))).unwrap();
    write_json(&root(&home).join("incidents/7.json"), &json!({"trigger": "whole_host_cpu_continuously_gt_95_for_120s",
        "hot_since_ms": 690_000.0, "detected_at_ms": 740_000.0})).unwrap();
    home
}

#[test]
fn compares_the_window_with_every_class_and_lists_heals() {
    let home = home("perf-attribute", false);
    let found = attribute(&home, &tree(false), 690_000.0, 740_000.0, &[json!({"outcome": "done", "decision": {"target": "x"}})]);
    assert_eq!(found["frames"], 2, "frames outside the padded window are ignored");
    assert_eq!(found["dominant"], "N8.1");
    assert!((found["classes"][0]["share"].as_f64().unwrap() - 1500.0 / 2000.0).abs() < 1e-9);
    assert_eq!(found["explained_by"], "N8.1");
    assert_eq!(found["heals"].as_array().unwrap().len(), 2, "the daemon restart before the episode and the surge action");
    assert_eq!(found["heal_failed"], false);
    assert_eq!(found["unclassified"], json!([]), "every frame name belongs to a class");
    // A name no class matches is listed with its share, so a missing class shows.
    let only_vm = json!({"nodes": [tree(false)["nodes"][2].clone()]});
    let gap = attribute(&home, &only_vm, 690_000.0, 740_000.0, &[]);
    assert_eq!(gap["unclassified"][0]["name"], "rustc");
    assert!((gap["unclassified"][0]["share"].as_f64().unwrap() - 400.0 / 2000.0).abs() < 1e-9);
    // An open node may dominate but never explains.
    let rust = attribute(&home, &json!({"nodes": [tree(false)["nodes"][3].clone()]}), 690_000.0, 740_000.0, &[]);
    assert_eq!((rust["dominant"].as_str(), rust["explained_by"].is_null()), (Some("N8.2"), true));
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_script_close_node_closes_the_episode_without_a_session() {
    let home = home("perf-close", true);
    assert_eq!(crate::wake::request(&home, 800_000.0, crate::wake::Condition::CpuSurge, "7", "surge").unwrap(), crate::wake::Outcome::Closed);
    assert!(root(&home).join("incidents/7.ack").exists());
    assert!(root(&home).join("analysis/7/perf-attribution.json").exists());
    let state = crate::telemetry::read_state(&root(&home).join("wake-perf.json")).unwrap();
    assert!(state["sessions"].as_array().unwrap().is_empty());
    assert_eq!(state["episodes"][0]["cause"]["explained_by"], "N8.1");
    assert_eq!(crate::cause_tree::ledger::rows(&home).last().unwrap()["node"], "N8.1");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_failed_heal_keeps_the_episode_for_a_session() {
    let home = home("perf-heal-failed", true);
    write_json(&root(&home).join("analysis/7/report.json"), &json!({"actions": [{"outcome": "failed: permission denied"}]})).unwrap();
    assert_eq!(crate::wake::request(&home, 800_000.0, crate::wake::Condition::CpuSurge, "7", "surge").unwrap(), crate::wake::Outcome::Queued);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn host_cpu_the_frames_miss_is_recorded_beside_the_share() {
    let home = home("perf-host-share", false);
    let window = root(&home).join("cpu-windows/600000.json");
    let mut state = crate::telemetry::read_state(&window).unwrap();
    // Frames see 2000 one-core percent; the host was busy enough for four times that.
    let cores = std::thread::available_parallelism().unwrap().get() as f64;
    state["samples"] = json!([{"start": 695_000.0, "end": 700_000.0, "percent": 8000.0 / cores}, {"start": 725_000.0, "end": 730_000.0, "percent": 8000.0 / cores}]);
    write_json(&window, &state).unwrap();
    let found = attribute(&home, &tree(false), 690_000.0, 740_000.0, &[]);
    assert_eq!((found["host_cpu_one_core"].as_f64(), found["observed_cpu_one_core"].as_f64(), found["unattributed_cpu_one_core"].as_f64()),
        (Some(8000.0), Some(2000.0), Some(6000.0)));
    assert!((found["classes"][0]["share"].as_f64().unwrap() - 1500.0 / 2000.0).abs() < 1e-9);
    assert!((found["classes"][0]["host_share"].as_f64().unwrap() - 1500.0 / 8000.0).abs() < 1e-9);
    assert_eq!(found["explained_by"], json!("N8.1"), "closure still compares against frame-visible CPU");
    std::fs::remove_dir_all(home).unwrap();
}

/// Read-only check on this host's real state: `cargo test -p cpu-watchdog perf_real -- --ignored --nocapture`.
#[test]
#[ignore]
fn perf_real_attribution_of_recent_episodes() {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let tree = super::super::load(&home).unwrap();
    // PERF_REAL_IDS="a,b" replays other incidents.
    let ids = std::env::var("PERF_REAL_IDS").unwrap_or_else(|_| "1793073487633,1792937207853,81325399107252315325,8432046041290125855".into());
    for id in ids.split(',') {
        let event = crate::telemetry::read_state(&root(&home).join(format!("incidents/{id}.json"))).unwrap();
        let (from, to) = window(&event, 0.0);
        let found = attribute(&home, &tree, from, to, &[]);
        let closes = found["explained_by"].as_str().and_then(|n| super::super::node(&tree, n)).is_some_and(|n| n["script_close"] == true)
            && found["heal_failed"] == false;
        println!("{id} {} frames={} dominant={} share={:.2} explained_by={} heals={} script_close={closes}", event["trigger"], found["frames"], found["dominant"],
            found["classes"][0]["share"].as_f64().unwrap_or(0.0), found["explained_by"], found["heals"].as_array().map_or(0, Vec::len));
        println!("  unclassified={}", found["unclassified"]);
    }
}
