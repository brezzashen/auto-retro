use super::*;
use crate::skills::seo_competitor::watch_geo_aisa::{parse_anthropic, parse_gemini, parse_openai, parse_verdict};
use crate::skills::seo_competitor::watch_geo_ark::{self as ark, Price};
use crate::skills::seo_competitor::watch_geo_view::{bucket_rates, rates, section, top_domains};
use serde_json::json;

fn at(t: &str) -> DateTime<FixedOffset> { DateTime::parse_from_rfc3339(t).unwrap() }

fn config(bank: &str) -> GeoConfig {
    GeoConfig {
        keychain_service: "none".into(), keychain_account: "none".into(), bank: bank.into(), questions_per_run: 2, daily_budget_usd: 6.0,
        est_question_usd: 0.11, judge_model: "gemini-3.8-flash".into(),
        engines: vec![GeoEngine { id: "chatgpt".into(), label: "ChatGPT".into(), model: "gpt-5-search-api".into(), kind: "openai-search".into(), ..Default::default() }],
        unmeasured: vec![Unmeasured { label: "豆包".into(), reason: "AIsa 没有豆包模型".into() }],
    }
}

fn rec(at: &str, hour: &str, ok: bool, mentioned: bool, rank: Option<u32>) -> GeoRecord {
    GeoRecord { at: at.into(), hour: hour.into(), engine: "chatgpt".into(), ok, mentioned, rank, ..Default::default() }
}

#[test]
fn each_engine_answer_is_read_with_its_citations_and_truncation() {
    let openai = parse_openai(&json!({"choices": [{"finish_reason": "stop", "message": {"content": "Try Clay", "annotations": [{"type": "url_citation", "url_citation": {"url": "https://example.com/x"}}]}}]}));
    assert_eq!((openai.text.as_str(), openai.cited.len(), openai.incomplete.is_none()), ("Try Clay", 1, true));
    let claude = parse_anthropic(&json!({"stop_reason": "max_tokens", "content": [{"type": "server_tool_use"}, {"type": "text", "text": "A ", "citations": [{"url": "https://getdex.com"}]}, {"type": "text", "text": "B"}]}));
    assert_eq!((claude.text.as_str(), claude.cited, claude.incomplete.as_deref()), ("A B", vec!["https://getdex.com".to_string()], Some("stopped with max_tokens")));
    let gemini = parse_gemini(&json!({"candidates": [{"finishReason": "STOP", "content": {"parts": [{"text": "Mars"}]}, "groundingMetadata": {"groundingChunks": [{"web": {"uri": "https://vertexaisearch/redirect", "title": "example.com"}}]}}]}));
    assert!(gemini.cited.iter().any(|c| aisa::is_ours(c)) && gemini.incomplete.is_none());
    let blocked = parse_gemini(&json!({"candidates": [{"finishReason": "SAFETY", "content": {"parts": []}}]}));
    assert_eq!(blocked.incomplete.as_deref(), Some("empty answer"), "an empty 200 answer is not a miss");
    let filtered = parse_openai(&json!({"choices": [{"finish_reason": "content_filter", "message": {"content": "partial"}}]}));
    assert_eq!(filtered.incomplete.as_deref(), Some("stopped with content_filter"));
}

#[test]
fn the_judge_needs_the_answer_to_name_mars() {
    let v = parse_verdict("```json\n{\"mentioned\": true, \"rank\": 2, \"products\": [\"Clay\", \"Mars\"]}\n```", "1. Clay 2. Mars by ExampleApp").unwrap();
    assert_eq!((v.mentioned, v.rank, v.products.len()), (true, Some(2), 2));
    let unbacked = parse_verdict("{\"mentioned\": true, \"rank\": 1, \"products\": [\"Clay\"]}", "1. Clay 2. Dex").unwrap();
    assert_eq!((unbacked.mentioned, unbacked.rank), (false, None), "a mention the text does not contain is dropped");
    assert!(parse_verdict("not json", "x").is_err());
}

#[test]
fn a_cut_or_failed_answer_is_an_error_not_a_miss() {
    let base = GeoRecord::default();
    let cut = record(base.clone(), Ok(Answer { incomplete: Some("stopped with max_tokens".into()), cost_usd: 0.01, ..Default::default() }), None);
    assert!(!cut.ok && cut.error.as_deref() == Some("stopped with max_tokens") && cut.cost_usd == 0.01);
    assert!(!record(base.clone(), Err("HTTP 502".into()), None).ok);
    let good = record(base, Ok(Answer { cited: vec!["https://example.com/".into()], cost_usd: 0.03, ..Default::default() }),
        Some(Ok(Verdict { mentioned: true, rank: Some(1), products: vec!["Mars".into()], cost_usd: 0.001 })));
    assert!(good.ok && good.mars_cited && good.rank == Some(1) && (good.cost_usd - 0.031).abs() < 1e-9);
}

