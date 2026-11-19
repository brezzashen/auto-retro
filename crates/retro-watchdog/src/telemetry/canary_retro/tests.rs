use super::*;
use serde_json::json;

fn run(id: &str, target: &str, state: &str, start: &str, end: &str, log: Option<&Path>) -> Value {
    json!({"id": id, "target": target, "queue": "Q2", "purpose": "p", "task": null, "state": state,
        "startedAt": start, "finishedAt": end, "updatedAt": end, "source": "a".repeat(40),
        "creator": {"pid": 0, "orcaTab": "tab"}, "log": log, "failure": if state == "failed" { json!(["canary build FAILURE:"]) } else { json!(null) }})
}

/// A live receipt: rust-build `rust` s, Cloud Build queue `queued` s, ready at +`ready` s, live at +`live` s.
fn receipt(start: &str, rust: f64, queued: f64, ready: &str, live: &str, tags: Value) -> Value {
    let step = |id: &str, s: f64| json!({"id": id, "status": "SUCCESS", "seconds": s});
    json!({"startedAt": start, "outcome": "live", "tags": tags, "built": ["api", "crm"], "rolled": ["api", "crm"],
        "preflight": {"seconds": 290}, "queuedAt": ready, "preparedAt": ready, "health": {"status": "ok"},
        "build": {"status": "SUCCESS", "queuedSeconds": queued, "runSeconds": rust + 200.0,
            "steps": [step("rust-restore", 19.0), step("rust-build", rust), step("rust-save", 60.0)]},
        "rollout": {"liveAt": live, "runSeconds": 84}})
}

#[test]
fn the_last_receipt_line_names_the_receipt_despite_colour_codes() {
    let log = "\u{1b}[2mreceipt /a/first.json\u{1b}[0m\nretry\n\u{1b}[2mreceipt /a/2026-10-28T13-33-46.538Z-f3a.json\u{1b}[0m\n";
    assert_eq!(analyze::receipt_path(log).as_deref(), Some("/a/2026-10-28T13-33-46.538Z-f3a.json"));
    assert_eq!(analyze::receipt_path("no receipt here"), None);
}

#[test]
fn a_slow_run_with_a_missed_rust_cache_is_over_the_hard_limit_by_cache() {
    let r = run("20261028T133331Z-canary-1", "canary", "succeeded", "2026-10-28T13:33:31.000Z", "2026-10-28T13:53:36.000Z", None);
    let mut rec = receipt("2026-10-28T13:33:46.000Z", 606.0, 647.0, "2026-10-28T13:50:22.000Z", "2026-10-28T13:52:49.000Z",
        json!({"api": "new-api", "crm": "same-crm"}));
    rec["build"]["steps"][0]["status"] = json!("FAILURE");
    let row = analyze::run_row(&r, "succeeded", "", &rec, &json!({"api": "old-api", "crm": "same-crm"}));
    assert_eq!(row["seconds"], json!(1205.0));
    assert_eq!(row["verdict"], "over_hard");
    assert_eq!((row["dominant"].as_str(), row["cause"].as_str()), (Some("prepare"), Some("cache")));
    assert_eq!(row["segments"]["start"], json!(15.0));
    assert_eq!(row["segments"]["rollout_wait"], json!(63.0));
    let kinds: Vec<&str> = row["cache"].as_array().unwrap().iter().filter_map(|c| c["kind"].as_str()).collect();
    assert_eq!(kinds, ["rust_cache_miss", "rust_restore", "unchanged_rebuilt", "unchanged_rolled"]);
    assert_eq!(row["cache"][2]["images"], json!(["crm"]));
    assert_eq!(row["queue"][0]["kind"], "build_queue");
}

/// Run 20261031T020321Z-canary-4211: the Rust cache missed, but preflight (1107 s) outlasted the
/// whole Cloud Build (48 s queued + 825 s run), so preflight set the release time.
#[test]
fn a_cache_miss_hidden_behind_a_longer_preflight_is_a_preflight_cause() {
    let r = run("20261031T020321Z-canary-4211", "canary", "succeeded", "2026-10-31T02:03:21.000Z", "2026-10-31T02:25:34.000Z", None);
    let mut rec = receipt("2026-10-31T02:03:31.000Z", 625.0, 48.0, "2026-10-31T02:22:50.000Z", "2026-10-31T02:24:29.000Z", json!({"api": "a"}));
    rec["preflight"]["seconds"] = json!(1107);
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!((row["verdict"].as_str(), row["cause"].as_str()), (Some("over_hard"), Some("preflight")));
    assert_eq!(row["cache"][0]["kind"], "rust_rebuild");
}

