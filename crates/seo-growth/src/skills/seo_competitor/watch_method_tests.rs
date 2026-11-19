use super::*;
use super::super::watch_discover::Candidate;
use super::super::watch_judge::WATCH;

fn cfg() -> DiscoveryConfig {
    serde_json::from_value(json!({
        "hourSgt": 8, "locationCode": 2840, "languageCode": "en", "domain": "example.com", "registry": "r.json",
        "seeds": [{"keyword": "text message analyzer", "page": "/text-message-analyzer", "fit": ["text", "message", "texting"]}],
        "extraBrands": [], "blocked": [], "minVolume": 50, "maxKd": 30, "dropKd": 40, "minWinRate": 0.45,
        "serpPerDay": 15, "maxPerDay": 3, "maxNewPagesPerDay": 3,
        "method": {"reference": {"keyword": "how to get your ex back", "volume": 5400}, "trendsPerDay": 4, "loopSites": 5,
            "domainChecksPerDay": 3, "maxFreshDomains": 5}
    })).unwrap()
}

fn word(keyword: &str, source: &str, volume: f64, kd: Option<f64>) -> Candidate {
    Candidate { keyword: keyword.into(), source: source.into(), volume, kd, intent: "informational".into(), ..Default::default() }
}

fn graph(points: Vec<serde_json::Value>) -> String { json!([{"items": [{"type": "google_trends_graph", "data": points}]}]).to_string() }

#[test]
fn a_zero_volume_trends_word_is_born_sized_and_done() {
    let c = cfg();
    let m = c.method.clone().unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
    let dir = std::env::temp_dir().join("seo-method-probe-test");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.to_str().unwrap();
    // 5 years of weekly zeros, then three weeks of searches: born the week of 2026-10-20.
    let weeks: Vec<_> = (0..260).map(|w| {
        let d = today - chrono::Duration::days(7 * (260 - w));
        json!({"date_from": d.to_string(), "values": [if w >= 257 { 60 } else { 0 }]})
    }).collect();
    std::fs::write(format!("{dir}/trend5y_vibe-texting.json"), graph(weeks)).unwrap();
    // 30 days against the reference: quiet until 2026-10-21, born 2026-10-22, then a third of the reference.
    let days: Vec<_> = (0..30).map(|n| {
        let d = today - chrono::Duration::days(30 - n);
        json!({"date_from": d.to_string(), "values": [if n >= 11 { 30 } else { 0 }, 90]})
    }).collect();
    std::fs::write(format!("{dir}/trend30_vibe-texting.json"), graph(days)).unwrap();
    let mut judged = vec![prejudge(&word("vibe texting", "trends:text message analyzer", 0.0, None), &c, &[])];
    assert_eq!(judged[0].verdict, WATCH, "0 searches before the probe");
    let mut errors = Vec::new();
    probe(&mut judged, &c, &m, &[], dir, today, &mut errors);
    let got = &judged[0];
    assert_eq!((got.candidate.age_days, got.candidate.trends_volume, got.candidate.seasonal), (Some(19), 1800.0, 0), "{errors:?}");
    assert_eq!(got.verdict, DO, "{}", got.reason);
    assert!(got.reason.contains("19 天前出现") && got.reason.contains("约 1800"), "{}", got.reason);
}

#[test]
fn probes_split_between_early_words_and_the_best_do_words() {
    let c = cfg();
    let judged: Vec<Judged> = [("text a", "trends:x", 0.0, "观察"), ("text b", "related:x", 900.0, DO), ("text c", "site-loop:y", 10.0, "观察"),
        ("text d", "related:x", 300.0, DO), ("text e", "gsc", 0.0, "观察"), ("text f", "anomaly:/p", 0.0, DROP)]
        .iter().map(|(k, s, v, verdict)| Judged { candidate: word(k, s, *v, Some(10.0)), verdict: verdict.to_string(), ..Default::default() }).collect();
    assert_eq!(probe_picks(&judged, &c, 4), [0, 2, 1, 3]);
    // Shape of the 2026-11-10 08:00 round: an 11-word GSC query became an empty Trends keyword.
    let long = Judged { candidate: word("best crm for startups for capturing signals from linkedin and email and calls", "trends:x", 0.0, Some(10.0)),
        verdict: DO.into(), ..Default::default() };
    assert!(probe_picks(&[long], &c, 4).is_empty());
}

#[test]
fn registered_domains_hurry_a_new_word_but_never_veto_it() {
    let mut j = Judged { verdict: DO.into(), objection: "未发现明显弱点".into(), ..Default::default() };
    close_window(&mut j, 2, 5);
    assert_eq!((j.verdict.as_str(), j.fresh_domains), (DO, Some(2)));
    assert!(j.objection.contains("尽快上线"));
    close_window(&mut j, 7, 5);
    assert_eq!(j.verdict, DO, "u7fwqnm12g: a new word is not always a one-week wave");
    assert!(j.objection.contains("窗口正在关闭"));
    let body = whois_call("vibe texting", "2026-10-11", None, "x".into()).body;
    assert_eq!(body["filters"][0][0][2], "%vibetexting%");
    assert_eq!(body["filters"][0][2][2], "%vibe-texting%");
    assert_eq!(body["filters"][2][2], "2026-10-11");
}

#[test]
fn ads_history_older_than_the_trends_birth_wins() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
    assert!(ads_older("2025-10", today, 16), "transcribe voice notes: Ads since 2025-10, Trends said 16 days");
    assert!(!ads_older("2026-02", today, 268), "pocket ai recorder: Ads from 2026-02, Trends birth 2026-02-15");
    assert!(!ads_older("", today, 16), "no Ads history: Trends decides");
    let months = json!({"monthly_searches": [{"year": 2026, "month": 1, "search_volume": 0}, {"year": 2026, "month": 2, "search_volume": 500}, {"year": 2026, "month": 3, "search_volume": 4400}]});
    assert_eq!(super::super::watch_signals::first_month(&months), "2026-02");
}

#[test]
fn a_hot_word_fading_from_its_seven_day_peak_is_measured() {
    let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
    let points = vec![(d("2026-11-07"), 40.0), (d("2026-11-08"), 100.0), (d("2026-11-09"), 30.0), (d("2026-11-09"), 20.0)];
    assert_eq!(fade(&points), 0.25);
    assert_eq!(fade(&[]), 0.0);
    let hours = json!([{"items": [{"type": "google_trends_graph", "data": [{"date_from": "2026-11-09 13:00:00", "values": [7]}]}]}]);
    assert_eq!(super::super::watch_birth::curve(&hours, 0), [(d("2026-11-09"), 7.0)], "an hourly point keeps its day");
    let c = Candidate { fade: 0.25, age_days: Some(3), ..word("vibe texting", "trends:x", 900.0, Some(5.0)) };
    assert!(super::super::watch_signals::notes(&c).contains("7 天峰值的 25%"));
}
