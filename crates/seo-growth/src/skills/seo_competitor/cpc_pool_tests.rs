use super::*;
use super::super::{cpc_pool_expand, cpc_pool_judge as judge, cpc_pool_preview as preview, cpc_pool_view as view};
use chrono::NaiveDate;
use serde_json::json;

/// The pool block of the live watchdog config, so the shipped regexes and targets are what the tests exercise.
fn config() -> PoolConfig {
    let path = format!("{}/tests/fixtures/seo_competitor/watch.config.json", env!("CARGO_MANIFEST_DIR"));
    let all: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    serde_json::from_value(all["googleAds"]["pool"].clone()).unwrap()
}

fn kw(campaign: &str, group: &str, word: &str, impressions: &str, clicks: &str, cost: &str, conversions: &str) -> Value {
    json!({"keyword_text": word, "match_type": "Exact match", "campaign_name": campaign, "ad_group_name": group, "bid_config.effective_cpc_bid": "$0.80",
        "primary_status": "Eligible", "stats.impressions": impressions, "stats.clicks": clicks, "stats.cost": cost, "stats.conversions": conversions})
}

fn term(campaign: &str, query: &str, impressions: &str, clicks: &str, cost: &str, conversions: &str) -> Value {
    json!({"query": query, "query_match_type": "Phrase match", "campaign_name": campaign, "ad_group_name": "AG2", "stats.impressions": impressions,
        "stats.clicks": clicks, "stats.cost": cost, "stats.conversions": conversions})
}

#[test]
fn words_are_normalized_themed_and_blocked_by_the_shipped_config() {
    let c = config();
    assert_eq!(normalize("[camcard]"), "camcard");
    assert_eq!(normalize("\"Business Card Scanner\""), "business card scanner");
    assert_eq!(theme(&c, "名片全能王"), "card-scanner");
    assert_eq!(theme(&c, "dex crm"), "dex", "dex before the generic crm themes");
    assert_eq!(theme(&c, "otter alternative"), "meeting-competitor");
    assert_eq!(theme(&c, "youtube 語音 轉 文字 免費"), "transcription");
    assert_eq!(theme(&c, "how to find clients"), "client-intent");
    assert_eq!(theme(&c, "pizza near me"), "other");
    assert!(blocked(&c, "dating app for introverts") && blocked(&c, "camcard mod apk") && !blocked(&c, "camcard"));
    assert!(blocked(&c, "youtube transcript generator free"), "subtitle seekers are not a business intent");
    assert!(blocked(&c, "otter ai logo") && !blocked(&c, "otter ai") && !blocked(&c, "biologist crm"), "logo image searches are not buyers");
    let mut pool = Pool::default();
    pool.words.insert("otter ai logo".into(), Entry { status: "candidate".into(), source: "idea".into(), ..Default::default() });
    pool.words.insert("otter ai logo live".into(), Entry { status: "live".into(), ..Default::default() });
    merge_seed(&mut pool, &c, &json!([]), "2026-11-19");
    assert_eq!((pool.words["otter ai logo"].status.as_str(), pool.words["otter ai logo live"].status.as_str()), ("blocked", "live"), "an old idea is blocked once the pattern widens; a live word is never touched");
}

#[test]
fn ads_tables_make_live_words_search_terms_and_removed_words() {
    let c = config();
    let mut pool = Pool::default();
    pool.words.insert("old word".into(), Entry { status: "live".into(), ..Default::default() });
    let tabs = json!({
        "keywords_7d": [kw("C2", "AG2", "[ai transcription]", "10", "2", "$1.60", "0.00")],
        "keywords_today": [kw("C2", "AG2", "[ai transcription]", "5", "1", "$0.80", "0.00")],
        "search_terms_7d": [],
        "search_terms_today": [term("C2", "transcript generator", "2", "2", "$1.55", "0.00"), term("C2", "expand_more", "227", "28", "$23.90", "3.00"), term("C2", "ai transcription", "1", "0", "$0.00", "0")]});
    merge_ads(&mut pool, &c, &tabs, "2026-11-18");
    let live = &pool.words["ai transcription"];
    assert_eq!((live.status.as_str(), live.window.clicks, live.window.impressions), ("live", 3.0, 15.0));
    assert_eq!(live.days["2026-11-18"].clicks, 1.0);
    assert_eq!(live.first_live.as_deref(), Some("2026-11-18"));
    assert_eq!(pool.words["transcript generator"].status, "search-term");
    assert_eq!(pool.words["transcript generator"].theme, "transcription");
    assert!(!pool.words.contains_key("expand_more"), "a table control row is not a search term");
    assert_eq!(pool.words["old word"].status, "removed", "a live word missing from the tables left the account");
}

