use super::*;

fn setup(name: &str) -> std::path::PathBuf {
    let home = crate::telemetry::test_home(name);
    write_json(
        &root(&home).join("crash-repair-policy.json"),
        &json!({"enabled":true,"enabled_at_ms":1000}),
    )
    .unwrap();
    home
}

fn event(home: &Path, id: &str, time: f64) {
    let path = home
        .join("Library/Logs/DiagnosticReports")
        .join(format!("MarsCRM-{id}.ips"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "{\"app_name\":\"MarsCRM\"}\n{\"private\":\"raw evidence\"}",
    )
    .unwrap();
    write_json(&root(home).join("incidents").join(format!("{id}.json")),
        &json!({"id":id,"trigger":"xcode_runtime_crash","detected_at_ms":time,"report":{"path":path,"known_issue":null}})).unwrap();
}

fn read(home: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(root(home).join("crash-repair.json")).unwrap()).unwrap()
}

fn complete(home: &Path, id: &str) {
    write_json(
        &root(home).join("analysis").join(id).join("report.json"),
        &json!({"status":"resolved","verification":{"passed":true}}),
    )
    .unwrap();
    write_json(
        &root(home).join("incidents").join(format!("{id}.ack")),
        &json!({}),
    )
    .unwrap();
}

#[test]
fn opt_in_baseline_dedup_and_private_evidence() {
    use std::os::unix::fs::PermissionsExt;
    let home = setup("crash-repair-baseline");
    event(&home, "1", 500.0);
    event(&home, "2", 1500.0);
    poll(&home, 2000.0).unwrap();
    poll(&home, 3000.0).unwrap();
    assert_eq!(read(&home)["dispatches"].as_array().unwrap().len(), 1);
    assert_eq!(read(&home)["dispatches"][0]["incident"], "2");
    let raw = root(&home).join("analysis/2/report.ips");
    assert!(std::fs::read_to_string(&raw)
        .unwrap()
        .contains("raw evidence"));
    assert_eq!(
        std::fs::metadata(raw).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(root(&home).join("analysis/2/evidence.json").exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn one_xcode_session_covers_every_pending_crash_and_keeps_the_daily_limit() {
    let home = setup("crash-repair-queue");
    for id in 1..=5 {
        event(&home, &id.to_string(), 1500.0 + id as f64);
    }
    poll(&home, 2000.0).unwrap();
    let covers = |home: &Path, i: usize| -> Vec<Value> {
        read(home)["dispatches"][i]["covers"].as_array().unwrap().iter().map(|c| c["incident"].clone()).collect()
    };
    assert_eq!(read(&home)["dispatches"].as_array().unwrap().len(), 1);
    assert_eq!(covers(&home, 0), vec![json!("2"), json!("3"), json!("4"), json!("5")]);
    assert!(root(&home).join("analysis/5/report.ips").exists());
    // An acknowledgement alone is not a verified repair; covered crashes never open their own session.
    write_json(&root(&home).join("incidents/1.ack"), &json!({})).unwrap();
    poll(&home, 2100.0).unwrap();
    complete(&home, "1");
    poll(&home, 2150.0).unwrap();
    assert_eq!(read(&home)["dispatches"].as_array().unwrap().len(), 1);
    for id in 6..=9 {
        event(&home, &id.to_string(), 2150.0 + id as f64);
        poll(&home, 2200.0 + id as f64).unwrap();
        complete(&home, &id.to_string());
    }
    assert_eq!(read(&home)["dispatches"].as_array().unwrap().len(), 4, "four sessions per 24 h");
    event(&home, "10", 2300.0);
    poll(&home, 2400.0).unwrap();
    assert_eq!(read(&home)["dispatches"].as_array().unwrap().len(), 4, "the fifth waits for the window");
    poll(&home, DAY_MS + 3000.0).unwrap();
    assert_eq!(read(&home)["dispatches"].as_array().unwrap().len(), 5);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn lead_ack_needs_every_covered_crash_then_closes_them() {
    let home = setup("crash-repair-batch-gate");
    event(&home, "1", 1500.0);
    event(&home, "2", 1600.0);
    poll(&home, 2000.0).unwrap();
    write_json(&root(&home).join("analysis/1/report.json"), &json!({"status":"resolved","verification":{"passed":true},
        "crashes":[{"incident":"1","family":"f","status":"resolved","cause":"c"}]})).unwrap();
    assert!(batch::validate(&home, "1").unwrap_err().to_string().contains("missing 2"));
    write_json(&root(&home).join("analysis/1/report.json"), &json!({"status":"resolved","verification":{"passed":true},
        "crashes":[{"incident":"1","status":"resolved","cause":"c"},{"incident":"2","status":"resolved","cause":"same family"}]})).unwrap();
    batch::validate(&home, "1").unwrap();
    batch::close_covered(&home, "1").unwrap();
    let ack: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents/2.ack")).unwrap()).unwrap();
    assert_eq!(ack["covered_by"], "1");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn explicit_current_incident_and_evidence_failure() {
    let home = setup("crash-repair-explicit");
    event(&home, "1", 500.0);
    write_json(
        &root(&home).join("crash-repair-policy.json"),
        &json!({"enabled":true,"enabled_at_ms":1000,"include_incidents":["1"]}),
    )
    .unwrap();
    std::fs::remove_file(home.join("Library/Logs/DiagnosticReports/MarsCRM-1.ips")).unwrap();
    poll(&home, 2000.0).unwrap();
    assert!(read(&home)["dispatches"].as_array().unwrap().is_empty());
    event(&home, "1", 500.0);
    poll(&home, 3000.0).unwrap();
    assert_eq!(read(&home)["dispatches"][0]["incident"], "1");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn absent_or_disabled_policy_never_launches_and_known_issues_are_skipped() {
    let home = setup("crash-repair-disabled");
    event(&home, "1", 1500.0);
    std::fs::remove_file(root(&home).join("crash-repair-policy.json")).unwrap();
    poll(&home, 2000.0).unwrap();
    assert!(!root(&home).join("crash-repair.json").exists());
    write_json(
        &root(&home).join("crash-repair-policy.json"),
        &json!({"enabled":false}),
    )
    .unwrap();
    poll(&home, 2000.0).unwrap();
    assert!(!root(&home).join("crash-repair.json").exists());
    write_json(
        &root(&home).join("crash-repair-policy.json"),
        &json!({"enabled":true,"enabled_at_ms":1000}),
    )
    .unwrap();
    let file = root(&home).join("incidents/1.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    value["report"]["known_issue"] = json!("Apple runtime fault");
    write_json(&file, &value).unwrap();
    poll(&home, 2000.0).unwrap();
    assert!(read(&home)["dispatches"].as_array().unwrap().is_empty());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn springboard_and_xcode_crashes_share_one_xcode_session() {
    let home = setup("springboard-repair");
    event(&home, "1", 1500.0);
    let path = root(&home).join("incidents/1.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["trigger"] = json!("springboard_runtime_crash");
    value["report"]["process"] = json!("SpringBoard");
    value["report"]["known_issue"] = json!("springboard_poster_snapshot_rbsassertion");
    value["report"]["modified_at_ms"] = json!(1500);
    write_json(&path, &value).unwrap();
    poll(&home, 2000.0).unwrap();
    assert!(read(&home)["dispatches"].as_array().unwrap().is_empty());
    write_json(&root(&home).join("crash-repair-policy.json"), &json!({
        "enabled":true,"enabled_at_ms":1000,
        "springboard":{"enabled":true,"enabled_at_ms":1000}})).unwrap();
    poll(&home, 2100.0).unwrap();
    assert_eq!(read(&home)["dispatches"][0]["incident"], "1");
    assert!(prompt(&home, "1").contains("route xcode"));
    assert!(root(&home).join("analysis/1/report.ips").exists());
    // An Xcode runtime crash while the SpringBoard session is open joins it.
    event(&home, "2", 2200.0);
    poll(&home, 2300.0).unwrap();
    assert_eq!(read(&home)["dispatches"].as_array().unwrap().len(), 1);
    assert_eq!(read(&home)["dispatches"][0]["covers"][0]["incident"], "2");
    assert_eq!(read(&home)["dispatches"][0]["covers"][0]["trigger"], "xcode_runtime_crash");
    complete(&home, "1");
    poll(&home, 2400.0).unwrap();
    assert_eq!(read(&home)["dispatches"].as_array().unwrap().len(), 1, "a covered crash never opens its own session");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn springboard_requires_complete_opt_in_even_for_an_explicit_report() {
    let report = json!({"process":"SpringBoard","path":"/report.ips","modified_at_ms":1500});
    let mut policy = json!({"enabled":true,"springboard":{"enabled":true,
        "include_reports":["/report.ips"]}});
    assert!(!springboard::eligible(&policy, &report));
    policy["springboard"]["enabled_at_ms"] = json!(1000);
    assert!(springboard::eligible(&policy, &report));
    policy["enabled"] = json!(false);
    assert!(!springboard::eligible(&policy, &report));
}

#[test]
fn duplicate_crash_signatures_do_not_flood_queue_while_incident_is_active() {
    let home = setup("crash-repair-sig-dedup");
    for (id, time) in [("1", 1500.0), ("2", 1600.0), ("3", 1700.0)] {
        event(&home, id, time);
        let path = root(&home).join("incidents").join(format!("{id}.json"));
        let mut val: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        val["report"]["process"] = json!("MarsCRM");
        val["report"]["exception_type"] = json!("EXC_CRASH");
        val["report"]["signal"] = json!("SIGABRT");
        val["report"]["frames"] = json!([
            {"symbol": "__pthread_kill"},
            {"symbol": "AG::precondition_failure(char const*, ...)"},
            {"symbol": "LayoutProxy.dimensions(in:)"}
        ]);
        write_json(&path, &val).unwrap();
    }
    poll(&home, 2000.0).unwrap();
    poll(&home, 2100.0).unwrap();
    let health: Value = serde_json::from_slice(&std::fs::read(root(&home).join("crash-repair-health.json")).unwrap()).unwrap();
    assert_eq!(health["active_incident"], "1");
    assert_eq!(health["queued"], 0);
    std::fs::remove_dir_all(home).unwrap();
}

/// Rewrites an incident as a preview crash on `udid` at `at` whose first meaningful frame is `frame`.
fn preview_crash(home: &Path, id: &str, at: f64, udid: &str, frame: &str) {
    event(home, id, at);
    let path = root(home).join("incidents").join(format!("{id}.json"));
    let mut val: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    val["report"]["process"] = json!("MarsCRM");
    val["report"]["exception_type"] = json!("EXC_CRASH");
    val["report"]["signal"] = json!("SIGABRT");
    val["report"]["simulator_udid"] = json!(udid);
    val["report"]["modified_at_ms"] = json!(at);
    val["report"]["frames"] = json!([{"symbol":"abort"},{"symbol":frame},{"symbol":format!("{id}-variant")}]);
    write_json(&path, &val).unwrap();
}

#[test]
fn crashes_a_repair_reproduces_on_its_device_are_attributed_not_requeued() {
    let home = setup("crash-repair-attribution");
    let ag = "AG::precondition_failure(char const*, ...)";
    preview_crash(&home, "1", 1500.0, "PREVIEW", ag);
    poll(&home, 2000.0).unwrap();
    // While repair 1 is open: its own reproductions (other assertion variants) and unrelated crashes.
    preview_crash(&home, "2", 2100.0, "PREVIEW", ag);
    preview_crash(&home, "3", 2200.0, "OTHER", ag);
    preview_crash(&home, "4", 2300.0, "PREVIEW", "_assertionFailure(_:_:file:line:flags:)");
    poll(&home, 2400.0).unwrap();
    complete(&home, "1");
    write_json(&root(&home).join("incidents/1.ack"), &json!({"acknowledged_at_ms":2500.0})).unwrap();
    // A crash after the repair closed is a possible regression and must still open a repair.
    preview_crash(&home, "5", DAY_MS, "PREVIEW", ag);
    poll(&home, DAY_MS + 100.0).unwrap();
    let ack: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents/2.ack")).unwrap()).unwrap();
    assert_eq!(ack["attributed_to"], "1");
    for id in ["3", "4", "5"] {
        assert!(!root(&home).join(format!("incidents/{id}.ack")).exists(), "{id} must stay open");
    }
    let started: Vec<_> = read(&home)["dispatches"].as_array().unwrap().iter().map(|d| d["incident"].clone()).collect();
    assert_eq!(started, vec![json!("1"), json!("5")]);
    // Unrelated crashes during repair 1 joined its session instead of waiting for their own.
    let covers: Vec<_> = read(&home)["dispatches"][0]["covers"].as_array().unwrap().iter().map(|c| c["incident"].clone()).collect();
    assert_eq!(covers, vec![json!("3"), json!("4")]);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn release_crashes_share_a_family_whatever_release_frame_is_on_top() {
    // Frame tops of incident 814504369575276730715 and its two reproductions; `null` is a JIT frame.
    let tail = json!([{"symbol":null},{"symbol":"<deduplicated_symbol>"},{"symbol":"_VariadicView.Tree.init(_:content:)"},{"symbol":"HStack.init(alignment:spacing:content:)"}]);
    let decrement = "bool swift::RefCounts<swift::RefCountBitsT<(swift::RefCountInlinedness)1>>::doDecrementSlow<(swift::PerformDeinit)1>(swift::RefCountBitsT<(swift::RefCountInlinedness)1>, unsigned int)";
    let tops = [vec![decrement], vec!["_swift_release_dealloc", decrement], vec!["_swift_release_dealloc", decrement]];
    let families: Vec<_> = tops
        .iter()
        .enumerate()
        .map(|(i, top)| {
            let mut frames: Vec<Value> = if i == 2 { vec![json!({"symbol":null})] } else { vec![] };
            frames.extend(top.iter().map(|s| json!({"symbol":s})));
            frames.extend(tail.as_array().unwrap().iter().cloned());
            let report = json!({"process":"MarsCRM","exception_type":"EXC_BAD_ACCESS","signal":"SIGSEGV","frames":frames});
            attribution::family(&report).unwrap()
        })
        .collect();
    assert_eq!(families[0], "MarsCRM|EXC_BAD_ACCESS|SIGSEGV|_VariadicView.Tree.init(_:content:)");
    assert!(families.iter().all(|f| f == &families[0]));
}

fn test_crash(home: &Path, id: &str, detected: f64, modified: f64) {
    let path = home.join("Library/Logs/DiagnosticReports").join(format!("MarsCRM-{id}.ips"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{\"app_name\":\"MarsCRM\"}\n{}").unwrap();
    write_json(&root(home).join("incidents").join(format!("{id}.json")), &json!({"id":id,"trigger":"xcode_runtime_crash",
        "detected_at_ms":detected,"report":{"path":path,"known_issue":null,"process":"MarsCRM","exception_type":"EXC_BREAKPOINT",
        "signal":"SIGTRAP","simulator_udid":"SIM-1","modified_at_ms":modified,"frames":[
            {"symbol":"_assertionFailure(_:_:file:line:flags:)"},{"symbol":"Array._checkSubscript(_:wasNativeTypeChecked:)"},
            {"symbol":"implicit closure #10 in TranscriptRowsTests.testLinkedProseAndCardsBreakAProseRun()"}]}})).unwrap();
}

fn xcresult(home: &Path, device: &str) {
    let bundle = home.join("Library/Developer/Xcode/DerivedData/App-tests.noindex/Logs/Test/Test-App.xcresult");
    std::fs::create_dir_all(&bundle).unwrap();
    write_json(&bundle.join("tests.json"), &json!({"devices":[{"deviceId":device}],"testNodes":[{"nodeType":"Unit test bundle","children":[
        {"nodeType":"Test Case","nodeIdentifier":"TranscriptRowsTests/testLinkedProseAndCardsBreakAProseRun()","children":[
            {"nodeType":"Failure Message","name":"XCTAssertEqual failed: (\"4\") is not equal to (\"6\")"},
            {"nodeType":"Failure Message","name":"Crash: MarsCRM at closure #1 in XCTAssertEqual<A>(_:_:_:file:line:)"}]}]}]})).unwrap();
}

fn wall_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64() * 1000.0
}

#[test]
fn test_frame_names_the_test_method() {
    let report = json!({"frames":[{"symbol":"Array.subscript.getter"},
        {"symbol":"partial apply for implicit closure #10 in TranscriptRowsTests.testLinkedProse()"}]});
    assert_eq!(test_run::test_frame(&report).as_deref(), Some("TranscriptRowsTests.testLinkedProse"));
    assert_eq!(test_run::test_frame(&json!({"frames":[{"symbol":"ReaderView.apply()"}]})), None);
}

#[test]
fn a_crash_its_own_test_run_records_goes_to_that_run_not_a_repair() {
    let home = setup("crash-repair-test-run");
    xcresult(&home, "SIM-1");
    let at = wall_ms();
    write_json(&home.join(".local/state/sim-lease/leases/SIM-1.json"),
        &json!({"owner":"session-e512","since":(at / 1000.0) - 60.0})).unwrap();
    test_crash(&home, "7", 1500.0, at);
    poll(&home, 2000.0).unwrap();
    assert!(read(&home)["dispatches"].as_array().unwrap().is_empty());
    let ack: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents/7.ack")).unwrap()).unwrap();
    assert_eq!(ack["attributed_to"], "test_run");
    assert_eq!(ack["session"], "session-e512");
    assert_eq!(ack["test_case"], "TranscriptRowsTests/testLinkedProseAndCardsBreakAProseRun()");
}

#[test]
fn a_test_crash_without_its_result_waits_then_queues_as_a_repair() {
    let home = setup("crash-repair-test-run-wait");
    xcresult(&home, "OTHER-SIM");
    test_crash(&home, "8", 1500.0, wall_ms());
    poll(&home, 2000.0).unwrap();
    assert!(read(&home)["dispatches"].as_array().unwrap().is_empty());
    let health: Value = serde_json::from_slice(&std::fs::read(root(&home).join("crash-repair-health.json")).unwrap()).unwrap();
    assert_eq!(health["awaiting_test_result"], json!(["8"]));
    poll(&home, 1500.0 + 16.0 * 60_000.0).unwrap();
    assert_eq!(read(&home)["dispatches"][0]["incident"], "8");
    assert!(!root(&home).join("incidents/8.ack").exists());
}

#[test]
fn the_lease_owner_seen_while_waiting_names_the_session_after_the_run_released_it() {
    let home = setup("crash-repair-test-run-owner");
    xcresult(&home, "OTHER-SIM");
    let at = wall_ms();
    let lease = home.join(".local/state/sim-lease/leases/SIM-1.json");
    write_json(&lease, &json!({"owner":"session-runner","since":(at / 1000.0) - 60.0})).unwrap();
    test_crash(&home, "9", 1500.0, at);
    poll(&home, 2000.0).unwrap();
    assert!(!root(&home).join("incidents/9.ack").exists());
    std::fs::remove_file(&lease).unwrap();
    xcresult(&home, "SIM-1");
    poll(&home, 3000.0).unwrap();
    let ack: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents/9.ack")).unwrap()).unwrap();
    assert_eq!(ack["session"], "session-runner");
    assert_eq!(std::fs::read_to_string(root(&home).join("crash-repair-test-runs.json")).unwrap().trim(), "{}");
}

#[test]
fn a_crash_whose_report_is_gone_after_an_hour_is_acknowledged_as_evidence_lost() {
    let home = setup("crash-repair-lost");
    event(&home, "7", 2000.0);
    std::fs::remove_file(home.join("Library/Logs/DiagnosticReports/MarsCRM-7.ips")).unwrap();
    let health = || -> Value { serde_json::from_slice(&std::fs::read(root(&home).join("crash-repair-health.json")).unwrap()).unwrap() };
    let ack = root(&home).join("incidents/7.ack");
    // Inside the hour the miss stays an error: the report may still be written.
    poll(&home, 2000.0 + 60_000.0).unwrap();
    assert_eq!(health()["evidence_errors"][0]["incident"], "7");
    assert!(!ack.exists());
    // After it, the crash is acknowledged, leaves the queue and the health record is clean.
    poll(&home, 2000.0 + 3_600_000.0).unwrap();
    assert_eq!(health()["evidence_errors"], json!([]));
    assert_eq!(health()["evidence_lost"][0]["incident"], "7");
    let record: Value = serde_json::from_slice(&std::fs::read(&ack).unwrap()).unwrap();
    assert_eq!(record["attributed_to"], "evidence_lost");
    assert!(root(&home).join("analysis/7/evidence-lost.json").exists());
    poll(&home, 2000.0 + 3_700_000.0).unwrap();
    assert_eq!((&health()["queued"], &health()["evidence_lost"]), (&json!(0), &json!([])));
    assert!(read(&home)["dispatches"].as_array().unwrap().is_empty());
    std::fs::remove_dir_all(home).unwrap();
}