/// Run 20261028T140605Z-canary-95641: rust-restore succeeded and every dependency stayed fresh, yet
/// the crm image recompiled nine changed workspace crates for 9 min 39 s (Cloud Build fe1c8b2c).
#[test]
fn a_long_compile_after_a_restored_cache_is_a_build_cause() {
    let r = run("20261028T140605Z-canary-95641", "canary", "succeeded", "2026-10-28T14:06:05.000Z", "2026-10-28T14:24:58.000Z", None);
    let mut rec = receipt("2026-10-28T14:06:17.000Z", 589.0, 53.0, "2026-10-28T14:21:57.000Z", "2026-10-28T14:24:24.000Z", json!({"api": "a"}));
    rec["preflight"]["seconds"] = json!(475);
    rec["rollout"]["runSeconds"] = json!(86);
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!((row["verdict"].as_str(), row["cause"].as_str()), (Some("over_target"), Some("build")));
    assert_eq!(row["cache"][0]["kind"], "rust_rebuild");
    assert_eq!(row["segments"]["rollout_wait"], json!(61.0));
}

/// Run 20261101T073812Z-canary-57304: its candidate check's `git fetch` sat on a wedged fsmonitor
/// daemon for 1158 s before the receipt began, and mars-ops' own fetch for another 951 s before
/// preflight (477 s) started. No queue held it and preflight was not the critical path.
#[test]
fn time_outside_every_working_step_is_a_stall_not_a_queue_or_preflight() {
    let r = run("20261101T073812Z-canary-57304", "canary", "succeeded", "2026-11-01T07:38:12.578Z", "2026-11-01T08:23:16.966Z", None);
    let mut rec = receipt("2026-11-01T07:57:30.728Z", 155.0, 49.0, "2026-11-01T08:21:18.392Z", "2026-11-01T08:23:14.459Z", json!({"api": "a"}));
    rec["preflight"]["seconds"] = json!(477);
    rec["build"]["runSeconds"] = json!(217);
    rec["rollout"]["runSeconds"] = json!(74);
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!((row["verdict"].as_str(), row["dominant"].as_str(), row["cause"].as_str()), (Some("over_hard"), Some("prepare"), Some("other")));
    assert_eq!(row["stall"], json!([{"kind": "start", "s": 1158.0}, {"kind": "prepare", "s": 951.0}]));
    assert!(row["queue"].as_array().unwrap().is_empty(), "{}", row["queue"]);
    let s = analyze::summarize(&[row], &[], &[], 0.0, 1.0);
    assert_eq!(s["stalls"]["by_kind"], json!({"prepare": 1, "start": 1}));
    assert_eq!(s["over_hard_causes"], json!({"other": 1}));
}

/// 20261105T170817Z-canary-58402: its tooling waited 979 s for a tool-prebuild held in render-queue's
/// exclusive memory window before the receipt began; that is a queue wait on the start segment.
#[test]
fn a_start_spent_waiting_for_a_tool_prebuild_is_a_queue_wait() {
    let r = run("20261105T170817Z-canary-58402", "canary", "succeeded", "2026-11-05T17:08:17.283Z", "2026-11-05T17:33:16.000Z", None);
    let rec = receipt("2026-11-05T17:24:36.000Z", 185.0, 20.0, "2026-11-05T17:31:13.000Z", "2026-11-05T17:33:14.000Z", json!({"api": "a"}));
    let log = "[cpu-build-gate] tooling: the mars-ops prebuild is stale (b063d6fff4..12b1d2dfb0 changes x); building the tip first\n\
               [cpu-build-gate] tooling: waiting for the running prebuild of 12b1d2dfb0\n";
    let row = analyze::run_row(&r, "succeeded", log, &rec, &Value::Null);
    assert_eq!((row["verdict"].as_str(), row["dominant"].as_str(), row["cause"].as_str()), (Some("over_hard"), Some("start"), Some("queue")));
    assert_eq!(row["queue"], json!([{"kind": "start_wait", "s": 979.0}]));
    assert!(row["stall"].as_array().unwrap().is_empty(), "{}", row["stall"]);
}

/// 20261104T032430Z-canary-48674 waited 2593 s on pid 40236's prepare of the same commit; the receipt
/// carries only that prepare's last pass (preflight 386 s, Cloud Build 44 + 161 s).
#[test]
fn a_long_wait_on_another_sessions_prepare_is_a_queue_cause() {
    let r = run("20261104T032430Z-canary-48674", "canary", "succeeded", "2026-11-04T03:24:30.811Z", "2026-11-04T04:08:58.467Z", None);
    let mut rec = receipt("2026-11-04T03:25:43.114Z", 53.0, 44.0, "2026-11-04T04:08:55.938Z", "2026-11-04T04:08:57.439Z", json!({"api": "a"}));
    rec["preparedBy"] = json!("another session");
    rec["preflight"]["seconds"] = json!(386);
    rec["build"]["runSeconds"] = json!(161);
    rec["rollout"] = json!({"build": null, "leader": 61817, "liveAt": "2026-11-04T04:08:57.439Z"});
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!((row["verdict"].as_str(), row["cause"].as_str()), (Some("over_hard"), Some("queue")));
    assert_eq!(row["queue"], json!([{"kind": "shared_prepare", "s": 2207.0}]));
    assert!(row["stall"].as_array().unwrap().is_empty(), "{}", row["stall"]);
}

