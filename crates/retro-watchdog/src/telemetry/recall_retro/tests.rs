use super::*;
use serde_json::json;

static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

// Every row asks a different question; identical rows are repeats (`batch.rs`).
fn line(sub: &str, vector: bool, corpus: &str, hits: u64) -> String {
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    json!({"sub": sub, "vector_used": vector, "corpus": corpus, "hits": hits, "embed_ms": 40.0, "search_ms": 60.0, "query": format!("q{n}"), "ts_ms": 1}).to_string()
}

fn append(home: &Path, lines: &[String]) {
    let path = log_path(home);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    for l in lines {
        text.push_str(l);
        text.push('\n');
    }
    std::fs::write(&path, text).unwrap();
}

#[test]
fn only_vector_queries_count_and_the_threshold_opens_one_incident() {
    let home = crate::telemetry::test_home("recall-retro");
    let mut watch = RecallRetro::default();
    watch.observe(&home, 1000.0).unwrap();
    assert_eq!(watch.window_count, 0, "no log yet");

    append(&home, &[line("search", false, "code", 3), line("query", false, "code", 3)]);
    append(&home, &(0..(THRESHOLD - 1) as usize).map(|_| line("query", true, "code", 5)).collect::<Vec<_>>());
    watch.observe(&home, 1000.0 + SAMPLE_SECS).unwrap();
    assert_eq!(watch.window_count, THRESHOLD - 1);
    assert!(watch.dispatches.is_empty());

    append(&home, &[line("query", true, "report", 0)]);
    watch.observe(&home, 1000.0 + 2.0 * SAMPLE_SECS).unwrap();
    assert_eq!(watch.dispatches.len(), 1);
    assert_eq!(watch.window_count, 0);
    let id = watch.dispatches[0]["incident"].as_str().unwrap().to_string();
    let incident: Value = serde_json::from_str(&std::fs::read_to_string(root(&home).join("incidents").join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(incident["trigger"], json!("recall_calls_reached"));
    assert_eq!(incident["score"]["recalls"], json!(THRESHOLD));
    assert_eq!(incident["score"]["zero_hit_pct"], json!(2.0));
    assert_eq!((&incident["window"]["from_ms"], &incident["window"]["to_ms"]), (&json!(1), &json!(1)), "retro-recall needs the window in ms for in_window");
    assert!(root(&home).join("recall-retro").join(format!("{id}.jsonl")).exists());
}

#[test]
fn a_first_start_seeds_the_window_with_the_last_recalls() {
    let home = crate::telemetry::test_home("recall-retro-seed");
    append(&home, &(0..(THRESHOLD + 7) as usize).map(|_| line("query", true, "code", 5)).collect::<Vec<_>>());
    append(&home, &[line("search", false, "code", 1)]);
    let mut watch = RecallRetro::default();
    watch.load(&home).unwrap();
    assert_eq!(watch.window_count, THRESHOLD);
    assert_eq!(watch.offset, std::fs::metadata(log_path(&home)).unwrap().len());
    let lines = watch.window_lines(&home).unwrap();
    assert_eq!(lines.iter().filter(|l| is_recall(l)).count() as u64, THRESHOLD);
}

#[test]
fn a_script_loop_from_one_parent_does_not_open_a_retro() {
    let home = crate::telemetry::test_home("recall-retro-batch");
    let mut watch = RecallRetro::default();
    watch.observe(&home, 1000.0).unwrap();
    let row = |ppid: u64, ts: u64| json!({"sub": "query", "vector_used": true, "corpus": "report", "hits": 30, "ppid": ppid, "ts_ms": ts, "query": format!("q{ppid}x{ts}")}).to_string();
    // One golden-set scorer: 2 * THRESHOLD queries within a minute from one parent.
    append(&home, &(0..2 * THRESHOLD).map(|i| row(4242, 60_000 + i)).collect::<Vec<_>>());
    // Agent tool calls: a fresh parent each, plus rows from a binary that logged no ppid.
    append(&home, &(0..10).map(|i| row(5000 + i, 60_000 + i)).collect::<Vec<_>>());
    append(&home, &(0..5).map(|_| line("query", true, "docs", 4)).collect::<Vec<_>>());
    watch.observe(&home, 1000.0 + SAMPLE_SECS).unwrap();
    assert_eq!(watch.window_count, 2 * THRESHOLD + 15, "the raw count still sees every row");
    assert!(watch.dispatches.is_empty(), "15 agent recalls stay below the threshold");
    let (agent, ex) = score::agent_recalls(&watch.window_lines(&home).unwrap());
    assert_eq!((agent.len(), ex.batch), (15, 2 * THRESHOLD as usize));

    append(&home, &(0..THRESHOLD).map(|i| row(9000 + i, 120_000 + i)).collect::<Vec<_>>());
    watch.observe(&home, 1000.0 + 2.0 * SAMPLE_SECS).unwrap();
    assert_eq!(watch.dispatches.len(), 1, "agent recalls reached the threshold");
    let id = watch.dispatches[0]["incident"].as_str().unwrap().to_string();
    let incident: Value = serde_json::from_str(&std::fs::read_to_string(root(&home).join("incidents").join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(incident["recalls"], json!(THRESHOLD + 15));
    assert_eq!(incident["score"]["batch_excluded"], json!(2 * THRESHOLD));
}

#[test]
fn tool_receipts_and_repeated_benchmark_queries_are_not_agent_recalls() {
    // Incident 83793984228306173904: cpu-watchdog retro-recall/log-recall receipts and a cold/warm
    // latency benchmark (one query 36 times, 2-39 s per cold call) made up most of the window.
    let row = |ppid: u64, ts: u64, q: &str, caller: Option<&str>| {
        let mut r = json!({"sub": "query", "vector_used": true, "corpus": "watchdog", "hits": 20, "ppid": ppid, "ts_ms": ts, "query": q});
        if let Some(c) = caller { r["caller"] = json!(c); }
        r.to_string()
    };
    let q = "ControlCenter Mach port leak growth over limit restart menu bar module";
    let mut lines = vec![row(1, 0, q, None)];
    for k in 1..12u64 {
        lines.push(row(1, k * 10_000, q, None));
        lines.push(row(1, k * 10_000 + 300, &format!("{q} {}", k % 3 + 1), None));
    }
    lines.push(row(2, 5_000, "why did the canary rollout stall", Some("cpu-watchdog retro-recall")));
    lines.push(row(3, 6_000, "ControlCenter restart history", None));
    // The same question again after REPEAT_MS is a new recall.
    lines.push(row(4, 700_000, q, None));
    let (agent, ex) = score::agent_recalls(&lines);
    // Kept: q, "q 1", "q 2", "q 3", the other question, and q again after REPEAT_MS.
    assert_eq!(ex, score::Excluded { batch: 0, tool: 1, repeat: 19, synthetic: 0 });
    assert_eq!(agent.len(), 6);
    let scored = score::score(&lines);
    assert_eq!((scored["tool_excluded"].clone(), scored["repeat_excluded"].clone()), (json!(1), json!(19)));
}

#[test]
fn a_repeat_is_the_same_text_not_a_similar_one() {
    assert_eq!(batch::normalized("  Leak Growth module "), "leak growth module");
    assert_ne!(batch::normalized("agent 0"), batch::normalized("agent 1"));
}

#[test]
fn a_replaced_shorter_log_resumes_after_the_last_counted_line() {
    // Incident 817029181313800309049: the index move copied queries.jsonl before its last lines,
    // and the old `len < offset` rule recounted the whole log as one window.
    let home = crate::telemetry::test_home("recall-retro-replaced");
    let row = |ts: u64| json!({"sub": "query", "vector_used": true, "corpus": "code", "hits": 5, "ppid": 7000 + ts, "ts_ms": ts, "query": format!("r{ts}")}).to_string();
    let mut watch = RecallRetro::default();
    watch.observe(&home, 1000.0).unwrap();
    append(&home, &(1..=40).map(row).collect::<Vec<_>>());
    watch.observe(&home, 1000.0 + SAMPLE_SECS).unwrap();
    assert_eq!(watch.window_count, 40);
    // A copy taken at ts 35, swapped in after ts 40 was counted, then three new lines.
    let path = log_path(&home);
    std::fs::remove_file(&path).unwrap();
    append(&home, &(1..=35).chain(41..=43).map(row).collect::<Vec<_>>());
    watch.observe(&home, 1000.0 + 2.0 * SAMPLE_SECS).unwrap();
    assert_eq!(watch.window_count, 43, "only ts 41-43 are new; history is not recounted");
    assert!(watch.dispatches.is_empty());
    let lines = watch.window_lines(&home).unwrap();
    assert_eq!(lines.len(), 38, "the window still starts at its first line (ts 1) in the new file");
}

#[test]
fn state_without_window_from_ms_dates_the_window_by_its_first_line() {
    // Live check after installing 874e2e8: a pre-cursor state got window_from_ms from the next
    // fresh line (16:48) although window_start pointed at a 16:32 line.
    let home = crate::telemetry::test_home("recall-retro-legacy");
    let row = |ts: u64| json!({"sub": "query", "vector_used": true, "corpus": "code", "hits": 5, "ppid": 8000 + ts, "ts_ms": ts, "query": format!("l{ts}")}).to_string();
    append(&home, &(1..=10).map(row).collect::<Vec<_>>());
    let line = std::fs::metadata(log_path(&home)).unwrap().len() / 10;
    let mut watch = RecallRetro { loaded: true, offset: 10 * line, window_start: 4 * line, window_count: 6, ..Default::default() };
    append(&home, &[row(11)]);
    watch.observe(&home, 1000.0).unwrap();
    assert_eq!((watch.window_from_ms, watch.last_ms, watch.window_count), (5.0, 11.0, 7));
}

#[test]
fn labelled_synthetic_probes_are_not_agent_recalls() {
    // Recall-retro 8701297881706895176: a retro timed one agent query under SEARCH_WATCH_SYNTHETIC.
    let mut probe = json!({"sub": "query", "vector_used": true, "corpus": "code", "hits": 2, "ppid": 1, "ts_ms": 0, "query": "agent resources route people facts"});
    probe["synthetic"] = json!("retro-83879279163991646107");
    let agent = json!({"sub": "query", "vector_used": true, "corpus": "code", "hits": 20, "ppid": 2, "ts_ms": 1_000, "query": "web pricing page monthly plan price"});
    let blank = json!({"sub": "query", "vector_used": true, "corpus": "code", "hits": 20, "ppid": 3, "ts_ms": 2_000, "query": "invite card", "synthetic": " "});
    let lines = [probe.to_string(), agent.to_string(), blank.to_string()];
    let (kept, ex) = score::agent_recalls(&lines);
    assert_eq!(ex, score::Excluded { batch: 0, tool: 0, repeat: 0, synthetic: 1 });
    assert_eq!(kept.len(), 2);
    assert_eq!(score::score(&lines)["synthetic_excluded"], json!(1));
}
