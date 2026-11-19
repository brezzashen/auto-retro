use super::*;

const HOUR: f64 = 3_600_000.0;

fn home(name: &str) -> std::path::PathBuf {
    let home = crate::telemetry::test_home(name);
    std::fs::create_dir_all(root(&home).join("incidents")).unwrap();
    home
}

fn close(home: &Path, id: &str) {
    write_json(&root(home).join(format!("incidents/{id}.ack")), &json!({})).unwrap();
}

#[test]
fn every_condition_joins_the_one_open_session() {
    let home = home("wake-join");
    assert_eq!(request(&home, 0.0, Condition::CpuSurge, "1", "surge").unwrap(), Outcome::Queued);
    assert_eq!(request(&home, 1.0, Condition::CpuSurge, "1", "surge").unwrap(), Outcome::Queued, "an id is decided once");
    for (n, condition) in [Condition::RenderStall, Condition::WindowServerHot, Condition::Recovery, Condition::CpuHot, Condition::DockerVmHot].into_iter().enumerate() {
        assert_eq!(request(&home, 10.0 + n as f64, condition, &format!("{}", 2 + n), "x").unwrap(), Outcome::Joined, "{condition:?}");
    }
    let state = load(&home);
    assert_eq!(state["sessions"].as_array().unwrap().len(), 1, "one session for the whole load");
    assert!(state["episodes"].as_array().unwrap().iter().all(|e| e["session"] == "1"));
    // A stale open session stops absorbing episodes.
    assert_eq!(request(&home, 13.0 * HOUR, Condition::RenderStall, "9", "stall").unwrap(), Outcome::Queued);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn gap_and_join_only_defer_to_the_next_session_which_covers_them() {
    let home = home("wake-gap");
    assert_eq!(request(&home, 0.0, Condition::DockerVmHot, "1", "vm").unwrap(), Outcome::Deferred, "join-only never opens");
    assert_eq!(request(&home, 1.0, Condition::RenderStall, "2", "stall").unwrap(), Outcome::Queued);
    close(&home, "2");
    assert_eq!(request(&home, HOUR, Condition::WindowServerHot, "3", "hot").unwrap(), Outcome::Deferred, "inside the 2 h gap");
    assert_eq!(request(&home, 2.5 * HOUR, Condition::Recovery, "4", "docker").unwrap(), Outcome::Queued);
    let state = load(&home);
    let session = |id: &str| state["episodes"].as_array().unwrap().iter().find(|e| e["incident"] == id).unwrap()["session"].clone();
    assert_eq!(session("1"), "2");
    assert_eq!(session("3"), "4");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn caps_sessions_per_rolling_day() {
    let home = home("wake-cap");
    for n in 0..MAX_PER_DAY {
        let id = format!("{}", 10 + n);
        assert_eq!(request(&home, n as f64 * 3.0 * HOUR, Condition::RenderStall, &id, "stall").unwrap(), Outcome::Queued);
        close(&home, &id);
    }
    assert_eq!(request(&home, 12.0 * HOUR, Condition::Recovery, "20", "docker").unwrap(), Outcome::Deferred);
    assert_eq!(request(&home, 25.0 * HOUR, Condition::RenderStall, "21", "stall").unwrap(), Outcome::Queued, "the window rolls");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn cpu_surge_is_an_emergency_gate_without_gap_or_daily_cap() {
    let home = home("wake-surge-uncapped");
    // Ten surges, each 10 min after the previous session closed: every one opens its own review.
    for n in 0..10 {
        let id = format!("{}", 100 + n);
        assert_eq!(request(&home, n as f64 * HOUR / 6.0, Condition::CpuSurge, &id, "surge").unwrap(), Outcome::Queued, "surge {n}");
        close(&home, &id);
    }
    // Surge sessions do not use up the other conditions' gap or cap.
    for n in 0..MAX_PER_DAY {
        let id = format!("{}", 200 + n);
        assert_eq!(request(&home, 2.0 * HOUR + n as f64 * 3.0 * HOUR, Condition::RenderStall, &id, "stall").unwrap(), Outcome::Queued, "stall {n}");
        close(&home, &id);
    }
    assert_eq!(request(&home, 12.0 * HOUR, Condition::WindowServerHot, "300", "hot").unwrap(), Outcome::Deferred, "others keep 4 per 24 h");
    assert_eq!(request(&home, 12.0 * HOUR + 1.0, Condition::CpuSurge, "301", "surge").unwrap(), Outcome::Queued, "a surge still opens one");
    assert_eq!(request(&home, 12.0 * HOUR + 2.0, Condition::RenderStall, "302", "stall").unwrap(), Outcome::Joined, "an open surge session still absorbs the load");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn legacy_sessions_keep_counting_and_keep_their_own_gate() {
    let home = home("wake-legacy");
    write_json(&root(&home).join("wake-lag.json"), &json!({"sessions": [{"incident": "5", "at_ms": 0.0}], "episodes": []})).unwrap();
    assert_eq!(request(&home, HOUR, Condition::CpuSurge, "6", "surge").unwrap(), Outcome::Joined, "the open legacy session absorbs it");
    assert!(validate(&home, "5").is_ok(), "a legacy session is not held to the new report fields");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn ack_needs_every_covered_episode_and_a_bottleneck_map() {
    let home = home("wake-gate");
    request(&home, 0.0, Condition::CpuSurge, "1", "surge").unwrap();
    trigger(&home, 1.0, Condition::RenderStall, "2", "stall", true, json!({})).unwrap();
    assert!(root(&home).join("incidents/2.ack").exists(), "a joined episode is acknowledged by the trigger");
    let dir = root(&home).join("analysis/1");
    std::fs::create_dir_all(&dir).unwrap();
    let review = json!({"resource_contention": "a", "error_prevention": "b", "change_or_no_change_reason": "c", "validation": "d", "remaining_risk": "e"});
    let map = json!([{"resource": "CPU", "workloads": ["cargo"], "evidence": "cpu-windows", "action": "lower build priority"}]);
    let write = |episodes: Value, map: Value| write_json(&dir.join("report.json"), &json!({"episodes": episodes, "bottleneck_map": map,
        "optimization_review": review, "verification": {"passed": true}})).unwrap();
    write(json!([{"incident": "1", "finding": "builds"}]), map.clone());
    assert!(validate(&home, "1").unwrap_err().to_string().contains("missing 2"));
    write(json!([{"incident": "1", "finding": "builds"}, {"incident": "2", "finding": "same load"}]), json!([]));
    assert!(validate(&home, "1").is_err(), "an empty bottleneck map is refused");
    write(json!([{"incident": "1", "finding": "builds"}, {"incident": "2", "finding": "same load"}]), map);
    validate(&home, "1").unwrap();
    std::fs::remove_dir_all(home).unwrap();
}
