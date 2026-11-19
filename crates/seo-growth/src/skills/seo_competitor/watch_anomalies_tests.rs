use super::*;
use crate::skills::seo_competitor::watch_bing::WordRank;
use crate::skills::seo_competitor::watch_config::Inspection;
use crate::skills::seo_competitor::watch_rank::RankRow;
use crate::skills::seo_competitor::watch_rank_history::Line;
use crate::skills::seo_competitor::watch_segments::{BatchRow, SegmentRow};
use crate::skills::seo_competitor::watch_machine_queries::MachineQueryConfig;
use serde_json::json;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 11).unwrap()
}

#[test]
fn catches_page1_low_ctr_and_ignores_canary_urls() {
    let pages = json!({"rows": [
        {"keys": ["https://example.com/ai-relationship-coach"], "impressions": 465.0, "clicks": 2.0, "position": 9.4},
        {"keys": ["https://canary.example.com/ai-relationship-coach"], "impressions": 300.0, "clicks": 0.0, "position": 4.0},
        {"keys": ["https://example.com/ai-voice-recorder"], "impressions": 95.0, "clicks": 0.0, "position": 24.0}
    ]});
    let got = page_ctr_anomalies(&pages, &[], &MachineQueryConfig::default(), "https://example.com");
    let keys: Vec<&str> = got.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(keys, ["anomaly:page1-ctr:/ai-relationship-coach", "anomaly:zero-click:/ai-voice-recorder"]);
}

#[test]
fn detects_keyword_cannibalization_across_distinct_base_pages_not_locales() {
    let rows = vec![
        QueryRow { query: "analyze this text".into(), page: "https://example.com/text-message-analyzer".into(), impressions: 18.0, position: 8.0, clicks: 1.0 },
        QueryRow { query: "analyze this text".into(), page: "https://example.com/zh-TW/text-message-analyzer".into(), impressions: 12.0, position: 6.0, clicks: 1.0 },
        QueryRow { query: "help me reply to this message".into(), page: "https://example.com/text-message-analyzer".into(), impressions: 11.0, position: 14.0, clicks: 0.0 },
        QueryRow { query: "help me reply to this message".into(), page: "https://example.com/ai-email-reply".into(), impressions: 9.0, position: 19.0, clicks: 0.0 },
    ];
    let got = cannibal_anomalies(&rows, "https://example.com");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].key, "anomaly:cannibal:en:help me reply to this message");
    assert!(got[0].current.contains("/text-message-analyzer") && got[0].current.contains("/ai-email-reply"));
}

#[test]
fn detects_rank_drop_serp_divergence_and_gap_regression() {
    let ranks = Ranks {
        rows: vec![
            RankRow { keyword: "personal ai".into(), page: "/personal-ai-assistant".into(), rank: 16.0, previous: Some(6.0), serp: Some(16.0), leader: "pi.ai".into(), ..Default::default() },
            RankRow { keyword: "relationship insight".into(), page: "/ai-relationship-coach".into(), rank: 3.8, gsc_position: 3.8, impressions: 22.0, serp: Some(101.0), ..Default::default() },
        ],
        history: vec![
            Line { day: "2026-11-10".into(), keyword: "personal ai".into(), page: "/personal-ai-assistant".into(), rank: 6.0, serp: Some(6.0), gsc_position: 6.0, impressions: 20.0, volume: Some(1000.0), measured: true },
            Line { day: "2026-11-11".into(), keyword: "personal ai".into(), page: "/personal-ai-assistant".into(), rank: 16.0, serp: Some(16.0), gsc_position: 6.0, impressions: 20.0, volume: Some(1000.0), measured: true },
        ],
    };
    let got = rank_anomalies(Some(&ranks), day());
    let keys: Vec<&str> = got.iter().map(|a| a.key.as_str()).collect();
    assert!(keys.contains(&"anomaly:gap-regress"));
    assert!(keys.contains(&"anomaly:rank-drop:personal ai"));
    assert!(keys.contains(&"anomaly:serp-gsc:relationship insight"));
}

