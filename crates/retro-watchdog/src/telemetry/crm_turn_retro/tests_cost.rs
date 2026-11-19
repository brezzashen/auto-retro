use super::*;
use serde_json::json;

/// 2026-11-12T00:00:00Z.
const DAY0: f64 = 1_794_441_600_000.0;

/// One provider attempt in the 12-column shape of `fetch::projection`.
fn call(start: f64, input: f64, cached: f64, model: &str) -> Value {
    json!([start, start + 1000.0, input, cached, 100, "main", 1, "completed", 5, model, 0, "relationship-analyst"])
}

fn turn(thread: &str, id: &str, at: f64, calls: Vec<Value>) -> Value {
    json!({"thread_id": thread, "u": "dee71265", "at_ms": at + 5000.0, "turn": id, "s": at, "trig": null, "lvl": "light",
        "txt": at + 4000.0, "status": "completed", "ended": at + 5000.0, "credits": 5, "calls": calls, "tools": [], "inv": null})
}

fn window_rows() -> Vec<Value> {
    let mut prior = turn("a", "a0", DAY0, vec![call(DAY0, 50_000.0, 40_000.0, "gemini-3.8-flash")]);
    prior["prior"] = json!(true);
    let mut prior_c = turn("c", "c0", DAY0, vec![call(DAY0, 50_000.0, 40_000.0, "gemini-3.8-flash")]);
    prior_c["prior"] = json!(true);
    let t1 = DAY0 + 60_000.0;
    let t2 = DAY0 + 15.0 * 60_000.0;
    vec![prior, prior_c,
        // Previous request 50K, 59 s gap, nothing cached: the provider missed.
        turn("a", "a1", t1, vec![call(t1, 52_000.0, 0.0, "gemini-3.8-flash"), call(t1 + 10_000.0, 60_000.0, 47_000.0, "gemini-3.8-flash")]),
        // 14 minutes after the last request: the entry expired.
        turn("a", "a2", t2, vec![call(t2, 62_000.0, 36_000.0, "gemini-3.8-flash")]),
        // A thread's first request reused nothing, not even the shared static floor.
        turn("b", "b1", t1, vec![call(t1, 40_000.0, 0.0, "gemini-4-argon")]),
        // The model changed since the previous turn: a new cache key.
        turn("c", "c1", t1, vec![call(t1, 52_000.0, 0.0, "gemini-4-argon")]),
        // Flash-Lite bills cached input at the full rate: its miss costs nothing.
        turn("d", "d1", t1, vec![call(t1, 40_000.0, 0.0, "gemini-3.5-flash-lite")]),
    ]
}

#[test]
fn every_main_step_gets_the_analyze_cache_ladder_cause() {
    let stats = analyze::summarize_rows(&window_rows());
    let cache = &stats["cache"];
    let calls = |cause: &str| cache["causes"][cause]["calls"].as_u64().unwrap_or(0);
    assert_eq!((calls("provider_miss"), calls("idle_expiry"), calls("cold_static_prefix"), calls("config_switch"), calls("ok")), (1, 1, 2, 1, 1));
    assert_eq!(cache["causes"]["provider_miss"]["loss_tokens"], json!(45_500.0));
    assert_eq!(cache["causes"]["idle_expiry"]["loss_tokens"], json!(19_500.0));
    // 14 minutes after the previous request ended at t1 + 11 s; each cause is also split by model.
    assert_eq!(cache["causes"]["idle_expiry"]["gap_min"]["p50"], json!(13.8));
    assert_eq!(cache["causes"]["provider_miss"]["by_model"]["gemini-3.8-flash"]["calls"], 1);
    assert_eq!(cache["by_scope"]["thread_start"]["calls"], 2);
    assert_eq!(cache["lookback_turns"], 2);
    // Every uncached token is either lost by the cache or new content.
    let main = &cache["main"];
    let uncached = main["input"].as_f64().unwrap() - main["cached"].as_f64().unwrap();
    assert_eq!(main["loss_tokens"].as_f64().unwrap() + main["fresh_tokens"].as_f64().unwrap(), uncached);
    assert_eq!(cache["below_target"], true);
    assert_eq!(cache["by_model"]["gemini-3.5-flash-lite"]["loss_usd"], json!(0.0));
    // 45 500 lost Flash tokens at (0.75 - 0.075) per 1M.
    assert_eq!(cache["causes"]["provider_miss"]["loss_usd"], json!(0.0307));
    // 45 500 Argon tokens lost on a model switch outweigh 36 000 cold Argon tokens plus a free Flash-Lite miss.
    assert_eq!(cache["ranked"][0]["cause"], "config_switch");
}

