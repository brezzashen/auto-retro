//! Turns one run's probes and Search Console rows into metrics and findings. A finding's `key`
//! (`kind:subject`) is stable across runs, so the dispatcher hands each one to a tab only once.

use super::watch_config::{TargetKeyword, WatchState};
use super::watch_probe::{only_slow, PageProbe};
use chrono::{DateTime, Duration, FixedOffset};
use serde::Serialize;
use serde_json::Value;

/// Days a sitemap URL may stay unindexed before it becomes a finding.
pub const INDEX_GRACE_DAYS: i64 = 7;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub key: String,
    /// `P0` site broken, `P1` act this week, `P2` watch.
    pub severity: String,
    pub title: String,
    pub action: String,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct QueryRow {
    pub query: String,
    pub page: String,
    pub clicks: f64,
    pub impressions: f64,
    pub position: f64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TargetRow {
    pub keyword: String,
    pub page: String,
    pub impressions: f64,
    pub clicks: f64,
    /// Impression-weighted average position; 0 when the keyword has no impressions.
    pub position: f64,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub impressions: f64,
    pub clicks: f64,
}

pub fn query_rows(query_page: &Value) -> Vec<QueryRow> {
    query_page["rows"].as_array().map(|rows| rows.iter().map(|row| QueryRow {
        query: row["keys"][0].as_str().unwrap_or_default().to_string(),
        page: row["keys"][1].as_str().unwrap_or_default().to_string(),
        clicks: row["clicks"].as_f64().unwrap_or(0.0),
        impressions: row["impressions"].as_f64().unwrap_or(0.0),
        position: row["position"].as_f64().unwrap_or(0.0),
    }).collect()).unwrap_or_default()
}

/// End of the complete Search Console hours: `metadata.firstIncompleteHour`, which trails the
/// wall clock by about 14 hours, else `now`.
pub fn data_end(hourly: &Value, now: DateTime<FixedOffset>) -> DateTime<FixedOffset> {
    hourly["metadata"]["firstIncompleteHour"].as_str()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .filter(|end| *end <= now)
        .unwrap_or(now)
}

/// Sums the hourly rows that start in `[end - to_hours_ago - 24h, end - to_hours_ago)`, where `end`
/// is [`data_end`], so a window never counts hours Search Console has not reported yet.
pub fn window(hourly: &Value, now: DateTime<FixedOffset>, to_hours_ago: i64) -> Window {
    let end = data_end(hourly, now) - Duration::hours(to_hours_ago);
    let start = end - Duration::hours(24);
    let mut sum = Window::default();
    for row in hourly["rows"].as_array().into_iter().flatten() {
        let Some(at) = row["keys"][0].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()) else { continue };
        if at >= start && at < end {
            sum.impressions += row["impressions"].as_f64().unwrap_or(0.0);
            sum.clicks += row["clicks"].as_f64().unwrap_or(0.0);
        }
    }
    sum
}

fn finding(key: String, severity: &str, title: String, action: String) -> Finding {
    Finding { key, severity: severity.into(), title, action }
}

/// A failing production page is P0 unless the same page is healthy on Canary: then the fix only
/// waits for promotion, which the founder starts, so one P2 finding lists those pages.
/// A page that is only slow is never fixed on Canary: a fast Canary refetch measures the probe host's
/// network path, not a release.
/// A page that only the Canary sitemap lists (not in `released`) was never on production, so a broken
/// production copy is expected; when Canary also fails it, Canary is mid-rollout or rolled back,
/// which one P2 reports without opening a tab.
pub fn health_findings(probes: &[PageProbe], mirror: &[PageProbe], released: &[String]) -> Vec<Finding> {
    let fixed = |probe: &PageProbe| !only_slow(probe) && mirror.iter().any(|m| m.url == probe.url && m.problems.is_empty());
    let failing: Vec<&PageProbe> = probes.iter().filter(|probe| !probe.problems.is_empty()).collect();
    let unreleased: Vec<&str> = failing.iter().filter(|p| !fixed(p) && !released.contains(&p.url)).map(|p| p.url.as_str()).collect();
    let mut out: Vec<Finding> = failing.iter().filter(|probe| !fixed(probe) && released.contains(&probe.url)).map(|probe| finding(
        format!("health:{}", probe.url), "P0",
        format!("{}：{}", probe.url, probe.problems.join("；")),
        "修复页面头部或路由，beta 验收后交付 Canary".into(),
    )).collect();
    let waiting = failing.iter().filter(|probe| fixed(probe)).count();
    if waiting > 0 {
        out.push(finding("promote:pending".into(), "P2",
            format!("生产有 {waiting} 个页面的问题已在 Canary 修好，等待 promote 到 example.com"),
            "需要 founder 明确要求后执行 mars canary promote；watchdog 不派工".into()));
    }
    if !unreleased.is_empty() {
        out.push(finding("canary:unreleased".into(), "P2",
            format!("{} 个仅 Canary sitemap 列出的未发布页在 Canary 上也异常：{}", unreleased.len(), unreleased.join("、")),
            "生产从未列出这些页，不派工；多为 Canary 正在滚动发布或刚回滚，持续出现时用 mars canary status 核对 Canary 版本".into()));
    }
    out
}

