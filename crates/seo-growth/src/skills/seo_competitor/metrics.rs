//! `.claude/skills/seo-competitor/scripts/metrics.ts`: derived SEO metrics shared by analyze and the
//! report: growth over the history window and the win rate. Its tests are at the bottom of this file.

use super::files::{num, num_or, text};
use crate::js_wire::js;
use serde::Serialize;
use serde_json::Value;

/// A non-brand keyword at or below this keyword difficulty counts as winnable traffic.
pub const WINNABLE_KD: f64 = 20.0;
/// A ranking domain counts as beatable when its rank is at most the target's rank plus this margin.
pub const BEATABLE_MARGIN: f64 = 10.0;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct MonthPoint {
    pub month: String,
    pub etv: f64,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Growth {
    pub points: Vec<MonthPoint>,
    pub yoy: Option<f64>,
    pub last3: Option<f64>,
}

/// `growthOf(history)`: history rows `{year, month, etv}` sorted by month, with the change against
/// the first month and against three months earlier.
pub fn growth_of(history: &[Value]) -> Growth {
    let mut points: Vec<MonthPoint> = history
        .iter()
        .map(|h| {
            let month = format!("{}-{}", month_text(h, "year"), pad_two(&month_text(h, "month")));
            MonthPoint { month, etv: js::round(num_or(&h["etv"], 0.0)) }
        })
        .collect();
    // localeCompare on fixed-width YYYY-MM keys orders like code points.
    points.sort_by(|a, b| a.month.cmp(&b.month));
    if points.len() < 2 {
        return Growth { points, yoy: None, last3: None };
    }
    let last = points[points.len() - 1].etv;
    let change = |from: f64| if from > 0.0 { Some(last / from - 1.0) } else { None };
    let yoy = change(points[0].etv);
    let three_back = if points.len() >= 4 { points[points.len() - 4].etv } else { points[0].etv };
    Growth { points, yoy, last3: change(three_back) }
}

/// `String(h[key])` for a history field: numbers print as JavaScript prints them.
fn month_text(h: &Value, key: &str) -> String {
    match h.get(key) {
        Some(Value::Number(n)) => js::number_string(n.as_f64().unwrap_or(f64::NAN)),
        Some(other) => text(other).to_string(),
        None => "undefined".to_string(),
    }
}

/// `String(x).padStart(2, "0")`.
fn pad_two(value: &str) -> String {
    if value.len() >= 2 { value.to_string() } else { format!("0{value}") }
}

/// `winRate(top, kd, targetRank)`: 70% on the share of first-page domains whose rank is within reach,
/// 30% on keyword difficulty. Without a difficulty score the first term carries the whole weight.
/// `top` holds each domain's backlink rank, `None` when unranked.
pub fn win_rate(top: &[Option<f64>], kd: Option<f64>, target_rank: f64) -> f64 {
    if top.is_empty() {
        return 0.0;
    }
    let reach = target_rank + BEATABLE_MARGIN;
    let beatable = top.iter().filter(|rank| rank.is_some_and(|r| r <= reach)).count() as f64 / top.len() as f64;
    match kd {
        None => beatable,
        Some(kd) => 0.7 * beatable + 0.3 * (1.0 - kd / 100.0),
    }
}

/// The rank a JSON field holds, when it is a number.
pub fn rank_of(value: &Value) -> Option<f64> {
    num(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn win_rate_weighs_beatable_first_page_domains_70_and_keyword_difficulty_30() {
        let top = [Some(30.0), Some(60.0), None, Some(95.0)];
        assert!((win_rate(&top, Some(0.0), 28.0) - (0.7 * 0.25 + 0.3)).abs() < 1e-12);
        assert!((win_rate(&top, None, 28.0) - 0.25).abs() < 1e-12);
        assert_eq!(win_rate(&[], Some(0.0), 28.0), 0.0);
    }

    #[test]
    fn growth_compares_the_latest_month_with_the_first_and_with_three_months_earlier() {
        let mut history: Vec<Value> = [100, 120, 150, 200, 300]
            .iter()
            .enumerate()
            .map(|(i, etv)| json!({"year": 2026, "month": i + 1, "etv": etv}))
            .collect();
        history.reverse();
        let g = growth_of(&history);
        let months: Vec<&str> = g.points.iter().map(|p| p.month.as_str()).collect();
        assert_eq!(months, ["2026-01", "2026-02", "2026-03", "2026-04", "2026-05"]);
        assert!((g.yoy.unwrap() - 2.0).abs() < 1e-12);
        assert!((g.last3.unwrap() - 1.5).abs() < 1e-12);
        let short = growth_of(&[json!({"year": 2026, "month": 1, "etv": 0}), json!({"year": 2026, "month": 2, "etv": 5})]);
        assert_eq!(short.yoy, None);
    }
}
