use super::*;
use super::super::watch_trends_ledger::{Ledger, labels, leads, record};
use super::super::watch_trends_view::{explore_link, findings, section};

fn seed(page: &str, fit: &[&str]) -> Seed {
    Seed { keyword: page.trim_start_matches('/').replace('-', " "), page: page.into(), fit: fit.iter().map(|f| f.to_string()).collect(), trends: true, evergreen: None, core_market: false }
}

fn lens() -> Lens {
    Lens { seeds: vec![seed("/text-message-analyzer", &["text", "texting", "message"]), seed("/ai-meeting-notes", &["meeting", "notes"])],
        targets: vec!["texting helper".into()], brands: vec!["otter".into(), "google".into()] }
}

fn cfg() -> TrendsConfig {
    TrendsConfig { every_hours: 6.0, location_code: 2840, time_range: "past_7_days".into(), topics: vec![Topic { keyword: "ai".into(), area: "AI".into() }],
        min_rise: 100.0, max_per_source: 8, heat_ratio: 1.5, surge: None, follow_up_per_day: None, similarweb: None, markets: vec![] }
}

fn q(list: &[(&str, f64)]) -> Vec<(String, f64)> { list.iter().map(|(k, v)| (k.to_string(), *v)).collect() }

#[test]
fn an_unknown_breakout_name_is_kept_for_judgment_not_dropped_as_off_topic() {
    let sources = vec![
        ("topic:ai".to_string(), q(&[("nano banana", 5000.0), ("what does nnn mean in texting", 1700.0), ("otter ai", 400.0), ("ai stocks", 80.0)])),
        ("seed:personal-crm".to_string(), q(&[("best time to visit maldives", 45000.0), ("mesh crm", 650.0)])),
        ("seed:ai-meeting-notes".to_string(), q(&[("best time to visit maldives", 43000.0), ("zoom ai meeting notes", 120.0)])),
    ];
    let (rises, filler) = classify(&sources, &BTreeMap::new(), &lens(), &cfg());
    let kind = |k: &str| rises.iter().find(|r| r.query == k).map(|r| r.kind.as_str());
    assert_eq!(kind("nano banana"), Some(UNKNOWN), "the nano banana case reaches a judgment");
    assert_eq!(kind("what does nnn mean in texting"), Some(FOCUS));
    assert_eq!(rises.iter().find(|r| r.query == "what does nnn mean in texting").unwrap().page, "/text-message-analyzer");
    assert_eq!(kind("otter ai"), Some(BRAND));
    assert_eq!(kind("ai stocks"), None, "below min_rise");
    assert_eq!((kind("best time to visit maldives"), filler.len()), (None, 1), "filler under two unrelated seeds is noise");
    assert_eq!(rises[0].query, "nano banana", "sorted by rise");
}

#[test]
fn a_query_under_two_sources_merges_with_its_highest_value() {
    let sources = vec![("topic:ai".to_string(), q(&[("keeper ai", 550.0)])), ("topic:dating".to_string(), q(&[("keeper ai", 900.0)]))];
    let (rises, _) = classify(&sources, &BTreeMap::new(), &lens(), &cfg());
    assert_eq!((rises.len(), rises[0].value, rises[0].source.as_str()), (1, 900.0, "topic:ai, topic:dating"));
}

#[test]
fn the_graph_compares_the_last_day_with_the_days_before() {
    let point = |ts: i64, day: &str, v: f64, missing: bool| json!({"timestamp": ts, "date_from": day, "values": [v], "missing_data": missing});
    let mut data: Vec<Value> = (0..48).map(|h| point(h * 3600, if h < 24 { "2026-11-01" } else { "2026-11-02" }, if h < 24 { 20.0 } else { 60.0 }, false)).collect();
    data.push(point(48 * 3600, "2026-11-03", 99.0, true));
    let json = json!([{"items": [{"type": "google_trends_graph", "data": data}]}]);
    let h = heat(&json, &Topic { keyword: "ai".into(), area: "AI".into() }).unwrap();
    assert_eq!((h.last24, h.prior), (60.0, 20.0), "the partial last point is skipped");
    assert_eq!(h.daily, vec![("2026-11-01".to_string(), 20.0), ("2026-11-02".to_string(), 60.0)]);
    let pull = Pull { at: "2026-11-07T10:17:00+08:00".into(), heat: vec![h], ..Default::default() };
    assert!(findings(&pull, &cfg(), &BTreeMap::new())[0].key.starts_with("trend:heat:ai:2026-11-07"));
}

