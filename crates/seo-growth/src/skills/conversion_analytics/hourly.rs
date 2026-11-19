//! The hourly view: production events per hour over 48 hours, and the fast silence rule that turns
//! a broken release into a P0 within `silence_hours` instead of a day.

use super::config::Finding;
use super::posthog::{num, text};
use chrono::{DateTime, Duration, FixedOffset, Timelike};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HourRow {
    /// Start of the Singapore hour (RFC 3339).
    pub at: String,
    pub events: f64,
    pub visitors: f64,
}

pub fn hours(rows: &[Vec<Value>]) -> Vec<HourRow> {
    rows.iter().map(|r| HourRow { at: text(r, 0), events: num(r, 1), visitors: num(r, 2) }).collect()
}

/// One row per Singapore hour for the 48 hours up to `now`, with 0 for an hour PostHog returned no row for.
pub fn fill(rows: Vec<HourRow>, now: DateTime<FixedOffset>) -> Vec<HourRow> {
    let top = now.date_naive().and_hms_opt(now.hour(), 0, 0).and_then(|t| t.and_local_timezone(*now.offset()).single()).unwrap_or(now);
    (0..48).rev().map(|n| {
        let at = top - Duration::hours(n);
        rows.iter().find(|r| DateTime::parse_from_rfc3339(&r.at).is_ok_and(|t| t == at)).cloned()
            .unwrap_or(HourRow { at: at.to_rfc3339(), events: 0.0, visitors: 0.0 })
    }).collect()
}

fn within(
rows: &[HourRow], now: DateTime<FixedOffset>, span: Duration) -> f64 {
    rows.iter().filter(|r| DateTime::parse_from_rfc3339(&r.at).is_ok_and(|t| t + Duration::hours(1) > now - span)).map(|r| r.events).sum()
}

/// P0 when the last `silence_hours` carried no production event although the last 24 h averaged
/// at least `min_hourly` events per hour.
pub fn silence_finding(rows: &[HourRow], now: DateTime<FixedOffset>, silence_hours: i64, min_hourly: f64) -> Option<Finding> {
    let recent = within(rows, now, Duration::hours(silence_hours));
    let day = within(rows, now, Duration::hours(24));
    (recent == 0.0 && day >= min_hourly * 24.0).then(|| Finding::new("health:silent-hours", "P0",
        format!("生产最近 {silence_hours} 小时没有任何事件（前 24 小时共 {day} 个）"),
        "查最近一次 example.com 发布是否去掉了 posthog-js 或改坏了 /ingest，并看浏览器控制台"))
}

pub fn section(rows: &[HourRow]) -> String {
    let mut md = String::from("## 每小时事件\n\n生产最近 48 小时每小时的事件数（新加坡时间，按 uuid 去重）。当前这一小时还没结束。\n\n");
    if rows.len() < 2 { return md + "这段时间的小时数据少于 2 个点。\n\n"; }
    let label = |r: &HourRow| DateTime::parse_from_rfc3339(&r.at).map_or(r.at.clone(), |t| t.format("%m-%d %H").to_string());
    md.push_str(&format!("```mermaid\nxychart-beta\ntitle \"Production events per hour\"\n%% series: Events, Visitors\nx-axis [{}]\ny-axis \"Events\" 0 --> {}\nbar [{}]\nline [{}]\n```\n\n",
        rows.iter().map(|r| format!("\"{}\"", label(r))).collect::<Vec<_>>().join(", "),
        rows.iter().map(|r| r.events).fold(1.0_f64, f64::max).ceil(),
        rows.iter().map(|r| format!("{}", r.events)).collect::<Vec<_>>().join(", "),
        rows.iter().map(|r| format!("{}", r.visitors)).collect::<Vec<_>>().join(", ")));
    md.push_str("柱为事件数，折线为访客数。\n\n");
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(at: &str, events: f64) -> HourRow { HourRow { at: at.into(), events, visitors: 0.0 } }

    #[test]
    fn missing_hours_are_filled_with_zero() {
        let now = DateTime::parse_from_rfc3339("2026-11-04T10:30:00+08:00").unwrap();
        let filled = fill(vec![row("2026-11-04T09:00:00+08:00", 7.0)], now);
        assert_eq!(filled.len(), 48);
        assert_eq!((filled[46].events, filled[47].events), (7.0, 0.0));
        assert!(filled[47].at.starts_with("2026-11-04T10:00:00"));
    }

    #[test]
    fn two_silent_hours_after_a_busy_day_are_p0_and_a_quiet_day_is_not() {
        let now = DateTime::parse_from_rfc3339("2026-11-04T10:30:00+08:00").unwrap();
        let busy: Vec<HourRow> = (0..22).map(|h| row(&format!("2026-11-03T{:02}:00:00+08:00", h + 2), 100.0)).collect();
        assert_eq!(silence_finding(&busy, now, 2, 10.0).unwrap().key, "health:silent-hours");
        let mut live = busy.clone();
        live.push(row("2026-11-04T10:00:00+08:00", 3.0));
        assert!(silence_finding(&live, now, 2, 10.0).is_none());
        let quiet: Vec<HourRow> = (0..5).map(|h| row(&format!("2026-11-03T{:02}:00:00+08:00", h + 12), 20.0)).collect();
        assert!(silence_finding(&quiet, now, 2, 10.0).is_none());
    }
}