/// A short stall beside a long preflight leaves preflight as the cause.
#[test]
fn a_stall_shorter_than_the_critical_path_keeps_the_working_cause() {
    let r = run("20261031T134345Z-canary-5", "canary", "succeeded", "2026-10-31T13:43:45.000Z", "2026-10-31T14:05:00.000Z", None);
    let mut rec = receipt("2026-10-31T13:45:55.000Z", 30.0, 48.0, "2026-10-31T14:03:00.000Z", "2026-10-31T14:04:40.000Z", json!({"api": "a"}));
    rec["preflight"]["seconds"] = json!(900);
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!(row["cause"], "preflight");
    assert_eq!(row["stall"], json!([{"kind": "start", "s": 130.0}, {"kind": "prepare", "s": 125.0}]));
}

#[test]
fn a_cached_run_in_five_minutes_meets_the_target_and_flags_nothing() {
    let r = run("20261028T130000Z-canary-2", "canary", "succeeded", "2026-10-28T13:00:00.000Z", "2026-10-28T13:04:50.000Z", None);
    let rec = receipt("2026-10-28T13:00:10.000Z", 13.0, 48.0, "2026-10-28T13:03:10.000Z", "2026-10-28T13:04:40.000Z", json!({"api": "x", "crm": "y"}));
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!(row["verdict"], "target");
    assert!(row["errors"].as_array().unwrap().is_empty() && row["cache"].as_array().unwrap().is_empty() && row["queue"].as_array().unwrap().is_empty());
    let s = analyze::summarize(&[row], &[], &[], 0.0, 1.0);
    assert_eq!(s["cache_hit_rate"], json!(100.0));
    assert_eq!(s["verdicts"]["target"], json!(1));
}

/// Runs 20261102T153145Z-canary-50272 and 20261102T153557Z-canary-82834 both reused prepare build
/// e3871aad (rust-build 339 s); 82834 re-released the commit 50272 had just put live, so its
/// receipt names tags already live although it built nothing and has no rollout build.
#[test]
fn a_reused_prepare_rebuilt_and_rolled_nothing_and_its_build_counts_once() {
    let tags = json!({"api": "f16f", "crm": "189b"});
    let first = run("20261102T153145Z-canary-50272", "canary", "succeeded", "2026-11-02T15:31:45.000Z", "2026-11-02T15:35:14.000Z", None);
    let mut a = receipt("2026-11-02T15:32:28.000Z", 339.0, 56.0, "2026-11-02T15:32:37.000Z", "2026-11-02T15:35:12.000Z", tags.clone());
    a["preparedBy"] = json!("another session");
    a["build"]["id"] = json!("e3871aad");
    let second = run("20261102T153557Z-canary-82834", "canary", "succeeded", "2026-11-02T15:35:57.000Z", "2026-11-02T15:37:01.000Z", None);
    let mut b = a.clone();
    b["startedAt"] = json!("2026-11-02T15:36:40.000Z");
    b["rollout"] = json!({"build": null, "leader": 92077, "liveAt": "2026-11-02T15:37:00.000Z"});
    let row_a = analyze::run_row(&first, "succeeded", "", &a, &json!({"api": "old", "crm": "old"}));
    let row_b = analyze::run_row(&second, "succeeded", "", &b, &tags);
    let kinds: Vec<&str> = row_b["cache"].as_array().unwrap().iter().filter_map(|c| c["kind"].as_str()).collect();
    assert_eq!(kinds, ["rust_rebuild"], "{}", row_b["cache"]);
    let s = analyze::summarize(&[row_a, row_b], &[], &[], 0.0, 1.0);
    assert_eq!(s["checks"]["cache"]["by_kind"], json!({"rust_rebuild": 1}));
    assert_eq!(s["checks"]["cache"]["runs"], json!(1));
    assert_eq!(s["cache_hit_rate"], json!(0.0));
}

#[test]
fn a_builder_vm_run_without_restore_or_save_steps_flags_no_cache_step() {
    let r = run("20261031T060000Z-canary-3", "canary", "succeeded", "2026-10-31T06:00:00.000Z", "2026-10-31T06:04:50.000Z", None);
    let mut rec = receipt("2026-10-31T06:00:10.000Z", 40.0, 48.0, "2026-10-31T06:03:10.000Z", "2026-10-31T06:04:40.000Z", json!({"api": "x"}));
    rec["build"]["steps"] = json!([{"id": "rust-build", "status": "SUCCESS", "seconds": 40.0}]);
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert!(row["cache"].as_array().unwrap().is_empty(), "{}", row["cache"]);
}