#[test]
fn a_word_entering_the_serp_depth_after_a_search_console_fallback_is_not_a_drop() {
    let line = |day: &str, rank: f64, serp: f64, gsc: f64| Line {
        day: day.into(), keyword: "relationship insight".into(), page: "/ai-relationship-coach".into(), rank, serp: Some(serp),
        gsc_position: gsc, impressions: 21.0, volume: None, measured: true,
    };
    let ranks = Ranks {
        rows: vec![RankRow { keyword: "relationship insight".into(), page: "/ai-relationship-coach".into(), rank: 24.0, previous: Some(4.1),
            serp: Some(24.0), gsc_position: 5.1, impressions: 22.0, ..Default::default() }],
        history: vec![line("2026-11-10", 4.1, 101.0, 4.1), line("2026-11-11", 24.0, 24.0, 5.1)],
    };
    let keys: Vec<String> = rank_anomalies(Some(&ranks), day()).into_iter().map(|a| a.key).collect();
    assert!(!keys.iter().any(|k| k.starts_with("anomaly:rank-drop")), "{keys:?}");
}

#[test]
fn an_account_created_after_a_start_click_counts_as_a_sign_up_without_a_code_event() {
    let row = |segment: &str, visitors: f64, cta: f64, signups: f64| SegmentRow {
        window_hours: 24, offset_hours: 0, source: "$direct".into(), segment: segment.into(), visitors, cta, signups, senders: 0.0,
    };
    let oauth = Traffic { segments: vec![row("新访客", 30.0, 10.0, 0.0), row("新注册", 4.0, 4.0, 2.0)], ..Default::default() };
    assert!(!traffic_anomalies(Some(&oauth)).iter().any(|a| a.key == "anomaly:funnel-leak"), "4 of 14 signed up, 2 of them by OAuth");
    let leak = Traffic { segments: vec![row("新访客", 30.0, 12.0, 0.0), row("新注册", 2.0, 2.0, 2.0)], ..Default::default() };
    assert!(traffic_anomalies(Some(&leak)).iter().any(|a| a.key == "anomaly:funnel-leak"), "2 of 14 is under a quarter");
}

#[test]
fn detects_canonical_conflict_funnel_leak_bot_batch_and_engine_gap() {
    let mut state = WatchState::default();
    state.inspections.insert("https://example.com/wiki/wiki-chat-workspace".into(), Inspection {
        coverage: "Duplicate, Google chose different canonical than user".into(),
        google_canonical: "https://example.com/wiki".into(),
        ..Default::default()
    });
    let traffic = Traffic {
        segments: vec![
            SegmentRow { window_hours: 24, offset_hours: 0, source: "$direct".into(), segment: "新访客".into(), visitors: 26.0, cta: 15.0, signups: 0.0, senders: 0.0 },
            SegmentRow { window_hours: 24, offset_hours: 24, source: "$direct".into(), segment: "新访客".into(), visitors: 62.0, cta: 10.0, signups: 3.0, senders: 2.0 },
        ],
        batches: vec![BatchRow {
            os: "Linux".into(), browser: "HeadlessChrome".into(), timezone: "UTC".into(),
            visitors: 7.0, pageview_only: 7.0, first_seen: "11-10 01:00".into(), last_seen: "11-10 01:02".into(),
            ips: "149.57.*".into(), landings: "/intro".into(),
        }],
        ..Default::default()
    };
    let mut bing = BingView::default();
    bing.ranks.insert("personal ai".into(), WordRank { page: "/personal-ai-assistant".into(), position: None, url: None, at: "2026-11-11".into() });
    let ranks = Ranks {
        rows: vec![RankRow { keyword: "personal ai".into(), page: "/personal-ai-assistant".into(), rank: 6.0, serp: Some(6.0), ..Default::default() }],
        history: Vec::new(),
    };
    let urls = vec!["https://example.com/wiki/wiki-chat-workspace".to_string()];
    let all = detect(&AnomalyInput {
        urls: &urls, state: &state, gsc_pages: &json!({}), query_rows: &[], machine_queries: &MachineQueryConfig::default(),
        ranks: Some(&ranks), traffic: Some(&traffic), bing: Some(&bing),
        last: &Window { impressions: 90.0, clicks: 2.0 }, previous: &Window { impressions: 65.0, clicks: 2.0 },
        site: "https://example.com", today: day(),
    });
    let keys: Vec<&str> = all.iter().map(|a| a.key.as_str()).collect();
    for expected in ["anomaly:canonical:/wiki/wiki-chat-workspace", "anomaly:funnel-leak", "anomaly:traffic-drop24h", "anomaly:bot-batch:Linux:HeadlessChrome:UTC", "anomaly:engine-gap", "anomaly:ctr-decouple"] {
        assert!(keys.contains(&expected), "missing {expected} in {keys:?}");
    }
    let md = section(&all);
    assert!(md.contains("xychart-beta") && md.contains("假设 B（反方证伪视角）"));
    let emitted = findings(&all, &[]);
    assert!(emitted.iter().any(|f| f.key == "anomaly:funnel-leak" && f.severity == "P1"));
}

