use super::*;
use serde_json::json;

/// A stored turn that starts at `s`: `steps` are main-model call starts (ms after `s`), `tools`
/// are `(start, name, failed)`, and the first text arrives at `txt` after `s`.
fn row(id: &str, s: f64, txt: f64, steps: &[f64], tools: &[(f64, &str, bool)]) -> Value {
    let calls: Vec<Value> = steps.iter().map(|at| json!([s + at, s + at + 900.0, 60_000, 50_000, 100, "main", 1, "completed", 3])).collect();
    let tools: Vec<Value> = tools.iter().map(|(at, name, failed)| json!([s + at, s + at + 50.0, name, "main", if *failed { "failed" } else { "completed" }])).collect();
    json!({"thread_id": format!("thread-{id}"), "u": "dee71265", "at_ms": s + txt + 2000.0, "turn": id, "s": s, "trig": null, "lvl": "light",
        "txt": s + txt, "status": "completed", "ended": s + txt + 1500.0, "credits": 3 * steps.len(), "calls": calls, "tools": tools, "inv": null})
}

fn classes(row: &Value) -> Vec<&'static str> {
    analyze::turn(row).unwrap().silent.iter().map(|s| s.class).collect()
}

#[test]
fn a_one_step_answer_has_no_silent_step() {
    let turn = analyze::turn(&row("a", 1000.0, 4000.0, &[500.0], &[])).unwrap();
    assert!(turn.silent.is_empty());
    assert_eq!((turn.steps, turn.first_text_ms), (1, Some(4000.0)));
}

#[test]
fn record_steps_before_text_are_record_first_then_split_record() {
    let r = row("b", 0.0, 21_000.0, &[500.0, 7000.0, 14_000.0, 20_000.0],
        &[(6000.0, "create", false), (13_000.0, "create", false), (13_100.0, "write-resource", false), (19_000.0, "read-resource", false)]);
    assert_eq!(classes(&r), ["record_first", "split_record", "read"]);
    let turn = analyze::turn(&r).unwrap();
    assert_eq!(turn.silent[0].wait_ms, 6500.0);
    assert_eq!(turn.silent[1].tools, ["create", "write-resource"]);
}

#[test]
fn a_step_that_calls_a_failed_tool_again_is_an_error_retry() {
    let r = row("c", 0.0, 16_000.0, &[500.0, 7000.0, 15_000.0], &[(6000.0, "create", true), (13_000.0, "create", false)]);
    assert_eq!(classes(&r), ["record_first", "error_retry"]);
    // A different tool after the failure is not a retry.
    let r = row("d", 0.0, 16_000.0, &[500.0, 7000.0, 15_000.0], &[(6000.0, "create", true), (13_000.0, "read-resource", false)]);
    assert_eq!(classes(&r), ["record_first", "read"]);
}

#[test]
fn consecutive_read_steps_are_split_reads_and_discovery_is_its_own_class() {
    let r = row("e", 0.0, 30_000.0, &[500.0, 7000.0, 14_000.0, 21_000.0, 28_000.0],
        &[(6000.0, "search_tools", false), (13_000.0, "list-resources", false), (20_000.0, "read-resource", false), (27_000.0, "fetch-web-page", false)]);
    assert_eq!(classes(&r), ["tool_discovery", "read", "split_read", "other"]);
}

#[test]
fn message_parts_add_error_codes_and_mark_repeated_identical_calls() {
    let mut r = row("f", 0.0, 16_000.0, &[500.0, 7000.0, 15_000.0], &[(6000.0, "read-resource", false), (13_000.0, "read-resource", false)]);
    r["inv"] = json!([["read-resource", null, "aaaa"], ["read-resource", null, "aaaa"]]);
    assert_eq!(classes(&r), ["read", "repeat_call"]);
    // An error code in the parts fails the call even when telemetry says completed.
    r["inv"] = json!([["read-resource", "not_found", "aaaa"], ["read-resource", null, "bbbb"]]);
    assert_eq!(classes(&r), ["read", "error_retry"]);
    let stats = analyze::summarize_rows(&[r.clone()]);
    assert_eq!(stats["tools"]["read-resource"]["errors"]["not_found"], 1);
    assert_eq!(analyze::metric(&stats, "tools.read-resource.errors_per_100_turns.not_found"), Some(100.0));
    // Parts that do not pair with telemetry by name are ignored.
    r["inv"] = json!([["create", "x", "aaaa"]]);
    assert_eq!(classes(&r), ["read", "split_read"]);
}

