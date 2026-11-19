use super::*;

fn window(id: &str, at_ms: f64, incident: Value, report: Value) -> Window {
    Window { id: id.to_string(), at_ms, incident, report, md: String::new(), counts: Value::Null, probes: Value::Null }
}

fn search_window(id: &str, at_ms: f64, fixed: bool) -> Window {
    window(id, at_ms, json!({}), json!({"calls": 100, "score": 97.0, "grade": "A",
        "quality": {"answer_rate": 1.0, "speed_rate": 0.97, "adoption_rate": 0.95},
        "failure_classes": [{"rate": "speed", "cause": "a|b /Users/x/y", "count": 2, "fixed": fixed}],
        "findings": [{"kind": "speed", "change": "x", "commit": "abc1234", "before": 0.9, "after": 0.97}],
        "report_url": format!("https://reports.example.com/search-retro-{id}/")}))
}

#[test]
fn search_series_has_one_case_per_window_newest_first_a_named_trend_and_the_retro_group() {
    let list = [search_window("1", 1_793_000_000_000.0, true), search_window("2", 1_793_100_000_000.0, false)];
    let refs: Vec<&Window> = list.iter().collect();
    let spec = spec("search", &refs, Path::new("/nonexistent"));
    assert_eq!((spec["slug"].as_str(), spec["group"].as_str()), (Some("search-retro"), Some("retro")));
    assert_eq!((spec["cases"][0]["id"].as_str(), spec["cases"][1]["id"].as_str()), (Some("W2"), Some("W1")));
    assert!(spec["cases"][0]["reason"].as_str().unwrap().contains("失败 1 类，已修 0 类"));
    assert!(spec["cases"][1]["actual"][0].as_str().unwrap().contains("a\\|b ~/y"));
    let mermaid = spec["sections"][0]["diagrams"][0]["mermaid"].as_str().unwrap();
    assert!(mermaid.contains("%% series: 答对率, 速度达标率, 采用率") && mermaid.contains("line [100.00, 100.00]"));
    assert_eq!(spec["sections"][1]["body"].as_array().unwrap().len(), 4);
    assert!(spec["cases"][0]["actual"].as_array().unwrap().iter().any(|a| a.as_str().unwrap().starts_with("原窗口页 ")));
    assert_eq!(window_url("search", "2"), "https://reports.example.com/search-retro/#W2");
}

/// search-retro 816668620510703927526: an object `calls` printed as JSON in the case title.
#[test]
fn search_case_title_counts_metered_calls_of_an_object() {
    let mut w = search_window("3", 1_793_000_000_000.0, false);
    w.report["calls"] = json!({"local_search": 158, "metered": 115, "hits": 111});
    assert!(search::case(&w)["title"].as_str().unwrap().contains(" · 115 次调用 · 评分 97"));
    w.report["calls"] = json!({"total": 88});
    assert!(search::case(&w)["title"].as_str().unwrap().contains(" · 88 次调用 · "));
    w.report["calls"] = json!({});
    let title = search::case(&w)["title"].as_str().unwrap().to_string();
    assert!(!title.contains("次调用") && !title.contains('{'), "{title}");
    assert!(search::case(&search_window("4", 1_793_000_000_000.0, false))["title"].as_str().unwrap().contains(" · 100 次调用 · "));
}

#[test]
fn canary_series_trends_count_windows_and_lists_over_hard_incidents() {
    let stats = json!({"seconds": {"all": {"p50": 600.0, "p90": 900.0, "max": 1200.0}}, "window": {"runs": 10},
        "outcomes": {"success_rate": 100.0}, "verdicts": {"target": 7, "over_target": 3, "over_hard": 0}});
    let check = json!({"verdict": "defect", "evidence": "preflight 580 s"});
    let counted = window("1", 1.0e12, json!({"stats": stats}), json!({"checks": {"errors": {"verdict": "ok", "evidence": "-"}, "cache": check, "queue": "fine", "ci": check},
        "changes": [{"commit": "0543a7ec12", "summary": "seed swap", "before": "316 s", "after": "0.61 s"}], "not_verified": ["live NOT-MEASURED"]}));
    let hard = window("2", 2.0e12, json!({"runs": [{"id": "run-9"}]}), json!({"checks": {}, "runs": [{"id": "run-9", "cause": "queue", "evidence": "lease wait"}]}));
    let refs = [&counted, &hard];
    let spec = spec("canary", &refs, Path::new("/nonexistent"));
    assert!(spec["sections"][0]["diagrams"][0]["mermaid"].as_str().unwrap().contains("line [10.00]"));
    assert_eq!(spec["cases"][0]["reason"], "超过硬上限：1 次发布（run-9）");
    assert!(spec["cases"][1]["reason"].as_str().unwrap().contains("p50 10.0 分"));
    let actual: Vec<&str> = spec["cases"][1]["actual"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(actual.contains(&"缓存检查 defect：preflight 580 s") && actual.contains(&"排队检查 —：fine") && actual.contains(&"未验证：live NOT-MEASURED"));
    assert!(spec["cases"][0]["actual"].as_array().unwrap().iter().any(|a| a == "慢发布 run-9（queue）：lease wait"));
}

/// A home with a probe registry and lifecycle, so conversation threads have versions to show.
fn conversation_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("retro-series-{tag}-{}", std::process::id()));
    let refs = home.join(".codex/skills/watchdog/references");
    std::fs::create_dir_all(&refs).unwrap();
    std::fs::create_dir_all(root(&home)).unwrap();
    std::fs::write(refs.join("retro-probes.json"), json!({"probes": [
        {"id": "R1", "finding": "1#0", "change": "deliver status --text", "shipped": "2026-11-01T00:00:00Z", "zh": "看板加过滤参数"},
        {"id": "R2", "finding": "1#1", "change": "deliver status prints acceptance", "shipped": "2026-11-03T00:00:00Z"},
        {"id": "R3", "finding": "1#2", "change": "spec.ts --help lists groups", "shipped": "2026-11-02T00:00:00Z", "zh": "spec.ts 列出分组"}]}).to_string()).unwrap();
    std::fs::write(root(&home).join("probe-lifecycle.json"), json!({"probes": {
        "R1": {"status": "escalate", "last": "regressed", "streak": 3}, "R2": {"status": "active", "last": "effective", "streak": 2},
        "R3": {"status": "graduated", "last": "effective", "streak": 3}, "C1": {"status": "active"}}}).to_string()).unwrap();
    home
}

