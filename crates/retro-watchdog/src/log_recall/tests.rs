use super::*;

pub(crate) fn capture_ok(dir: &Path) {
    let manifest = dir.join("manifest.json");
    write_json(&manifest, &json!({"roots":["/logs"],"gaps":[]})).unwrap();
    capture(dir, &["repeated timeouts".into()], |args| Ok(match args[0] {
        "watchdog-status" => json!({"pending":0,"chunks":2,"manifest":manifest}).to_string(),
        "query" if args[3] == "perf" => format!("{}\n{}",json!({"corpus":"perf","locator":"/perf/cpu-windows/1.json:1"}),json!({"route":"vector","timing":{}})),
        "query" => format!("{}\n{}",json!({"corpus":"watchdog","locator":"/logs/test.log:3"}),json!({"route":"vector","timing":{}})),
        _ => String::new(),
    })).unwrap();
}
pub(crate) fn report(dir: &Path) {
    write_json(&dir.join("report.json"), &json!({"log_recall_evidence":["/logs/test.log:3"],"duplicate_analysis":"Same timeout signature, but different host; root cause remains unproven."})).unwrap();
    std::fs::write(dir.join("report.md"), "Evidence reviewed").unwrap();
}
#[test]
fn requires_vector_receipt_and_grounded_duplicate_analysis() {
    let dir = crate::telemetry::test_home("log-recall");
    assert!(verify(&dir).is_err());
    capture_ok(&dir); report(&dir); verify(&dir).unwrap();
    write_json(&dir.join("report.json"), &json!({"log_recall_evidence":["/invented:1"],"duplicate_analysis":"guess"})).unwrap();
    assert!(verify(&dir).is_err());
    report(&dir);
    write_json(&dir.join("log-recall-1.json"), &json!({"stdout":"changed"})).unwrap();
    assert!(verify(&dir).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn perf_corpus_hits_count_as_evidence_and_older_receipts_still_verify() {
    let dir = crate::telemetry::test_home("log-recall-perf");
    capture_ok(&dir);
    write_json(&dir.join("report.json"), &json!({"log_recall_evidence":["/logs/test.log:3","/perf/cpu-windows/1.json:1"],"duplicate_analysis":"Two saturated windows share the same top consumer."})).unwrap();
    std::fs::write(dir.join("report.md"), "Evidence reviewed").unwrap();
    verify(&dir).unwrap();
    // A tampered perf output no longer matches its recorded hash.
    write_json(&dir.join("log-recall-perf-1.json"), &json!({"stdout":"changed"})).unwrap();
    assert!(verify(&dir).is_err());
    // A receipt from before the perf corpus (no `perf` block) verifies on watchdog evidence alone.
    let mut receipt = read(&dir.join("log-recall.json")).unwrap();
    receipt.as_object_mut().unwrap().remove("perf");
    write_json(&dir.join("log-recall.json"), &receipt).unwrap();
    report(&dir);
    verify(&dir).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn incomplete_index_keyword_fallback_and_failed_rerun_block_completion() {
    assert!(hits("{\"route\":\"keyword\",\"timing\":{}}").is_err());
    let dir = crate::telemetry::test_home("log-recall-failure");
    capture_ok(&dir); report(&dir);
    assert!(capture(&dir, &["q".into()], |_| anyhow::bail!("model offline")).is_err());
    assert!(verify(&dir).is_err());
    assert!(capture(&dir, &["q".into()], |args| Ok(if args[0]=="watchdog-status" {json!({"pending":1,"chunks":2}).to_string()} else {String::new()})).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn syncs_wait_for_a_concurrent_sync_instead_of_failing_fast() {
    let dir = crate::telemetry::test_home("log-recall-wait");
    let manifest = dir.join("manifest.json");
    write_json(&manifest, &json!({"roots":["/logs"],"gaps":[]})).unwrap();
    let mut syncs: Vec<Vec<String>> = Vec::new();
    capture(&dir, &["q".into()], |args| {
        if args[0] == "sync" { syncs.push(args.iter().map(|a| a.to_string()).collect()); }
        Ok(match args[0] {
            "watchdog-status" => json!({"pending":0,"chunks":2,"manifest":manifest}).to_string(),
            "query" if args[3] == "perf" => json!({"route":"vector","timing":{}}).to_string(),
            "query" => format!("{}\n{}",json!({"corpus":"watchdog","locator":"/logs/test.log:3"}),json!({"route":"vector","timing":{}})),
            _ => String::new(),
        })
    }).unwrap();
    assert_eq!(syncs.len(), 2);
    for args in &syncs {
        let at = args.iter().position(|a| a == "--wait-lock").expect("--wait-lock passed");
        assert!(args[at + 1].parse::<u64>().unwrap() >= 1200, "{args:?}");
    }
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn verify_errors_name_the_file_type_and_foreign_locator() {
    let dir = crate::telemetry::test_home("log-recall-messages");
    capture_ok(&dir);
    write_json(&dir.join("report.json"), &json!({"log_recall_evidence":["/logs/test.log:3"],"duplicate_analysis":{"repetitions":"x"}})).unwrap();
    let err = verify(&dir).unwrap_err().to_string();
    assert!(err.contains("report.md"), "{err}");
    std::fs::write(dir.join("report.md"), "Evidence reviewed").unwrap();
    let err = verify(&dir).unwrap_err().to_string();
    assert!(err.contains("ONE non-empty string") && err.contains("an object"), "{err}");
    write_json(&dir.join("report.json"), &json!({"log_recall_evidence":["/invented:1"],"duplicate_analysis":"ok"})).unwrap();
    assert!(verify(&dir).unwrap_err().to_string().contains("/invented:1"));
    std::fs::remove_dir_all(&dir).unwrap();
}
#[test]
fn verify_cli_prints_usage_and_accepts_a_relative_directory() {
    let home = crate::telemetry::test_home("log-verify-cli");
    verify_cli(&home, vec!["--help".into()]).unwrap();
    let cwd = std::env::current_dir().unwrap();
    let rel = pathdiff(&home, &cwd);
    assert_eq!(directory(&home, &rel).unwrap(), std::path::absolute(&rel).unwrap());
    std::fs::remove_dir_all(&home).unwrap();
}
fn pathdiff(target: &Path, base: &Path) -> String {
    let up = base.components().count() - 1;
    format!("{}{}", "../".repeat(up), target.strip_prefix("/").unwrap().display())
}
#[test]
fn query_markers_are_dropped_and_other_flags_refused() {
    let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(questions(&words(&["--query", "a b", "-q", "c"])).unwrap(), ["a b", "c"]);
    assert_eq!(questions(&words(&["all-days: x", "--mode vector y"])).unwrap(), ["all-days: x", "--mode vector y"]);
    assert!(questions(&words(&["--query"])).is_err());
    assert!(questions(&words(&["--limit", "x"])).unwrap_err().to_string().contains("flag"));
    assert!(questions(&words(&["  "])).is_err());
}