#[test]
fn reminders_are_not_turns_and_a_turn_id_counts_once() {
    let mut reminder = row("r", 0.0, 4000.0, &[500.0], &[]);
    reminder["trig"] = json!("reminder");
    let rows = [reminder, row("a", 10_000.0, 4000.0, &[500.0], &[]), row("a", 10_000.0, 4000.0, &[500.0], &[]), row("b", 20_000.0, 4000.0, &[500.0], &[])];
    assert_eq!(analyze::turns(&rows).len(), 2);
}

#[test]
fn the_summary_ranks_classes_by_wait_and_answers_metric_paths() {
    let rows = [
        row("a", 0.0, 4000.0, &[500.0], &[]),
        row("b", 100_000.0, 16_000.0, &[500.0, 7000.0, 15_000.0], &[(6000.0, "create", true), (13_000.0, "create", false)]),
        row("c", 200_000.0, 9000.0, &[500.0, 8000.0], &[(7000.0, "create", false)]),
    ];
    let stats = analyze::summarize_rows(&rows);
    assert_eq!((stats["turns"].as_u64(), stats["answered"].as_u64()), (Some(3), Some(3)));
    assert_eq!(stats["steps_before_first_text"], json!({"1": 1, "2": 1, "3": 1}));
    assert_eq!(stats["ranked"][0]["class"], "record_first");
    assert_eq!(stats["classes"]["error_retry"]["wait_ms"], json!(8000.0));
    assert_eq!(analyze::metric(&stats, "classes.error_retry.wait_share"), Some(round4(8000.0 / 29_000.0)));
    assert_eq!(analyze::metric(&stats, "tools.create.failure_rate"), Some(round4(1.0 / 3.0)));
    assert_eq!(stats["worst"][0]["thread"], "thread-b");
    assert_eq!(stats["first_text"]["silent_step_share"], json!(round4(22_000.0 / 29_000.0)));
}

fn round4(v: f64) -> f64 { (v * 10_000.0).round() / 10_000.0 }

#[test]
fn the_queries_exclude_the_demo_account_and_reminders_and_stay_in_the_window() {
    let count = fetch::count_sql(1_000_000.0, 2_000_000.0);
    assert!(count.contains("not like '89d9%'") && count.contains("is distinct from 'reminder'"));
    assert!(count.contains("to_timestamp(1000.000)") && count.contains("to_timestamp(2000.000)"));
    let turns = fetch::turns_sql(1_000_000.0, 2_000_000.0);
    assert!(turns.contains("not like '89d9%'") && turns.contains("marsTurnTelemetry") && turns.contains("md5("));
    // Message text never leaves the database.
    assert!(!turns.contains("p->>'text'") && !turns.contains("'result'->>'message'"));
}

#[test]
fn every_statement_reads_content_as_jsonb_only_through_the_validity_guard() {
    // One stored row that is valid json but not jsonb (a lone low surrogate) failed every sample for three days.
    let guard = "case when pg_input_is_valid(content, 'jsonb') then content::jsonb end";
    let statements = [fetch::count_sql(1_000_000.0, 2_000_000.0), fetch::turns_sql(1_000_000.0, 2_000_000.0),
        fetch::prior_sql(&["0f80bd66-8cbe-46ff-b03c-b03f1fca38f8".into()], 2_000_000.0)];
    for sql in &statements {
        assert!(sql.contains(guard), "{sql}");
        // No unguarded cast: every `content::jsonb` sits inside the guard's CASE.
        assert_eq!(sql.matches("content::jsonb").count(), sql.matches(guard).count(), "{sql}");
    }
    // The count reports skipped rows as a number; the row reads leave them out.
    assert!(statements[0].contains("count(*) filter (where not ok) skipped"));
    for sql in &statements[1..] { assert!(sql.contains("and pg_input_is_valid(content, 'jsonb') offset 0"), "{sql}"); }
    let rows = fetch::rows("[{\"n\": \"100\", \"newest_ms\": 5, \"skipped\": \"1\"}]").unwrap();
    assert_eq!(fetch::number(&rows[0]["skipped"]), Some(1.0));
}