pub fn index_findings(urls: &[String], state: &WatchState, now: DateTime<FixedOffset>) -> Vec<Finding> {
    urls.iter().filter_map(|url| {
        let inspection = state.inspections.get(url)?;
        if inspection.coverage.is_empty() || inspection.coverage == "Submitted and indexed" { return None; }
        let seen = state.first_seen.get(url).and_then(|t| DateTime::parse_from_rfc3339(t).ok())?;
        (now - seen > Duration::days(INDEX_GRACE_DAYS)).then(|| finding(
            format!("index:{url}"), "P1",
            format!("{url} 上线超过 {INDEX_GRACE_DAYS} 天仍未收录（{}）", inspection.coverage),
            "加站内链接、补充正文，并在 GSC 手动请求编入索引".into(),
        ))
    }).collect()
}

/// Low-CTR pages and new query candidates; rank work per target lives in `watch_rank`.
pub fn keyword_findings(rows: &[QueryRow], targeted: &[TargetKeyword]) -> Vec<Finding> {
    let mut out: Vec<Finding> = Vec::new();
    let mut pages: std::collections::BTreeMap<&str, (f64, f64)> = Default::default();
    for row in rows { let entry = pages.entry(&row.page).or_default(); entry.0 += row.impressions; entry.1 += row.clicks; }
    out.extend(pages.into_iter().filter(|(_, (impressions, clicks))| *impressions >= 100.0 && clicks / impressions < 0.01).map(|(page, (impressions, clicks))| finding(
        format!("ctr:{page}"), "P1",
        format!("{page} 展示 {impressions} 次、点击 {clicks} 次，点击率低于 1%"),
        "改写该页标题与描述，突出搜索词与免费开始".into(),
    )));
    let is_targeted = |query: &str| targeted.iter().any(|t| query.contains(&t.keyword));
    let mut candidates: Vec<&QueryRow> = rows.iter()
        .filter(|row| row.impressions >= 3.0 && !row.query.contains("mars") && !is_targeted(&row.query)).collect();
    candidates.sort_by(|a, b| b.impressions.total_cmp(&a.impressions));
    out.extend(candidates.into_iter().take(10).map(|row| finding(
        format!("candidate:{}", row.query), "P2",
        format!("新词候选「{}」：{} 次展示，排名 {:.1}，落在 {}", row.query, row.impressions, row.position, row.page),
        "判断是否与 Mars 功能对口；对口则并入最接近的落地页或新建一页".into(),
    )));
    out
}

