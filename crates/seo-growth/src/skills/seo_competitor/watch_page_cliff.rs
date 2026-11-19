//! Page-level impression cliff: a production page that lost most of its daily Search Console
//! impressions while the site total hid it. The 28-day page row and the site-wide 24-hour
//! window both miss this (2026-11-12: `/ai-relationship-coach` fell 117 → 18 while the site grew).

use super::watch_anomalies::Anomaly;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const BASE_DAYS: usize = 3;
const MIN_BASE: f64 = 50.0;
const MIN_BASE_CLICKED: f64 = 20.0;
const MIN_CLICKS: f64 = 5.0;
const DROP: f64 = 0.3;

/// `day_page` holds Search Console rows keyed `[date, page]`. The newest date is still filling,
/// so the day before it is compared with the best of the `BASE_DAYS` days before that.
/// A page with `MIN_CLICKS` clicks over the rows needs only `MIN_BASE_CLICKED` impressions on its best day.
pub fn anomalies(day_page: &Value, site: &str) -> Vec<Anomaly> {
    let mut days = BTreeSet::new();
    let mut by_page: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    let mut clicks: BTreeMap<String, f64> = BTreeMap::new();
    for row in day_page["rows"].as_array().into_iter().flatten() {
        let (Some(date), Some(page)) = (row["keys"][0].as_str(), row["keys"][1].as_str()) else { continue };
        let Some(path) = page.strip_prefix(site).filter(|rest| rest.is_empty() || rest.starts_with('/')) else { continue };
        days.insert(date.to_string());
        let path = if path.is_empty() { "/" } else { path };
        *by_page.entry(path.to_string()).or_default().entry(date.to_string()).or_default() += row["impressions"].as_f64().unwrap_or(0.0);
        *clicks.entry(path.to_string()).or_default() += row["clicks"].as_f64().unwrap_or(0.0);
    }
    let ordered: Vec<&String> = days.iter().collect();
    if ordered.len() < BASE_DAYS + 2 { return Vec::new(); }
    let last = ordered[ordered.len() - 2];
    let base_days = &ordered[ordered.len() - 2 - BASE_DAYS..ordered.len() - 2];
    by_page.iter().filter_map(|(path, series)| {
        let at = |day: &String| series.get(day).copied().unwrap_or(0.0);
        let base = base_days.iter().map(|day| at(day)).fold(0.0, f64::max);
        let current = at(last);
        let min_base = if clicks.get(path).copied().unwrap_or(0.0) >= MIN_CLICKS { MIN_BASE_CLICKED } else { MIN_BASE };
        (base >= min_base && current < base * DROP).then(|| Anomaly {
            key: format!("anomaly:page-cliff:{path}"),
            category: "搜索量价背离".into(),
            severity: "P1".into(),
            subject: path.clone(),
            metric: "单页展示断崖".into(),
            current: format!("{last} 展示 {current:.0}"),
            baseline: format!("此前 {BASE_DAYS} 天最高 {base:.0}"),
            hypothesis_a: format!("{path} 近几天的标题、描述或 H2 改动经重抓后被重新评估：对照 URL Inspection 的 lastCrawl 与按小时展示的断点时间，断点晚于重抓才考虑回退"),
            hypothesis_b: "外因：匿名长尾查询（AI 搜索扇出或自动化查询）退潮。按日期 × 查询拆分，若可见真人词没有同步下降，或断点早于重抓，则不改文案".into(),
        })
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rows(series: &[(&str, &str, f64)]) -> Value {
        json!({"rows": series.iter().map(|(date, page, impressions)| json!({"keys": [date, page], "impressions": impressions})).collect::<Vec<_>>()})
    }

    #[test]
    fn a_page_that_lost_most_of_its_impressions_on_the_last_complete_day_is_a_cliff() {
        let coach = "https://example.com/ai-relationship-coach";
        let steady = "https://example.com/text-message-analyzer";
        let data = rows(&[
            ("2026-11-07", coach, 74.0), ("2026-11-08", coach, 79.0), ("2026-11-09", coach, 117.0), ("2026-11-10", coach, 18.0), ("2026-11-11", coach, 1.0),
            ("2026-11-07", steady, 59.0), ("2026-11-08", steady, 60.0), ("2026-11-09", steady, 60.0), ("2026-11-10", steady, 87.0), ("2026-11-11", steady, 52.0),
        ]);
        let got = anomalies(&data, "https://example.com");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "anomaly:page-cliff:/ai-relationship-coach");
        assert_eq!(got[0].current, "2026-11-10 展示 18");
        assert_eq!(got[0].baseline, "此前 3 天最高 117");
    }

    #[test]
    fn the_filling_day_small_pages_and_other_hosts_never_fire() {
        let page = "https://example.com/personal-crm";
        let small = "https://example.com/ja/ai-relationship-coach";
        let canary = "https://canary.example.com/text-message-analyzer";
        let data = rows(&[
            ("2026-11-07", page, 80.0), ("2026-11-08", page, 90.0), ("2026-11-09", page, 85.0), ("2026-11-10", page, 88.0), ("2026-11-11", page, 2.0),
            ("2026-11-07", small, 17.0), ("2026-11-08", small, 18.0), ("2026-11-09", small, 24.0), ("2026-11-10", small, 2.0),
            ("2026-11-07", canary, 90.0), ("2026-11-08", canary, 95.0), ("2026-11-09", canary, 120.0), ("2026-11-10", canary, 1.0),
        ]);
        assert!(anomalies(&data, "https://example.com").is_empty());
        assert!(anomalies(&rows(&[("2026-11-10", page, 0.0), ("2026-11-11", page, 0.0)]), "https://example.com").is_empty());
    }

    #[test]
    fn a_page_that_earns_clicks_is_a_cliff_from_a_smaller_base() {
        let clicked = "https://example.com/zh-TW/text-message-analyzer";
        let unclicked = "https://example.com/zh-CN/text-message-analyzer";
        let series = [("2026-11-13", 27.0, 1.0), ("2026-11-14", 41.0, 2.0), ("2026-11-15", 30.0, 2.0), ("2026-11-16", 8.0, 0.0), ("2026-11-17", 4.0, 0.0)];
        let data = json!({"rows": series.iter().flat_map(|(date, impressions, clicks)| [
            json!({"keys": [date, clicked], "impressions": impressions, "clicks": clicks}),
            json!({"keys": [date, unclicked], "impressions": impressions, "clicks": 0.0}),
        ]).collect::<Vec<_>>()});
        let got = anomalies(&data, "https://example.com");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "anomaly:page-cliff:/zh-TW/text-message-analyzer");
        assert_eq!(got[0].current, "2026-11-16 展示 8");
        assert_eq!(got[0].baseline, "此前 3 天最高 41");
    }
}