#[test]
fn a_failed_run_is_an_error_whatever_segment_dominates() {
    let r = run("20261028T132602Z-canary-3", "canary", "failed", "2026-10-28T13:26:02.000Z", "2026-10-28T13:46:36.000Z", None);
    let rec = json!({"startedAt": "2026-10-28T13:26:14.000Z", "outcome": "failed", "failure": ["canary build FAILURE:\noffline-validation: (fail) x"]});
    let row = analyze::run_row(&r, "failed", "", &rec, &Value::Null);
    assert_eq!((row["verdict"].as_str(), row["cause"].as_str()), (Some("over_hard"), Some("error")));
    assert_eq!(row["errors"][0]["class"], "cloud_build");
}

#[test]
fn only_canary_runs_count_and_a_window_opens_at_ten() {
    let home = crate::telemetry::test_home("canary-retro");
    let dir = home.join(".local/state/mars-deliver/releases");
    std::fs::create_dir_all(&dir).unwrap();
    let write = |r: &Value| std::fs::write(dir.join(format!("{}.json", r["id"].as_str().unwrap())), r.to_string()).unwrap();
    write(&run("20261028T000000Z-beta-1", "beta", "succeeded", "2026-10-28T00:00:00.000Z", "2026-10-28T00:05:00.000Z", None));
    let canary: Vec<Value> = (0..THRESHOLD + 1).map(|i| run(&format!("20261028T01{i:04}Z-canary-{i}"), "canary", "succeeded",
        "2026-10-28T01:00:00.000Z", &format!("2026-10-28T01:{:02}:00.000Z", 10 + 2 * i), None)).collect();
    for r in &canary[..THRESHOLD - 1] { write(r); }
    let mut retro = CanaryRetro { loaded: true, ..Default::default() };
    retro.observe(&home, 1.0).unwrap();
    assert!(retro.dispatches.is_empty());
    for r in &canary { write(r); }
    retro.observe(&home, 1.0 + SAMPLE_SECS).unwrap();
    assert_eq!(retro.dispatches.len(), 1);
    let id = retro.dispatches[0]["incident"].as_str().unwrap().to_string();
    let path = root(&home).join(format!("incidents/{id}.json"));
    let incident: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(incident["stats"]["window"]["runs"], json!(THRESHOLD));
    // Eleven runs are waiting: the window is the newest ten and the oldest one is skipped.
    assert_eq!(incident["window"]["skipped"], json!(1));
    assert_eq!(incident["stats"]["runs"][0]["id"], json!(canary[1]["id"]));
    // Runs 1-10 took 12-30 min without a receipt: the hard limit list holds the five past 20 min.
    assert_eq!(incident["requirements"]["canary_review"]["over_hard"].as_array().unwrap().len(), 5);
    retro.observe(&home, 1.0 + 2.0 * SAMPLE_SECS).unwrap();
    assert_eq!(retro.dispatches.len(), 1);

    let analysis = root(&home).join("analysis").join(&id);
    std::fs::create_dir_all(&analysis).unwrap();
    let over: Vec<Value> = incident["requirements"]["canary_review"]["over_hard"].as_array().unwrap().clone();
    let check = json!({"verdict": "ok", "evidence": "receipts"});
    let mut report = json!({"runs": [], "checks": {"errors": check, "cache": check, "queue": check}, "report_url": "https://reports.example.com/x"});
    std::fs::write(analysis.join("report.json"), report.to_string()).unwrap();
    std::fs::write(analysis.join("report.md"), "发布时长 错误 缓存 队列 CI 前移").unwrap();
    assert!(validate_ack(&home, &id, &path).unwrap_err().to_string().contains("has no cause"));
    report["runs"] = json!(over.iter().map(|id| json!({"id": id, "cause": "queue", "evidence": "rollout_wait 600 s"})).collect::<Vec<_>>());
    report["checks"]["queue"] = json!({"verdict": "defect", "evidence": "rollout queue"});
    std::fs::write(analysis.join("report.json"), report.to_string()).unwrap();
    assert!(validate_ack(&home, &id, &path).unwrap_err().to_string().contains("no_change_reason"));
    report["checks"]["queue"]["change"] = json!("commit abc");
    std::fs::write(analysis.join("report.json"), report.to_string()).unwrap();
    // The CI check (founder, 2026-11-02) is required, and a CI change states its release cost.
    assert!(validate_ack(&home, &id, &path).unwrap_err().to_string().contains("checks.ci.verdict"));
    report["checks"]["ci"] = json!({"verdict": "defect", "evidence": "7 test failures found at release", "change": "pre-push runs touched suites"});
    std::fs::write(analysis.join("report.json"), report.to_string()).unwrap();
    assert!(validate_ack(&home, &id, &path).unwrap_err().to_string().contains("release_cost_s"));
    report["checks"]["ci"]["release_cost_s"] = json!(0);
    std::fs::write(analysis.join("report.json"), report.to_string()).unwrap();
    validate_ack(&home, &id, &path).unwrap();
}

