use super::*;

fn sample(m: &mut Monitor, start: f64, percent: f64) -> Option<String> {
    m.ingest(
        Sample {
            start,
            end: start + 5000.0,
            percent,
        },
        start + 5000.0,
    )
}

#[test]
fn strict_continuous_threshold_and_one_event_per_episode() {
    let mut m = Monitor::default();
    for i in 0..59 {
        assert!(sample(&mut m, i as f64 * 5000.0, 91.0).is_none());
    }
    assert!(sample(&mut m, 295_000.0, 91.0).is_some());
    assert!(sample(&mut m, 300_000.0, 99.0).is_none());
    sample(&mut m, 305_000.0, 90.0);
    assert!(m.since.is_none());
    for i in 0..59 {
        assert!(sample(&mut m, 310_000.0 + i as f64 * 5000.0, 95.0).is_none());
    }
    assert!(sample(&mut m, 605_000.0, 95.0).is_some());
}

#[test]
fn gap_resets_and_duplicates_do_not_extend_duration() {
    let mut m = Monitor::default();
    for i in 0..59 {
        sample(&mut m, i as f64 * 5000.0, 95.0);
    }
    assert!(sample(&mut m, 290_000.0, 95.0).is_none());
    assert!(sample(&mut m, 310_000.0, 95.0).is_none());
    assert_eq!(m.since, Some(310_000.0));
    m.ingest(
        Sample {
            start: 315_000.0,
            end: 335_000.0,
            percent: 95.0,
        },
        335_000.0,
    );
    assert!(m.since.is_none());
}

#[test]
fn below_threshold_breaks_sequence_and_nan_is_rejected() {
    let mut m = Monitor::default();
    for i in 0..59 {
        sample(&mut m, i as f64 * 5000.0, 95.0);
    }
    sample(&mut m, 295_000.0, 89.0);
    assert!(sample(&mut m, 300_000.0, 99.0).is_none());
    sample(&mut m, 305_000.0, f64::NAN);
    assert!(m.since.is_none());
}

#[test]
fn future_sample_does_not_poison_subsequent_valid_history() {
    let mut m = Monitor::default();
    m.ingest(
        Sample {
            start: 0.0,
            end: 5000.0,
            percent: 95.0,
        },
        4000.0,
    );
    assert!(m.since.is_none());
    sample(&mut m, 0.0, 95.0);
    assert_eq!(m.since, Some(0.0));
    assert!(!m.latched);
}

#[test]
fn persisted_incident_acknowledgement_staleness_and_collector_restart() {
    let home = std::env::temp_dir().join(format!("cpu-monitor-test-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let history_path = home.join(".local/state/mars-cpu-build-gate/history.json");
    let mut m = Monitor::default();
    for i in 0..60 {
        let start = i as f64 * 5000.0;
        let history = serde_json::json!({"pid": 1, "updated": start + 5000.0,
            "samples": [{"start": start, "end": start + 5000.0, "percent": 95.0}]});
        write_json(&history_path, &history).unwrap();
        m.observe(&home, (start + 4000.0) / 1000.0, &[], || {
            (start + 5000.0) / 1000.0
        })
        .unwrap();
    }
    let incident = root(&home).join("incidents/300000.json");
    let event: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&incident).unwrap()).unwrap();
    assert_eq!(event["host_samples"].as_array().unwrap().len(), 60);
    assert_eq!(event["hot_since_ms"], 0.0);
    assert!(ack(&home, Some("300000".into())).is_err());
    let dir = root(&home).join("analysis/300000");
    std::fs::create_dir_all(&dir).unwrap();
    crate::log_recall::tests::capture_ok(&dir);
    crate::log_recall::tests::report(&dir);
    ack(&home, Some("300000".into())).unwrap();
    assert!(incident.with_extension("ack").is_file());
    assert!(ack(&home, Some("../monitor".into())).is_err());
    assert!(ack(&home, Some("123".into())).is_err());
    m.observe(&home, 321.0, &[], || 321.0).unwrap();
    assert!(m.since.is_none());
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root(&home).join("monitor.json")).unwrap()).unwrap();
    assert!(state["error"].is_string());
    write_json(
        &history_path,
        &serde_json::json!({"pid": 2, "updated": 325000,
        "samples": [{"start": 320000, "end": 325000, "percent": 95}]}),
    )
    .unwrap();
    m.observe(&home, 325.0, &[], || 325.0).unwrap();
    assert_eq!(m.since, Some(320000.0));
    assert!(!m.latched);
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn process_evidence_uses_deltas_and_excludes_arguments_and_reused_pids() {
    let mut m = Monitor::default();
    let mut p = Proc {
        pid: 42,
        ppid: 1,
        uid: 501,
        start: 10,
        cpu_secs: 50.0,
        rss_kib: 100,
        name: "tool".into(),
        args: "credential=secret".into(),
    };
    m.frame(100.0, &[p.clone()]);
    p.cpu_secs += 15.0;
    m.frame(115.0, &[p.clone()]);
    assert_eq!(
        m.frames.back().unwrap().consumers[0].cpu_percent_one_core,
        100.0
    );
    assert!(!serde_json::to_string(&m.frames).unwrap().contains("secret"));
    p.start = 115;
    m.frame(130.0, &[p]);
    assert!(m.frames.back().unwrap().consumers.is_empty());
}
