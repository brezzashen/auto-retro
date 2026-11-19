use super::*;

fn describe(pid: i32) -> (String, String) {
    match pid {
        7 => ("Typeless Helper".into(), "Typeless.app".into()),
        _ => ("ffmpeg".into(), "Orca.app".into()),
    }
}

fn lines(home: &Path) -> Vec<Value> {
    let directory = root(home).join("mic-sessions");
    std::fs::read_dir(directory).map(|entries| entries.flat_map(|entry| {
        std::fs::read_to_string(entry.unwrap().path()).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect::<Vec<Value>>()
    }).collect()).unwrap_or_default()
}

#[test]
fn a_session_records_its_app_and_the_settled_controlcenter_delta() {
    let home = super::super::test_home("mic-one");
    let mut ledger = Ledger::default();
    // A baseline poll first: the session takes the last count before it started.
    let mut ports = vec![2000u64, 2000, 2085];
    let mut count = || Some((1, if ports.len() > 1 { ports.remove(0) } else { ports[0] }));
    ledger.step(&home, 95.0, &[], &mut count, &describe).unwrap();
    ledger.step(&home, 100.0, &[7], &mut count, &describe).unwrap();
    ledger.step(&home, 103.0, &[], &mut count, &describe).unwrap();
    // Nothing is written until SETTLE_SECS after input stops.
    assert!(lines(&home).is_empty());
    ledger.step(&home, 113.0, &[], &mut count, &describe).unwrap();
    let events = lines(&home);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["app"], "Typeless.app");
    assert_eq!(events[0]["duration_s"], 3.0);
    assert_eq!(events[0]["cc_port_delta"], 85);
    assert_eq!(events[0]["cc_before_age_s"], 5.0);
    assert_eq!(ledger.state(113.0, None)["today"][0]["cc_port_delta"], 85);
    // The port series logs each change once: 2000, then 2085.
    let series = std::fs::read_dir(root(&home).join("controlcenter-ports")).unwrap().next().unwrap().unwrap().path();
    assert_eq!(std::fs::read_to_string(series).unwrap().lines().count(), 2);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn overlapping_sessions_name_each_other_and_a_missing_count_stays_unknown() {
    let home = super::super::test_home("mic-overlap");
    let mut ledger = Ledger::default();
    let mut count = || None;
    ledger.step(&home, 100.0, &[7], &mut count, &describe).unwrap();
    ledger.step(&home, 101.0, &[7, 9], &mut count, &describe).unwrap();
    ledger.step(&home, 102.0, &[9], &mut count, &describe).unwrap();
    ledger.step(&home, 104.0, &[], &mut count, &describe).unwrap();
    ledger.step(&home, 120.0, &[], &mut count, &describe).unwrap();
    let events = lines(&home);
    assert_eq!(events.len(), 2);
    let typeless = events.iter().find(|e| e["app"] == "Typeless.app").unwrap();
    assert_eq!(typeless["overlapped"][0], "Orca.app");
    assert!(typeless["cc_port_delta"].is_null());
    assert_eq!(ledger.state(120.0, None)["active"].as_array().unwrap().len(), 0);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_restart_rebuilds_todays_totals_from_the_day_file() {
    let home = super::super::test_home("mic-load");
    let mut ledger = Ledger::default();
    let mut ports = vec![2000u64, 2000, 2056];
    let mut count = || Some((1, if ports.len() > 1 { ports.remove(0) } else { ports[0] }));
    ledger.step(&home, 95.0, &[], &mut count, &describe).unwrap();
    ledger.step(&home, 100.0, &[7], &mut count, &describe).unwrap();
    ledger.step(&home, 102.0, &[], &mut count, &describe).unwrap();
    ledger.step(&home, 113.0, &[], &mut count, &describe).unwrap();
    let reloaded = Ledger::load(&home, 200.0);
    assert_eq!(reloaded.state(200.0, None)["today"][0]["cc_port_delta"], 56);
    assert_eq!(reloaded.state(200.0, None)["today"][0]["sessions"], 1);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_session_spanning_a_controlcenter_respawn_has_no_delta() {
    let home = super::super::test_home("mic-respawn");
    let mut ledger = Ledger::default();
    // 2026-10-21: 5,024 on the old pid and 1,079 on the respawned one were recorded as -3,945.
    let mut counts = vec![(4064, 5024u64), (4064, 5024), (93490, 1079)];
    let mut count = || Some(if counts.len() > 1 { counts.remove(0) } else { counts[0] });
    ledger.step(&home, 95.0, &[], &mut count, &describe).unwrap();
    ledger.step(&home, 100.0, &[7], &mut count, &describe).unwrap();
    ledger.step(&home, 103.0, &[], &mut count, &describe).unwrap();
    ledger.step(&home, 113.0, &[], &mut count, &describe).unwrap();
    let events = lines(&home);
    assert!(events[0]["cc_port_delta"].is_null());
    assert_eq!(events[0]["cc_restarted"], true);
    assert_eq!(events[0]["cc_pid_after"], 93490);
    assert_eq!(ledger.state(113.0, None)["today"][0]["cc_port_delta"], 0);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn mic_warm_does_not_share_a_delta_but_a_session_overlapping_it_is_shared() {
    let home = super::super::test_home("mic-shared");
    let describe = |pid: i32| -> (String, String) { match pid {
        7 => ("Typeless Helper".into(), "Typeless.app".into()),
        8 => ("mic-warm".into(), "mic-warm".into()),
        _ => ("replayd".into(), "replayd".into()),
    } };
    let mut ledger = Ledger::default();
    let mut ports = vec![2000u64, 2000, 2000, 2090, 2090, 2650];
    let mut count = || Some((1, if ports.len() > 1 { ports.remove(0) } else { ports[0] }));
    ledger.step(&home, 95.0, &[], &mut count, &describe).unwrap();
    ledger.step(&home, 100.0, &[8], &mut count, &describe).unwrap();
    // Typeless dictates over mic-warm alone: its +90 stays attributed.
    ledger.step(&home, 101.0, &[8, 7], &mut count, &describe).unwrap();
    ledger.step(&home, 103.0, &[8], &mut count, &describe).unwrap();
    ledger.step(&home, 113.0, &[8], &mut count, &describe).unwrap();
    // A screen recording joins mic-warm: the +560 jump goes to replayd, mic-warm is shared.
    ledger.step(&home, 120.0, &[8, 9], &mut count, &describe).unwrap();
    ledger.step(&home, 130.0, &[], &mut count, &describe).unwrap();
    ledger.step(&home, 140.0, &[], &mut count, &describe).unwrap();
    let events = lines(&home);
    let by = |app: &str| events.iter().find(|e| e["app"] == app).unwrap().clone();
    assert_eq!(by("Typeless.app")["attributed"], true);
    assert_eq!(by("Typeless.app")["cc_port_delta"], 90);
    assert_eq!(by("replayd")["attributed"], true);
    assert_eq!(by("replayd")["cc_port_delta"], 560);
    assert_eq!(by("mic-warm")["overlapped"], json!(["Typeless.app", "replayd"]));
    assert_eq!(by("mic-warm")["attributed"], false);
    let today = ledger.state(140.0, None)["today"].clone();
    let total = |app: &str| today.as_array().unwrap().iter().find(|t| t["app"] == app).unwrap().clone();
    assert_eq!(total("Typeless.app")["cc_port_delta"], 90);
    assert_eq!(total("replayd")["cc_port_delta"], 560);
    assert_eq!(total("mic-warm")["cc_port_delta"], 0);
    assert_eq!(total("mic-warm")["shared_sessions"], 1);
    // Reloading the day file reproduces the same split.
    assert_eq!(Ledger::load(&home, 200.0).state(200.0, None)["today"], today);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_session_starting_while_another_settles_shares_its_growth() {
    let home = super::super::test_home("mic-settling");
    let mut ledger = Ledger::default();
    let mut ports = vec![2000u64, 2000, 2050, 2140];
    let mut count = || Some((1, if ports.len() > 1 { ports.remove(0) } else { ports[0] }));
    ledger.step(&home, 95.0, &[], &mut count, &describe).unwrap();
    ledger.step(&home, 100.0, &[7], &mut count, &describe).unwrap();
    ledger.step(&home, 103.0, &[], &mut count, &describe).unwrap();
    // Orca captures 4 s after Typeless stopped, before Typeless's count settles.
    ledger.step(&home, 107.0, &[9], &mut count, &describe).unwrap();
    ledger.step(&home, 109.0, &[], &mut count, &describe).unwrap();
    ledger.step(&home, 113.0, &[], &mut count, &describe).unwrap();
    ledger.step(&home, 119.0, &[], &mut count, &describe).unwrap();
    let events = lines(&home);
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|e| e["attributed"] == false));
    assert!(ledger.state(119.0, None)["today"].as_array().unwrap().iter().all(|t| t["cc_port_delta"] == 0));
    std::fs::remove_dir_all(home).unwrap();
}
