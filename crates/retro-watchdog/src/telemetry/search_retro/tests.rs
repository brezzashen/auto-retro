use super::*;
use serde_json::json;

fn event(kind: &str, target: &str, scope: &str, verdict: &str, pattern: &str, session: &str) -> Value {
    json!({"kind": kind, "target": target, "scope": scope, "verdict": verdict, "pattern": pattern,
        "cmd": "rg -n 'x' /a/b/c/d", "agent": "claude", "profile": "team3", "session": session, "ts_ms": 1})
}

fn append(home: &Path, events: &[Value]) {
    let path = log_path(home);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    for e in events {
        text.push_str(&e.to_string());
        text.push('\n');
    }
    std::fs::write(&path, text).unwrap();
}

#[test]
fn score_follows_the_formula() {
    let mut events = vec![event("local_search", "history", "", "allowed", "", "s1"); 3];
    events.push(event("grep", "history", "tree", "blocked", "identifier", "s1"));
    events.extend(vec![event("grep", "code", "tree", "allowed", "phrase", "s2"); 3]);
    events.push(event("grep", "code", "file", "allowed", "identifier", "s2"));
    events.push(event("local_search", "code", "", "allowed", "", "s3"));
    let s = score::score(&events);
    // history_report 3/4, no_tree_grep 4/7, phrase_routing 1/4.
    assert_eq!(s["components"]["history_report"], json!(0.75));
    assert_eq!(s["components"]["no_tree_grep"], json!(0.571));
    assert_eq!(s["components"]["phrase_routing"], json!(0.25));
    assert_eq!(s["score"], json!(58.0));
    assert_eq!(s["grade"], json!("D"));
    assert_eq!(s["sessions_without_local_search"][0]["session"], json!("s2"));
    assert_eq!(s["top_tree_greps"][0]["pattern"], json!("rg -n <pat> <path>"));
}

#[test]
fn empty_window_scores_full() {
    assert_eq!(score::score(&[])["grade"], json!("A"));
}

