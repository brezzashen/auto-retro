use super::*;
use serde_json::json;

/// A stored turn of `thread` at `s`: each step is `(start after s, input, cached, credits)` of a main call.
fn row(thread: &str, id: &str, s: f64, steps: &[(f64, f64, f64, f64)], extra: &[Value]) -> Value {
    let mut calls: Vec<Value> = steps.iter().map(|(at, input, cached, credits)|
        json!([s + at, s + at + 900.0, input, cached, 300, "main", 1, "completed", credits, "gemini-test", 0, "mars"])).collect();
    calls.extend(extra.iter().cloned());
    let credits: f64 = calls.iter().filter_map(|c| c[8].as_f64()).sum();
    json!({"thread_id": thread, "u": "dee71265", "at_ms": s + 60_000.0, "turn": id, "s": s, "trig": null, "lvl": "light",
        "txt": s + steps.last().map_or(0.0, |x| x.0) + 100.0, "status": "completed", "ended": s + 60_000.0, "credits": credits,
        "calls": calls, "tools": [], "inv": null})
}

fn keep(ids: &[&str]) -> HashSet<String> { ids.iter().map(|s| s.to_string()).collect() }

#[test]
fn a_warm_thread_turn_is_heavy_and_its_cache_is_intact() {
    // The previous turn of the thread primes the cache; the costly turn reuses it.
    let prior = { let mut r = row("t1", "p", 0.0, &[(500.0, 80_000.0, 70_000.0, 4.0)], &[]); r["prior"] = json!(true); r };
    let costly = row("t1", "c", 120_000.0, &[(500.0, 82_000.0, 79_000.0, 6.0), (5000.0, 84_000.0, 81_500.0, 6.0), (9000.0, 86_000.0, 83_500.0, 6.0),
        (13_000.0, 88_000.0, 85_500.0, 6.0), (17_000.0, 90_000.0, 87_500.0, 6.0), (21_000.0, 92_000.0, 89_500.0, 6.0)], &[]);
    let out = profile::profiles(&[prior, costly], &keep(&["c"]));
    assert_eq!(out.len(), 1);
    let p = &out[0];
    assert_eq!(p["credits"], 36.0);
    assert_eq!(p["cache"]["invalidated"], false);
    assert_eq!(p["cache"]["steps"][0]["scope"], "cross_turn");
    assert_eq!(p["signature"], "heavy:many_steps");
}

#[test]
fn a_cold_restart_mid_turn_marks_the_cache_invalidated() {
    // The second step reads nothing back from a 90K-token prefix it just sent.
    let costly = row("t2", "c", 0.0, &[(500.0, 90_000.0, 85_000.0, 8.0), (4000.0, 92_000.0, 0.0, 18.0)], &[]);
    let out = profile::profiles(&[costly], &keep(&["c"]));
    let p = &out[0];
    assert_eq!(p["cache"]["invalidated"], true);
    assert_eq!(p["cache"]["top_cause"], "provider_miss");
    assert_eq!(p["signals"][0], "cache_invalidated:provider_miss");
    assert_eq!(p["signature"], "suspect:cache_invalidated:provider_miss");
}

#[test]
fn sub_agent_credits_drive_a_heavy_turn() {
    let child = json!([700.0, 9000.0, 40_000, 0, 2000, "child", 1, "completed", 30, "gemini-test", 0, "researcher"]);
    let costly = row("t3", "c", 0.0, &[(500.0, 20_000.0, 0.0, 2.0)], &[child]);
    let p = &profile::profiles(&[costly], &keep(&["c"]))[0].clone();
    assert_eq!(p["driver"], "sub_agents");
    assert_eq!(p["credits_by_group"]["child/researcher/gemini-test"], 30.0);
}

#[test]
fn review_requires_the_costliest_turns_and_every_group() {
    let turns: Vec<Value> = (0..12).map(|i| json!({"key": format!("k{i}"), "credits": 21.0 + i as f64, "thread": "t",
        "signals": [], "cache": {"invalidated": i % 3 == 0}, "signature": if i % 2 == 0 { "heavy:base" } else { "suspect:tool_failures" }})).collect();
    let r = review(&turns);
    assert_eq!(r["turns"].as_array().unwrap().len(), REQUIRED_TURNS);
    assert_eq!(r["turns"][0], "k11");
    assert_eq!(r["groups"].as_array().unwrap().len(), 2);
    assert_eq!(summary(&turns)["cache_invalidated"], 4);
}