#[test]
fn the_target_is_ten_minutes_and_the_hard_limit_twenty() {
    let at = |end: &str| analyze::run_row(&run("r", "canary", "succeeded", "2026-10-28T13:00:00.000Z", end, None), "succeeded", "", &Value::Null, &Value::Null)["verdict"].clone();
    assert_eq!(at("2026-10-28T13:10:00.000Z"), "target");
    assert_eq!(at("2026-10-28T13:20:00.000Z"), "over_target");
    assert_eq!(at("2026-10-28T13:20:01.000Z"), "over_hard");
}

#[test]
fn only_k8s_runs_past_twenty_minutes_count_as_over_the_hard_limit() {
    let now = stats_ms("2026-10-31T02:00:00.000Z");
    let mut live = run("live-beta", "beta", "running", "2026-10-31T01:39:00.000Z", "", None);
    live["finishedAt"] = Value::Null;
    live["creator"]["pid"] = json!(std::process::id());
    let mut young = live.clone();
    young["id"] = json!("young-canary");
    young["target"] = json!("canary");
    young["startedAt"] = json!("2026-10-31T01:41:00.000Z");
    let board = [live, young,
        run("slow-web", "beta-web", "succeeded", "2026-10-31T01:00:00.000Z", "2026-10-31T01:21:40.000Z", None),
        run("ok-canary", "canary", "succeeded", "2026-10-31T01:00:00.000Z", "2026-10-31T01:18:20.000Z", None),
        run("old-canary", "canary", "succeeded", "2026-10-30T01:00:00.000Z", "2026-10-30T01:30:00.000Z", None),
        run("slow-ios", "ios", "succeeded", "2026-10-31T01:00:00.000Z", "2026-10-31T01:40:00.000Z", None)];
    let found: Vec<(String, &str, f64)> = over_hard::over(&board, now, stats_ms("2026-10-31T00:00:00.000Z")).into_iter()
        .map(|(r, state, s)| (r["id"].as_str().unwrap().to_string(), state, s)).collect();
    assert_eq!(found, [("live-beta".to_string(), "running", 1260.0), ("slow-web".to_string(), "finished", 1300.0)]);
}

#[test]
fn a_run_past_the_hard_limit_opens_one_incident_without_waiting_for_ten() {
    let home = crate::telemetry::test_home("over-hard");
    let dir = home.join(".local/state/mars-deliver/releases");
    std::fs::create_dir_all(&dir).unwrap();
    let write = |r: &Value| std::fs::write(dir.join(format!("{}.json", r["id"].as_str().unwrap())), r.to_string()).unwrap();
    write(&run("20261031T010000Z-canary-1", "canary", "succeeded", "2026-10-31T01:00:00.000Z", "2026-10-31T01:25:00.000Z", None));
    let now = stats_ms("2026-10-31T01:30:00.000Z") / 1000.0;
    let mut watch = over_hard::OverHard { loaded: true, ..Default::default() };
    watch.observe(&home, now);
    assert_eq!(watch.dispatches.len(), 1);
    let id = watch.dispatches[0]["incident"].as_str().unwrap().to_string();
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join(format!("incidents/{id}.json"))).unwrap()).unwrap();
    assert_eq!(incident["requirements"]["canary_review"]["over_hard"], json!(["20261031T010000Z-canary-1"]));
    assert_eq!(incident["stats"]["window"]["runs"], json!(1));
    // The same run never opens a second incident; a new one waits while the first is unacknowledged.
    write(&run("20261031T011000Z-beta-2", "beta", "failed", "2026-10-31T01:10:00.000Z", "2026-10-31T01:31:00.000Z", None));
    watch.observe(&home, now + over_hard::SAMPLE_SECS + 60.0);
    assert_eq!(watch.dispatches.len(), 1);
    std::fs::write(root(&home).join(format!("incidents/{id}.ack")), "").unwrap();
    watch.observe(&home, now + 2.0 * over_hard::SAMPLE_SECS + 60.0);
    assert_eq!(watch.dispatches.len(), 2);
    assert_eq!(watch.dispatches[1]["runs"], json!(["20261031T011000Z-beta-2"]));
}

fn stats_ms(text: &str) -> f64 { crate::telemetry::release_retro::stats::ms(Some(text)).unwrap() }

