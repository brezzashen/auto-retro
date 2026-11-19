use super::*;

fn probe(raw: Value) -> Probe {
    probes(&json!({"probes": [raw]})).unwrap().remove(0)
}

fn sessions(rows: &[(&str, f64, bool, &str)]) -> Sessions {
    let mut out = Sessions::new();
    for (session, at, result, text) in rows { out.entry(session.to_string()).or_default().push((*at, *result, text.to_string(), format!("{session}.jsonl:{at}"))); }
    out
}

#[test]
fn counts_sessions_and_calls_in_the_window() {
    let p = probe(json!({"id": "R1", "needles": ["simctl"], "pattern": r"xcrun\s+simctl\s+list", "shipped": "1970-01-01T00:00:10Z"}));
    let s = sessions(&[("a", 1_000.0, false, "xcrun simctl list devices"), ("a", 2_000.0, false, "xcrun simctl list"),
        ("b", 3_000.0, false, "ls"), ("c", 20_000.0, false, "xcrun simctl list")]);
    let before = measure(&p, &s, &BTreeSet::new(), 0.0, 10_000.0);
    assert_eq!((before["sessions"].as_u64(), before["sessions_hit"].as_u64(), before["calls"].as_u64()), (Some(2), Some(1), Some(2)));
    assert_eq!(before["calls_per_100_sessions"], 100.0);
    assert_eq!(before["examples"], json!(["a.jsonl:1000", "a.jsonl:2000"]));
    assert_eq!(measure(&p, &s, &BTreeSet::new(), 10_000.0, 30_000.0)["sessions_hit"], 1);
}

#[test]
fn after_counts_only_matches_within_the_next_calls() {
    let p = probe(json!({"id": "R3", "needles": ["commit_owned_paths", "git "], "after": "commit_owned_paths.*--push", "within": 2,
        "pattern": r"git\s+(log|status)", "shipped": "1970-01-01T00:00:10Z"}));
    let s = sessions(&[("a", 1.0, false, "bun commit_owned_paths.ts --push mastra"), ("a", 2.0, true, "pushed"),
        ("a", 3.0, false, "git log -1"), ("a", 4.0, false, "ls"), ("a", 5.0, false, "git status"),
        ("b", 1.0, false, "git status"), ("c", 1.0, false, "bun commit_owned_paths.ts --push mastra"), ("c", 2.0, false, "ls"), ("c", 3.0, false, "ls"), ("c", 4.0, false, "git log")]);
    let m = measure(&p, &s, &BTreeSet::new(), 0.0, 100.0);
    // b never pushed; a's git log is the first call after the push, its git status and c's git log the third.
    assert_eq!((m["sessions"].as_u64(), m["sessions_hit"].as_u64(), m["calls"].as_u64()), (Some(2), Some(1), Some(1)));
}

#[test]
fn repeat_counts_only_a_match_that_follows_another_within_the_window() {
    let p = probe(json!({"id": "R18", "needles": ["session-status"], "repeat": true, "within": 3,
        "pattern": r"session-status\s+[0-9a-f]{6,}", "shipped": "1970-01-01T00:00:10Z"}));
    let s = sessions(&[("a", 1.0, false, "session-status abcdef12"), ("a", 2.0, true, "busy"), ("a", 3.0, false, "ls"),
        ("a", 4.0, false, "session-status abcdef12"), ("a", 5.0, false, "session-status abcdef12"),
        ("b", 1.0, false, "session-status abcdef12"), ("b", 2.0, false, "ls"), ("b", 3.0, false, "ls"), ("b", 4.0, false, "ls"),
        ("b", 5.0, false, "session-status abcdef12")]);
    let m = measure(&p, &s, &BTreeSet::new(), 0.0, 100.0);
    // a re-ran its lookup two calls later and again at once; b's two lookups are four calls apart: checkpoints.
    assert_eq!((m["sessions"].as_u64(), m["sessions_hit"].as_u64(), m["calls"].as_u64()), (Some(2), Some(1), Some(2)));
    assert_eq!(m["examples"], json!(["a.jsonl:4", "a.jsonl:5"]));
}