#[test]
fn the_bank_keeps_global_unbranded_questions_and_rotates() {
    let dir = tempfile::tempdir().unwrap();
    let bank = dir.path().join("bank.jsonl");
    std::fs::write(&bank, [
        r#"{"id":"A","market":"global","branded":false,"question":"a"}"#, r#"{"id":"B","market":"global","branded":true,"question":"b"}"#,
        r#"{"id":"C","market":"cn","branded":false,"question":"c"}"#, r#"{"id":"D","market":"global","branded":false,"question":"d"}"#,
        r#"{"id":"E","market":"global","branded":false,"question":"e"}"#].join("\n")).unwrap();
    let questions = read_bank(bank.to_str().unwrap(), "global").unwrap();
    assert_eq!(questions.iter().map(|q| q.id.as_str()).collect::<Vec<_>>(), ["A", "D", "E"]);
    assert_eq!(read_bank(bank.to_str().unwrap(), "cn").unwrap().iter().map(|q| q.id.as_str()).collect::<Vec<_>>(), ["C"]);
    let (picked, cursor) = rotate(&questions, 2, 2);
    assert_eq!((picked.iter().map(|q| q.id.as_str()).collect::<Vec<_>>(), cursor), (vec!["E", "A"], 1));
}

#[test]
fn a_run_past_the_daily_cap_asks_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().to_string_lossy().to_string();
    let bank = format!("{state}/bank.jsonl");
    std::fs::write(&bank, r#"{"id":"A","market":"global","branded":false,"question":"a"}"#).unwrap();
    std::fs::create_dir_all(format!("{state}/geo")).unwrap();
    std::fs::write(format!("{state}/geo/state.json"), r#"{"cursor":0,"day":"2026-11-12","spentUsd":5.95}"#).unwrap();
    let mut errors = Vec::new();
    let run = collect(Some(&config(&bank)), &state, &format!("{state}/run"), at("2026-11-12T13:17:00+08:00"), false, &mut errors).unwrap();
    assert!(run.skipped.as_deref().unwrap().contains("每日上限") && run.records.is_empty() && errors.is_empty());
    let next_day = collect(Some(&config(&bank)), &state, &format!("{state}/run"), at("2026-11-13T00:17:00+08:00"), true, &mut errors).unwrap();
    assert!(next_day.skipped.as_deref().unwrap().starts_with("演练") && next_day.spent_today_usd == 0.0, "the cap resets each Singapore day");
}

#[test]
fn rates_split_this_hour_the_day_and_the_week() {
    let now = at("2026-11-12T13:17:00+08:00");
    let records = vec![
        rec("2026-11-12T13:17:00+08:00", "2026-11-12T13", true, true, Some(1)),
        rec("2026-11-12T13:17:00+08:00", "2026-11-12T13", false, false, None),
        rec("2026-11-11T20:17:00+08:00", "2026-11-11T20", true, false, None),
        rec("2026-11-08T10:17:00+08:00", "2026-11-08T10", true, true, Some(3)),
        rec("2026-11-01T10:17:00+08:00", "2026-11-01T10", true, true, Some(1)),
    ];
    let (hour, day, week) = rates(&records, "2026-11-12T13", now)["chatgpt"].clone();
    assert_eq!((hour.answers, hour.mentioned, hour.failed), (1, 1, 1));
    assert_eq!((day.answers, day.mentioned), (2, 1));
    assert_eq!((week.answers, week.mentioned, week.first), (3, 2, 1), "the 11-01 answer is older than 7 days");
}

#[test]
fn the_section_names_doubao_as_unmeasured_without_a_number() {
    let cfg = config("unused");
    let now = at("2026-11-12T13:17:00+08:00");
    let run = GeoRun { hour: "2026-11-12T13".into(), daily_budget_usd: 6.0, unmeasured: cfg.unmeasured.clone(), records: vec![rec("2026-11-12T13:17:00+08:00", "2026-11-12T13", true, false, None)], ..Default::default() };
    let md = section(Some(&cfg), Some(&run), &run.records, now);
    assert!(md.starts_with("## AI 推荐率") && md.contains("| ChatGPT | gpt-5-search-api | 0/1（0%）"));
    assert!(md.contains("| 豆包 | — | 未测量") && md.contains("豆包未测量：AIsa 没有豆包模型"));
    assert!(section(None, Some(&run), &[], now).is_empty());
}

/// Live smoke run against AIsa with the repository config: `MARS_GEO_LIVE=<scratch dir> cargo test -p mars-skill-tools live_geo -- --ignored`.
#[test]
#[ignore]
fn live_geo_hour_through_aisa() {
    let Ok(state) = std::env::var("MARS_GEO_LIVE") else { return };
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../..");
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{root}/.claude/skills/seo-competitor/references/watch.config.json")).unwrap()).unwrap();
    let mut cfg: GeoConfig = serde_json::from_value(raw["geo"].clone()).unwrap();
    cfg.bank = format!("{root}/{}", cfg.bank);
    let now = chrono::Utc::now().with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap());
    let mut errors = Vec::new();
    let run = collect(Some(&cfg), &state, &format!("{state}/run"), now, false, &mut errors).unwrap();
    println!("{}", serde_json::to_string_pretty(&run).unwrap());
    println!("errors {errors:?}");
    println!("{}", section(Some(&cfg), Some(&run), &read_records(&state), now));
    assert_eq!(run.records.len(), cfg.questions_per_run * cfg.engines.len());
}

#[test]
fn an_ark_answer_keeps_its_citations_and_costs_its_tokens() {
    let price = Price { input_per_m: 1.0, output_per_m: 4.0 };
    let body = json!({"status": "completed", "usage": {"input_tokens": 20000, "output_tokens": 1000}, "output": [
        {"type": "web_search_call", "status": "completed"},
        {"type": "message", "content": [{"type": "output_text", "text": "推荐 Dex", "annotations": [{"type": "url_citation", "url": "https://getdex.com"}, {"type": "url_citation", "url": "https://getdex.com"}]}]}]});
    let a = ark::parse(&body, price);
    assert_eq!((a.text.as_str(), a.cited.len(), a.incomplete.is_none()), ("推荐 Dex", 1, true));
    assert!((a.cost_usd - 0.024).abs() < 1e-9);
    let cut = ark::parse(&json!({"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}, "output": [{"type": "message", "content": [{"type": "output_text", "text": "partial"}]}]}), price);
    assert_eq!(cut.incomplete.as_deref(), Some("stopped with max_output_tokens"));
}

#[test]
fn records_keep_cited_urls_and_search_queries() {
    let claude = parse_anthropic(&json!({"stop_reason": "end_turn", "content": [{"type": "server_tool_use", "input": {"query": "best personal crm 2026"}}, {"type": "text", "text": "Dex", "citations": [{"url": "https://terachat.co/blog/best-personal-crm/"}]}]}));
    assert_eq!(claude.fan_out, vec!["best personal crm 2026".to_string()]);
    let gemini = parse_gemini(&json!({"candidates": [{"finishReason": "STOP", "content": {"parts": [{"text": "Dex"}]}, "groundingMetadata": {"webSearchQueries": ["personal crm ai"], "groundingChunks": []}}]}));
    assert_eq!(gemini.fan_out, vec!["personal crm ai".to_string()]);
    let doubao = ark::parse(&json!({"status": "completed", "output": [{"type": "web_search_call", "action": {"query": "人脉管理 App"}}, {"type": "message", "content": [{"type": "output_text", "text": "x"}]}]}), Price::default());
    assert_eq!(doubao.fan_out, vec!["人脉管理 App".to_string()]);
    let rec = record(GeoRecord::default(), Ok(claude), Some(Ok(Verdict::default())));
    assert_eq!((rec.cited_urls.len(), rec.fan_out.len()), (1, 1));
    let now = at("2026-11-12T16:17:00+08:00");
    let urls = GeoRecord { at: "2026-11-12T16:17:00+08:00".into(), cited_urls: vec!["https://www.terachat.co/a".into(), "https://terachat.co/b".into(), "https://getdex.com/".into()], ..Default::default() };
    assert_eq!(top_domains(&[urls], now), "terachat.co 2、getdex.com 1");
}

#[test]
fn the_headline_rate_counts_tool_questions_only() {
    let now = at("2026-11-12T16:17:00+08:00");
    let mut advice = rec("2026-11-12T16:17:00+08:00", "2026-11-12T16", true, true, Some(1));
    advice.bucket = "advice".into();
    let mut dating = rec("2026-11-12T16:17:00+08:00", "2026-11-12T16", true, false, None);
    dating.bucket = "dating".into();
    let old = rec("2026-11-12T16:17:00+08:00", "2026-11-12T16", true, false, None);
    let records = vec![advice, dating, old];
    let (hour, _, week) = rates(&records, "2026-11-12T16", now)["chatgpt"].clone();
    assert_eq!((hour.answers, week.mentioned), (1, 0), "a record without a bucket is a tool question");
    assert_eq!((bucket_rates(&records, "advice", now)["chatgpt"].mentioned, bucket_rates(&records, "dating", now)["chatgpt"].answers), (1, 1));
}

#[test]
fn each_market_rotates_its_own_questions() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().to_string_lossy().to_string();
    let bank = format!("{state}/bank.jsonl");
    std::fs::write(&bank, [r#"{"id":"G1","market":"global","branded":false,"question":"a"}"#, r#"{"id":"C1","market":"cn","branded":false,"question":"c"}"#].join("\n")).unwrap();
    let mut cfg = config(&bank);
    cfg.engines.push(GeoEngine { id: "doubao".into(), market: Some("cn".into()), ..Default::default() });
    let mut errors = Vec::new();
    let run = collect(Some(&cfg), &state, &format!("{state}/run"), at("2026-11-12T16:17:00+08:00"), true, &mut errors).unwrap();
    assert_eq!(run.skipped.as_deref(), Some("演练：将问 1 题 × 2 个引擎"));
    assert_eq!((cfg.engines[0].market(), cfg.engines[1].market()), ("global", "cn"));
}
