use super::*;

fn read(home: &Path, name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(root(home).join(name)).unwrap()).unwrap()
}

fn incidents(home: &Path) -> Vec<Value> {
    std::fs::read_dir(root(home).join("incidents")).map(|entries| entries
        .map(|entry| serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap())
        .collect()).unwrap_or_default()
}

#[test]
fn parses_the_total_and_ranks_peers_from_lsmp_output() {
    let text = "Process (174) : WindowServer\n\
        0x00000203  0xab31bdb5  send   ---  3   0x0  TASK-CONTROL SELF (174) WindowServer\n\
                  +     send        --------        ---            2         <-      0x002ba133  (548) ControlCenter\n\
                  +     send        --------        ---            1         <-      0x0038a8b3  (548) ControlCenter\n\
                  +     send        --------        ---            1         <-      0x00004b03  (102) logd\n\
        total     = 84658\n\
        Process (548) : ControlCenter\n\
        total     = 68705\n";
    let count = parse(text.as_bytes()).unwrap().unwrap();
    assert_eq!(count.total, 84658);
    assert_eq!(count.peers[0], ("(548) ControlCenter".to_string(), 2));
    assert!(parse("no total here\n".as_bytes()).unwrap().is_none());
}

#[test]
fn a_peer_name_cut_inside_a_multibyte_character_still_counts() {
    // lsmp truncated "客服消息弹窗" after the first two bytes of "窗" (e7 aa 97).
    let mut text = b"Process (174) : WindowServer\n\
                  +     send        --------        ---            1         <-      0x0038a8b3  (31262) SimMetalHost (\xe5\xae\xa2\xe6\x9c\x8d\xe6\xb6\x88\xe6\x81\xaf\xe5\xbc\xb9\xe7\xaa\n".to_vec();
    text.extend_from_slice(b"total     = 24334\n");
    let count = parse(text.as_slice()).unwrap().unwrap();
    assert_eq!(count.total, 24334);
    assert_eq!(count.peers.len(), 1);
    assert!(count.peers[0].0.starts_with("(31262) SimMetalHost (客服消息弹"));
}