#[test]
fn results_requires_and_min_calls_narrow_the_probe() {
    let p = probe(json!({"id": "R5", "needles": ["stale"], "on": "result", "pattern": "collector is stale", "requires": "cpu-build-gate",
        "min_calls": 2, "shipped": "1970-01-01T00:00:10Z"}));
    let s = sessions(&[("a", 1.0, false, "cpu-build-gate run -- cargo test"), ("a", 2.0, true, "collector is stale"), ("a", 3.0, true, "collector is stale"),
        ("b", 1.0, false, "cpu-build-gate run -- x"), ("b", 2.0, true, "collector is stale"), ("c", 1.0, true, "collector is stale")]);
    let m = measure(&p, &s, &BTreeSet::new(), 0.0, 100.0);
    assert_eq!((m["sessions"].as_u64(), m["sessions_hit"].as_u64(), m["calls"].as_u64()), (Some(2), Some(1), Some(3)));
}

#[test]
fn suggestions_need_enough_sessions_and_a_clear_change() {
    let v = |sessions: u64, rate: f64| json!({"sessions": sessions, "calls_per_100_sessions": rate});
    assert_eq!(suggest(&v(5, 10.0), &v(50, 1.0)), "insufficient_data");
    assert_eq!(suggest(&v(50, 10.0), &v(50, 6.0)), "effective");
    assert_eq!(suggest(&v(50, 10.0), &v(50, 9.0)), "ineffective");
    assert_eq!(suggest(&v(50, 10.0), &v(50, 14.0)), "regressed");
    assert_eq!(suggest(&v(477, 0.2), &v(406, 0.7)), "effective", "both windows under the noise floor");
    assert_eq!(suggest(&v(265, 3.4), &v(406, 6.9)), "regressed");
}