#[test]
fn the_judge_finds_cheap_winners_waste_dead_words_and_cheap_candidates() {
    let t = config().thresholds;
    let today = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
    let campaign = Stats { impressions: 100.0, clicks: 10.0, cost: 10.0, conversions: 0.0 };
    let account = campaign.clone();
    let live = |w: Stats, first: &str| Entry { status: "live".into(), window: w, first_live: Some(first.into()), ..Default::default() };
    let golden = live(Stats { impressions: 20.0, clicks: 4.0, cost: 1.6, conversions: 0.0 }, "2026-11-18");
    assert_eq!(judge::flags(&golden, &campaign, &account, &t, today), vec![judge::GOLDEN]);
    let waste = live(Stats { impressions: 300.0, clicks: 6.0, cost: 7.0, conversions: 0.0 }, "2026-11-18");
    assert!(judge::flags(&waste, &campaign, &account, &t, today).contains(&judge::WASTE));
    let dead = live(Stats::default(), "2026-11-18");
    assert_eq!(judge::flags(&dead, &campaign, &account, &t, today), vec![judge::DEAD]);
    assert!(judge::flags(&live(Stats::default(), "2026-11-25"), &campaign, &account, &t, today).is_empty(), "too young to retire");
    let term = Entry { status: "search-term".into(), window: Stats { impressions: 3.0, clicks: 2.0, cost: 1.4, conversions: 0.0 }, ..Default::default() };
    assert_eq!(judge::flags(&term, &campaign, &account, &t, today), vec![judge::TERM_GOLDEN]);
    let mut cheap = Entry { status: "candidate".into(), ..Default::default() };
    cheap.markets.insert("2158".into(), Market { volume: 170.0, bid_high: 0.94, ..Default::default() });
    assert_eq!(judge::flags(&cheap, &Stats::default(), &account, &t, today), vec![judge::PLANNER_CHEAP]);
    cheap.markets.insert("2702".into(), Market { volume: 590.0, bid_high: 12.54, ..Default::default() });
    assert!(judge::flags(&cheap, &Stats::default(), &account, &t, today).is_empty(), "attio-like bids are not cheap");
}

#[test]
fn proposals_place_words_by_theme_and_script_and_pause_dead_slots() {
    let c = config();
    let mut pool = Pool::default();
    let flagged = |theme: &str, flag: &str| Entry { theme: theme.into(), status: "search-term".into(), flags: vec![flag.into()], ..Default::default() };
    pool.words.insert("名片辨識".into(), flagged("card-scanner", judge::TERM_GOLDEN));
    pool.words.insert("transcript generator".into(), flagged("transcription", judge::TERM_CONVERTS));
    pool.words.insert("how to find clients".into(), flagged("client-intent", judge::TERM_GOLDEN));
    let rows = view::add_rows(&pool, &c);
    assert!(rows.contains(&("SEA-C4 繁中-會議紀錄與錄音轉文字".into(), "AG5 名片掃描".into(), "名片辨識".into(), "https://example.com/zh-TW/personal-crm".into())));
    assert!(rows.iter().any(|r| r.2 == "transcript generator" && r.1 == "AG2 录音转写"));
    assert!(!rows.iter().any(|r| r.2 == "how to find clients"), "a theme without an ad group is not proposed");
    pool.words.insert("cloze crm".into(), Entry { status: "live".into(), flags: vec![judge::DEAD.into()],
        slots: vec![Slot { campaign: "C1".into(), ad_group: "AG2".into(), match_type: "Exact match".into(), ..Default::default() }], ..Default::default() });
    assert_eq!(view::pause_rows(&pool), vec![("C1".into(), "AG2".into(), "cloze crm".into(), "Exact".into())]);
}

