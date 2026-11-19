use super::*;
use super::super::watch_trends::{Lens, UNKNOWN, classify};
use super::super::watch_trends_surge::ads_keyword;
use std::collections::BTreeMap;

fn topic(keyword: &str) -> Topic { Topic { keyword: keyword.into(), area: "x".into() } }

fn cfg() -> TrendsConfig {
    let market = |code: &str, location_code: u32, language: &str, topics: &[&str], cap: Option<usize>| Market {
        code: code.into(), label: code.into(), location_code, language_code: language.into(), topics: topics.iter().map(|t| topic(t)).collect(), max_per_source: cap };
    TrendsConfig { every_hours: 6.0, location_code: 2840, time_range: "past_7_days".into(), topics: vec![topic("ai")],
        min_rise: 100.0, max_per_source: 8, heat_ratio: 1.5, surge: None, follow_up_per_day: None, similarweb: None,
        markets: vec![market("JP", 2392, "ja", &["ai", "恋愛"], Some(2)), market("TH", 2764, "th", &["ความรัก"], None)] }
}

#[test]
fn every_market_pulls_its_own_topics_in_its_own_location() {
    let pulls: Vec<(String, u32, String)> = topic_pulls(&cfg()).iter().map(|p| (p.market.clone(), p.location_code, p.topic.keyword.clone())).collect();
    assert_eq!(pulls, vec![("".into(), 2840, "ai".into()), ("JP".into(), 2392, "ai".into()), ("JP".into(), 2392, "恋愛".into()), ("TH".into(), 2764, "ความรัก".into())]);
}

#[test]
fn a_market_source_names_its_market_and_the_default_source_names_none() {
    assert_eq!(topic_source("", "ai"), "topic:ai");
    assert_eq!(topic_source("JP", "恋愛"), "topic:jp:恋愛");
    assert_eq!(market_of("topic:jp:恋愛"), "JP");
    assert_eq!(market_of("topic:ai"), "");
    assert_eq!(market_of("seed:personal-crm"), "");
    assert_eq!(market_of(&topic_source("SG-EN", "dating")), "SG-EN", "one country can carry several languages");
    assert_eq!(market_of("topic:ai app"), "");
    assert_eq!((geo("SG-TW"), geo("JP"), geo("")), ("SG", "JP", "US"));
    assert_eq!(location(&cfg(), "JP"), (2392, "ja".into()));
    assert_eq!(location(&cfg(), ""), (2840, "en".into()));
}

#[test]
fn non_latin_topics_get_distinct_files_and_the_default_market_keeps_its_old_names() {
    assert_eq!(topic_file("", "ai app"), "topic_ai-app");
    let (jp_love, th_love) = (topic_file("JP", "恋愛"), topic_file("TH", "ความรัก"));
    assert!(jp_love.starts_with("topic_jp_") && th_love.starts_with("topic_th_"));
    assert_ne!(file_key("恋愛"), file_key("연애"), "two scripts must not share one file");
    assert_eq!(file_key("chatgpt 使い方"), format!("chatgpt-{}", file_key("chatgpt 使い方").rsplit('-').next().unwrap()));
    assert_ne!(topic_file("JP", "ai"), topic_file("", "ai"));
}

#[test]
fn market_risers_carry_their_market_and_its_own_cap() {
    let lens = Lens { seeds: vec![], targets: vec![], brands: vec![] };
    let sources = vec![
        ("topic:ai".to_string(), vec![("nano banana".to_string(), 5000.0)]),
        ("topic:jp:恋愛".to_string(), vec![("無痛 恋愛".to_string(), 300.0), ("平成 恋愛 展".to_string(), 200.0), ("乗り換え 恋愛 3".to_string(), 950.0)]),
    ];
    let (rises, _) = classify(&sources, &BTreeMap::new(), &lens, &cfg());
    let jp: Vec<&str> = rises.iter().filter(|r| r.market == "JP").map(|r| r.query.as_str()).collect();
    assert_eq!(jp, vec!["乗り換え 恋愛 3", "無痛 恋愛"], "JP keeps its own cap of 2, highest first");
    assert!(rises.iter().all(|r| r.kind == UNKNOWN), "local-language words wait for a judgment");
    assert_eq!(rises.iter().find(|r| r.query == "nano banana").map(|r| r.market.as_str()), Some(""));
}

#[test]
fn volume_keys_keep_markets_apart_and_thai_marks_survive_the_ads_cleanup() {
    assert_eq!(volume_key("", "ai"), "ai");
    assert_eq!(volume_key("JP", "ai"), "jp:ai");
    assert_eq!(ads_keyword("ความ สัมพันธ์ toxic คือ").as_deref(), Some("ความ สัมพันธ์ toxic คือ"));
    assert_eq!(ads_keyword("恋愛 相談").as_deref(), Some("恋愛 相談"));
}