#[test]
fn a_skipped_row_is_counted_in_the_state_never_its_text() {
    let home = crate::telemetry::test_home("crm-turn-retro-skipped");
    let mut retro = CrmTurnRetro { loaded: true, ..Default::default() };
    retro.absorb(&home, 1000.0, Ok((40, 0.0, 1, None))).unwrap();
    retro.write(&home, 1.0).unwrap();
    let state = crate::telemetry::read(&root(&home).join("crm-turn-retro.json")).unwrap();
    assert_eq!((state["skipped_invalid_jsonb"].as_u64(), state["error"].is_null(), state["window_count"].as_u64()), (Some(1), true, Some(40)));
}

#[test]
fn rows_are_the_json_array_after_the_header_lines_and_a_long_span_is_read_a_day_at_a_time() {
    let rows = fetch::rows("[mars] mars-ops-crm canary sql\n[{\"n\": \"3\", \"newest_ms\": 5}]\n").unwrap();
    assert_eq!(fetch::number(&rows[0]["n"]), Some(3.0));
    assert!(fetch::rows("[mars] no rows here").is_err());
    let day = 86_400_000.0;
    assert_eq!(fetch::slices(0.0, 2.5 * day), [(0.0, day), (day, 2.0 * day), (2.0 * day, 2.5 * day)]);
}

fn window(n: usize) -> Vec<Value> {
    (0..n).map(|i| row(&format!("t{i}"), 1_000_000.0 + i as f64 * 60_000.0, 16_000.0, &[500.0, 7000.0, 15_000.0],
        &[(6000.0, "create", true), (13_000.0, "create", false)])).collect()
}

#[test]
fn a_hundred_turns_open_one_incident_and_move_the_baseline() {
    let home = crate::telemetry::test_home("crm-turn-retro");
    std::fs::create_dir_all(root(&home).join("incidents")).unwrap();
    let mut retro = CrmTurnRetro { loaded: true, baseline_ms: 500_000.0, ..Default::default() };
    assert_eq!(retro.escalate(&home, 9_000_000.0, &window(99)).unwrap(), None);
    let rows = window(100);
    let id = retro.escalate(&home, 9_000_000.0, &rows).unwrap().unwrap();
    let incident = crate::telemetry::read(&root(&home).join("incidents").join(format!("{id}.json"))).unwrap();
    assert_eq!(incident["trigger"], "crm_turn_count_reached");
    assert_eq!(incident["requirements"]["crm_turn_review"]["classes"], json!(["error_retry"]));
    assert_eq!(retro.baseline_ms, rows[99]["at_ms"].as_f64().unwrap());
    assert_eq!(fetch::saved(&root(&home).join("crm-turn-retro").join(format!("{id}.json"))).unwrap().len(), 100);
    // No second window opens while the first is unacknowledged; an ack frees it.
    assert!(!retro.may_open(&home, 9_100_000.0));
    retro.absorb(&home, 9_100_000.0, Ok((150, 0.0, 0, Some(window(150))))).unwrap();
    assert_eq!(retro.dispatches.len(), 1);
    std::fs::write(root(&home).join("incidents").join(format!("{id}.ack")), "").unwrap();
    assert!(retro.may_open(&home, 9_100_000.0));
}

#[test]
fn a_failed_read_is_recorded_and_at_most_four_sessions_start_a_day() {
    let home = crate::telemetry::test_home("crm-turn-retro-cap");
    std::fs::create_dir_all(root(&home).join("incidents")).unwrap();
    let mut retro = CrmTurnRetro { loaded: true, ..Default::default() };
    retro.absorb(&home, 1000.0, Err("mars canary sql timed out".into())).unwrap();
    assert_eq!(retro.error.as_deref(), Some("mars canary sql timed out"));
    for i in 0..4 {
        let id = format!("8{i}");
        std::fs::write(root(&home).join("incidents").join(format!("{id}.ack")), "").unwrap();
        retro.dispatches.push(json!({"incident": id, "at_ms": 1000.0 + i as f64, "started": true}));
    }
    assert!(!retro.may_open(&home, 2000.0));
    assert!(retro.may_open(&home, 1000.0 + DAY_MS + 10.0));
}

#[test]
fn the_prompt_is_one_line_and_names_the_route_and_the_follow_up() {
    let text = prompt(Path::new("/h"), "81", &json!({"turns": 100}));
    assert!(!text.contains('\n'));
    assert!(text.contains("references/crm-turn-retro.md") && text.contains("crm-turn-stats --followup 81") && text.contains("monitor-ack 81"));
}