#[test]
fn idea_seeds_prefer_proven_words_and_skip_used_ones() {
    let mut pool = Pool::default();
    let e = |source: &str, flags: Vec<&str>, volume: f64| {
        let mut e = Entry { theme: "card-scanner".into(), status: "candidate".into(), source: source.into(), flags: flags.into_iter().map(String::from).collect(), ..Default::default() };
        e.markets.insert("2702".into(), Market { volume, ..Default::default() });
        e
    };
    pool.words.insert("camcard".into(), e("seed", vec![], 540.0));
    pool.words.insert("card scanner".into(), e("seed", vec![judge::PLANNER_CHEAP], 320.0));
    pool.words.insert("scan business card".into(), e("live", vec![judge::GOLDEN], 10.0));
    pool.words.insert("名片 app".into(), e("seed", vec![], 900.0));
    pool.expanded_from.push("名片 app".into());
    assert_eq!(cpc_pool_expand::seeds(&pool, 3), vec!["scan business card", "card scanner", "camcard"]);
}

#[test]
fn the_summary_reports_only_flags_that_appeared_today() {
    let c = config();
    let dir = std::env::temp_dir().join(format!("cpc-pool-test-{}", std::process::id()));
    let state = dir.to_string_lossy().to_string();
    let mut pool = Pool::default();
    let mut fresh = Entry { theme: "card-scanner".into(), status: "search-term".into(), flags: vec![judge::TERM_GOLDEN.into()], ..Default::default() };
    fresh.flagged_since.insert(judge::TERM_GOLDEN.into(), "2026-11-18".into());
    let mut old = fresh.clone();
    old.flagged_since.insert(judge::TERM_GOLDEN.into(), "2026-11-17".into());
    pool.words.insert("名片辨識".into(), fresh);
    pool.words.insert("名片掃描器".into(), old);
    let mut errors = Vec::new();
    let s = view::summary(&pool, &c, &state, "2026-11-18", "", &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(s["newFlags"].as_array().unwrap().len(), 1);
    let add = std::fs::read_to_string(s["proposals"]["add"].as_str().unwrap()).unwrap();
    assert!(add.starts_with("Campaign,Ad Group,Keyword,Criterion Type,Status,Final URL\n") && add.contains("\"名片辨識\",\"Exact\""));
    assert!(view::section(Some(&s)).contains("| 名片辨識 | 搜索词高效：建议加为精确词 | 0 | 0 | $0 | — | 0 | 0 |"), "{}", view::section(Some(&s)));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_partial_read_keeps_the_table_groups_it_did_not_read() {
    let c = config();
    let mut pool = Pool::default();
    let both = json!({"keywords_7d": [], "keywords_today": [kw("C1", "AG4", "[camcard]", "3", "1", "$0.50", "0")],
        "search_terms_7d": [], "search_terms_today": [term("C1", "camcard scanner", "3", "2", "$1.00", "0")]});
    merge_ads(&mut pool, &c, &both, "2026-11-18");
    let keywords_only = json!({"keywords_7d": [], "keywords_today": [kw("C1", "AG4", "[camcard]", "5", "2", "$0.90", "0")]});
    merge_ads(&mut pool, &c, &keywords_only, "2026-11-18");
    assert_eq!(pool.words["camcard"].window.impressions, 5.0, "keywords were read again");
    assert_eq!(pool.words["camcard scanner"].window.clicks, 2.0, "search terms were not read, so they keep their numbers");
    merge_ads(&mut pool, &c, &json!({"keywords_today": []}), "2026-11-18");
    assert_eq!(pool.words["camcard"].status, "live", "a cut-short keyword read (no Last 7 days table) removes nothing");
}

#[test]
fn each_proposal_file_is_previewed_once_and_shown_unapplied() {
    let dir = std::env::temp_dir().join(format!("cpc-pool-preview-{}", std::process::id()));
    let state = dir.to_string_lossy().to_string();
    std::fs::create_dir_all(&dir).unwrap();
    let summary = || json!({"proposals": {"add": format!("{state}/cpc-pool/proposals/2026-11-19-add-1a2b3c4d.csv"), "pause": null,
        "addRows": [{"campaign": "SEA-C1", "adGroup": "AG4", "keyword": "card scanner"}], "pauseRows": []}});
    let calls = std::cell::Cell::new(0);
    let ok = |file: &str, _out: &str| { calls.set(calls.get() + 1); assert!(file.ends_with("-add-1a2b3c4d.csv")); Ok(json!({"ok": true, "validChanges": 1, "preview": "Preview finished successfully 1 valid change"})) };
    let (mut first, mut errors) = (summary(), Vec::new());
    preview::run_with(&mut first, &state, &state, ("2026-11-19T01:17:00+08:00", preview::Reader::Manual), false, &mut errors, &ok);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!((calls.get(), first["proposals"]["preview"][0]["fresh"].as_bool()), (1, Some(true)));
    assert!(view::section(Some(&json!({"counts": {}, "proposals": first["proposals"]}))).contains("提案加词 1 个：card scanner（SEA-C1 / AG4）。01:17 在 Ads 预览通过，1 条有效；未应用"));
    let mut again = summary();
    preview::run_with(&mut again, &state, &state, ("2026-11-19T03:17:00+08:00", preview::Reader::Retro), false, &mut errors, &ok);
    assert_eq!((calls.get(), again["proposals"]["preview"][0]["fresh"].as_bool()), (1, Some(true)), "the same file is not uploaded twice; a manual preview still reaches the next hourly retro");
    assert!(preview::section(&again["proposals"]).contains("01:17 在 Ads 预览通过"));
    let mut later = summary();
    preview::run_with(&mut later, &state, &state, ("2026-11-19T04:17:00+08:00", preview::Reader::Retro), false, &mut errors, &ok);
    assert_eq!((calls.get(), later["proposals"]["preview"][0]["fresh"].as_bool()), (1, Some(false)), "the retro asks about a preview once");
    let fail = |_: &str, _: &str| Err::<Value, String>("lease timed out".into());
    let mut other = json!({"proposals": {"pause": format!("{state}/p/2026-11-19-pause-99999999.csv"), "pauseRows": [{"campaign": "SEA-C2", "adGroup": "AG1", "keyword": "x"}]}});
    preview::run_with(&mut other, &state, &state, ("2026-11-19T02:17:00+08:00", preview::Reader::Retro), false, &mut errors, &fail);
    assert_eq!(errors.len(), 1);
    assert!(preview::section(&other["proposals"]).contains("预览失败：lease timed out"), "a failed preview is shown and retried next hour");
    let mut dry = json!({"proposals": {"pause": format!("{state}/p/2026-11-19-pause-99999999.csv")}});
    preview::run_with(&mut dry, &state, &state, ("x", preview::Reader::Retro), true, &mut errors, &fail);
    assert_eq!((errors.len(), dry["proposals"]["preview"][0]["skipped"].as_str()), (1, Some("dry run")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_filtered_keyword_table_removes_nothing_and_removed_words_are_not_proposed() {
    let c = config();
    let mut pool = Pool::default();
    let words: Vec<String> = (0..10).map(|i| format!("ai note taker {i}")).collect();
    let all: Vec<Value> = words.iter().map(|w| kw("C2", "AG1", &format!("[{w}]"), "3", "1", "$0.50", "0")).collect();
    merge_ads(&mut pool, &c, &json!({"keywords_7d": all, "keywords_today": []}), "2026-11-19");
    assert_eq!(pool.words.values().filter(|e| e.status == "live").count(), 10);
    let two = json!({"keywords_7d": [kw("C2", "AG1", "[ai note taker 0]", "9", "2", "$1.00", "0")], "keywords_today": [kw("C2", "AG1", "[ai note taker 1]", "1", "0", "$0", "0")]});
    let note = merge_ads(&mut pool, &c, &two, "2026-11-19");
    assert!(note.is_some_and(|n| n.contains("listed 2 words but 10 are live")), "2026-11-19 11:17: a filter cut 95 rows to 16");
    assert_eq!((pool.words.values().filter(|e| e.status == "live").count(), pool.words["ai note taker 0"].window.impressions), (10, 3.0), "nothing removed, numbers kept");
    let one_gone: Vec<Value> = words[1..].iter().map(|w| kw("C2", "AG1", &format!("[{w}]"), "3", "1", "$0.50", "0")).collect();
    assert!(merge_ads(&mut pool, &c, &json!({"keywords_7d": one_gone, "keywords_today": []}), "2026-11-19").is_none());
    assert_eq!(pool.words["ai note taker 0"].status, "removed", "a real removal still lands");
    let mut removed = Entry { status: "removed".into(), ..Default::default() };
    removed.markets.insert("SG".into(), Market { volume: 5000.0, bid_high: 0.5, ..Default::default() });
    assert!(judge::flags(&removed, &Stats::default(), &Stats::default(), &c.thresholds, NaiveDate::from_ymd_opt(2026, 11, 19).unwrap()).is_empty(),
        "a removed word is not a cheap untested candidate");
}