#[test]
fn labels_hide_noise_and_opportunities_become_keyword_leads() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(format!("{state}/trends")).unwrap();
    std::fs::write(format!("{state}/trend-labels.jsonl"), [
        r#"{"query":"Nano Banana","label":"观察","note":"Google image model"}"#,
        r#"{"query":"nano banana","label":"机会","page":"/personal-ai-assistant","note":"Google image model"}"#,
        r#"{"query":"medvi","label":"噪声","note":"telehealth ad"}"#,
    ].join("\n")).unwrap();
    let pull = Pull { at: "2026-11-07T10:17:00+08:00".into(), rising: vec![
        Rise { query: "nano banana".into(), value: 5000.0, source: "topic:ai".into(), kind: UNKNOWN.into(), page: String::new(), ..Default::default() },
        Rise { query: "medvi".into(), value: 9000.0, source: "seed:personal-ai-assistant".into(), kind: UNKNOWN.into(), page: String::new(), ..Default::default() },
        Rise { query: "what does nnn mean in texting".into(), value: 1700.0, source: "topic:texting".into(), kind: FOCUS.into(), page: "/text-message-analyzer".into(), ..Default::default() },
    ], ..Default::default() };
    std::fs::write(format!("{state}/trends/latest.json"), serde_json::to_string(&pull).unwrap()).unwrap();
    let judged = labels(&state);
    assert_eq!(judged["nano banana"].label, OPPORTUNITY, "the latest line wins");
    let md = section(Some(&pull), &cfg(), &Ledger::new(), &judged);
    assert!(md.contains("| nano banana | 未知新词 | — | +5000%（Breakout） |") && !md.contains("| medvi |"));
    let waiting = findings(&pull, &cfg(), &judged);
    assert!(waiting.len() == 1 && waiting[0].title.starts_with("1 个异常上升词待判断：what does nnn mean in texting"), "a topic focus word needs a judgment too");
    assert_eq!(leads(&state), vec![("nano banana".to_string(), "anomaly:/personal-ai-assistant".to_string())],
        "a topic focus riser waits for a judgment like an unknown word");
}

#[test]
fn the_ledger_keeps_the_first_sighting_and_the_peak() {
    let mut known = Ledger::new();
    let rise = |v: f64, s: &str| Rise { query: "keeper ai".into(), value: v, source: s.into(), kind: UNKNOWN.into(), page: String::new(), ..Default::default() };
    record(&mut known, &Pull { at: "a".into(), rising: vec![rise(550.0, "topic:ai")], ..Default::default() });
    record(&mut known, &Pull { at: "b".into(), rising: vec![rise(300.0, "topic:ai, topic:dating")], ..Default::default() });
    let s = &known["keeper ai"];
    assert_eq!((s.first_seen.as_str(), s.last_seen.as_str(), s.peak, s.pulls, s.sources.len()), ("a", "b", 550.0, 2, 2));
    assert_eq!(explore_link("what's up?", "US"), "[看趋势](https://trends.google.com/trends/explore?date=now%207-d&geo=US&q=what%27s%20up%3F)");
}

#[test]
fn filler_under_one_seed_today_is_caught_by_the_last_7_days_of_seed_pulls() {
    let mut l = lens();
    l.seeds.push(seed("/win-him-back", &["him", "attract"]));
    let sources = vec![("seed:ai-meeting-notes".to_string(), q(&[("best time to visit maldives", 45100.0), ("top tourist attractions in paris", 4450.0), ("zoom ai meeting notes", 300.0)]))];
    let (rises, filler) = classify(&sources, &BTreeMap::new(), &l, &cfg());
    assert_eq!((rises.len(), filler.len()), (3, 0), "one seed today is not enough");
    let week: BTreeMap<String, BTreeSet<String>> = [("best time to visit maldives", "personal-crm"), ("top tourist attractions in paris", "personal-crm"), ("zoom ai meeting notes", "ai-voice-recorder")]
        .iter().map(|(k, s)| (k.to_string(), BTreeSet::from([s.to_string()]))).collect();
    let (rises, filler) = classify(&sources, &week, &l, &cfg());
    assert_eq!(filler, vec!["best time to visit maldives".to_string(), "top tourist attractions in paris".to_string()],
        "a stem hit on a seed it never appeared under (attract-ions) does not save filler");
    assert_eq!(rises.iter().map(|r| r.query.as_str()).collect::<Vec<_>>(), vec!["zoom ai meeting notes"], "it fits a seed it appeared under");
}

#[test]
fn seed_history_reads_every_seed_pull_of_the_week() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let pull = |q: &str| json!([{"items": [{"type": "google_trends_queries_list", "data": {"rising": [{"query": q, "value": 900}]}}]}]).to_string();
    for (day, seed, query) in [("2026-11-04", "personal-crm", "how to make sushi"), ("2026-11-07", "ai-meeting-notes", "How To Make Sushi")] {
        std::fs::create_dir_all(format!("{root}/{day}")).unwrap();
        std::fs::write(format!("{root}/{day}/trends_{seed}.json"), pull(query)).unwrap();
    }
    let history = seed_history(&[format!("{root}/2026-11-04"), format!("{root}/2026-11-07")]);
    assert_eq!(history["how to make sushi"], BTreeSet::from(["ai-meeting-notes".to_string(), "personal-crm".to_string()]));
}