#[test]
fn costly_turns_wait_while_a_review_is_open() {
    let home = crate::telemetry::test_home("crm-costly-turn");
    let mut watch = CrmCostlyTurn { loaded: true, cursor_ms: 0.0, ..Default::default() };
    let p = |k: &str| json!({"key": k, "credits": 25.0, "thread": "t", "signals": [], "cache": {"invalidated": false}, "signature": "heavy:base"});
    watch.absorb(&home, 10_000.0, Ok((5_000.0, vec!["a".into()], vec![p("a")]))).unwrap();
    assert_eq!(watch.dispatches.len(), 1);
    assert!(watch.pending.is_empty());
    assert!(watch.seen.contains_key("a"));
    // The open incident holds the next turn back; a repeated id is not queued twice.
    watch.absorb(&home, 20_000.0, Ok((15_000.0, vec!["b".into()], vec![p("b"), p("b")]))).unwrap();
    assert_eq!((watch.dispatches.len(), watch.pending.len()), (1, 1));
    let id = watch.dispatches[0]["incident"].as_str().unwrap().to_string();
    std::fs::write(root(&home).join("incidents").join(format!("{id}.ack")), "{}").unwrap();
    watch.absorb(&home, 30_000.0, Ok((25_000.0, vec![], vec![]))).unwrap();
    assert_eq!((watch.dispatches.len(), watch.pending.len()), (2, 0));
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents").join(format!("{}.json", watch.dispatches[1]["incident"].as_str().unwrap()))).unwrap()).unwrap();
    assert_eq!(incident["requirements"]["crm_costly_review"]["turns"], json!(["b"]));
}

#[test]
fn the_ack_gate_needs_a_verdict_cache_result_and_handoff() {
    let review = json!({"turns": ["k1"], "groups": ["heavy:base"]});
    let good = json!({"report_url": "https://reports.example.com/x",
        "turns": [{"turn": "k1", "verdict": "abnormal", "cache": "invalidated", "cache_cause": "provider_miss", "evidence": "thread t step 2 cached 0 of 92K",
            "reason": "the prefix was sent unchanged", "cause": "best-effort miss", "handoff": "crm-turn-retro"}],
        "groups": [{"signature": "heavy:base", "verdict": "normal", "cache": "intact", "evidence": "hit 96%", "reason": "long answer"}]});
    let md = "判定\n缓存\n原因\n";
    validate::validate_report(&review, &good, md).unwrap();
    let mut bad = good.clone();
    bad["turns"][0]["cache_cause"] = json!("");
    assert!(validate::validate_report(&review, &bad, md).unwrap_err().to_string().contains("cache_cause"));
    let mut bad = good.clone();
    bad["turns"][0]["handoff"] = Value::Null;
    assert!(validate::validate_report(&review, &bad, md).unwrap_err().to_string().contains("handoff"));
    let mut bad = good.clone();
    bad["groups"] = json!([]);
    assert!(validate::validate_report(&review, &bad, md).unwrap_err().to_string().contains("heavy:base"));
    assert!(validate::validate_report(&review, &good, "判定").is_err());
}

#[test]
fn the_costly_query_reads_only_ended_user_turns_over_the_threshold() {
    let sql = fetch::costly_sql(0.0, 1000.0, THRESHOLD_CREDITS, slow::SLOW_FIRST_TEXT_MS);
    assert!(sql.contains("::numeric > 20"));
    assert!(sql.contains("(t->'identity'->>'startedAt')::bigint >= 180000)"), "a slow first text qualifies a cheap turn");
    assert!(sql.contains("'terminal'->>'status' is not null"));
    assert!(sql.contains("is distinct from 'reminder'"));
    assert!(sql.contains("not like '89d9%'"));
    assert!(sql.contains("pg_input_is_valid"));
}