#[test]
fn a_leak_opens_one_incident_per_process_instance_and_quiet_counts_do_not() {
    let home = super::super::test_home("ports");
    let count = |total| Some(Ok(Count { total, peers: vec![("(174) WindowServer".into(), total / 2)], dead_names: 0 }));
    evaluate(&home, 1000.0, vec![(&TARGETS[0], Some((548, 10)), count(1_100)), (&TARGETS[1], Some((174, 1)), count(17_000))]).unwrap();
    assert!(incidents(&home).is_empty());
    let state = read(&home, "port-census.json");
    assert_eq!(state["controlcenter"]["ports"], 1_100);
    assert_eq!(state["probe_errors"].as_array().unwrap().len(), 0);
    // Over the limit twice: still one incident and one dispatch for this ControlCenter.
    for now in [1300.0, 1600.0] {
        evaluate(&home, now, vec![(&TARGETS[0], Some((548, 10)), count(68_705)), (&TARGETS[1], Some((174, 1)), count(17_000))]).unwrap();
    }
    let found = incidents(&home);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["trigger"], "controlcenter_port_leak");
    assert_eq!(found[0]["ports"], 68_705);
    assert_eq!(read(&home, "dispatch.json")["dispatches"].as_array().unwrap().len(), 1);
    // A restarted ControlCenter is a new instance with a fresh incident key.
    evaluate(&home, 1900.0, vec![(&TARGETS[0], Some((4064, 1800)), count(12_000)), (&TARGETS[1], None, None)]).unwrap();
    assert_eq!(incidents(&home).len(), 2);
    assert_eq!(read(&home, "port-census.json")["windowserver"]["error"], "process not running");
    let lines = std::fs::read_dir(root(&home).join("port-census")).unwrap().count();
    assert_eq!(lines, 1);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_failed_count_is_a_probe_error() {
    let home = super::super::test_home("ports-error");
    evaluate(&home, 1000.0, vec![(&TARGETS[0], Some((548, 10)), Some(Err("sudo lsmp exited 1".into())))]).unwrap();
    let state = read(&home, "port-census.json");
    assert_eq!(state["probe_errors"][0], "ControlCenter: sudo lsmp exited 1");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn counts_dead_name_rights() {
    let text = "Process (660) : WindowServer\n\
        0x00011b03  0x00000000  dead-name   --------        ---            1\n\
        0x00011c03  0x00000000  dead-name   --------        ---            1\n\
        0x00000303  0xac6d015f  recv        --------     0  ---      1               N        5         0  0x0\n\
        total     = 3\n";
    let count = parse(text.as_bytes()).unwrap().unwrap();
    assert_eq!(count.dead_names, 2);
}

fn ws(total: u64, dead_names: u64) -> Option<Result<Count, String>> {
    Some(Ok(Count { total, peers: vec![], dead_names }))
}

#[test]
fn windowserver_dead_name_growth_opens_one_session_before_the_limit() {
    let home = super::super::test_home("ports-growth");
    // 12 samples five minutes apart, +150 dead names each: 1,800 per hour; sample 10 spans 50 minutes.
    for i in 0..12u64 {
        let now = 10_000.0 + i as f64 * 300.0;
        evaluate(&home, now, vec![(&TARGETS[1], Some((660, 1)), ws(30_000 + i * 150, 20_000 + i * 150))]).unwrap();
        let fired = incidents(&home).len();
        assert_eq!(fired, usize::from(i >= 10), "sample {i}: needs 50 minutes of window");
    }
    let found = incidents(&home);
    assert_eq!(found[0]["trigger"], "windowserver_port_growth");
    assert!(found[0]["dead_names_per_hour"].as_f64().unwrap() >= 1_000.0);
    let state = read(&home, "port-census.json");
    assert_eq!(state["windowserver"]["early"]["growth_incident"], found[0]["id"]);
    assert_eq!(read(&home, "dispatch.json")["dispatches"].as_array().unwrap().len(), 1);
    // Keeps growing: still one incident for this WindowServer instance.
    evaluate(&home, 10_000.0 + 12.0 * 300.0, vec![(&TARGETS[1], Some((660, 1)), ws(31_950, 21_950))]).unwrap();
    assert_eq!(incidents(&home).len(), 1);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn slow_dead_name_growth_only_marks_the_notify_line() {
    let home = super::super::test_home("ports-notify");
    for i in 0..14u64 {
        let now = 10_000.0 + i as f64 * 300.0;
        evaluate(&home, now, vec![(&TARGETS[1], Some((660, 1)), ws(34_990 + i * 10, 20_000 + i * 10))]).unwrap();
    }
    assert!(incidents(&home).is_empty(), "120 dead names per hour stays below the rate");
    let early = &read(&home, "port-census.json")["windowserver"]["early"];
    assert_eq!(early["notified"], true);
    assert!(early["dead_per_hour"].as_f64().unwrap() < 1_000.0);
    // A new WindowServer instance starts a fresh window and notify state.
    evaluate(&home, 20_000.0, vec![(&TARGETS[1], Some((900, 19_000)), ws(12_000, 100))]).unwrap();
    let early = &read(&home, "port-census.json")["windowserver"]["early"];
    assert_eq!(early["notified"], false);
    assert_eq!(read(&home, "port-census.json")["windowserver"]["dead_window"].as_array().unwrap().len(), 1);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_start_time_drifting_by_a_second_keeps_one_incident_and_the_dead_name_window() {
    // WindowServer pid 660 on 2026-11-05: start 1793668353 at 18:22, 1793668354 at 19:00.
    let home = super::super::test_home("ports-drift");
    for (i, start) in [1_000i64, 1_001, 999, 1_002].into_iter().enumerate() {
        let now = 50_000.0 + i as f64 * 300.0;
        evaluate(&home, now, vec![(&TARGETS[1], Some((660, start)), ws(51_000 + i as u64, 40_000 + i as u64))]).unwrap();
    }
    let found = incidents(&home);
    assert_eq!(found.len(), 1, "one leak incident for one WindowServer instance");
    let state = &read(&home, "port-census.json")["windowserver"];
    assert_eq!(state["process_start"], 1_000);
    assert_eq!(state["dead_window"].as_array().unwrap().len(), 4);
    // Beyond the tolerance it is another process instance.
    evaluate(&home, 52_000.0, vec![(&TARGETS[1], Some((660, 5_000)), ws(51_000, 40_000))]).unwrap();
    assert_eq!(incidents(&home).len(), 2);
    std::fs::remove_dir_all(home).unwrap();
}
