use super::*;

/// Singapore wall time `day`-th day since 1970-01-01 at `minutes` after midnight, as UTC ms.
fn at(day: i64, minutes: f64) -> f64 {
    day as f64 * DAY_MS + minutes * 60_000.0 - SINGAPORE_OFFSET_MS
}

/// 2026-10-26 is a Monday.
const MONDAY: i64 = 20_752;

fn record(period: &str, day: i64, at_ms: f64, ok: bool) -> Value {
    json!({"period": period, "day": day, "at_ms": at_ms, "ok": ok})
}

#[test]
fn singapore_days_and_mondays() {
    assert!(is_monday(MONDAY));
    assert!(!is_monday(MONDAY + 1));
    assert_eq!(singapore(at(MONDAY, 9.0 * 60.0)), (MONDAY, 9.0 * 60.0));
    // 23:30 UTC on Sunday is already Monday 07:30 in Singapore.
    assert_eq!(singapore(MONDAY as f64 * DAY_MS - 30.0 * 60_000.0).0, MONDAY);
}

#[test]
fn daily_runs_once_from_nine() {
    let mut watch = QuotaWatch::default();
    assert_eq!(watch.due(at(MONDAY + 1, 8.0 * 60.0)), None);
    assert_eq!(watch.due(at(MONDAY + 1, 9.0 * 60.0)), Some("daily"));
    watch.runs.push(record("daily", MONDAY + 1, at(MONDAY + 1, 9.0 * 60.0), true));
    assert_eq!(watch.due(at(MONDAY + 1, 20.0 * 60.0)), None);
    assert_eq!(watch.due(at(MONDAY + 2, 9.0 * 60.0)), Some("daily"));
}

#[test]
fn weekly_follows_the_monday_daily_run() {
    let mut watch = QuotaWatch::default();
    assert_eq!(watch.due(at(MONDAY, 10.0 * 60.0)), Some("daily"));
    watch.runs.push(record("daily", MONDAY, at(MONDAY, 10.0 * 60.0), true));
    assert_eq!(watch.due(at(MONDAY, 10.0 * 60.0 + 5.0)), Some("weekly"));
    watch.runs.push(record("weekly", MONDAY, at(MONDAY, 10.0 * 60.0 + 5.0), true));
    assert_eq!(watch.due(at(MONDAY, 12.0 * 60.0)), None);
}

#[test]
fn failures_retry_hourly_up_to_three_times() {
    let mut watch = QuotaWatch::default();
    let first = at(MONDAY + 1, 9.0 * 60.0);
    watch.runs.push(record("daily", MONDAY + 1, first, false));
    assert_eq!(watch.due(first + 30.0 * 60_000.0), None);
    assert_eq!(watch.due(first + 60.0 * 60_000.0), Some("daily"));
    watch.runs.push(record("daily", MONDAY + 1, first + 3_600_000.0, false));
    watch.runs.push(record("daily", MONDAY + 1, first + 7_200_000.0, false));
    assert_eq!(watch.due(first + 5.0 * 3_600_000.0), None);
}

#[test]
fn receipt_is_the_last_json_line() {
    assert_eq!(receipt("$ bun scripts/quotaEconomics/cli.ts\n{\"ok\":true,\"url\":\"u\"}\n")["url"], "u");
    assert!(receipt("no json here").is_null());
}
