use super::*;

fn cfg() -> SurgeConfig { SurgeConfig { min_ratio: 2.0, min_volume: 100.0, new_baseline: 10.0, weekly_top: 20 } }

/// Google Ads `monthly_searches`, newest first as DataForSEO returns them, ending 2026-09.
fn ads(volumes: &[f64]) -> Value {
    let list: Vec<Value> = volumes.iter().enumerate().map(|(i, v)| {
        let back = i as i64;
        let (year, month) = if back < 9 { (2026, 9 - back) } else { (2025, 21 - back) };
        json!({"year": year, "month": month, "search_volume": v})
    }).collect();
    json!({"keyword": "x", "monthly_searches": list})
}

/// A `past_12_months` graph: complete weeks, then the partial current week.
fn graph(weeks: &[Option<f64>]) -> Value {
    let mut data: Vec<Value> = weeks.iter().map(|v| json!({"values": [v], "missing_data": false})).collect();
    data.push(json!({"values": [99], "missing_data": true}));
    json!([{"items": [{"type": "google_trends_graph", "data": data}]}])
}

fn weeks(base: f64, recent: [f64; 2], first: [f64; 2]) -> Vec<Option<f64>> {
    let mut w = vec![Some(first[0]), Some(first[1])];
    w.extend(std::iter::repeat_n(Some(base), 46));
    w.extend([Some(base), Some(base), Some(recent[0]), Some(recent[1])]);
    w
}

#[test]
fn surge_is_the_latest_month_over_the_mean_of_the_eleven_before() {
    // instinct ai, 2026-11-07 Google Ads (US): 165000 in September after a year near 50-390.
    let m = monthly(&ads(&[165000.0, 33100.0, 390.0, 70.0, 50.0, 70.0, 70.0, 70.0, 50.0, 50.0, 110.0, 260.0])).unwrap();
    assert_eq!(m.month, "2026-09");
    assert!((m.base - 3117.27).abs() < 0.01, "{}", m.base);
    assert!((ratio(m.latest, m.base) - 52.9).abs() < 0.1);
    assert_eq!(flag(Some(&m), None, &cfg()), RISING);
    // A list longer than 12 months still compares with the 11 months right before the latest.
    let mut long = ads(&[300.0; 12]);
    long["monthly_searches"].as_array_mut().unwrap().push(json!({"year": 2024, "month": 9, "search_volume": 1_000_000}));
    assert_eq!(monthly(&long).unwrap().base, 300.0);
}

#[test]
fn monthly_flags_new_rising_and_respect_the_volume_floor() {
    let new = monthly(&ads(&[320.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])).unwrap();
    assert_eq!(flag(Some(&new), None, &cfg()), NEW, "a word with almost no history");
    let small = monthly(&ads(&[90.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])).unwrap();
    assert_eq!(flag(Some(&small), None, &cfg()), "", "under minVolume never flags");
    // shop lc: 90500 against about 97000 a month, a steady brand.
    let flat = monthly(&ads(&[90500.0, 90500.0, 90500.0, 90500.0, 110000.0, 90500.0, 90500.0, 90500.0, 110000.0, 110000.0, 110000.0, 110000.0])).unwrap();
    assert!((ratio(flat.latest, flat.base) - 0.91).abs() < 0.01);
    assert_eq!(flag(Some(&flat), None, &cfg()), "");
    assert_eq!(monthly(&json!({"keyword": "x", "monthly_searches": null})), None, "no data, no flag");
}

#[test]
fn weekly_acceleration_skips_the_partial_week_and_counts_gaps_as_zero() {
    let mut w = weeks(10.0, [40.0, 60.0], [10.0, 10.0]);
    w[5] = None;
    let got = weekly(&graph(&w)).unwrap();
    assert_eq!(got.recent, 50.0, "the partial week (99) is skipped");
    assert!((got.base - 480.0 / 48.0 + 10.0 / 48.0).abs() < 1e-9, "the null week counts as 0");
    assert_eq!(got.year_ago, 10.0);
    assert_eq!(flag(None, Some(&got), &cfg()), RISING);
    assert_eq!(weekly(&graph(&[Some(1.0); 5])), None, "too short to compare");
}

#[test]
fn the_weekly_curve_overrides_the_lagging_month() {
    let month_up = Monthly { month: "2026-09".into(), latest: 320.0, base: 160.0 };
    // elixamd: September still doubled, but the spike ended in October.
    let faded = Weekly { recent: 0.5, base: 8.0, year_ago: 0.0, current: 0.0 };
    assert_eq!(flag(Some(&month_up), Some(&faded), &cfg()), FADED);
    // what does nnn mean: No Nut November, the same week a year ago was far higher.
    let nnn = Weekly { recent: 13.0, base: 6.0, year_ago: 35.0, current: 20.0 };
    assert_eq!(flag(None, Some(&nnn), &cfg()), SEASONAL);
    // A name that was flat zero for a year.
    let fresh = Weekly { recent: 80.0, base: 0.4, year_ago: 0.0, current: 70.0 };
    assert_eq!(flag(Some(&month_up), Some(&fresh), &cfg()), NEW);
    let flat = Weekly { recent: 8.0, base: 7.5, year_ago: 7.0, current: 8.0 };
    assert_eq!(flag(None, Some(&flat), &cfg()), "");
    assert_eq!(flag(Some(&Monthly { month: "2026-09".into(), latest: 50.0, base: 0.0 }), Some(&fresh), &cfg()), "", "the volume floor still applies");
    let rise = |flag: &str, volume: Option<f64>| Rise { flag: flag.into(), volume, ..Default::default() };
    // 11:17 on 2026-11-07: instinct ai (快速上升, 165000 a month) ranks before week-only news words.
    let order = [rise(NEW, Some(320.0)), rise(RISING, Some(165000.0)), rise(RISING, None), rise(NEW, None), rise(SEASONAL, Some(900.0)), rise("", None)];
    assert!(order.windows(2).all(|w| rise_rank(&w[0]) < rise_rank(&w[1]) || (w[0].flag == SEASONAL && rise_rank(&w[0]) == rise_rank(&w[1]))));
}

#[test]
fn ads_keywords_drop_what_google_ads_rejects() {
    assert_eq!(ads_keyword("bill belichick, jordon hudson relationship").as_deref(), Some("bill belichick jordon hudson relationship"));
    assert_eq!(ads_keyword("What's up?").as_deref(), Some("what's up"));
    assert_eq!(ads_keyword("in the santa class, what is the relationship between kate north and dan?"), None, "over 10 words");
    assert_eq!(ads_keyword(" ,? "), None);
}

#[test]
fn a_word_that_exists_only_in_the_partial_week_is_new() {
    // troy aikman joe buck espn relationship, 2026-11-07: every complete week empty, this week 100.
    let mut w: Vec<Option<f64>> = vec![None; 52];
    let mut json = graph(&w);
    json[0]["items"][0]["data"][52]["values"] = json!([100]);
    let news = weekly(&json).unwrap();
    assert_eq!((news.recent, news.base, news.current), (0.0, 0.0, 100.0));
    assert_eq!(ratio(news.recent, news.base), 0.0, "acceleration stays on complete weeks");
    assert_eq!(flag(None, Some(&news), &cfg()), NEW);
    w[51] = Some(30.0);
    let older = weekly(&graph(&w)).unwrap();
    assert_eq!(flag(None, Some(&older), &cfg()), NEW, "a word that started last week is new as well");
}
