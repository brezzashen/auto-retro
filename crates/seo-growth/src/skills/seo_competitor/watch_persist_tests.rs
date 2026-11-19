use super::*;
use crate::skills::seo_competitor::watch_signals::{fresh, weight};

/// Weekly points ending 2026-11-01, four per monthly mean, oldest month first.
fn weekly(months: &[f64]) -> Vec<(NaiveDate, f64)> {
    let end = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let n = months.len() * 4;
    (0..n).map(|i| (end - Duration::weeks((n - 1 - i) as i64), months[i / 4])).collect()
}

fn word(age: i64, rising: u32) -> Candidate {
    Candidate { keyword: "vibe texting".into(), volume: 900.0, kd: Some(10.0), age_days: Some(age), rising_months: rising, ..Default::default() }
}

fn picked(age: i64, rising: u32) -> Judged {
    Judged { candidate: word(age, rising), verdict: DO.into(), reason: "月搜 900".into(), win_rate: Some(0.6), ..Default::default() }
}

#[test]
fn rising_months_count_back_from_the_latest_block() {
    assert_eq!(rising_months(&weekly(&[0.0, 0.0, 10.0, 20.0, 35.0, 60.0])), 4, "four rises in a row, the first from nothing");
    assert_eq!(rising_months(&weekly(&[5.0, 80.0, 30.0, 20.0, 22.0])), 1, "a spike that fell back, then one small rise");
    assert_eq!(rising_months(&weekly(&[0.0, 0.0, 90.0, 40.0])), 0, "the latest month is below the one before");
    assert_eq!(rising_months(&weekly(&[10.0])), 0, "one block has nothing to compare");
    let mut ragged = weekly(&[10.0, 20.0, 30.0, 40.0]);
    ragged.remove(0);
    assert_eq!(rising_months(&ragged), 2, "an incomplete oldest block is left out");
}

#[test]
fn a_word_older_than_30_days_keeps_its_rising_credit_only_after_three_rising_months() {
    assert!(fresh(&word(200, 3)) && weight(&word(200, 3)) == 1.3, "sustained demand stays a new word");
    assert!(!fresh(&word(200, 2)) && weight(&word(200, 2)) == 1.0);
    assert!(past_window(&word(31, 0)) && !past_window(&word(30, 0)), "the first 30 days need no history");
    assert!(!past_window(&word(400, 0)), "older than a year was never a new word");
    assert!(!past_window(&Candidate { seasonal: 2, ..word(200, 0) }), "a seasonal word has its own rule");
    assert_eq!(note(&word(200, 2)), "；Trends 200 天前出现，只连续上升 2 个月（需 3 个月），不计新词加权");
    assert_eq!(note(&word(200, 3)), "");
}

#[test]
fn a_hot_word_past_its_window_is_dropped_when_a_competitor_domain_is_over_20_days_old() {
    let mut hot = picked(45, 1);
    give_up(&mut hot, 2);
    assert_eq!(hot.verdict, DROP);
    assert!(hot.reason.starts_with("群友规则（张瑜 2026-03-10 比赛复盘，非哥飞本人）") && hot.reason.contains("出现 45 天") && hot.reason.contains("2 个含该词的域名注册超过 20 天"));
    let mut open = picked(45, 1);
    give_up(&mut open, 0);
    assert_eq!(open.verdict, DO, "no competitor domain, no drop");
    let mut lasting = picked(200, 3);
    give_up(&mut lasting, 5);
    assert_eq!(lasting.verdict, DO, "three rising months are lasting demand, not a hot word");
    let mut newborn = picked(12, 0);
    give_up(&mut newborn, 5);
    assert_eq!(newborn.verdict, DO, "inside 30 days the window is still open (G6 only adds a note)");
}

#[test]
fn the_old_domain_count_asks_for_registrations_between_the_birth_and_20_days_ago() {
    let call = whois_call("vibe texting", "2026-08-27", Some("2026-10-21"), "/tmp/w.json".into());
    let filters = &call.body["filters"];
    assert_eq!((&filters[0][0], &filters[0][2]), (&serde_json::json!(["domain", "like", "vibetexting.%"]), &serde_json::json!(["domain", "like", "vibe-texting.%"])),
        "a drop counts only a domain named exactly after the word");
    assert_eq!(filters[2], serde_json::json!([["created_datetime", ">", "2026-08-27"], "and", ["created_datetime", "<", "2026-10-21"]]));
    let fresh = whois_call("vibe texting", "2026-10-11", None, "/tmp/w.json".into());
    assert_eq!((&fresh.body["filters"][2], &fresh.body["filters"][0][0][2]), (&serde_json::json!(["created_datetime", ">", "2026-10-11"]), &serde_json::json!("%vibetexting%")), "the G6 call is unchanged");
}