/// Runs 20261101T071259Z-canary-48504 (a test failed after 1364 s) and 20261031T015044Z-canary-726
/// (migration gate at 16 s): only the late code failure is CI work. Run 20261031T134345Z-canary-59149:
/// preflight 722 s against a 47 + 149 s Cloud Build put 526 s on the release path.
#[test]
fn late_code_failures_and_preflight_beyond_the_build_are_ci_work() {
    let failed = |id: &str, end: &str, tail: &str| {
        let mut r = run(id, "canary", "failed", "2026-11-01T07:00:00.000Z", end, None);
        r["failure"] = json!([tail]);
        analyze::run_row(&r, "failed", "", &Value::Null, &Value::Null)
    };
    let test = failed("20261101T071259Z-canary-48504", "2026-11-01T07:22:44.000Z", "(fail) contact merge > keeps notes");
    assert_eq!(test["ci"], json!([{"kind": "late_failure", "class": "test", "group": "code", "s": 1364.0}]));
    assert_eq!(failed("20261031T015044Z-canary-726", "2026-11-01T07:00:16.000Z", "rerun with --migrate=0042")["ci"], json!([]));
    assert_eq!(failed("env", "2026-11-01T07:20:00.000Z", "rollout did not complete")["ci"], json!([]));

    let r = run("20261031T134345Z-canary-59149", "canary", "succeeded", "2026-10-31T13:43:45.000Z", "2026-10-31T13:59:33.000Z", None);
    let mut rec = receipt("2026-10-31T13:43:55.000Z", 15.0, 47.0, "2026-10-31T13:56:20.000Z", "2026-10-31T13:57:44.000Z", json!({"api": "a"}));
    rec["preflight"]["seconds"] = json!(722);
    rec["build"]["runSeconds"] = json!(149);
    let live = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!(live["ci"][0]["kind"], "preflight_on_path");
    assert_eq!(live["ci"][0]["s"], json!(526.0));
    // The default fixture's preflight (290 s) ends inside its Cloud Build (50 + 215 s): within 60 s, no signal.
    let quick = analyze::run_row(&r, "succeeded", "", &receipt("2026-10-31T13:43:55.000Z", 15.0, 50.0, "2026-10-31T13:56:20.000Z", "2026-10-31T13:57:44.000Z", json!({})), &Value::Null);
    assert_eq!(quick["ci"], json!([]));
    // Run 20261102T035537Z-canary-30159 found its prepare done (9 s): its 339 s preflight was off its path.
    let mut done = rec.clone();
    (done["startedAt"], done["queuedAt"]) = (json!("2026-10-31T13:56:11.000Z"), json!("2026-10-31T13:56:20.000Z"));
    assert_eq!(analyze::run_row(&r, "succeeded", "", &done, &Value::Null)["ci"], json!([]));

    let cost = analyze::summarize(&[test, live], &[], &[], 0.0, 1.0);
    assert_eq!(cost["checks"]["ci"]["runs"], json!(2));
    assert_eq!(cost["ci_cost"]["total_s"], json!(1890.0));
    assert_eq!(cost["ci_cost"]["ranked"][0], json!({"key": "late_failure:test", "s": 1364.0}));
}

#[test]
fn the_ci_runner_rows_inside_the_window_give_the_false_positive_rate() {
    let home = crate::telemetry::test_home("crm-ci-rows");
    let dir = home.join(".local/state/crm-ci");
    std::fs::create_dir_all(&dir).unwrap();
    let rows = [json!({"sha": "a", "at_ms": 10.0, "verdict": "red", "failures": ["t"]}), json!({"sha": "b", "at_ms": 20.0, "verdict": "flaky"}),
        json!({"sha": "c", "at_ms": 30.0, "verdict": "green"}), json!({"sha": "d", "at_ms": 99.0, "verdict": "red"})];
    std::fs::write(dir.join("runs.jsonl"), rows.iter().map(|r| r.to_string() + "\n").collect::<String>()).unwrap();
    let out = super::ci::runner(&home, 0.0, 50.0);
    assert_eq!((out["heads"].clone(), out["false_positive_rate"].clone()), (json!(3), json!(50.0)));
    assert_eq!(out["red_heads"], json!([{"sha": "a", "failures": ["t"]}]));
}

/// crm-ci went red on 1757869ced at 09:33:05Z (start) and finished 760 s later; canary run 92284 opened the
/// window at 09:33:09.8Z, so the start time missed the red head the window's failed release shipped.
#[test]
fn a_ci_runner_row_counts_by_when_its_verdict_was_known() {
    let home = crate::telemetry::test_home("crm-ci-finish");
    let dir = home.join(".local/state/crm-ci");
    std::fs::create_dir_all(&dir).unwrap();
    let rows = [json!({"sha": "1757869ced", "at_ms": 1794216785053.0, "seconds": 760.0, "verdict": "red", "failures": ["ROUTE_FILE_DOMAIN"]}),
        json!({"sha": "late", "at_ms": 1794227000000.0, "seconds": 900.0, "verdict": "green"})];
    std::fs::write(dir.join("runs.jsonl"), rows.iter().map(|r| r.to_string() + "\n").collect::<String>()).unwrap();
    let out = super::ci::runner(&home, 1794216789809.0, 1794227636291.0);
    assert_eq!((out["heads"].clone(), out["red"].clone()), (json!(1), json!(1)));
    assert_eq!(out["red_heads"][0]["sha"], "1757869ced");
}