#[test]
fn a_provider_count_above_the_prompt_is_clamped_and_counted() {
    let rows = [turn("e", "e1", DAY0, vec![call(DAY0, 40_000.0, 48_000.0, "gemini-3.8-flash")])];
    let cache = &analyze::summarize_rows(&rows)["cache"];
    assert_eq!((cache["hit"].as_f64(), cache["cached_over_input_calls"].as_u64()), (Some(1.0), Some(1)));
}

#[test]
fn the_review_requires_cache_causes_only_under_target_and_always_the_top_cost_lines() {
    let stats = analyze::summarize_rows(&window_rows());
    let review = review(&stats, vec![json!("error_retry")], &serde_json::Map::new());
    assert_eq!(review["classes"], json!(["error_retry"]));
    // Every cause holding 5% or more of the lost tokens is required, not only the top three.
    assert_eq!(review["cache"]["causes"], json!(["config_switch", "cold_static_prefix", "provider_miss", "idle_expiry"]));
    assert_eq!(review["cost"]["lines"].as_array().unwrap().len(), 3);
    let warm = [turn("f", "f1", DAY0, vec![call(DAY0, 40_000.0, 39_000.0, "gemini-3.8-flash")])];
    assert_eq!(review_of(&warm)["cache"]["causes"], json!([]));
}

fn review_of(rows: &[Value]) -> Value { review(&analyze::summarize_rows(rows), Vec::new(), &serde_json::Map::new()) }

#[test]
fn the_cost_block_splits_list_usd_by_component_and_keeps_settled_credits_apart() {
    let mut argon = call(DAY0, 1_000_000.0, 500_000.0, "gemini-4-argon");
    argon[4] = json!(10_000); argon[10] = json!(5_000); argon[8] = json!(365);
    let title = json!([DAY0 + 10.0, DAY0 + 20.0, 137, 0, 5, "auxiliary", 1, "completed", 8, "gemini-3.5-flash-lite", null, "thread-title"]);
    let mystery = json!([DAY0 + 30.0, DAY0 + 40.0, 100, 0, 5, "child", 1, "completed", 2, "mystery-1", null, "researcher"]);
    let cost = &analyze::summarize_rows(&[turn("g", "g1", DAY0, vec![argon, title, mystery])])["cost"];
    // 500K uncached x $2 + 500K cached x $0.1 + 10K output x $10 + 5K reasoning x $10, per 1M.
    assert_eq!(cost["components"]["uncached_input"]["usd"], json!(1.0));
    assert_eq!(cost["components"]["cached_input"]["usd"], json!(0.05));
    assert_eq!(cost["components"]["reasoning"]["usd"], json!(0.05));
    assert_eq!(cost["groups"][0]["group"], "main/relationship-analyst/gemini-4-argon");
    assert_eq!(cost["groups"][0]["credits_from_tokens"], json!(360.0));
    // A one-credit minimum on a tiny call; the rest of its settlement is not token cost.
    assert_eq!((cost["credits"]["settled_calls"].as_f64(), cost["credits"]["from_tokens"].as_f64(), cost["credits"]["residual"].as_f64()), (Some(375.0), Some(361.0), Some(12.0)));
    assert_eq!(cost["unpriced_models"]["mystery-1"], 1);
    assert_eq!(cost["upcoming_prices"][0], json!({"model": "gemini-4-argon", "from": "2026-12-01", "usd_now": 1.2, "usd_then": 2.4}));
    assert_eq!(cost["lines"][0]["line"], "main/relationship-analyst/gemini-4-argon:uncached_input");
}

#[test]
fn old_saved_rows_without_model_columns_still_parse() {
    let old = json!([DAY0, DAY0 + 900.0, 60_000, 50_000, 100, "main", 1, "completed", 3]);
    let stats = analyze::summarize_rows(&[turn("h", "h1", DAY0, vec![old])]);
    assert_eq!(stats["cache"]["main"]["calls"], 1);
    assert_eq!(stats["cost"]["unpriced_models"][""], 1);
}