#[test]
fn conversation_series_follows_each_problem_through_its_fix_versions() {
    let home = conversation_home("threads");
    let mut a = window("1", 1.0e12, json!({"count": 134}), json!({"scope": "cc4",
        "repeats": [{"signature": "deliver status", "verdict": "scripted", "probe": "R1"}, {"signature": "bunx tsc", "verdict": "necessary"}],
        "findings": [{"change": "x", "commit": "mastra 0e2d372e77 (0.1.1.1)", "probe": "R1", "zh": {"problem": "手写 grep 读看板", "solution": "看板加过滤参数", "gain": "49 次降到 0 次"}}]}));
    a.counts = json!({"sessions": 200, "top": [{"signature": "deliver status", "calls": 98}]});
    let mut b = window("2", 1.1e12, json!({"count": 100}), json!({"scope": "all",
        "repeats": [{"signature": "deliver status", "verdict": "existing_entry", "zh": "手写 grep 读发布看板"}],
        "followups": [{"probe": "R1", "verdict": "regressed"}, {"probe": "R2", "verdict": "effective"}, {"probe": "R3", "verdict": "effective"}], "findings": []}));
    b.counts = json!({"sessions": 100, "top": [{"signature": "deliver status", "calls": 20}]});
    b.probes = json!({"probes": [{"probe": "R1", "after": {"calls_per_100_sessions": 7.2}}]});
    let spec = spec("conversation", &[&a, &b], &home);
    let cases = spec["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 1, "only the open thread is a case: {cases:?}");
    let t = &cases[0];
    assert_eq!((t["id"].as_str(), t["verdict"].as_str()), (Some("S-deliver-status"), Some("FAIL")));
    assert_eq!(t["title"], "手写 grep 读发布看板（deliver status）");
    assert!(t["reason"].as_str().unwrap().starts_with("需升级："));
    let actual: Vec<&str> = t["actual"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(actual[0].starts_with("首次发现 ") && actual[0].contains("出现在 2 个批次"), "{actual:?}");
    assert!(actual[1].contains("第 1 版 11-01 R1：看板加过滤参数（0e2d372e77）｜跟进 退化｜") && actual[1].contains("每百会话 7.2｜需升级"), "{actual:?}");
    assert!(actual[2].contains("第 2 版 11-03 R2：deliver status prints acceptance"), "R2 joins by its change prefix: {actual:?}");
    assert!(t["diagrams"][0]["mermaid"].as_str().unwrap().contains("V1[\"第 1 版 R1 11-01<br/>需升级\"]") && t["diagrams"][0]["mermaid"].as_str().unwrap().contains("class N toneRed"));
    assert!(t["diagrams"][1]["mermaid"].as_str().unwrap().contains("line [49.0, 20.0]"));
    let titles: Vec<&str> = spec["sections"].as_array().unwrap().iter().filter_map(|s| s["title"].as_str()).collect();
    assert_eq!(titles, ["问题状态", "已解决的问题（1）", "判定为必要操作（1）", "复盘批次（2）"]);
    assert!(spec["sections"][1]["body"][2].as_str().unwrap().contains("R3：spec.ts 列出分组"));
    assert_eq!(spec["brief"]["problems"][0], "需升级：手写 grep 读发布看板（deliver status），出现在 2 个批次");
    assert_eq!(spec["brief"]["solutions"][0], "已出 2 版，最新 R2 11-03：deliver status prints acceptan…");
    assert!(!spec.to_string().contains("C1"), "lifecycle-only ids stay off the page");
    assert_eq!(window_url("conversation", "2"), "https://reports.example.com/conversation-retro/");
}

#[test]
fn instruction_names_the_anchor_the_series_command_and_the_ack() {
    let text = instruction("canary", "77", Path::new("/s"));
    assert!(text.contains("report_url https://reports.example.com/canary-retro/#W77"));
    assert!(text.contains("cpu-watchdog retro-series canary") && text.contains("canary-retro-77 page only when") && text.ends_with("monitor-ack 77."));
}

#[test]
fn an_object_cell_is_plain_text_even_when_cut() {
    let long = "x".repeat(CELL_CHARS * 2);
    let got = cell(&json!({"replay": long, "status": "NOT-MEASURED"}));
    assert!(got.starts_with("replay: x"), "{got}");
    assert!(!got.starts_with('{'), "a cut JSON cell fails the publication guard: {got}");
    assert_eq!(cell(&json!({"a": 1, "b": [1, "c"]})), "a: 1; b: 1, c");
    assert_eq!(cell(&json!(0.97)), "0.97");
}
