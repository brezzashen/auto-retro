use super::*;

fn process(pid: i32, ppid: i32, name: &str, cpu: f64, rss: u64) -> Proc {
    Proc {
        pid,
        ppid,
        uid: 501,
        start: 10,
        cpu_secs: cpu,
        rss_kib: rss,
        name: name.into(),
        args: "xcodebuild -derivedDataPath /tmp/dd secret=token".into(),
    }
}

#[test]
fn tree_peak_ends_durably_and_restart_keeps_observed_peaks() {
    let home = super::super::test_home("task");
    let mut tracker = Tasks::default();
    tracker
        .observe(
            &home,
            100.0,
            &[
                process(1, 0, "xcodebuild", 2.0, 10),
                process(2, 1, "swift-frontend", 3.0, 20),
            ],
        )
        .unwrap();
    tracker
        .observe(
            &home,
            115.0,
            &[
                process(1, 0, "xcodebuild", 17.0, 20),
                process(2, 1, "swift-frontend", 33.0, 40),
            ],
        )
        .unwrap();
    assert_eq!(tracker.peaks.len(), 1);
    assert_eq!(tracker.peaks[0].peak_tree_rss_kib, 60);
    assert_eq!(tracker.peaks[0].peak_tree_cpu_percent_one_core, Some(300.0));
    assert_eq!(tracker.peaks[0].peak_cpu_processes.len(), 2);
    assert_eq!(tracker.peaks[0].peak_cpu_processes[0]["name"], "swift-frontend");
    assert_eq!(tracker.peaks[0].peak_cpu_processes[0]["cpu_percent_one_core"], 200.0);
    let json = std::fs::read_to_string(root(&home).join("tasks.json")).unwrap();
    assert!(!json.contains("secret"));
    assert!(!json.contains("token"));
    assert!(json.contains("process-context.v1"));
    let mut restarted = Tasks::default();
    restarted
        .observe(&home, 130.0, &[process(1, 0, "xcodebuild", 18.0, 5)])
        .unwrap();
    assert_eq!(restarted.peaks[0].peak_tree_rss_kib, 60);
    restarted.observe(&home, 145.0, &[]).unwrap();
    assert!(root(&home).join("task-records/1-10.json").is_file());
    let saved: Value = serde_json::from_slice(&std::fs::read(root(&home).join("task-records/1-10.json")).unwrap()).unwrap();
    assert_eq!(saved["peak_cpu_processes"][0]["name"],"swift-frontend");
    assert_eq!(saved["program"]["schema"],"process-context.v1");
    assert_eq!(restarted.peaks[0].ended_observed_at_ms, Some(145000.0));
    let mut reused = process(1, 0, "xcodebuild", 0.0, 3);
    reused.start = 145;
    restarted.observe(&home, 160.0, &[reused]).unwrap();
    assert_eq!(restarted.peaks.len(), 2);
    assert_eq!(restarted.peaks[1].peak_tree_cpu_percent_one_core, None);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn missing_child_history_does_not_invent_tree_cpu() {
    let home = super::super::test_home("task-gap");
    let mut tracker = Tasks::default();
    tracker
        .observe(&home, 100.0, &[process(1, 0, "xcodebuild", 2.0, 10)])
        .unwrap();
    tracker
        .observe(
            &home,
            115.0,
            &[
                process(1, 0, "xcodebuild", 17.0, 20),
                process(2, 1, "clang", 3.0, 40),
            ],
        )
        .unwrap();
    assert_eq!(tracker.peaks[0].peak_tree_cpu_percent_one_core, None);
    assert_eq!(tracker.peaks[0].peak_tree_rss_kib, 60);
    std::fs::remove_dir_all(home).unwrap();
}