#[test]
fn prices_follow_dated_bands_long_context_and_image_output() {
    let p = prices::parse(prices::FIXTURE, "fixture").unwrap();
    let on = |d: &str| super::super::release_retro::stats::ms(Some(&format!("{d}T12:00:00Z"))).unwrap();
    assert_eq!(p.rates("gemini-4-argon", 1000.0, on("2026-11-30")).unwrap().input, 2.0);
    assert_eq!(p.rates("models/gemini-4-argon-2026-09-01", 1000.0, on("2026-12-01")).unwrap().input, 4.0);
    assert_eq!(p.rates("gemini-3.1-pro-preview", 250_000.0, on("2026-11-12")).unwrap().input, 4.0);
    assert_eq!(p.cost("gemini-3.1-flash-image", 0.0, 0.0, 1_000_000.0, 0.0, on("2026-11-12"), None).unwrap().output, 60.0);
    assert!(p.rates("unknown-model", 1.0, DAY0).is_none());
    assert_eq!(prices::day(DAY0), "2026-11-12");
    assert_eq!(prices::day(on("2028-02-29")), "2028-02-29");
}

#[test]
fn the_owning_price_table_still_parses() {
    let Ok(text) = std::fs::read_to_string(prices::SOURCE) else { return };
    let p = prices::parse(&text, prices::SOURCE).unwrap();
    for model in ["gemini-4-argon", "gemini-4-carbon", "gemini-3.8-flash", "gemini-3.5-flash-lite"] {
        assert!(p.rates(model, 1000.0, DAY0).is_some(), "{model} is unpriced in {}", prices::SOURCE);
    }
}