#[test]
fn threshold_opens_one_incident_and_restarts_the_window() {
    let home = crate::telemetry::test_home("search-retro");
    // The window counts local-search calls only; rg/grep events ride along in the batch.
    append(&home, &vec![event("local_search", "code", "", "allowed", "", "s1"); (THRESHOLD - 1) as usize]);
    append(&home, &vec![event("grep", "code", "tree", "allowed", "identifier", "s1"); 5]);
    let mut watch = SearchRetro::default();
    watch.observe(&home, 1000.0).unwrap();
    assert_eq!(watch.window_count, THRESHOLD - 1);
    assert!(watch.dispatches.is_empty());

    append(&home, &[event("local_search", "report", "", "allowed", "", "s1")]);
    watch.observe(&home, 1000.0 + SAMPLE_SECS).unwrap();
    assert_eq!(watch.dispatches.len(), 1);
    assert_eq!(watch.window_count, 0);
    let id = watch.dispatches[0]["incident"].as_str().unwrap().to_string();
    let batch = std::fs::read_to_string(root(&home).join("search-retro").join(format!("{id}.jsonl"))).unwrap();
    assert_eq!(batch.lines().count() as u64, THRESHOLD + 5);
    let incident: Value = serde_json::from_str(&std::fs::read_to_string(root(&home).join("incidents").join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(incident["trigger"], json!("search_calls_reached"));
    assert_eq!(incident["score"]["counts"]["events"], json!(THRESHOLD + 5));
    assert_eq!(incident["quality"]["goal"]["target"], json!(quality::TARGET));

    // Another full window waits while the incident is open, then dispatches after the ack.
    append(&home, &vec![event("local_search", "code", "", "allowed", "", "s1"); THRESHOLD as usize]);
    watch.observe(&home, 1000.0 + 2.0 * SAMPLE_SECS).unwrap();
    assert_eq!(watch.dispatches.len(), 1);
    std::fs::write(root(&home).join("incidents").join(format!("{id}.ack")), "").unwrap();
    watch.observe(&home, 1000.0 + 3.0 * SAMPLE_SECS).unwrap();
    assert_eq!(watch.dispatches.len(), 2);
}

#[test]
fn a_full_window_is_measured_after_its_last_calls_adoption_window() {
    // Search-retro 814337939526298636027: measured 3 s after the last call, a hit read two minutes
    // later counted unadopted. The window waits SETTLE_MS and keeps only the calls that filled it.
    let home = crate::telemetry::test_home("search-retro-settle");
    let at = |ms: f64, e: Value| { let mut e = e; e["ts_ms"] = json!(ms); e };
    let last_ms = 50_000_000.0;
    append(&home, &vec![at(last_ms, event("local_search", "code", "", "allowed", "", "s1")); THRESHOLD as usize]);
    let mut watch = SearchRetro::default();
    watch.observe(&home, last_ms / 1000.0 + 3.0).unwrap();
    assert!(watch.dispatches.is_empty());
    let cut = watch.offset;
    assert_eq!(watch.cut, Some((cut, last_ms)));

    // Calls logged while the window settles, and a restart of the watch, keep the cut.
    append(&home, &vec![at(last_ms + 60_000.0, event("local_search", "code", "", "allowed", "", "s2")); 3]);
    let mut watch = SearchRetro::default();
    watch.observe(&home, (last_ms + SETTLE_MS) / 1000.0 - 1.0).unwrap();
    assert!(watch.dispatches.is_empty());
    assert_eq!(watch.cut, Some((cut, last_ms)));

    watch.observe(&home, (last_ms + SETTLE_MS) / 1000.0 + SAMPLE_SECS).unwrap();
    assert_eq!(watch.dispatches.len(), 1);
    let id = watch.dispatches[0]["incident"].as_str().unwrap().to_string();
    let batch = std::fs::read_to_string(root(&home).join("search-retro").join(format!("{id}.jsonl"))).unwrap();
    assert_eq!(batch.lines().count() as u64, THRESHOLD);
    assert_eq!((watch.window_start, watch.window_count, watch.cut), (cut, 3, None));
}

#[test]
fn partial_lines_wait_and_a_truncated_log_restarts() {
    let home = crate::telemetry::test_home("search-retro-partial");
    let path = log_path(&home);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{\"kind\":\"local_search\"}\n{\"kind\":\"grep\"}\n{\"kind\":").unwrap();
    let mut watch = SearchRetro::default();
    watch.observe(&home, 1000.0).unwrap();
    assert_eq!(watch.window_count, 1);
    std::fs::write(&path, "{}\n").unwrap();
    watch.observe(&home, 1000.0 + SAMPLE_SECS).unwrap();
    assert_eq!((watch.offset, watch.window_count), (3, 0));
}

/// 2026-11-11: a cleanup removed 16,406 rows; the restart at 0 made the whole log one window.
#[test]
fn a_log_that_lost_rows_restarts_the_window_at_its_end() {
    let home = crate::telemetry::test_home("search-retro-shrunk");
    append(&home, &vec![event("local_search", "code", "", "allowed", "", "s1"); 50]);
    let mut watch = SearchRetro::default();
    watch.observe(&home, 1000.0).unwrap();
    assert_eq!(watch.window_count, 50);
    let kept: String = std::fs::read_to_string(log_path(&home)).unwrap().lines().take(30).map(|line| format!("{line}\n")).collect();
    std::fs::write(log_path(&home), &kept).unwrap();
    watch.observe(&home, 1000.0 + SAMPLE_SECS).unwrap();
    assert_eq!((watch.offset, watch.window_start, watch.window_count), (kept.len() as u64, kept.len() as u64, 0), "the kept rows are not counted again");
    append(&home, &vec![event("local_search", "code", "", "allowed", "", "s2"); 2]);
    watch.observe(&home, 1000.0 + 2.0 * SAMPLE_SECS).unwrap();
    assert_eq!(watch.window_count, 2);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_window_counted_in_search_events_is_recounted_in_local_search_calls() {
    let home = crate::telemetry::test_home("search-retro-unit");
    append(&home, &[event("local_search", "code", "", "allowed", "", "s1"), event("grep", "code", "tree", "allowed", "identifier", "s1")]);
    let len = std::fs::metadata(log_path(&home)).unwrap().len();
    std::fs::create_dir_all(root(&home)).unwrap();
    std::fs::write(root(&home).join("search-retro.json"), json!({"offset": len, "window_start": 0, "window_count": 2, "total": 2}).to_string()).unwrap();
    let mut watch = SearchRetro::default();
    watch.observe(&home, 1000.0).unwrap();
    assert_eq!(watch.window_count, 1);
    let state: Value = serde_json::from_str(&std::fs::read_to_string(root(&home).join("search-retro.json")).unwrap()).unwrap();
    assert_eq!(state["unit"], json!("local_search"));
}

#[test]
fn prompt_requires_history_vector_recall_before_analysis() {
    let quality = json!({"goal": {"answer_rate": 0.99, "speed_rate": 0.9, "adoption_rate": 0.7}});
    let text = prompt(std::path::Path::new("/h"), "81", 1000, &json!({"score":90,"grade":"A","counts":{"local_search":100}}), &quality);
    assert!(text.contains("100 local-search calls") && text.contains("answer rate 99.000%") && text.contains("target 99.999%"), "{text}");
    assert!(text.contains("MANDATORY before analysis or edits") && text.contains("retro-recall 81"));
    assert!(!text.contains('\n'));
}


/// Rescores a saved window: `SEARCH_RETRO_BATCH=<id>.jsonl cargo test -p cpu-watchdog rescore_batch -- --ignored --nocapture`.
#[test]
#[ignore]
fn rescore_batch() {
    let path = std::env::var("SEARCH_RETRO_BATCH").expect("SEARCH_RETRO_BATCH");
    let events: Vec<Value> = std::fs::read_to_string(path).unwrap().lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let s = score::score(&events);
    println!("{}", json!({"score": s["score"], "grade": s["grade"], "components": s["components"], "counts": s["counts"]}));
}

#[test]
fn penalties_stay_with_the_governed_targets() {
    let mut events = vec![event("local_search", "history", "", "allowed", "", "s1")];
    // A code escape is not a history/report miss; unindexed, file-list and scope-less sweeps are not tree sweeps.
    events.push(event("grep", "code", "tree", "escape", "identifier", "s1"));
    events.push(event("grep", "other", "tree", "allowed", "identifier", "s1"));
    events.push(event("grep", "code", "files", "allowed", "identifier", "s1"));
    events.push(json!({"kind": "grep", "target": "code", "verdict": "allowed", "cmd": "rg x", "session": "s1"}));
    events.push(event("grep", "code", "tree", "allowed", "identifier", "s1"));
    let s = score::score(&events);
    assert_eq!(s["components"]["history_report"], json!(1.0));
    assert_eq!(s["components"]["no_tree_grep"], json!(0.5));
    assert_eq!(s["counts"]["escaped_code"], json!(1));
    assert_eq!(s["counts"]["grep_file"], json!(1));
}

#[test]
fn an_escape_without_a_recent_local_search_counts_as_a_tree_sweep() {
    let mut events = vec![event("local_search", "code", "", "allowed", "", "s1")];
    let mut free = event("grep", "code", "tree", "escape", "identifier", "s1");
    free["escape_basis"] = json!("recent_search");
    let mut bare = event("grep", "code", "tree", "escape", "identifier", "s2");
    bare["escape_basis"] = json!("none");
    events.extend([free, bare, event("grep", "code", "tree", "escape", "identifier", "s3")]);
    let s = score::score(&events);
    assert_eq!(s["counts"]["grep_tree_ran"], json!(1));
    assert_eq!(s["counts"]["escaped_code_unverified"], json!(1));
    assert_eq!(s["components"]["no_tree_grep"], json!(0.5));
}

#[test]
fn a_segment_of_a_blocked_command_never_ran() {
    // Search-retro 8486853407474113856, 7473551a…jsonl:451: one Bash command held an "allowed"
    // regex tree sweep and a blocked one; script-guard refused the whole command.
    let mut events = vec![event("local_search", "code", "", "allowed", "", "s1")];
    events.push(event("grep", "code", "tree", "allowed", "regex", "s1"));
    events.push(event("grep", "code", "tree", "blocked", "phrase", "s1"));
    let mut later = event("grep", "code", "tree", "allowed", "regex", "s1");
    later["ts_ms"] = json!(2);
    events.push(later);
    let s = score::score(&events);
    assert_eq!(s["counts"]["grep_tree_ran"], json!(1));
    assert_eq!(s["components"]["no_tree_grep"], json!(0.5));
}

#[test]
fn a_fallback_sweep_ran_and_its_reasons_are_tallied() {
    // script-guard fallback mode (2026-11-05): a refused code sweep runs after a local-search call.
    let mut events = vec![event("local_search", "code", "", "allowed", "", "s1")];
    let mut auto = event("grep", "code", "tree", "fallback", "identifier", "s1");
    auto["fallback_reason"] = json!("after_search");
    let mut named = event("grep", "code", "tree", "fallback", "identifier", "s1");
    named["fallback_reason"] = json!("rank");
    named["ts_ms"] = json!(2);
    events.extend([auto, named]);
    let s = score::score(&events);
    assert_eq!(s["counts"]["grep_tree_ran"], json!(2));
    assert_eq!(s["fallback"]["count"], json!(2));
    assert_eq!(s["fallback"]["reasons"].as_array().map(Vec::len), Some(2));
}

#[test]
fn labelled_synthetic_calls_never_fill_a_window() {
    // Founder decision on search-retro 810495355461229102702: benchmark and probe traffic is left out.
    let real = json!({"kind": "local_search", "session": "a"}).to_string();
    let synthetic = json!({"kind": "local_search", "session": "b", "synthetic": "ab-bench"}).to_string();
    let bytes = format!("{real}\n{synthetic}\n{real}\n");
    assert_eq!(local_calls(bytes.as_bytes()), 2);
}

#[test]
fn replays_never_fill_a_window() {
    // Founder decision on search-retro 813254723049142099088: LOCAL_SEARCH_NO_LOG replays, which the
    // meter leaves out of every rate, do not count toward the window either.
    let real = json!({"kind": "local_search", "session": "a", "cmd": "~/.local/bin/local-search search x"}).to_string();
    let flagged = json!({"kind": "local_search", "session": "b", "replay": true, "cmd": "local-search search x"}).to_string();
    let legacy = json!({"kind": "local_search", "session": "b", "cmd": "LOCAL_SEARCH_NO_LOG=1 ~/.local/bin/local-search query y"}).to_string();
    let bytes = format!("{real}\n{flagged}\n{legacy}\n{real}\n");
    assert_eq!(local_calls(bytes.as_bytes()), 2);
}
