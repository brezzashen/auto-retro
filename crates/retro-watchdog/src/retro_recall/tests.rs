use super::*;

const ID: &str = "123456";
const LOCATOR: &str = "/history/session.jsonl:42";

fn raw(route: &str) -> String {
    format!("{}\n{}\n", json!({"corpus":"history","locator":LOCATOR}), json!({"route":route,"timing":{"embed_ms":4}}))
}

fn setup(label: &str) -> std::path::PathBuf {
    let home = crate::telemetry::test_home(label);
    write_json(&root(&home).join(format!("incidents/{ID}.json")), &json!({"trigger":"conversation_count_reached"})).unwrap();
    home
}

fn capture(home: &Path) {
    run_with(home, vec![ID.into(), "Repeated report edits".into()], |args| {
        Ok(if args[0] == "query" { raw("vector") } else { "coverage output".into() })
    }).unwrap();
}

/// A real transcript whose line 42 is the original evidence a finding cites.
fn transcript(home: &Path) -> String {
    let path = home.join("transcripts/session.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let lines: Vec<String> = (1..=42).map(|n| json!({"timestamp": format!("2026-10-28T01:00:{:02}.000Z", n % 60)}).to_string()).collect();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    format!("{}:42", path.display())
}

fn report(home: &Path, locator: &str) {
    let dir = root(home).join(format!("analysis/{ID}"));
    write_json(&dir.join("report.json"), &json!({"incident":ID,"findings":[{
        "recall_evidence":[locator],"evidence":[transcript(home)],"analysis":"The same unchanged report is parsed repeatedly; reuse its validated result."}]})).unwrap();
    std::fs::write(dir.join("report.md"), "Verified analysis").unwrap();
}

#[test]
fn captures_sync_stats_and_forced_vector_query_before_ack() {
    let home = setup("recall-capture");
    let mut calls = Vec::new();
    run_with(&home, vec![ID.into(), "Repeated report edits".into()], |args| {
        calls.push(args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        Ok(if args[0] == "query" { raw("vector") } else { "coverage output".into() })
    }).unwrap();
    assert_eq!(calls[0], ["sync", "--corpus", "history"]);
    assert_eq!(calls[1], ["stats"]);
    assert!(!calls[2].contains(&"--mode".to_string()), "session questions run on the default route");
    report(&home, LOCATOR);
    let dir = root(&home).join(format!("analysis/{ID}"));
    crate::log_recall::tests::capture_ok(&dir);
    let mut value = read(&dir.join("report.json")).unwrap();
    value["log_recall_evidence"] = json!(["/logs/test.log:3"]);
    value["duplicate_analysis"] = json!("Compared repeated failures against original logs.");
    write_json(&dir.join("report.json"), &value).unwrap();
    std::fs::write(dir.join("report.md"), "Verified analysis after both recalls").unwrap();
    crate::monitor::ack(&home, Some(ID.into())).unwrap();
    assert!(root(&home).join(format!("incidents/{ID}.ack")).exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn keyword_empty_and_fabricated_retrieval_are_rejected() {
    assert!(hits(&raw("keyword"), &["vector"]).is_err());
    assert!(hits(&raw("vector"), &["hybrid"]).is_err());
    assert!(hits("{\"route\":\"vector\",\"timing\":{}}", &["vector"]).is_err());
    let home = setup("recall-invalid");
    let incident = root(&home).join(format!("incidents/{ID}.json"));
    assert!(validate_ack(&home, ID, &incident).is_err());
    capture(&home);
    report(&home, "/invented.jsonl:9");
    assert!(validate_ack(&home, ID, &incident).is_err());
    report(&home, LOCATOR);
    let dir = root(&home).join(format!("analysis/{ID}"));
    write_json(&dir.join("history-recall-1.json"), &json!({"stdout":raw("keyword")})).unwrap();
    assert!(validate_ack(&home, ID, &incident).is_err());
    assert!(!incident.with_extension("ack").exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn failed_rerun_invalidates_previous_receipt_and_unrelated_incidents_keep_working() {
    let home = setup("recall-failure");
    capture(&home);
    assert!(run_with(&home, vec![ID.into(), "Repeated waits".into()], |_| anyhow::bail!("embedding service unavailable")).is_err());
    let receipt = read(&root(&home).join(format!("analysis/{ID}/history-recall.json"))).unwrap();
    assert_eq!(receipt["status"], "running");
    assert!(crate::monitor::ack(&home, Some(ID.into())).is_err());
    write_json(&root(&home).join("incidents/77.json"), &json!({"trigger":"host_cpu"})).unwrap();
    assert!(crate::monitor::ack(&home, Some("77".into())).is_err());
    assert!(run_with(&home, vec!["../77".into(), "query".into()], |_| Ok(String::new())).is_err());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn shared_instruction_requires_recall_and_analysis_before_changes() {
    let prompt = instruction(ID);
    assert!(prompt.contains("MANDATORY before analysis or edits"));
    assert!(prompt.contains("retro-recall 123456") && prompt.contains("recall_evidence"));
    assert!(!prompt.contains('\n'));
}

#[test]
fn no_findings_needs_reviewed_hits_and_old_reports_cannot_be_retrofitted() {
    let home = setup("recall-no-findings");
    capture(&home);
    let dir = root(&home).join(format!("analysis/{ID}"));
    let incident = root(&home).join(format!("incidents/{ID}.json"));
    let empty = json!({"incident":ID,"findings":[],"no_findings_reason":"The recalled checks followed changed inputs.","recall_evidence":[LOCATOR]});
    write_json(&dir.join("report.json"), &empty).unwrap();
    std::fs::write(dir.join("report.md"), "No avoidable repetition after source review.").unwrap();
    validate_ack(&home, ID, &incident).unwrap();
    let mut receipt = read(&dir.join("history-recall.json")).unwrap();
    receipt["finished_at_ms"] = json!(crate::unix_now() * 1000.0 + 10000.0);
    write_json(&dir.join("history-recall.json"), &receipt).unwrap();
    assert!(validate_ack(&home, ID, &incident).is_err());
    std::fs::remove_dir_all(home).unwrap();
}

/// Opt-in: synchronizes the real history index, but uses an isolated incident/state directory.
#[test]
#[ignore]
fn live_history_vector_recall() {
    let home = setup("recall-live");
    // A real release-stats incident seeds the hybrid queries from its exact counts.
    if let Some(file) = std::env::var_os("WATCHDOG_RECALL_INCIDENT") {
        std::fs::copy(file, root(&home).join(format!("incidents/{ID}.json"))).unwrap();
    }
    let bin = home.join(".local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let real = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/bin/local-search");
    std::os::unix::fs::symlink(real, bin.join("local-search")).unwrap();
    // The subprocess inherits the real HOME for its index; only watchdog evidence is isolated.
    run(&home, vec![ID.into(), "重复读取技能文档和重复生成报告的原因".into()]).unwrap();
    let dir = root(&home).join(format!("analysis/{ID}"));
    let receipt = read(&dir.join("history-recall.json")).unwrap();
    assert_eq!(receipt["status"], "complete");
    assert!(!receipt["queries"][0]["locators"].as_array().unwrap().is_empty());
    if let Some(out) = std::env::var_os("WATCHDOG_RECALL_RECEIPT_OUT") {
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        for name in ["history-recall.json", "history-recall-1.json"] {
            std::fs::copy(dir.join(name), out.join(name)).unwrap();
        }
    }
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn release_counts_seed_tool_call_scans_and_hits_are_read_back() {
    let home = setup("recall-seeds");
    let evidence = transcript(&home);
    let path = evidence.rsplit_once(':').unwrap().0.to_string();
    let commands = json!({"unclassified": {"n": 1, "examples": [{"command": "W=/tmp/x && cd $W && git sparse-checkout set --no-cone '/*'", "output": "fatal: release lane jammed"}]},
        "classes": {"missing_flag": {"n": 2, "examples": [{"command": "cd ~/m/Mars_CRM && bun run deliver canary --purpose=\"x\" 2>&1 | tail -3", "output": "deliver canary needs --purpose"}, {"command": "ignored second"}]}}});
    let incident = root(&home).join(format!("incidents/{ID}.json"));
    write_json(&incident, &json!({"trigger": "release_count_reached", "stats": {"commands": commands},
        "window": {"from_ms": 1_793_149_220_000.0, "to_ms": 1_793_149_250_000.0}})).unwrap();
    let hit = |route: &str, line: usize| format!("{}\n{}\n", json!({"corpus": "history", "locator": format!("{path}:{line}")}),
        json!({"route": route, "timing": {"embed_ms": 4}}));
    let scan = |line: usize| format!("  {path}:{line}  [Bash] 2026-10-28 bun run deliver canary\n");
    let mut calls = Vec::new();
    run_with(&home, vec![ID.into(), "Release retries".into()], |args| {
        calls.push(args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        Ok(match args { ["tools", ..] => scan(30), ["query", ..] => hit("vector", 5), _ => "ok".into() })
    }).unwrap();
    assert!(calls.iter().all(|c| !c.contains(&"hybrid".to_string())), "seeds no longer query history");
    let scans: Vec<_> = calls.iter().filter(|c| c[0] == "tools").map(|c| c[8..].join(" ")).collect();
    assert!(calls.iter().filter(|c| c[0] == "tools").all(|c| c[1..8] == ["--limit", "20", "--from-ms", "1793149220000", "--to-ms", "1793149250000", "--"]),
        "seed scans cover the incident window");
    assert_eq!(scans, ["git sparse-checkout set --no-cone", "bun run deliver canary"]);
    let receipt = read(&root(&home).join(format!("analysis/{ID}/history-recall.json"))).unwrap();
    assert_eq!(receipt["queries"][1]["seed"], "commands.unclassified");
    assert_eq!(receipt["queries"][1]["mode"], "tools");
    // 2026-10-28T01:00:05Z is before the window, 01:00:30Z inside it.
    assert_eq!(receipt["queries"][0]["read_back"][0]["in_window"], false);
    assert_eq!(receipt["queries"][1]["read_back"][0]["in_window"], true);
    // A cited line that is not in the file is not evidence.
    report(&home, &format!("{path}:30"));
    validate_ack(&home, ID, &incident).unwrap();
    let dir = root(&home).join(format!("analysis/{ID}"));
    let mut value = read(&dir.join("report.json")).unwrap();
    value["findings"][0]["evidence"] = json!([format!("{path}:99")]);
    write_json(&dir.join("report.json"), &value).unwrap();
    assert!(validate_ack(&home, ID, &incident).is_err());
    // A receipt without the seeded scans is stale for these counts.
    let mut receipt = receipt;
    receipt["queries"] = json!([receipt["queries"][0]]);
    write_json(&dir.join("history-recall.json"), &receipt).unwrap();
    report(&home, &format!("{path}:5"));
    assert!(validate_ack(&home, ID, &incident).is_err());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn seed_words_keep_the_program_and_drop_paths_ids_and_plumbing() {
    assert_eq!(exact::command_words("cpu-build-gate run --slot -- xcodebuild test -project M.xcodeproj -destination 'id=8FD8'").as_deref(), Some("\"cpu-build-gate run --slot\""));
    assert_eq!(exact::seed_args("\"cpu-build-gate run --slot\""), ["cpu-build-gate run --slot"], "a phrase is one tools word");
    let split = exact::command_words("cd ~/x/ios && ~/.local/bin/cpu-build-gate run -- xcodebuild -project MarsCRM.xcodeproj").unwrap();
    assert_eq!(exact::seed_args(&split), ["run", "xcodebuild", "marscrm.xcodeproj"], "no adjacent pair: separate words");
    assert_eq!(exact::command_words("U=57AC; cd /x && ls -d $U 2>&1").as_deref(), None);
    assert_eq!(exact::tool_hits("nothing").is_err(), true);
    assert!(exact::tool_hits("NONE: no tool call in 9 transcripts (since 14d) has every word.\n").unwrap().is_empty());
}

#[test]
fn questions_run_on_the_default_route_with_the_incident_days_and_fall_back_to_vector() {
    let home = setup("recall-auto");
    let incident = root(&home).join(format!("incidents/{ID}.json"));
    // 2026-10-28T17:00Z..18:00Z is 2026-10-29 in Singapore.
    write_json(&incident, &json!({"trigger": "conversation_count_reached", "window": {"from_ms": 1_793_206_800_000.0, "to_ms": 1_793_210_400_000.0}})).unwrap();
    let mut calls = Vec::new();
    run_with(&home, vec![ID.into(), "Repeated report edits".into(), "MARS_CUTOVER_TEST_URL".into(), "all-days: same edits in earlier weeks".into()], |args| {
        calls.push(args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let forced = args.contains(&"--mode");
        Ok(match args {
            ["query", q, ..] if q.starts_with("MARS_CUTOVER") && !forced => raw("exact"),
            ["query", q, ..] if q.starts_with("Repeated") => raw("mixed"),
            ["query", ..] => raw("vector"),
            _ => "ok".into(),
        })
    }).unwrap();
    let asked: Vec<&Vec<String>> = calls.iter().filter(|c| c[0] == "query").collect();
    assert_eq!(asked[0][1], "Repeated report edits 2026-10-29 01:00-02:00");
    assert_eq!(asked[1][1], "MARS_CUTOVER_TEST_URL 2026-10-29 01:00-02:00");
    assert!(asked[2].windows(2).any(|a| a == ["--mode", "vector"]), "a keyword-first route is rerun on vectors");
    assert_eq!(asked[3][1], "same edits in earlier weeks");
    let receipt = read(&root(&home).join(format!("analysis/{ID}/history-recall.json"))).unwrap();
    let modes: Vec<&str> = receipt["queries"].as_array().unwrap().iter().map(|q| q["mode"].as_str().unwrap()).collect();
    assert_eq!(modes, ["auto", "vector", "auto"]);
    assert_eq!(receipt["queries"][0]["sent"], "Repeated report edits 2026-10-29 01:00-02:00");
    // A mixed-route capture validates; a keyword-first capture of the same question does not.
    report(&home, LOCATOR);
    validate_ack(&home, ID, &incident).unwrap();
    let dir = root(&home).join(format!("analysis/{ID}"));
    let mut tampered = receipt.clone();
    tampered["queries"][0]["sha256"] = json!(digest(&raw("exact")));
    write_json(&dir.join("history-recall.json"), &tampered).unwrap();
    write_json(&dir.join("history-recall-1.json"), &json!({"stdout": raw("exact")})).unwrap();
    assert!(validate_ack(&home, ID, &incident).is_err());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_receipt_recorded_before_the_default_route_still_validates() {
    let home = setup("recall-legacy");
    capture(&home);
    let dir = root(&home).join(format!("analysis/{ID}"));
    let mut receipt = read(&dir.join("history-recall.json")).unwrap();
    receipt["queries"][0]["mode"] = json!("vector");
    receipt["queries"][0].as_object_mut().unwrap().remove("sent");
    write_json(&dir.join("history-recall.json"), &receipt).unwrap();
    report(&home, LOCATOR);
    validate_ack(&home, ID, &root(&home).join(format!("incidents/{ID}.json"))).unwrap();
    std::fs::remove_dir_all(home).unwrap();
}