/// Runs 20261103T083429Z-canary-26205 and 20261103T083726Z-canary-67188 released 1e6131faa together:
/// 67188 reused 26205's prepare (build 3060191b, preflight 488 s) and was absorbed into its rollout build
/// a9bd9a74, so the preflight excess (207 s) and the rollout wait (136 s) happened once, not twice.
#[test]
fn an_absorbed_run_counts_the_shared_preflight_and_rollout_wait_once() {
    let tags = json!({"api": "a1", "crm": "c1", "agent": "g1"});
    let shared = |start: &str, ready: &str, live: &str| {
        let mut r = receipt(start, 128.0, 56.0, ready, live, tags.clone());
        r["preflight"]["seconds"] = json!(488);
        r["build"]["id"] = json!("3060191b");
        r["build"]["runSeconds"] = json!(225);
        r["rollout"] = json!({"build": "a9bd9a74", "leader": 36013, "liveAt": live, "runSeconds": 115});
        r
    };
    let lead = run("20261103T083429Z-canary-26205", "canary", "succeeded", "2026-11-03T08:34:29.000Z", "2026-11-03T08:48:17.000Z", None);
    let joined = run("20261103T083726Z-canary-67188", "canary", "succeeded", "2026-11-03T08:37:26.000Z", "2026-11-03T08:48:21.000Z", None);
    let a = shared("2026-11-03T08:35:22.000Z", "2026-11-03T08:44:04.000Z", "2026-11-03T08:48:15.000Z");
    let mut b = shared("2026-11-03T08:38:33.000Z", "2026-11-03T08:44:06.000Z", "2026-11-03T08:48:17.000Z");
    b["preparedBy"] = json!("another session");
    let old = json!({"api": "old", "crm": "old", "agent": "old"});
    let (row_a, row_b) = (analyze::run_row(&lead, "succeeded", "", &a, &old), analyze::run_row(&joined, "succeeded", "", &b, &old));
    assert_eq!(row_b["ci"][0]["kind"], "preflight_on_path", "{}", row_b["ci"]);
    assert_eq!(row_b["queue"][0]["kind"], "rollout_wait", "{}", row_b["queue"]);
    let s = analyze::summarize(&[row_a, row_b], &[], &[], 0.0, 1.0);
    assert_eq!(s["checks"]["ci"]["by_kind"], json!({"preflight_on_path": 1}));
    assert_eq!(s["ci_cost"]["total_s"], json!(207.0));
    assert_eq!(s["checks"]["queue"]["by_kind"], json!({"rollout_wait": 1}));
    assert_eq!(s["checks"]["queue"]["items"][0]["run"], "20261103T083429Z-canary-26205");
}

/// 20261104T045239Z-canary-47222 shared pid 2723's prepare of 9e818e29b: preflight 1697 s wall with 35 s of
/// install and a longest other chain of 258 + 19 s. The rest was the host CPU window (Blender at >= 95 %), so the
/// cause is a queue, not the preflight's checks.
fn preflight_that_waited() -> (Value, Value) {
    let r = run("20261104T045239Z-canary-47222", "canary", "succeeded", "2026-11-04T04:52:39.515Z", "2026-11-04T05:15:31.686Z", None);
    let mut rec = receipt("2026-11-04T04:53:37.902Z", 27.0, 50.0, "2026-11-04T05:12:25.060Z", "2026-11-04T05:15:28.290Z", json!({"api": "a"}));
    rec["preparedBy"] = json!("another session");
    rec["build"]["runSeconds"] = json!(134);
    rec["rollout"]["runSeconds"] = json!(114);
    let step = |label: &str, s: f64| json!({"label": label, "seconds": s, "exitCode": 0});
    rec["preflight"] = json!({"seconds": 1697, "steps": [step("install", 35.0), step("typecheck", 48.0),
        step("build agent CRM fixture", 258.0), step("affected agent tests (84)", 19.0), step("page map", 76.0)]});
    (r, rec)
}

#[test]
fn a_preflight_that_mostly_waited_for_the_host_is_a_queue_cause() {
    let (r, rec) = preflight_that_waited();
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!((row["verdict"].as_str(), row["cause"].as_str()), (Some("over_hard"), Some("queue")));
    assert_eq!(row["queue"], json!([{"kind": "preflight_wait", "s": 1385.0}]));
}

#[test]
fn a_measured_cpu_window_wait_names_the_build_gate_and_a_short_one_keeps_the_preflight_cause() {
    let (r, mut rec) = preflight_that_waited();
    rec["preflight"]["steps"][0]["cpuWaitSeconds"] = json!(1370.0);
    rec["preflight"]["steps"][2]["cpuWaitSeconds"] = json!(4.0);
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!(row["cause"], "queue");
    assert_eq!(row["queue"], json!([{"kind": "preflight_cpu_wait", "s": 1374.0}]));
    rec["preflight"]["steps"][0]["cpuWaitSeconds"] = json!(30.0);
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!(row["cause"], "preflight");
    assert!(row["queue"].as_array().unwrap().is_empty(), "{}", row["queue"]);
}