fn q(query: &str, page: &str, impressions: f64, clicks: f64, position: f64) -> QueryRow {
    QueryRow { query: query.into(), page: page.into(), clicks, impressions, position }
}

#[test]
fn a_page_dominated_by_machine_phrasings_is_judged_on_its_human_queries() {
    let coach = "https://example.com/ai-relationship-coach";
    let pages = json!({"rows": [{"keys": [coach], "impressions": 476.0, "clicks": 2.0, "position": 10.1}]});
    let rows = vec![
        q("relationship advice and interpretation assistant", coach, 14.0, 0.0, 5.4),
        q("relationship insight and interpretation assistant", coach, 34.0, 0.0, 6.4),
        q("ai relationship coach", coach, 23.0, 0.0, 23.0),
        q("relationship coach app", coach, 45.0, 1.0, 34.0),
    ];
    let got = page_ctr_anomalies(&pages, &rows, &MachineQueryConfig::default(), "https://example.com");
    let keys: Vec<&str> = got.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(keys, ["anomaly:machine-queries:/ai-relationship-coach"], "human rows rank about 30, so no Page-1 P1");
    assert_eq!(got[0].severity, "P2");
}

#[test]
fn an_anonymized_page_without_machine_phrasings_still_raises_page1_ctr() {
    let page = "https://example.com/personal-ai-assistant";
    let pages = json!({"rows": [{"keys": [page], "impressions": 69.0, "clicks": 0.0, "position": 6.0}]});
    let rows = vec![q("personal ai", page, 3.0, 0.0, 6.0), q("ai personal assistant", page, 2.0, 0.0, 9.0)];
    let got = page_ctr_anomalies(&pages, &rows, &MachineQueryConfig::default(), "https://example.com");
    let keys: Vec<&str> = got.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(keys, ["anomaly:page1-ctr:/personal-ai-assistant"]);
}

#[test]
fn a_small_machine_share_keeps_the_page_row() {
    let page = "https://example.com/text-message-analyzer";
    let pages = json!({"rows": [{"keys": [page], "impressions": 200.0, "clicks": 0.0, "position": 8.0}]});
    let rows = vec![q("text message analyzer", page, 90.0, 0.0, 8.0), q("relationship and message interpretation assistant", page, 10.0, 0.0, 6.0)];
    let got = page_ctr_anomalies(&pages, &rows, &MachineQueryConfig::default(), "https://example.com");
    assert_eq!(got.iter().map(|a| a.key.as_str()).collect::<Vec<_>>(), ["anomaly:page1-ctr:/text-message-analyzer"]);
}
