use super::*;

fn crash(home: &Path, name: &str, complete: bool) {
    let directory = home.join("Library/Logs/DiagnosticReports");
    std::fs::create_dir_all(&directory).unwrap();
    let body = json!({"procName":"MarsCRM","pid":12,"incident":name,
        "procPath":"/Users/USER/Library/Developer/CoreSimulator/Devices/11111111-2222-3333-4444-555555555555/data/MarsCRM.app/MarsCRM",
        "usedImages":[{"name":"MarsCRM.debug.dylib","uuid":"test-image"}],
        "exception":{"type":"EXC_BAD_ACCESS","signal":"SIGSEGV"},
        "threads":[{"triggered":true,"frames":[{"symbol":"ComposerTokenField.updateUIView","sourceFile":"ComposerTokenField.swift","sourceLine":44}]}],
        "environment":{"TOKEN":"secret"},"procArgs":["secret"]});
    let text = if complete { format!("{{\"app_name\":\"MarsCRM\"}}\n{body}") } else { "{}\n{".into() };
    std::fs::write(directory.join(name), text).unwrap();
}

#[test]
fn historical_crashes_are_visible_without_replaying_incidents() {
    let home = super::super::test_home("diagnostics-history");
    crash(&home,"MarsCRM-old.ips",true);
    collect(&home).unwrap();
    let state = super::super::read(&root(&home).join("xcode-errors.json")).unwrap();
    assert_eq!(state["reports"][0]["historical_at_activation"],true);
    assert_eq!(state["reports"][0]["frames"][0]["line"],44);
    assert_eq!(state["reports"][0]["simulator_udid"],"11111111-2222-3333-4444-555555555555");
    assert_eq!(state["reports"][0]["image_uuid"],"test-image");
    assert!(!state.to_string().contains("secret"));
    assert!(!root(&home).join("incidents").exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn new_crash_alerts_once_across_restarts_and_acknowledgement() {
    let home = super::super::test_home("diagnostics-new");
    std::fs::create_dir_all(home.join("Library/Logs/DiagnosticReports")).unwrap();
    collect(&home).unwrap();
    let checkpoint = root(&home).join("xcode-errors-cursor.json");
    let mut cursor = super::super::read(&checkpoint).unwrap();
    cursor["started_at_ms"] = json!(1.0);
    write_json(&checkpoint,&cursor).unwrap();
    crash(&home,"MarsCRM-new.ips",true);
    crash(&home,"Unrelated-new.ips",true);
    collect(&home).unwrap();
    let directory = root(&home).join("incidents");
    let path = std::fs::read_dir(&directory).unwrap().next().unwrap().unwrap().path();
    let before = std::fs::read(&path).unwrap();
    write_json(&path.with_extension("ack"),&json!({"done":true})).unwrap();
    let mut cursor = super::super::read(&checkpoint).unwrap();
    cursor["files"] = json!({});
    write_json(&checkpoint,&cursor).unwrap();
    collect(&home).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(),before);
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(),2);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn incomplete_report_is_retried_and_marks_probe_unhealthy() {
    let home = super::super::test_home("diagnostics-partial");
    crash(&home,"MarsCRM-partial.ips",false);
    collect(&home).unwrap();
    let state = super::super::read(&root(&home).join("xcode-errors.json")).unwrap();
    assert_eq!(state["probe_errors"].as_array().unwrap().len(),1);
    crash(&home,"MarsCRM-partial.ips",true);
    collect(&home).unwrap();
    let state = super::super::read(&root(&home).join("xcode-errors.json")).unwrap();
    assert_eq!(state["reports"].as_array().unwrap().len(),1);
    assert_eq!(state["probe_errors"],json!([]));
    std::fs::remove_dir_all(home).unwrap();
}

fn springboard(home: &Path, name: &str, poster: bool) {
    let directory = home.join("Library/Logs/DiagnosticReports");
    std::fs::create_dir_all(&directory).unwrap();
    let udid = "7588F756-F687-4398-B128-57ECE8EBF854";
    let device = home.join("Library/Developer/CoreSimulator/Devices").join(udid);
    std::fs::create_dir_all(&device).unwrap();
    std::fs::write(device.join("device.plist"), "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
        <key>runtime</key><string>com.apple.CoreSimulator.SimRuntime.iOS-27-0</string></dict></plist>\n").unwrap();
    let snapshot = if poster { "-[FBScene(PosterUIFoundation) pui_executeSnapshotForDescriptor:outputDestination:candidateBundlesFutureProvider:]" } else { "-[SBAlertItem present]" };
    let body = json!({"procName":"SpringBoard","pid":4465,"incident":name,"parentProc":"launchd_sim",
        "procPath":"/Volumes/VOLUME/*/SpringBoard.app/SpringBoard",
        "coalitionName":format!("com.apple.CoreSimulator.SimDevice.{udid}"),
        "exception":{"type":"EXC_CRASH","signal":"SIGABRT"},
        "lastExceptionBacktrace":[{"symbol":"objc_exception_throw"},{"symbol":"-[RBSAssertion dealloc]"},{"symbol":snapshot}],
        "threads":[{"triggered":true,"frames":[{"symbol":"__pthread_kill"}]}]});
    std::fs::write(directory.join(name), format!("{{\"app_name\":\"SpringBoard\"}}\n{body}")).unwrap();
}

#[test]
fn simulator_springboard_poster_abort_is_indexed_without_incident() {
    let home = super::super::test_home("diagnostics-springboard");
    std::fs::create_dir_all(home.join("Library/Logs/DiagnosticReports")).unwrap();
    collect(&home).unwrap();
    let checkpoint = root(&home).join("xcode-errors-cursor.json");
    let mut cursor = super::super::read(&checkpoint).unwrap();
    cursor["started_at_ms"] = json!(1.0);
    write_json(&checkpoint,&cursor).unwrap();
    springboard(&home,"SpringBoard-known.ips",true);
    collect(&home).unwrap();
    let state = super::super::read(&root(&home).join("xcode-errors.json")).unwrap();
    assert_eq!(state["reports"][0]["process"],"SpringBoard");
    assert_eq!(state["reports"][0]["simulator_udid"],"7588F756-F687-4398-B128-57ECE8EBF854");
    assert_eq!(state["reports"][0]["simulator_runtime"],"com.apple.CoreSimulator.SimRuntime.iOS-27-0");
    assert_eq!(state["reports"][0]["known_issue"],POSTER_SNAPSHOT_ABORT);
    assert!(!root(&home).join("incidents").exists());
    springboard(&home,"SpringBoard-other.ips",false);
    collect(&home).unwrap();
    let state = super::super::read(&root(&home).join("xcode-errors.json")).unwrap();
    assert_eq!(state["reports"].as_array().unwrap().len(),2);
    assert_eq!(std::fs::read_dir(root(&home).join("incidents")).unwrap().count(),1);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn springboard_opt_in_replays_only_explicit_history_and_keeps_exception_evidence() {
    let home = super::super::test_home("springboard-opt-in");
    springboard(&home, "SpringBoard-old.ips", true);
    collect(&home).unwrap();
    let path = root(&home).join("crash-repair-policy.json");
    let since = crate::unix_now() * 1000.0 + 60_000.0;
    let mut policy = json!({"enabled":true,"enabled_at_ms":1,
        "springboard":{"enabled":true,"enabled_at_ms":since}});
    write_json(&path, &policy).unwrap();
    collect(&home).unwrap();
    assert!(!root(&home).join("incidents").exists());
    let report = home.join("Library/Logs/DiagnosticReports/SpringBoard-old.ips");
    policy["springboard"]["include_reports"] = json!([report]);
    write_json(&path, &policy).unwrap();
    collect(&home).unwrap();
    collect(&home).unwrap();
    let directory = root(&home).join("incidents");
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    let incident = std::fs::read_dir(directory).unwrap().next().unwrap().unwrap().path();
    let event = super::super::read(&incident).unwrap();
    assert_eq!(event["trigger"], "springboard_runtime_crash");
    assert_eq!(event["report"]["exception_frames"][1]["symbol"], "-[RBSAssertion dealloc]");
    assert_eq!(event["report"]["known_issue"], POSTER_SNAPSHOT_ABORT);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn poster_classification_does_not_hide_other_processes_or_signals() {
    let mut body = json!({"procName":"MarsCRM","parentProc":"launchd_sim",
        "exception":{"type":"EXC_CRASH","signal":"SIGABRT"},
        "lastExceptionBacktrace":[{"symbol":"-[RBSAssertion dealloc]"},{"symbol":"pui_executeSnapshotForDescriptor"}]});
    assert!(known_issue(&body).is_none());
    body["procName"] = json!("SpringBoard");
    assert_eq!(known_issue(&body), Some(POSTER_SNAPSHOT_ABORT));
    body["exception"]["signal"] = json!("SIGSEGV");
    assert!(known_issue(&body).is_none());
}

#[test]
fn fresh_springboard_fault_flows_from_report_to_private_evidence_and_single_dispatch() {
    let home = super::super::test_home("springboard-pipeline");
    let since = crate::unix_now() * 1000.0 - 1000.0;
    write_json(&root(&home).join("crash-repair-policy.json"), &json!({
        "enabled":true,"enabled_at_ms":since,
        "springboard":{"enabled":true,"enabled_at_ms":since}})).unwrap();
    springboard(&home, "SpringBoard-fresh.ips", true);
    collect(&home).unwrap();
    crate::crash_repair::poll(&home, crate::unix_now() * 1000.0).unwrap();
    collect(&home).unwrap();
    crate::crash_repair::poll(&home, crate::unix_now() * 1000.0).unwrap();
    let state = super::super::read(&root(&home).join("crash-repair.json")).unwrap();
    assert_eq!(state["dispatches"].as_array().unwrap().len(), 1);
    assert_eq!(state["dispatches"][0]["agent"]["mode"], "test");
    let id = state["dispatches"][0]["incident"].as_str().unwrap();
    let directory = root(&home).join("analysis").join(id);
    assert_eq!(std::fs::read(directory.join("report.ips")).unwrap(),
        std::fs::read(home.join("Library/Logs/DiagnosticReports/SpringBoard-fresh.ips")).unwrap());
    let incident = super::super::read(&directory.join("incident.json")).unwrap();
    assert_eq!(incident["trigger"], "springboard_runtime_crash");
    assert_eq!(incident["report"]["known_issue"], POSTER_SNAPSHOT_ABORT);
    assert!(directory.join("evidence.json").exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn xcode_previews_simulator_runtime_and_crash_dialog_heal_are_recorded() {
    let home = super::super::test_home("diagnostics-previews-and-dialog");
    let udid = "99940AF8-1F3C-4ED5-AE98-1B4DEB1BE7E6";
    let device = home.join("Library/Developer/Xcode/UserData/Previews/Simulator Devices").join(udid);
    std::fs::create_dir_all(&device).unwrap();
    std::fs::write(device.join("device.plist"), "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
        <key>runtime</key><string>com.apple.CoreSimulator.SimRuntime.iOS-27-1</string></dict></plist>\n").unwrap();
    let directory = home.join("Library/Logs/DiagnosticReports");
    std::fs::create_dir_all(&directory).unwrap();
    let body = json!({"procName":"MarsCRM","pid":50278,"incident":"preview-crash","parentProc":"launchd_sim",
        "procPath":format!("/Users/USER/Library/Developer/Xcode/UserData/Previews/Simulator Devices/{udid}/data/Containers/Bundle/Application/A9/MarsCRM.app/MarsCRM"),
        "usedImages":[{"name":"MarsCRM","uuid":"b656441b"},{"name":"__preview.dylib","uuid":"4252e399"}],
        "exception":{"type":"EXC_CRASH","signal":"SIGABRT"},
        "threads":[{"triggered":true,"frames":[{"symbol":"AG::precondition_failure(char const*, ...)"}]}]});
    std::fs::write(directory.join("MarsCRM-preview.ips"), format!("{{\"app_name\":\"MarsCRM\"}}\n{body}")).unwrap();
    collect(&home).unwrap();
    let state = super::super::read(&root(&home).join("xcode-errors.json")).unwrap();
    assert_eq!(state["reports"][0]["simulator_udid"], udid);
    assert_eq!(state["reports"][0]["simulator_runtime"], "com.apple.CoreSimulator.SimRuntime.iOS-27-1");
    assert_eq!(state["reports"][0]["preview_injection"], true);

    let mut dialog = super::super::crash_dialog::CrashDialog::default();
    let procs = vec![crate::procs::Proc {
        pid: 49295, ppid: 1, uid: 501, start: 1000, cpu_secs: 0.3, rss_kib: 68000,
        name: "Problem Reporter".into(), args: String::new(),
    }];
    dialog.observe(&home, 2000.0, &procs).unwrap();
    let heal = super::super::read(&root(&home).join("crash-dialog-heal.json")).unwrap();
    assert_eq!(heal["dismissed_total"], 1);
    assert_eq!(heal["last_dismissed"]["pid"], 49295);
    std::fs::remove_dir_all(home).unwrap();
}
