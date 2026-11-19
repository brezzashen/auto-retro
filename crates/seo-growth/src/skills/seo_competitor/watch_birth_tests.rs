use super::*;
use serde_json::json;

fn day(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }

/// Weekly points from `start`, one value each.
fn weeks(start: &str, values: &[f64]) -> Vec<(NaiveDate, f64)> {
    values.iter().enumerate().map(|(i, v)| (day(start) + chrono::Duration::days(7 * i as i64), *v)).collect()
}

#[test]
fn a_new_word_is_born_after_its_last_quiet_run() {
    let new = weeks("2026-08-02", &[0.0, 0.0, 0.0, 0.0, 30.0, 60.0, 100.0, 80.0]);
    assert_eq!(birth(&new), Born::At(day("2026-08-30")));
    assert_eq!(birth(&weeks("2026-01-04", &[40.0, 55.0, 61.0, 70.0, 100.0])), Born::Before, "searched from the start");
    assert_eq!(birth(&weeks("2026-01-04", &[0.0; 6])), Born::Never);
    let noise = weeks("2026-01-04", &[0.0, 0.0, 0.0, 100.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 70.0, 0.0, 0.0]);
    assert_eq!(birth(&noise), Born::Before, "scattered spikes are no birth");
}

#[test]
fn the_daily_curve_refines_a_recent_weekly_birth() {
    let today = day("2026-11-10");
    assert_eq!(age_days(Born::At(day("2026-10-25")), Born::At(day("2026-10-28")), today), Some(13));
    assert_eq!(age_days(Born::At(day("2026-03-01")), Born::Before, today), Some(254));
    assert_eq!(age_days(Born::Before, Born::Before, today), None, "older than the 5-year window");
}

#[test]
fn a_word_peaking_in_the_same_month_every_year_is_seasonal() {
    let year = |peak: u32| (1..=12).map(move |m| if m == peak { 100.0 } else { 20.0 });
    let mut points = Vec::new();
    for (y, peak) in [(2022, 2), (2023, 2), (2024, 3), (2025, 2)] {
        for (m, v) in (1..=12).zip(year(peak)) { points.push((NaiveDate::from_ymd_opt(y, m, 1).unwrap(), v)); }
    }
    assert_eq!(seasonal_month(&points), Some(2), "Valentine's-day shape: February ±1 month");
    let flat: Vec<_> = points.iter().map(|(d, _)| (*d, 50.0)).collect();
    assert_eq!(seasonal_month(&flat), None);
    let drift: Vec<_> = points.iter().map(|(d, v)| (*d, if d.year() == 2024 && d.month() == 9 { 300.0 } else { *v })).collect();
    assert_eq!(seasonal_month(&drift), None, "one year peaks in September");
}

#[test]
fn a_zero_volume_word_scales_from_its_reference() {
    // Gefei, dva3ltp2ex: a word at 3x a reference of known volume searches 3x as much.
    let points: Vec<_> = (1..=30).map(|d| json!({"date_from": format!("2026-10-{d:02}"), "missing_data": false, "values": [30, 10]})).collect();
    let graph = json!([{"items": [{"type": "google_trends_graph", "data": points}]}]);
    assert_eq!(scaled_volume(&graph, 5400.0), Some(16200.0));
    assert_eq!(curve(&graph, 0).len(), 30);
    let silent = json!([{"items": [{"type": "google_trends_graph", "data": [{"date_from": "2026-10-01", "values": [5, 0]}]}]}]);
    assert_eq!(scaled_volume(&silent, 5400.0), None);
}

#[test]
fn an_old_word_with_sampling_gaps_is_not_born_again() {
    // "red flags in a guy" (8,100 a month), 5-year curve of 2026-11-10: values with scattered nulls and a
    // run of five nulls 32 weeks before the end; read as zeros it looked born 14 days ago.
    let mut values: Vec<Option<f64>> = (0..220).map(|i| if i % 4 == 1 { None } else { Some(40.0 + (i % 7) as f64 * 5.0) }).collect();
    values.extend([None, None, None, None, None, Some(42.0), Some(32.0), None, Some(46.0), None, None, None, Some(55.0), None, Some(46.0), Some(22.0), Some(26.0), Some(37.0)]);
    let points: Vec<_> = values.iter().enumerate().map(|(i, v)| json!({"date_from": (day("2021-11-07") + chrono::Duration::days(7 * i as i64)).to_string(), "values": [v]})).collect();
    let graph = json!([{"items": [{"type": "google_trends_graph", "data": points}]}]);
    assert_eq!(birth(&curve(&graph, 0)), Born::Before);
}

#[test]
fn rows_where_every_keyword_is_null_are_gaps() {
    let rows = json!([{"items": [{"type": "google_trends_graph", "data": [
        {"date_from": "2026-10-09", "values": [null, null]}, {"date_from": "2026-10-10", "values": [null, 91]},
        {"date_from": "2026-10-11", "values": [25, 78]}]}]}]);
    assert_eq!(curve(&rows, 0), [(day("2026-10-10"), 0.0), (day("2026-10-11"), 25.0)]);
    let single = json!([{"items": [{"type": "google_trends_graph", "data": [{"date_from": "2026-10-09", "values": [null]}]}]}]);
    assert_eq!(curve(&single, 0), [(day("2026-10-09"), 0.0)], "one keyword: null is too few searches");
}
