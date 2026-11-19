use super::*;

const RUNTIME: &str = "com.apple.CoreSimulator.SimRuntime.iOS-27-2";
const MINUTE: f64 = 60_000.0;

fn setup(name: &str, guarded_runtimes: Value) -> std::path::PathBuf {
    let home = crate::telemetry::test_home(name);
    write_json(&root(&home).join("crash-repair-policy.json"), &json!({"enabled":true,"enabled_at_ms":1000,
        "springboard":{"enabled":true,"enabled_at_ms":1000,"guarded_runtimes":guarded_runtimes}})).unwrap();
    home
}

fn wall_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64() * 1000.0
}

fn abort(home: &Path, id: &str, at: f64, runtime: &str, label: Value) {
    let path = home.join("Library/Logs/DiagnosticReports").join(format!("SpringBoard-{id}.ips"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{\"app_name\":\"SpringBoard\"}\n{}").unwrap();
    write_json(&root(home).join("incidents").join(format!("{id}.json")), &json!({"id":id,"trigger":"springboard_runtime_crash",
        "detected_at_ms":at,"report":{"path":path,"process":"SpringBoard","known_issue":label,"simulator_runtime":runtime,
        "simulator_udid":"SIM-1","modified_at_ms":at,"exception_type":"EXC_CRASH","signal":"SIGABRT",
        "frames":[{"symbol":"__pthread_kill"},{"symbol":"__abort_message"}]}})).unwrap();
}

fn label() -> Value {
    json!("springboard_poster_snapshot_rbsassertion")
}

fn app_crash(home: &Path, id: &str, at: f64, udid: &str) {
    write_json(&root(home).join("incidents").join(format!("{id}.json")), &json!({"id":id,"trigger":"xcode_runtime_crash",
        "detected_at_ms":at,"report":{"process":"MarsCRM","known_issue":null,"simulator_udid":udid,"modified_at_ms":at}})).unwrap();
    // Already handled elsewhere: it still counts as damage on the simulator.
    write_json(&root(home).join("incidents").join(format!("{id}.ack")), &json!({})).unwrap();
}

fn test_run(home: &Path, name: &str, device: &str, result: &str) {
    let bundle = home.join("Library/Developer/Xcode/DerivedData/App-tests.noindex/Logs/Test").join(format!("{name}.xcresult"));
    std::fs::create_dir_all(&bundle).unwrap();
    write_json(&bundle.join("summary.json"), &json!({"result":result,"devicesAndConfigurations":[{"device":{"deviceId":device}}]})).unwrap();
}

fn state(home: &Path, file: &str) -> Value {
    serde_json::from_slice(&std::fs::read(root(home).join(file)).unwrap()).unwrap()
}

fn sessions(home: &Path) -> Vec<Value> {
    state(home, "crash-repair.json")["dispatches"].as_array().unwrap().clone()
}

#[test]
fn a_guarded_abort_that_hurt_nothing_waits_out_its_window_then_is_acknowledged_and_counted() {
    let home = setup("crash-repair-guarded", json!([RUNTIME]));
    let at = wall_ms();
    abort(&home, "1", at, RUNTIME, label());
    test_run(&home, "passed-here", "SIM-1", "Passed");
    test_run(&home, "failed-elsewhere", "SIM-2", "Failed");
    app_crash(&home, "9", at + MINUTE, "SIM-2");
    poll(&home, at + 5.0 * MINUTE).unwrap();
    assert!(sessions(&home).is_empty(), "no session while the damage window is open");
    assert_eq!(state(&home, "crash-repair-health.json")["known_issue_guarded"]["awaiting"], json!(["1"]));
    assert!(!root(&home).join("incidents/1.ack").exists());
    poll(&home, at + 18.0 * MINUTE).unwrap();
    assert!(sessions(&home).is_empty(), "a guarded abort that hurt nothing opens no session");
    let ack = state(&home, "incidents/1.ack");
    assert_eq!(ack["attributed_to"], "known_issue_guarded");
    assert_eq!(ack["simulator_udid"], "SIM-1");
    let health = state(&home, "crash-repair-health.json");
    assert_eq!(health["known_issue_guarded"], json!({"total":1,"last_24h":1,"awaiting":[]}));
    assert_eq!(health["attributed"][0]["incident"], "1");
    assert_eq!(state(&home, "crash-repair-guarded.json")["recent"][0]["incident"], "1");
    // A second abort a day later adds to the total and has the last 24 hours to itself.
    abort(&home, "2", at + 86_400_000.0, RUNTIME, label());
    poll(&home, at + 86_400_000.0 + 18.0 * MINUTE).unwrap();
    assert_eq!(state(&home, "crash-repair-health.json")["known_issue_guarded"], json!({"total":2,"last_24h":1,"awaiting":[]}));
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn an_app_crash_on_the_simulator_inside_the_window_sends_the_abort_to_a_session() {
    let home = setup("crash-repair-guarded-app-crash", json!([RUNTIME]));
    let at = wall_ms();
    abort(&home, "1", at, RUNTIME, label());
    app_crash(&home, "9", at + 3.0 * MINUTE, "SIM-1");
    poll(&home, at + 5.0 * MINUTE).unwrap();
    assert!(sessions(&home).is_empty());
    poll(&home, at + 18.0 * MINUTE).unwrap();
    assert_eq!(sessions(&home)[0]["incident"], "1");
    assert!(!root(&home).join("incidents/1.ack").exists());
    assert_eq!(state(&home, "crash-repair-health.json")["known_issue_guarded"]["total"], 0);
    assert_eq!(state(&home, "crash-repair-guarded.json")["hurt"]["1"]["app_crash"], "9");
    // Once the session owns it, the judged entry is dropped.
    poll(&home, at + 19.0 * MINUTE).unwrap();
    assert_eq!(state(&home, "crash-repair-guarded.json")["hurt"], json!({}));
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_failed_test_run_on_the_simulator_sends_the_abort_to_a_session() {
    let home = setup("crash-repair-guarded-failed-run", json!([RUNTIME]));
    let at = wall_ms();
    abort(&home, "1", at, RUNTIME, label());
    test_run(&home, "failed-here", "SIM-1", "Failed");
    poll(&home, at + 18.0 * MINUTE).unwrap();
    assert_eq!(sessions(&home)[0]["incident"], "1");
    assert!(state(&home, "crash-repair-guarded.json")["hurt"]["1"]["failed_test_run"].as_str().unwrap().ends_with("failed-here.xcresult"));
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn an_unguarded_runtime_an_unlabelled_abort_or_no_policy_list_queues_at_once() {
    for (name, runtimes, runtime, known) in [
        ("crash-repair-guarded-other-runtime", json!([RUNTIME]), "com.apple.CoreSimulator.SimRuntime.iOS-27-2-iPhone19-4", label()),
        ("crash-repair-guarded-unlabelled", json!([RUNTIME]), RUNTIME, Value::Null),
        ("crash-repair-guarded-no-list", Value::Null, RUNTIME, label()),
    ] {
        let home = setup(name, runtimes);
        let at = wall_ms();
        abort(&home, "1", at, runtime, known);
        poll(&home, at + MINUTE).unwrap();
        assert_eq!(sessions(&home)[0]["incident"], "1", "{name}");
        assert!(!root(&home).join("crash-repair-guarded.json").exists(), "{name}");
        std::fs::remove_dir_all(home).unwrap();
    }
}