#[test]
fn ack_needs_a_receipt_verdicts_and_probes_on_findings() {
    let home = crate::telemetry::test_home("retro-followup");
    let incident = root(&home).join("incidents/7.json");
    std::fs::create_dir_all(incident.parent().unwrap()).unwrap();
    std::fs::write(&incident, json!({"trigger": "conversation_count_reached", "requirements": {"followups": 1}}).to_string()).unwrap();
    let analysis = root(&home).join("analysis/7");
    std::fs::create_dir_all(&analysis).unwrap();
    let report = |value: Value| std::fs::write(analysis.join("report.json"), value.to_string()).unwrap();
    report(json!({"findings": []}));
    assert!(validate_ack(&home, "7", &incident).is_err());
    std::fs::write(analysis.join("followups.json"), json!({"probes": [{"probe": "R1"}]}).to_string()).unwrap();
    assert!(validate_ack(&home, "7", &incident).is_err());
    report(json!({"followups": [{"probe": "R1", "verdict": "ineffective", "analysis": "still 30 sessions"}], "findings": []}));
    assert!(validate_ack(&home, "7", &incident).unwrap_err().to_string().contains("next"));
    report(json!({"followups": [{"probe": "R1", "verdict": "ineffective", "analysis": "still 30 sessions", "next": "finding 0"}],
        "findings": [{"kind": "repeated"}]}));
    assert!(validate_ack(&home, "7", &incident).unwrap_err().to_string().contains("no_probe_reason"));
    report(json!({"followups": [{"probe": "R1", "verdict": "ineffective", "analysis": "still 30 sessions", "next": "finding 0"}],
        "findings": [{"kind": "repeated", "no_probe_reason": "one-off migration"}]}));
    validate_ack(&home, "7", &incident).unwrap();
    std::fs::write(&incident, json!({"trigger": "conversation_count_reached"}).to_string()).unwrap();
    std::fs::remove_file(analysis.join("followups.json")).unwrap();
    validate_ack(&home, "7", &incident).unwrap();
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn graduated_probes_need_a_verdict_only_when_they_drift() {
    assert!(required("active", "insufficient_data"));
    assert!(required("escalate", "effective"));
    assert!(!required("graduated", "effective"));
    assert!(!required("graduated", "insufficient_data"));
    assert!(required("graduated", "ineffective"));
    assert!(required("graduated", "regressed"));
}

#[test]
fn retro_sessions_are_recognised_by_their_own_tool_calls() {
    assert!(is_retro_call(false, "jq . ~/.local/state/cpu-watchdog/analysis/1/followups.json"));
    assert!(is_retro_call(false, "cpu-watchdog retro-repeats 1 --show sim-lease"));
    assert!(is_retro_call(false, "cat ~/.codex/skills/watchdog/references/retro-probes.json"));
    assert!(!is_retro_call(true, "cpu-watchdog retro-recall printed this"));
    assert!(!is_retro_call(false, "session-status 0123abcd"));
}

#[test]
fn the_after_window_is_the_latest_72_hours_once_a_change_is_older() {
    let hour = 3_600_000.0;
    let shipped = 1_000.0 * hour;
    // Shipped 10 days ago: after covers only the last 72 h, the same length as before.
    assert_eq!(after_window(shipped, shipped + 240.0 * hour), (shipped + 168.0 * hour, shipped + 240.0 * hour));
    // Shipped 5 h ago: after starts at the change.
    assert_eq!(after_window(shipped, shipped + 5.0 * hour), (shipped, shipped + 5.0 * hour));
}

#[test]
fn retro_sessions_count_only_for_retro_probes() {
    let s = sessions(&[("w", 1.0, false, "cpu-watchdog log-verify 1"), ("w", 2.0, true, "Error: report needs duplicate_analysis as ONE"),
        ("u", 1.0, false, "cpu-watchdog log-verify 2"), ("u", 2.0, true, "log vector evidence verified")]);
    let retro: BTreeSet<String> = ["w".to_string()].into();
    let raw = json!({"id": "R67", "needles": ["log-verify"], "on": "result", "pattern": "report needs duplicate_analysis", "shipped": "1970-01-01T00:00:10Z"});
    let plain = measure(&probe(raw.clone()), &s, &retro, 0.0, 100.0);
    assert_eq!((plain["sessions"].as_u64(), plain["calls"].as_u64()), (Some(1), Some(0)));
    let mut kept = raw; kept["retro"] = json!(true);
    let m = measure(&probe(kept), &s, &retro, 0.0, 100.0);
    assert_eq!((m["sessions"].as_u64(), m["sessions_hit"].as_u64(), m["calls"].as_u64()), (Some(2), Some(1), Some(1)));
}

#[test]
fn within_ms_leaves_out_a_repeat_that_comes_minutes_later() {
    let raw = json!({"id": "R18", "needles": ["session-status"], "repeat": true, "within": 3, "within_ms": 180_000,
        "pattern": r"session-status\s+[0-9a-f]{6,}", "shipped": "1970-01-01T00:00:10Z"});
    // a: a /loop report every 5 minutes, two calls a turn; b: the same lookup re-run 20 s later.
    let s = sessions(&[("a", 0.0, false, "session-status abcdef12"), ("a", 1_000.0, false, "day_render status"),
        ("a", 300_000.0, false, "session-status abcdef12"), ("a", 301_000.0, false, "day_render status"),
        ("b", 0.0, false, "session-status abcdef12"), ("b", 20_000.0, false, "session-status abcdef12")]);
    let m = measure(&probe(raw.clone()), &s, &BTreeSet::new(), 0.0, 1_000_000.0);
    assert_eq!((m["sessions"].as_u64(), m["sessions_hit"].as_u64(), m["calls"].as_u64()), (Some(2), Some(1), Some(1)));
    assert_eq!(m["examples"], json!(["b.jsonl:20000"]));
    let mut calls_only = raw; calls_only.as_object_mut().unwrap().remove("within_ms");
    assert_eq!(measure(&probe(calls_only), &s, &BTreeSet::new(), 0.0, 1_000_000.0)["calls"], 2);
}

#[test]
fn same_match_counts_only_a_repeat_on_the_same_captured_key() {
    let p = probe(json!({"id": "R18", "needles": ["session-status"], "repeat": true, "within": 3, "same_match": true,
        "pattern": r"session-status\s+([0-9a-f]{6})[0-9a-f-]*", "shipped": "1970-01-01T00:00:10Z"}));
    let s = sessions(&[("a", 1.0, false, "session-status abcdef12; git status"), ("a", 2.0, false, "session-status 99887766"),
        ("a", 3.0, false, "session-status 99887766-aa"), ("b", 1.0, false, "session-status abcdef12"), ("b", 2.0, false, "session-status 12345678")]);
    let m = measure(&p, &s, &BTreeSet::new(), 0.0, 100.0);
    // Only a's second lookup of 998877 re-runs the same session; lookups of two different sessions are one-offs.
    assert_eq!((m["sessions"].as_u64(), m["sessions_hit"].as_u64(), m["calls"].as_u64()), (Some(2), Some(1), Some(1)));
    assert_eq!(m["examples"], json!(["a.jsonl:3"]));
}

#[test]
fn printed_counts_only_reads_of_a_range_the_search_just_printed() {
    // R11 (conversation retro 813259229750867721848): a read near a search is redundant only when the
    // search printed that range; the file the agent edits next or another range of it is new.
    let p = probe(json!({"id": "R11", "needles": ["local-search", "sed -n", "file_path"], "after": "local-search", "within": 3, "printed": true,
        "pattern": r#"sed\s+-n|^\{"file_path":"[^"]*"(?:,"(?:offset|limit)":\d+)*\}$"#, "shipped": "1970-01-01T00:00:10Z"}));
    let s = sessions(&[("a", 1.0, false, "local-search query \"followups\" --corpus code"),
        ("a", 2.0, true, "  /h/automation/crates/w/src/f.rs:128-162  [code/block] fn f\n\n==> /h/automation/crates/w/src/f.rs:128-162\n  128\tfn f"),
        ("a", 3.0, false, "sed -n 130,150p crates/w/src/f.rs"), ("a", 4.0, false, r#"{"file_path":"/h/automation/crates/w/src/f.rs","limit":40,"offset":1}"#),
        ("b", 1.0, false, "local-search query \"x\" --corpus code"), ("b", 2.0, true, "  /h/g.rs:1-9  [code/block] fn g"), ("b", 3.0, false, "sed -n 1,9p /h/g.rs")]);
    let m = measure(&p, &s, &BTreeSet::new(), 0.0, 100.0);
    // a re-reads 130-150 of the printed 128-162 (counted) and then lines 1-40 (new); b's hit had no body.
    assert_eq!((m["sessions"].as_u64(), m["sessions_hit"].as_u64(), m["calls"].as_u64()), (Some(2), Some(1), Some(1)));
    assert_eq!(m["examples"], json!(["a.jsonl:3"]));
}

#[test]
fn printed_spans_count_when_a_sibling_call_ran_before_the_search_result() {
    // A search batched with another call gets its result after that call; its bodies still count.
    let p = probe(json!({"id": "R11", "needles": ["local-search", "sed -n"], "after": "local-search", "within": 3, "printed": true,
        "pattern": r"sed\s+-n", "shipped": "1970-01-01T00:00:10Z"}));
    let s = sessions(&[("a", 1.0, false, "local-search query \"x\""), ("a", 2.0, false, "git status"),
        ("a", 3.0, true, "==> /h/automation/crates/w/src/f.rs:10-40"), ("a", 4.0, true, "clean"),
        ("a", 5.0, false, "sed -n 12,30p crates/w/src/f.rs")]);
    assert_eq!(measure(&p, &s, &BTreeSet::new(), 0.0, 100.0)["calls"], 1);
}