#[test]
fn a_probe_is_judged_by_its_metric_direction_and_sample_size() {
    let probe = json!({"id": "T1", "metric": "classes.error_retry.wait_share", "direction": "lower", "shipped": "2026-11-11T00:00:00Z"});
    let side = |turns: u64, share: Value| json!({"answered": turns, "classes": {"error_retry": {"wait_share": share}}});
    assert_eq!(probes::judge(&probe, &side(200, json!(0.10)), &side(200, json!(0.04)))["suggested"], "effective");
    assert_eq!(probes::judge(&probe, &side(200, json!(0.10)), &side(200, json!(0.095)))["suggested"], "ineffective");
    assert_eq!(probes::judge(&probe, &side(200, json!(0.10)), &side(200, json!(0.15)))["suggested"], "regressed");
    assert_eq!(probes::judge(&probe, &side(200, json!(0.10)), &side(20, json!(0.0)))["suggested"], "insufficient_data");
    // A class that no longer occurs reads as zero, not as missing.
    let gone = json!({"answered": 200, "classes": {}});
    assert_eq!(probes::judge(&probe, &side(200, json!(0.10)), &gone)["suggested"], "effective");
}

#[test]
fn the_ack_needs_class_verdicts_measured_fixes_and_probe_verdicts() {
    let home = crate::telemetry::test_home("crm-turn-retro-ack");
    let (id, dir) = ("81", root(&home).join("analysis/81"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(probes::registry(&home).parent().unwrap()).unwrap();
    let incident = root(&home).join("incident.json");
    write_json(&incident, &json!({"requirements": {"crm_turn_review": {"classes": ["error_retry"]}}})).unwrap();
    write_json(&probes::registry(&home), &json!({"probes": [{"id": "T1"}, {"id": "T2"}]})).unwrap();
    let refusal = |report: &Value| { write_json(&dir.join("report.json"), report).unwrap(); validate_ack(&home, id, &incident).err().map(|e| format!("{e:#}")).unwrap_or_default() };
    let mut report = json!({"classes": [], "report_url": "https://reports.example.com/x/"});
    assert!(refusal(&report).contains("no verdict for error_retry"));
    report["classes"] = json!([{"class": "error_retry", "verdict": "fixed", "evidence": "thread-b round 2"}]);
    assert!(refusal(&report).contains("findings[] or no_fix_reason"));
    report["findings"] = json!([{"cause": "c", "layer": "tool", "evidence": "e", "change": "ch", "commit": "abc", "quality": "same facts",
        "before": {"beta": 3, "canary": 3}, "after": {"beta": 2}, "production": {"digest": "sha256:1"}, "probe": "T2"}]);
    assert!(refusal(&report).contains("after.canary is missing"));
    report["findings"][0]["after"]["canary"] = json!(2);
    assert!(refusal(&report).contains("crm-turn-stats --followup 81"));
    write_json(&probes::receipt(&home, id), &json!({"probes": [{"probe": "T1"}]})).unwrap();
    assert!(refusal(&report).contains("no verdict for probe T1"));
    report["followups"] = json!([{"probe": "T1", "verdict": "regressed", "analysis": "a"}]);
    assert!(refusal(&report).contains("needs next"));
    report["followups"][0]["next"] = json!("n");
    assert!(refusal(&report).contains("report.md needs"));
    std::fs::write(dir.join("report.md"), "## 无效消耗\n## 修复\n## 上线\n## 后续探针\n").unwrap();
    assert_eq!(refusal(&report), "");
    // No fix is acceptable only with its reason; other incidents are not gated.
    report["classes"] = json!([{"class": "error_retry", "verdict": "deferred", "evidence": "e", "reason": "owned by another session"}]);
    report["findings"] = json!([]);
    assert!(refusal(&report).contains("no_fix_reason"));
    report["no_fix_reason"] = json!("every ranked class is owned elsewhere");
    assert_eq!(refusal(&report), "");
    write_json(&incident, &json!({"trigger": "canary_cache_zero"})).unwrap();
    std::fs::remove_file(dir.join("report.json")).unwrap();
    assert!(validate_ack(&home, id, &incident).is_ok());
}