/// A main call `(start after s, first chunk after its start or None, end after its start)`, stored as fetch projects it.
fn main_call(s: f64, at: f64, first: Option<f64>, dur: f64, attempt: u32) -> Value {
    json!([s + at, s + at + dur, 60_000, 50_000, 300, "main", attempt, if first.is_some() { "completed" } else { "cancelled" }, 1, "gemini-4-argon", 0,
        "relationship-analyst", first.map(|f| s + at + f)])
}

fn timed(id: &str, s: f64, calls: Vec<Value>, text_after: Option<f64>, credits: f64) -> Value {
    json!({"thread_id": "t9", "u": "d53a9941", "at_ms": s + 400_000.0, "turn": id, "s": s, "trig": null, "lvl": "medium",
        "txt": text_after.map(|t| s + t), "status": "completed", "ended": s + 400_000.0, "credits": credits, "calls": calls, "tools": [], "inv": null})
}

#[test]
fn a_turn_cut_at_the_first_chunk_deadline_is_slow_on_provider_wait() {
    // 2026-11-17 turn 5a1b60ec: 33 s first chunk, a 120 s silent call, its resend 63 s; first text at 294 s.
    let s = 1_794_881_000_000.0;
    let row = timed("5a1b", s, vec![main_call(s, 1_100.0, Some(33_000.0), 108_600.0, 1), main_call(s, 110_000.0, None, 120_000.0, 1),
        main_call(s, 230_000.0, Some(63_200.0), 85_100.0, 2)], Some(293_700.0), 9.0);
    let p = &profile::profiles(&[row], &keep(&["5a1b"]))[0];
    assert_eq!(p["slow"]["cause"], "provider_wait");
    assert_eq!(p["slow"]["deadline_cuts"], 1);
    assert_eq!(p["slow"]["user"], "d53a9941");
    assert_eq!(p["slow"]["wait_ms"], 293_700.0);
}

#[test]
fn many_quick_rounds_before_the_text_are_serial_rounds_and_a_fast_turn_is_not_slow() {
    let s = 1_000_000.0;
    let rounds: Vec<Value> = (0..5).map(|i| main_call(s, 40_000.0 * i as f64, Some(20_000.0), 38_000.0, 1)).collect();
    let slow = timed("r5", s, rounds, Some(190_000.0), 12.0);
    let fast = timed("f1", s, vec![main_call(s, 500.0, Some(8_000.0), 20_000.0, 1)], Some(9_000.0), 30.0);
    let out = profile::profiles(&[slow, fast], &keep(&["r5", "f1"]));
    let by = |k: &str| out.iter().find(|p| p["key"] == k).unwrap().clone();
    assert_eq!(by("r5")["slow"]["cause"], "serial_rounds");
    assert_eq!(by("r5")["slow"]["main_steps"], 5);
    assert!(by("f1")["slow"].is_null());
    // The slow cheap turn is judged per account, never as a cost group; the costly fast one is judged on cost.
    let r = review(&out);
    assert_eq!(r["turns"], json!(["f1"]));
    assert_eq!(r["slow_accounts"][0]["account"], "d53a9941");
    assert_eq!(r["slow_accounts"][0]["pre_causes"]["serial_rounds"], 1);
    assert_eq!(summary(&out)["slow"], 1);
}

#[test]
fn the_ack_gate_needs_a_cause_for_every_slow_account() {
    let review = json!({"turns": [], "groups": [], "slow_accounts": ["d53a9941"]});
    let good = json!({"report_url": "https://reports.example.com/x", "turns": [], "groups": [],
        "slow": [{"account": "d53a9941", "cause": "serial_rounds", "evidence": "5 Blaze steps 25-60 s each", "reason": "tool chain", "action": "crm-turn-retro"}]});
    let md = "判定\n缓存\n原因\n首字等待\n";
    validate::validate_report(&review, &good, md).unwrap();
    assert!(validate::validate_report(&review, &good, "判定\n缓存\n原因\n").unwrap_err().to_string().contains("首字等待"));
    let mut bad = good.clone();
    bad["slow"][0]["cause"] = json!("slow");
    assert!(validate::validate_report(&review, &bad, md).unwrap_err().to_string().contains("cause"));
    let mut bad = good.clone();
    bad["slow"] = json!([]);
    assert!(validate::validate_report(&review, &bad, md).unwrap_err().to_string().contains("d53a9941"));
}
