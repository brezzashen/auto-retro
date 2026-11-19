use super::*;
use crate::monitor::write_json;
use crate::wake::{request, Condition, Outcome};

const SKY: &str = "SkyComputerUseService";

fn tree(script_close: bool, status: &str) -> Value {
    json!({"nodes": [
        {"id": "N3", "parent": null, "status": "open", "triggers": [TRIGGER]},
        {"id": "N8.9", "parent": "N3", "status": "open", "probe": super::super::perf::PROBE, "match": ["WindowServer"], "close_share": 0.4},
        {"id": "N3.4.1", "parent": "N3", "status": status, "probe": PROBE, "match": [SKY], "min_frames": 2,
         "script_close": script_close, "question": "Agent computer use?", "answer": "Screenshots start capture streams."},
    ]})
}

/// A hot window 700..830 s: WindowServer at 85% in three frames, the computer-use service in
/// `agent` of them, and render-probe ticks carrying `stalls` stalls (None writes no tick).
fn home(name: &str, tree: &Value, agent: usize, stalls: Option<u64>, trigger: &str) -> std::path::PathBuf {
    let home = crate::telemetry::test_home(name);
    let refs = home.join(".codex/skills/watchdog/references");
    std::fs::create_dir_all(&refs).unwrap();
    std::fs::write(refs.join("cause-tree.json"), serde_json::to_vec(tree).unwrap()).unwrap();
    for directory in ["cpu-windows", "incidents", "render-probe"] { std::fs::create_dir_all(root(&home).join(directory)).unwrap(); }
    let frames: Vec<Value> = (0..3).map(|n| {
        let mut by_name = vec![json!({"name": "WindowServer", "cpu_percent_one_core": 85.0}), json!({"name": "rustc", "cpu_percent_one_core": 300.0})];
        if n < agent { by_name.push(json!({"name": SKY, "cpu_percent_one_core": 15.0})); }
        json!({"at_ms": 710_000.0 + 40_000.0 * n as f64, "cpu": {"total_cpu_percent_one_core": 800.0, "by_name": by_name}})
    }).collect();
    write_json(&root(&home).join("cpu-windows/600000.json"), &json!({"frames": frames})).unwrap();
    if let Some(stalls) = stalls {
        let ticks: Vec<String> = [(720_000.0, 0), (760_000.0, stalls), (5_000_000.0, 9)].iter()
            .map(|(at, stall_count)| json!({"at_ms": at, "stall_count": stall_count}).to_string()).collect();
        std::fs::write(root(&home).join("render-probe/0.ndjson"), ticks.join("\n")).unwrap();
    }
    write_json(&root(&home).join("incidents/7.json"), &json!({"trigger": trigger, "hot_since_ms": 700_000.0, "detected_at_ms": 830_000.0})).unwrap();
    home
}

fn explained(home: &Path) -> Value {
    crate::telemetry::read_state(&root(home).join("wake-perf.json")).unwrap()["episodes"][0]["cause"]["explained_by"].clone()
}

#[test]
fn agent_frames_without_a_render_stall_explain_a_hot_windowserver() {
    // Not marked for script closure: the session still opens, and starts from the named node.
    let home = home("gui-explains", &tree(false, "closed"), 2, Some(0), TRIGGER);
    assert_eq!(request(&home, 900_000.0, Condition::WindowServerHot, "7", "hot").unwrap(), Outcome::Queued);
    assert_eq!(explained(&home), "N3.4.1");
    let found = crate::telemetry::read_state(&root(&home).join("analysis/7/perf-attribution.json")).unwrap();
    assert_eq!(found["gui"], json!({"node": "N3.4.1", "agent_frames": 2, "frames": 3, "min_frames": 2, "agent_cpu_one_core": 15.0,
        "render_ticks": 2, "render_stalls": 0, "explains": true}), "the tick outside the window is not counted");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_script_close_node_closes_the_hot_window_and_names_its_basis() {
    let home = home("gui-close", &tree(true, "closed"), 3, Some(0), TRIGGER);
    assert_eq!(request(&home, 900_000.0, Condition::WindowServerHot, "7", "hot").unwrap(), Outcome::Closed);
    assert!(root(&home).join("incidents/7.ack").exists());
    let report = std::fs::read_to_string(root(&home).join("analysis/7/report.md")).unwrap();
    assert!(report.contains("N3.4.1") && report.contains("计算机操作服务出现在 3 帧") && !report.contains("占主机 CPU"), "{report}");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_stall_missing_ticks_too_few_frames_or_an_open_node_leave_it_unexplained() {
    for (name, tree, agent, stalls) in [("gui-stall", tree(true, "closed"), 3, Some(1)), ("gui-no-ticks", tree(true, "closed"), 3, None),
        ("gui-one-frame", tree(true, "closed"), 1, Some(0)), ("gui-open-node", tree(true, "open"), 3, Some(0))] {
        let home = home(name, &tree, agent, stalls, TRIGGER);
        assert_eq!(request(&home, 900_000.0, Condition::WindowServerHot, "7", "hot").unwrap(), Outcome::Queued, "{name}");
        assert!(explained(&home).is_null(), "{name}");
        std::fs::remove_dir_all(home).unwrap();
    }
}

#[test]
fn another_trigger_is_never_probed() {
    let home = home("gui-other-trigger", &tree(true, "closed"), 3, Some(0), "windowserver_render_stall");
    assert_eq!(request(&home, 900_000.0, Condition::RenderStall, "7", "stall").unwrap(), Outcome::Queued);
    let found = crate::telemetry::read_state(&root(&home).join("analysis/7/perf-attribution.json")).unwrap();
    assert!(found["gui"].is_null() && found["explained_by"].is_null());
    std::fs::remove_dir_all(home).unwrap();
}

/// Read-only check on this host's real state: `cargo test -p cpu-watchdog gui_real -- --ignored --nocapture`.
/// GUI_REAL_IDS="a,b" replays other windowserver_hot incidents.
#[test]
#[ignore]
fn gui_real_probe_of_recent_hot_windows() {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let tree = super::super::load(&home).unwrap();
    let ids = std::env::var("GUI_REAL_IDS").unwrap_or_else(|_| "84652822411718304550".into());
    for id in ids.split(',') {
        let event = crate::telemetry::read_state(&root(&home).join(format!("incidents/{id}.json"))).unwrap();
        let (from, to) = super::super::perf::window(&event, 0.0);
        let frames = super::super::perf::frames(&home, from - 60_000.0, to + 60_000.0).0;
        println!("{id} {}", probe(&home, &tree, &event, &frames, from - 60_000.0, to + 60_000.0).unwrap_or(Value::Null));
    }
}