#[test]
fn the_ack_needs_verdicts_for_required_cache_causes_and_cost_lines() {
    let home = crate::telemetry::test_home("crm-turn-retro-cache-ack");
    let (id, dir) = ("82", root(&home).join("analysis/82"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(probes::registry(&home).parent().unwrap()).unwrap();
    write_json(&probes::registry(&home), &json!({"probes": []})).unwrap();
    write_json(&probes::receipt(&home, id), &json!({"probes": []})).unwrap();
    let incident = root(&home).join("incident-82.json");
    write_json(&incident, &json!({"requirements": {"crm_turn_review": {"classes": [],
        "cache": {"target": 0.9, "hit": 0.75, "causes": ["provider_miss"]}, "cost": {"lines": ["main/relationship-analyst/gemini-4-argon:uncached_input"]}}}})).unwrap();
    let refusal = |report: &Value| { write_json(&dir.join("report.json"), report).unwrap(); validate_ack(&home, id, &incident).err().map(|e| format!("{e:#}")).unwrap_or_default() };
    std::fs::write(dir.join("report.md"), "## 无效消耗\n## 修复\n## 上线\n## 后续探针\n").unwrap();
    let mut report = json!({"classes": [], "no_fix_reason": "r", "report_url": "https://reports.example.com/x/"});
    assert!(refusal(&report).contains("cache.causes[] has no verdict for provider_miss"));
    report["cache"] = json!({"causes": [{"cause": "provider_miss", "verdict": "deferred", "evidence": "58 calls"}]});
    assert!(refusal(&report).contains("deferred needs a reason"));
    // Under the target no cache cause may be called not waste.
    report["cache"]["causes"][0] = json!({"cause": "provider_miss", "verdict": "not_waste", "evidence": "58 calls", "reason": "Vertex best effort"});
    assert!(refusal(&report).contains("not_waste is refused while the hit rate is under target"));
    report["cache"]["causes"][0]["verdict"] = json!("deferred");
    assert!(refusal(&report).contains("cost.lines[] has no verdict"));
    report["cost"] = json!({"lines": [{"line": "main/relationship-analyst/gemini-4-argon:uncached_input", "verdict": "fixed", "evidence": "e"}]});
    assert!(refusal(&report).contains("marked fixed but findings[] is empty"));
    report["cost"]["lines"][0] = json!({"line": "main/relationship-analyst/gemini-4-argon:uncached_input", "verdict": "deferred", "evidence": "e", "reason": "the cache fix covers it"});
    assert!(refusal(&report).contains("缓存命中"));
    std::fs::write(dir.join("report.md"), "## 无效消耗\n## 缓存命中\n## 成本结构\n## 修复\n## 上线\n## 后续探针\n").unwrap();
    assert_eq!(refusal(&report), "");
}

#[test]
fn a_cause_the_last_retro_settled_leaves_its_required_slot_to_the_next() {
    let home = crate::telemetry::test_home("crm-turn-retro-settled");
    let dir = root(&home).join("analysis/83");
    std::fs::create_dir_all(&dir).unwrap();
    write_json(&dir.join("report.json"), &json!({"cache": {"causes": [
        {"cause": "config_switch", "verdict": "needs_founder", "evidence": "e", "reason": "model routing"},
        {"cause": "idle_expiry", "verdict": "deferred", "evidence": "e", "reason": "trial running"},
        {"cause": "provider_miss", "verdict": "fixed", "evidence": "e"}]}})).unwrap();
    let dispatches = [json!({"incident": "83", "started": true})];
    let settled = settled_cache_causes(&home, &dispatches);
    assert_eq!(settled.keys().collect::<Vec<_>>(), ["config_switch"]);
    let review = review(&analyze::summarize_rows(&window_rows()), Vec::new(), &settled);
    // config_switch ranks first but was settled: the next three take the slots, and it is still listed.
    assert_eq!(review["cache"]["causes"], json!(["cold_static_prefix", "provider_miss", "idle_expiry"]));
    assert_eq!(review["cache"]["settled"]["config_switch"]["incident"], "83");
}

/// Turn 0322e482 (costly-turn 84507709929894033359) in miniature: OM buffered after step 0, steps 1 kept the
/// prefix, step 2 fell back to the static floor. `oma` is the stored activation times, `None` a saved old row.
fn floor_fallback(oma: Option<Value>) -> Value {
    let t = DAY0 + 3_600_000.0;
    let observer = json!([t + 2_000.0, t + 3_000.0, 39_000, 0, 100, "auxiliary", 1, "completed", 4, "gemini-3.5-flash-lite", 0, "observational-memory"]);
    let mut row = turn("o", "o1", t, vec![call(t, 60_000.0, 36_000.0, "gemini-4-argon"), observer,
        call(t + 10_000.0, 70_000.0, 64_000.0, "gemini-4-argon"), call(t + 20_000.0, 72_000.0, 36_000.0, "gemini-4-argon")]);
    if let Some(oma) = oma { row["oma"] = oma; }
    analyze::summarize_rows(&[row])["cache"]["causes"].clone()
}

#[test]
fn a_floor_fallback_is_blamed_on_om_only_after_an_activation_before_the_request() {
    let t = DAY0 + 3_600_000.0;
    let calls = |causes: &Value, cause: &str| causes[cause]["calls"].as_u64().unwrap_or(0);
    // Saved rows from before the activation column keep the observer-call test.
    assert_eq!(calls(&floor_fallback(None), "om_observation_block_changed"), 1);
    // A buffer seal without an activation keeps the prefix (mastra 6c04ecb874): the change lies elsewhere.
    let sealed = floor_fallback(Some(json!([])));
    assert_eq!((calls(&sealed, "om_observation_block_changed"), calls(&sealed, "history_prefix_diverged")), (0, 1));
    // An activation after step 1 started and before step 2's request changed the block.
    assert_eq!(calls(&floor_fallback(Some(json!([t + 15_000.0]))), "om_observation_block_changed"), 1);
    // One before step 1 started was already in step 1's request, which step 2 could reuse.
    assert_eq!(calls(&floor_fallback(Some(json!([t + 5_000.0]))), "history_prefix_diverged"), 1);
}

#[test]
fn the_window_reads_a_week_of_previous_turns_and_each_turn_s_activations() {
    let sql = fetch::turns_sql(1_000_000.0, 2_000_000.0);
    assert!(sql.contains("'data-om-activation'") && sql.contains("activatedAt") && sql.contains(") oma"));
    // A thread resumed after a day still finds its previous turn (9d185810 resumed after 24 h 20 min).
    let prior = fetch::prior_sql(&["0f80bd66-8cbe-46ff-b03c-b03f1fca38f8".into()], 700_000_000.0);
    assert!(prior.contains("to_timestamp(95200.000)"), "{prior}");
    // Only marker metadata leaves the database, never the observations text.
    assert!(!sql.contains("'observations'"));
}