pub fn trend_findings(last: &Window, previous: &Window) -> Vec<Finding> {
    if previous.impressions >= 20.0 && last.impressions < previous.impressions * 0.5 {
        return vec![finding("trend:drop".into(), "P1",
            format!("Search Console 最近 24 个完整小时展示 {} 次，比前 24 小时 {} 次下降超过一半", last.impressions, previous.impressions),
            "先查页面健康与收录，再查是否有排名词掉出首页".into())];
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> DateTime<FixedOffset> { DateTime::parse_from_rfc3339("2026-11-02T12:00:00+08:00").unwrap() }

    #[test]
    fn windows_split_hourly_rows_at_24_hours() {
        let hourly = json!({"rows": [
            {"keys": ["2026-11-01T18:00:00-08:00"], "impressions": 5, "clicks": 1},
            {"keys": ["2026-10-31T10:00:00-07:00"], "impressions": 7, "clicks": 0},
        ]});
        assert_eq!(window(&hourly, now(), 0).impressions, 5.0);
        assert_eq!(window(&hourly, now(), 24).impressions, 7.0);
    }

    #[test]
    fn windows_end_at_the_first_incomplete_search_console_hour() {
        let run_at = DateTime::parse_from_rfc3339("2026-11-07T03:24:00+08:00").unwrap();
        let hourly = json!({"metadata": {"firstIncompleteHour": "2026-11-05T21:00:00-08:00"}, "rows": [
            {"keys": ["2026-11-05T21:00:00-08:00"], "impressions": 3, "clicks": 0},
            {"keys": ["2026-11-05T20:00:00-08:00"], "impressions": 5, "clicks": 0},
            {"keys": ["2026-11-04T21:00:00-08:00"], "impressions": 14, "clicks": 0},
            {"keys": ["2026-11-04T20:00:00-08:00"], "impressions": 9, "clicks": 0},
        ]});
        assert_eq!(window(&hourly, run_at, 0).impressions, 19.0);
        assert_eq!(window(&hourly, run_at, 24).impressions, 9.0);
        assert!(trend_findings(&window(&hourly, run_at, 0), &window(&hourly, run_at, 24)).is_empty());
    }

    #[test]
    fn non_brand_untargeted_queries_become_candidates() {
        let rows = vec![
            QueryRow { query: "personal crm app".into(), page: "/personal-crm".into(), impressions: 30.0, position: 9.0, ..Default::default() },
            QueryRow { query: "mars".into(), page: "/".into(), impressions: 50.0, position: 1.0, ..Default::default() },
            QueryRow { query: "relationship tracker".into(), page: "/".into(), impressions: 4.0, position: 30.0, ..Default::default() },
        ];
        let targeted = vec![TargetKeyword { keyword: "personal crm".into(), page: "/personal-crm".into() }];
        let keys: Vec<String> = keyword_findings(&rows, &targeted).into_iter().map(|f| f.key).collect();
        assert_eq!(keys, ["candidate:relationship tracker"]);
    }

    #[test]
    fn a_page_already_fixed_on_canary_waits_for_promotion_instead_of_paging() {
        let site = "https://example.com";
        let broken = crate::skills::seo_competitor::watch_probe::judge("https://example.com/pricing", site, 200, 10, "<html lang=\"zh-CN\">");
        let other = crate::skills::seo_competitor::watch_probe::judge("https://example.com/terms", site, 404, 10, "");
        let mut fixed = broken.clone();
        fixed.problems.clear();
        let released = vec!["https://example.com/pricing".to_string(), "https://example.com/terms".to_string()];
        let keys: Vec<String> = health_findings(&[broken, other], &[fixed], &released).into_iter().map(|f| f.key).collect();
        assert_eq!(keys, ["health:https://example.com/terms", "promote:pending"]);
    }

    #[test]
    fn an_unreleased_canary_page_broken_on_canary_too_is_p2_not_a_production_p0() {
        let site = "https://example.com";
        let judge = crate::skills::seo_competitor::watch_probe::judge;
        let fallback = "<html lang=\"en\"><title>ExampleAI: Personal CRM</title><link rel=\"canonical\" href=\"https://example.com\" />";
        let page = judge("https://example.com/zh-CN/dex-alternatives", site, 200, 10, fallback);
        let canary = page.clone();
        let findings = health_findings(&[page], &[canary], &["https://example.com/pricing".to_string()]);
        assert_eq!(findings.iter().map(|f| (f.key.as_str(), f.severity.as_str())).collect::<Vec<_>>(), [("canary:unreleased", "P2")]);
    }

    #[test]
    fn a_slow_page_stays_p0_even_when_its_canary_refetch_is_fast() {
        let url = "https://example.com/pricing";
        let links = r#"<link rel="alternate" hreflang="a" /><link rel="alternate" hreflang="b" /><link rel="alternate" hreflang="c" /><link rel="alternate" hreflang="d" />"#;
        let html = format!(r#"<html lang="en"><title>Pricing</title><link rel="canonical" href="{url}" />{links}"#);
        let judge = crate::skills::seo_competitor::watch_probe::judge;
        let slow = judge(url, "https://example.com", 200, 6500, &html);
        let fast = judge(url, "https://example.com", 200, 300, &html);
        let keys: Vec<String> = health_findings(&[slow], &[fast], &[url.to_string()]).into_iter().map(|f| f.key).collect();
        assert_eq!(keys, [format!("health:{url}")]);
    }

    #[test]
    fn a_halved_day_is_a_trend_finding_only_above_twenty_impressions() {
        let w = |impressions| Window { impressions, clicks: 0.0 };
        assert_eq!(trend_findings(&w(5.0), &w(40.0)).len(), 1);
        assert!(trend_findings(&w(1.0), &w(10.0)).is_empty());
    }
}