/// 7da6fb948's prepare (2026-11-04, shared by three runs): install 141 s, then the fixture chain
/// 205 + 127 s, 576 s wall. Only 103 s were outside the steps, and the three runs count them once.
#[test]
fn a_fixture_chain_counts_as_one_path_and_a_shared_preflight_wait_counts_once() {
    let step = |label: &str, s: f64| json!({"label": label, "seconds": s, "exitCode": 0});
    let mut rows = Vec::new();
    for (i, id) in ["20261104T035535Z-canary-58402", "20261104T035904Z-canary-75772"].iter().enumerate() {
        let r = run(id, "canary", "succeeded", "2026-11-04T03:55:35.000Z", "2026-11-04T04:07:03.000Z", None);
        let mut rec = receipt("2026-11-04T03:56:27.726Z", 27.0, 48.0, "2026-11-04T04:05:40.000Z", "2026-11-04T04:06:59.000Z", json!({"api": "a"}));
        if i == 1 { rec["preparedBy"] = json!("another session"); }
        rec["build"]["id"] = json!("56d9d68d");
        rec["preflight"] = json!({"seconds": 576, "steps": [step("install", 141.0), step("build agent CRM fixture", 205.0),
            step("affected agent tests (166)", 127.0), step("route conformance", 98.0)]});
        let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
        assert_eq!(row["queue"], json!([{"kind": "preflight_wait", "s": 103.0}]));
        assert_eq!(row["cause"], "preflight");
        rows.push(row);
    }
    let s = analyze::summarize(&rows, &[], &[], 0.0, 1.0);
    assert_eq!(s["checks"]["queue"]["by_kind"], json!({"preflight_wait": 1}));
}

/// Run 20261111T062302Z-canary-3019 (canary-retro 84095928688001342103): its prepare took 1020 s against
/// 91 s preflight and 43 + 146 s Cloud Build, because `gcloud builds submit` spent 787 s uploading the
/// source on a slow path to storage.googleapis.com. A receipt with `uploadSeconds` names that cause; an
/// older receipt without it keeps reading the seconds as an unexplained prepare stall.
fn slow_upload_run() -> (Value, Value) {
    let r = run("20261111T062302Z-canary-3019", "canary", "succeeded", "2026-11-11T06:23:02.946Z", "2026-11-11T06:43:24.163Z", None);
    let mut rec = receipt("2026-11-11T06:23:34.458Z", 57.0, 43.0, "2026-11-11T06:40:34.163Z", "2026-11-11T06:43:18.163Z", json!({"api": "a"}));
    rec["preflight"]["seconds"] = json!(91);
    rec["build"]["runSeconds"] = json!(146);
    rec["rollout"]["runSeconds"] = json!(113);
    (r, rec)
}

#[test]
fn a_prepare_spent_uploading_the_source_is_an_upload_cause() {
    let (r, mut rec) = slow_upload_run();
    rec["build"]["uploadSeconds"] = json!(787.0);
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!((row["verdict"].as_str(), row["dominant"].as_str(), row["cause"].as_str()), (Some("over_hard"), Some("prepare"), Some("upload")));
    assert_eq!(row["prepare"]["upload_s"], json!(787.0));
    assert_eq!(row["stall"], json!([]));
}

#[test]
fn a_receipt_without_upload_seconds_keeps_the_prepare_stall() {
    let (r, rec) = slow_upload_run();
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!(row["cause"], "other");
    assert_eq!(row["stall"], json!([{"kind": "prepare", "s": 831.0}]));
}

#[test]
fn a_normal_upload_beside_a_long_build_stays_a_build_cause() {
    let (r, mut rec) = slow_upload_run();
    rec["build"]["uploadSeconds"] = json!(250.0);
    rec["build"]["runSeconds"] = json!(683);
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!(row["cause"], "build");
}

/// Run 20261111T061607Z-canary-8776 rode dac03a227's build 98112476 (`preparedBy`): its receipt carries that
/// build, so the leader's 787 s upload is on the rider's path too and names the cause, while the 466 s it
/// waited beyond that build (cancelling its own queued build for a cover still uploading) stays a queue item.
#[test]
fn a_rider_of_a_slow_upload_names_upload_and_keeps_its_extra_wait_as_queue() {
    let r = run("20261111T061607Z-canary-8776", "canary", "succeeded", "2026-11-11T06:16:07.000Z", "2026-11-11T06:43:23.000Z", None);
    let mut rec = receipt("2026-11-11T06:16:33.000Z", 57.0, 43.0, "2026-11-11T06:40:35.000Z", "2026-11-11T06:43:19.000Z", json!({"api": "a"}));
    rec["preparedBy"] = json!("covering commit dac03a227");
    rec["preflight"]["seconds"] = json!(91);
    rec["build"]["runSeconds"] = json!(146);
    rec["build"]["uploadSeconds"] = json!(787.0);
    rec["rollout"]["runSeconds"] = json!(113);
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!(row["cause"], "upload");
    assert_eq!(row["queue"][0], json!({"kind": "shared_prepare", "s": 466.0}));
}
