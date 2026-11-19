use super::*;
use crate::skills::seo_competitor::watch_rank_history::{self as history, Line};
use crate::skills::seo_competitor::watch_rank_serp::{RankConfig, RankMarket, parse};
use chrono::NaiveDate;
use serde_json::json;

const SITE: &str = "https://example.com";

fn row(query: &str, page: &str, impressions: f64, position: f64) -> QueryRow {
    QueryRow { query: query.into(), page: page.into(), impressions, position, ..Default::default() }
}

fn target(keyword: &str, page: &str) -> TargetKeyword { TargetKeyword { keyword: keyword.into(), page: page.into() } }

fn rank(keyword: &str, r: f64) -> RankRow {
    RankRow { keyword: keyword.into(), page: "/p".into(), serp: Some(r), rank: r, leader: "a.com（A）".into(), ..Default::default() }
}

#[test]
fn production_target_pages_only_and_exact_queries() {
    // Verified 2026-11-09: the substring rule averaged Canary (80.4) into production (47.0) as 63.7.
    let rows = vec![
        row("text message analyzer", "https://example.com/text-message-analyzer", 5.0, 47.0),
        row("text message analyzer", "https://canary.example.com/text-message-analyzer", 5.0, 80.4),
        row("text message analyzer app", "https://example.com/text-message-analyzer", 9.0, 20.0),
        row("\"Text Message  Analyzer\"", "https://example.com/zh-TW/text-message-analyzer/", 5.0, 47.0),
        row("text message analyzer", "https://example.com/research/x", 9.0, 41.0),
    ];
    let got = gsc_targets(&rows, &[target("text message analyzer", "/text-message-analyzer")], SITE);
    assert_eq!((got[0].impressions, got[0].position), (10.0, 47.0));
}

#[test]
fn locale_prefixes_share_one_page_family() {
    assert_eq!(base_path("/ja/ai-relationship-coach"), "/ai-relationship-coach");
    assert_eq!(base_path("/zh-CN"), "/");
    assert_eq!(base_path("/japan"), "/japan");
    assert_eq!(base_path("/ko/ai-relationship-coach"), "/ai-relationship-coach");
    assert_eq!(base_path("/th"), "/");
    assert_eq!(production_path("https://example.com", SITE), Some("/"));
    assert_eq!(production_path("https://example.com.evil.com/x", SITE), None);
}

#[test]
fn the_serp_rank_wins_and_unmeasured_words_rank_last() {
    assert_eq!(effective_rank(Some(30.0), 18.0), 30.0);
    assert_eq!(effective_rank(None, 18.0), 18.0);
    assert_eq!(effective_rank(None, 0.0), UNRANKED);
    assert_eq!(effective_rank(Some(UNRANKED), 3.8), 3.8, "GSC impressions beat an out-of-depth US SERP");
    assert_eq!(effective_rank(Some(UNRANKED), 0.0), UNRANKED);
}

#[test]
fn the_gap_is_volume_weighted_and_zero_when_all_first() {
    assert_eq!(weighted_gap([(1.0, 500.0), (1.0, 10.0)].into_iter()), 0.0);
    assert_eq!(weighted_gap([(11.0, 90.0), (1.0, 10.0)].into_iter()), 9.0);
}

#[test]
fn every_band_has_its_own_action_key() {
    let rows = vec![rank("top", 1.0), rank("near", 2.0), rank("page", 9.0), rank("deep", 40.0), rank("gone", UNRANKED)];
    let keys: Vec<String> = findings(&rows, "2026-11-09").into_iter().map(|f| f.key).collect();
    assert_eq!(keys, ["rank:absent:gone", "rank:deep:deep", "rank:page", "rank:top3:near"]);
    let unmeasured = RankRow { keyword: "x".into(), rank: UNRANKED, ..Default::default() };
    assert!(findings(&[unmeasured], "2026-11-09").is_empty(), "a failed fetch is not an absent rank");
}

#[test]
fn reaching_and_losing_first_place_are_recorded() {
    let up = RankRow { previous: Some(3.0), ..rank("w", 1.0) };
    let got = findings(&[up], "2026-11-09");
    assert_eq!((got[0].key.as_str(), got[0].severity.as_str()), ("rank:top:w:2026-11-09", "P2"));
    let down = RankRow { previous: Some(1.0), ..rank("w", 2.0) };
    assert_eq!(findings(&[down], "2026-11-09")[0].key, "rank:lost-top:w:2026-11-09");
}

#[test]
fn a_stall_escalates_every_fourteen_days() {
    let stalled = RankRow { stalled_days: 29, ..rank("w", 9.0) };
    let keys: Vec<String> = findings(&[stalled], "2026-11-09").into_iter().map(|f| f.key).collect();
    assert_eq!(keys, ["stall:w:2", "rank:w"]);
}

fn line(day: &str, rank: f64) -> Line {
    Line { day: day.into(), keyword: "w".into(), page: "/p".into(), rank, serp: Some(rank), gsc_position: 0.0, impressions: 0.0, volume: None, measured: true }
}

#[test]
fn stall_days_count_from_the_last_new_best() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 30).unwrap();
    let lines = [line("2026-11-01", 30.0), line("2026-11-10", 12.0), line("2026-11-20", 11.5)];
    let earlier: Vec<&Line> = lines.iter().collect();
    assert_eq!(history::stalled_days(&earlier, 12.0, today), 20, "half a position is not progress");
    assert_eq!(history::stalled_days(&earlier, 10.0, today), 0);
    let mut rows = vec![rank("w", 10.0)];
    history::annotate(&mut rows, &lines, today);
    assert_eq!((rows[0].previous, rows[0].delta7, rows[0].stalled_days), (Some(11.5), Some(1.5), 0));
}

#[test]
fn the_serp_reader_finds_our_breadcrumb_behind_google_redirects() {
    let serp = json!([{"keyword": "ai relationship coach", "items": [
        {"type": "organic", "rank_group": 1, "domain": "google.com", "breadcrumb": "https://www.mosaicchats.com › Research", "title": "Mosaic"},
        {"type": "organic", "rank_group": 30, "domain": "google.com", "breadcrumb": "https://example.com › ai-relationship-coach", "title": "AI"}]}]);
    let got = parse(&serp, "example.com").unwrap();
    assert_eq!((got.position, got.leader.as_str()), (Some(30.0), "mosaicchats.com（Mosaic）"));
    let absent = parse(&json!([{"keyword": "x", "items": []}]), "example.com").unwrap();
    assert_eq!(absent.position, None);
    assert!(parse(&json!({"tasks": []}), "example.com").is_none(), "an error body is no SERP");
}

#[test]
fn a_locale_page_is_ranked_in_its_market() {
    let cfg = RankConfig { serp_depth: 100, location_code: 2840, language_code: "en".into(), max_work_per_tab: 8, markets: vec![
        RankMarket { prefix: "/zh-TW".into(), location_code: 2158, language_code: "zh-TW".into(), ads_language_code: Some("zh_TW".into()) }] };
    assert_eq!(cfg.market("/zh-TW/text-message-analyzer"), (2158, "zh-TW".into(), "zh_TW".into()));
    assert_eq!(cfg.market("/zh-TWx"), (2840, "en".into(), "en".into()));
}
