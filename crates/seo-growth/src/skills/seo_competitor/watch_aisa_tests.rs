use super::*;
use super::super::watch_config::read_config;
use chrono::DateTime;

const API: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/seo_competitor/api");

fn fixture(name: &str) -> serde_json::Value { read_json(&format!("{API}/{name}")).unwrap() }

fn live_config() -> WatchConfig {
    read_config(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/seo_competitor/watch.config.json")).unwrap()
}

#[test]
fn an_organic_serp_yields_the_ai_overview_sources_without_google() {
    let row = parse::ai_cite(&fixture("aisa_organic_ai_overview.json"), "w", "/zh-TW/x", 2158, "2026-11-13");
    assert!(row.present && !row.deferred && !row.mars_cited);
    assert_eq!(row.domains, ["threads.com", "orange.udn.com", "wiki.openfate.ai"]);
}

#[test]
fn an_ai_mode_answer_yields_its_references_and_marks_our_domain() {
    let answer = fixture("aisa_ai_mode.json");
    let row = parse::ai_cite(&answer, "ai relationship coach", "/ai-relationship-coach", 2840, "t");
    assert!(row.present && !row.mars_cited);
    assert_eq!(row.domains, ["deepai.org", "npr.org", "connectedcouples.app"]);
    let ours = serde_json::json!([{"items": [{"type": "ai_overview", "references": [{"domain": "www.example.com"}]}]}]);
    assert!(parse::ai_cite(&ours, "k", "/", 2840, "t").mars_cited);
    let none = serde_json::json!([{"items": [{"type": "organic"}]}]);
    assert!(!parse::ai_cite(&none, "k", "/", 2840, "t").present, "no AI answer is not a missed citation");
}

#[test]
fn intent_difficulty_and_link_days_are_read_from_the_saved_answers() {
    let intents = parse::intents(&fixture("aisa_search_intent.json"));
    assert_eq!(intents["ai relationship coach"].0, "commercial");
    assert_eq!(intents["text message analyzer"].2.as_deref(), Some("informational"));
    let kd = parse::difficulties(&fixture("aisa_keyword_difficulty.json"));
    assert_eq!(kd["personal crm"], 11.0);
    let days = parse::link_days(&fixture("aisa_links_new_lost.json"));
    assert_eq!(days.len(), 3);
    assert_eq!((days[0].date.as_str(), days[0].new_backlinks, days[0].lost_backlinks), ("2026-11-11", 1, 10));
}

#[test]
fn a_dry_run_reads_the_free_ai_overview_and_buys_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().to_string_lossy().to_string();
    let mut config = live_config();
    config.aisa_seo = Some(AisaSeoConfig { ai_mode_per_run: 3, daily_budget_usd: 1.0 });
    let target = config.targets[0].clone();
    let rank = config.rank.clone().unwrap();
    std::fs::create_dir_all(format!("{state}/rank/2026-11-13")).unwrap();
    std::fs::copy(format!("{API}/aisa_organic_ai_overview.json"), serp_file(&rank, &target, &format!("{state}/rank/2026-11-13"))).unwrap();
    let mut errors = Vec::new();
    let now = DateTime::parse_from_rfc3339("2026-11-13T11:17:00+08:00").unwrap();
    let run = collect(&config, &state, now, true, &mut errors).unwrap();
    assert_eq!(run.overview.len(), 1);
    assert!(run.ai_mode_run.is_empty() && run.links.is_empty() && run.spent_today_usd == 0.0 && errors.is_empty());
    assert_eq!(run.skipped.len(), 1, "a dry run stops at the first paid call: {:?}", run.skipped);
    assert!(!std::path::Path::new(&format!("{state}/aisa/state.json")).exists(), "a dry run writes no state");
    let md = super::super::watch_aisa_view::section(Some(&run));
    assert!(md.starts_with("## AIsa 搜索信号") && md.contains("1 个目标词中 1 个出现 AI 答案") && md.contains("orange.udn.com（1）"));
}

#[test]
fn a_spent_day_skips_every_paid_call() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(format!("{state}/aisa")).unwrap();
    std::fs::write(format!("{state}/aisa/state.json"), r#"{"cursor":4,"day":"2026-11-13","spentUsd":2.0}"#).unwrap();
    let mut config = live_config();
    config.aisa_seo = Some(AisaSeoConfig { ai_mode_per_run: 3, daily_budget_usd: 1.0 });
    let mut errors = Vec::new();
    let run = collect(&config, &state, DateTime::parse_from_rfc3339("2026-11-13T12:17:00+08:00").unwrap(), false, &mut errors).unwrap();
    assert!(run.ai_mode_run.is_empty() && run.props.is_empty() && run.links.is_empty());
    assert!(run.skipped[0].contains("达到上限"), "{:?}", run.skipped);
    let saved: serde_json::Value = read_json(&format!("{state}/aisa/state.json")).unwrap();
    assert_eq!(saved["cursor"], 4, "the rotation does not advance without an answer");
}

#[test]
fn the_live_config_turns_the_signals_on_within_a_dollar_a_day() {
    let cfg = live_config().aisa_seo.expect("watch.config.json has an aisaSeo block");
    assert!(cfg.daily_budget_usd <= 1.0 && cfg.ai_mode_per_run * 24 >= live_config().targets.len(), "every target is asked about once a day");
}

#[test]
fn a_live_run_on_a_fresh_state_creates_the_cache_directory() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().to_string_lossy().to_string();
    let mut config = live_config();
    config.aisa_seo = Some(AisaSeoConfig { ai_mode_per_run: 3, daily_budget_usd: 0.0 });
    let mut errors = Vec::new();
    collect(&config, &state, DateTime::parse_from_rfc3339("2026-11-13T12:17:00+08:00").unwrap(), false, &mut errors).unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(std::path::Path::new(&format!("{state}/aisa/state.json")).exists(), "the first live run saves its state");
}

#[test]
fn a_failed_cache_write_names_no_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = format!("{}/missing/aisa/links_2026-11-13.json", dir.path().display());
    let mut errors = Vec::new();
    super::save(&path, &serde_json::json!([]), "links", &mut errors);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].starts_with("aisa links (aisa/links_2026-11-13.json): ") && !errors[0].contains(&dir.path().display().to_string()), "{errors:?}");
}
