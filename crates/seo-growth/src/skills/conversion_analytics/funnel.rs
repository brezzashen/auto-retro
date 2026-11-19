//! Funnel cohorts and the locate rule: the step pair whose conversion fell most against its own
//! trailing baseline, or, while there is no baseline yet, the pair that loses the most people.

use super::config::Finding;
use super::posthog::{num, text};
use super::queries::STEPS;
use chrono::{Duration, NaiveDate};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FunnelDay {
    pub day: String,
    /// People of this first-visit cohort at each step of `STEPS` within 24 h, in order.
    pub counts: Vec<f64>,
    /// Every visitor of the day has had the full 24 hours.
    pub matured: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Leak {
    /// Index of the step people fail to leave (`k` → `k + 1`).
    pub step: usize,
    /// `drop` against the trailing baseline, or `leak` (lowest pooled conversion, no baseline yet).
    pub mode: String,
    pub day: Option<String>,
    pub from_count: f64,
    pub rate: f64,
    pub baseline: Option<f64>,
    pub baseline_days: usize,
}

pub fn funnel_days(rows: &[Vec<Value>], today: NaiveDate) -> Vec<FunnelDay> {
    rows.iter().map(|row| {
        let day = text(row, 0);
        let matured = NaiveDate::parse_from_str(&day, "%Y-%m-%d").is_ok_and(|d| d + Duration::days(1) < today);
        FunnelDay { day, counts: (1..=STEPS.len()).map(|i| num(row, i)).collect(), matured }
    }).collect()
}

fn pooled(days: &[&FunnelDay], k: usize) -> (f64, f64) {
    days.iter().fold((0.0, 0.0), |(a, b), d| (a + d.counts[k], b + d.counts[k + 1]))
}

pub fn locate(days: &[FunnelDay], baseline_days: usize, min_sample: f64) -> Option<Leak> {
    let matured: Vec<&FunnelDay> = days.iter().filter(|d| d.matured).collect();
    if let Some((last, earlier)) = matured.split_last() {
        let baseline: Vec<&FunnelDay> = earlier.iter().rev().take(baseline_days).copied().collect();
        let drop = (0..STEPS.len() - 1).filter_map(|k| {
            let (from, next) = (last.counts[k], last.counts[k + 1]);
            let (base_from, base_next) = pooled(&baseline, k);
            (from >= min_sample && base_from >= min_sample).then(|| (k, next / from, base_next / base_from))
        }).filter(|(_, rate, base)| base > rate).max_by(|a, b| (a.2 - a.1).total_cmp(&(b.2 - b.1)));
        if let Some((step, rate, base)) = drop {
            return Some(Leak { step, mode: "drop".into(), day: Some(last.day.clone()), from_count: last.counts[step], rate, baseline: Some(base), baseline_days: baseline.len() });
        }
    }
    let all: Vec<&FunnelDay> = days.iter().collect();
    (0..STEPS.len() - 1).filter_map(|k| {
        let (from, next) = pooled(&all, k);
        (from >= min_sample).then(|| (k, from, next / from))
    }).min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(step, from, rate)| Leak { step, mode: "leak".into(), day: None, from_count: from, rate, baseline: None, baseline_days: matured.len().saturating_sub(1) })
}

/// The report-only trend finding for the located step (never dispatched: product changes need the founder).
pub fn leak_finding(leak: &Leak) -> Finding {
    let (from, to) = (STEPS[leak.step].label, STEPS[leak.step + 1].label);
    let title = match (leak.mode.as_str(), leak.baseline) {
        ("drop", Some(base)) => format!("{}「{from} → {to}」转化 {:.0}%，低于前 {} 天基线 {:.0}%", leak.day.as_deref().unwrap_or(""), leak.rate * 100.0, leak.baseline_days, base * 100.0),
        _ => format!("追踪以来最大流失在「{from} → {to}」：{:.0} 人里只有 {:.0}% 走到下一步（还没有可比基线）", leak.from_count, leak.rate * 100.0),
    };
    Finding::new(format!("funnel:{}:{}", leak.mode, STEPS[leak.step].key), "P2", title, "见「定位」一节的拆分与 HogQL；产品改动只作建议，需 founder 授权")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: &str, counts: [f64; 7], matured: bool) -> FunnelDay { FunnelDay { day: d.into(), counts: counts.to_vec(), matured } }

    #[test]
    fn a_fall_against_the_baseline_wins_over_a_steady_low_step() {
        let days = vec![
            day("2026-11-10", [100.0, 20.0, 10.0, 8.0, 4.0, 1.0, 0.0], true),
            day("2026-11-11", [100.0, 20.0, 10.0, 8.0, 4.0, 1.0, 0.0], true),
            day("2026-11-12", [100.0, 20.0, 3.0, 2.0, 1.0, 0.0, 0.0], true),
        ];
        let leak = locate(&days, 7, 5.0).unwrap();
        assert_eq!((leak.mode.as_str(), leak.step, leak.baseline_days), ("drop", 1, 2));
        assert_eq!(leak.day.as_deref(), Some("2026-11-12"));
    }

    #[test]
    fn without_a_baseline_the_lowest_pooled_conversion_is_reported_as_a_leak() {
        let days = vec![day("2026-11-03", [200.0, 10.0, 6.0, 5.0, 1.0, 0.0, 0.0], false)];
        let leak = locate(&days, 7, 5.0).unwrap();
        assert_eq!((leak.mode.as_str(), leak.step), ("leak", 0));
        assert!(leak_finding(&leak).title.contains("还没有可比基线"));
    }

    #[test]
    fn cohorts_mature_one_full_day_after_their_date() {
        let rows = vec![vec![serde_json::json!("2026-11-01"), serde_json::json!(5)], vec![serde_json::json!("2026-11-02"), serde_json::json!(5)]];
        let days = funnel_days(&rows, NaiveDate::from_ymd_opt(2026, 11, 3).unwrap());
        assert_eq!(days.iter().map(|d| d.matured).collect::<Vec<_>>(), vec![true, false]);
    }
}
