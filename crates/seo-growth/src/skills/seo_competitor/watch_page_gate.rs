//! The gate on the keyword room's new pages (G12; founder's choice B on report
//! `gefei-keyword-method-20261110`, 2026-11-10). Gefei, WeChat 2025-03-20: publish ten pages, and only when
//! all ten are indexed and bring clicks publish twenty more. `maxNewPagesPerDay` holds while the pages the
//! keyword room built inside the window are indexed at `minIndexRate` and each has clicks; else `fallbackCap`.

use super::files::{items, read_json};
use super::watch_analyze::INDEX_GRACE_DAYS;
use super::watch_config::{WatchConfig, WatchState};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const INDEXED: &str = "Submitted and indexed";

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct GateConfig {
    /// Days back in which a keyword page counts as new.
    pub window_days: i64,
    /// Share of the judged new pages Search Console must report as indexed.
    pub min_index_rate: f64,
    /// New pages a day while the gate is closed.
    pub fallback_cap: usize,
    /// Days after its first sighting in the sitemap before a page is judged: Search Console reports
    /// clicks about two days late, so a younger page can show none.
    pub grace_days: i64,
}

impl Default for GateConfig {
    fn default() -> Self { Self { window_days: 14, min_index_rate: 0.8, fallback_cap: 1, grace_days: 3 } }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NewPage {
    pub path: String,
    /// Singapore date the watchdog first saw the page in the sitemap.
    pub first_seen: String,
    /// `待定` (not judged yet), `已收录` or `未收录`.
    pub status: String,
    /// Clicks of the page and its locale copies in the 28-day Search Console page report.
    pub clicks: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Gate {
    /// New pages the round may build today.
    pub cap: usize,
    pub open: bool,
    pub pages: Vec<NewPage>,
    pub note: String,
}

pub const PENDING: &str = "待定";
pub const IN_INDEX: &str = "已收录";
pub const NOT_INDEXED: &str = "未收录";

/// Clicks per page URL from a Search Console `page` report; `None` when the read failed.
pub fn clicks(pages: &Value) -> Option<BTreeMap<String, f64>> {
    Some(pages["rows"].as_array()?.iter().filter_map(|r| Some((r["keys"][0].as_str()?.to_string(), r["clicks"].as_f64().unwrap_or(0.0)))).collect())
}

/// Clicks per page path, a page's locale copies added to it: most clicks land on a copy
/// (2026-11-10: `/zh-TW/text-message-analyzer` 11 clicks, the English URL none).
pub fn by_page(clicks: &BTreeMap<String, f64>, site: &str, prefixes: &[String]) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for (url, n) in clicks {
        let Some(path) = url.strip_prefix(site.trim_end_matches('/')) else { continue };
        let own = prefixes.iter().find_map(|p| path.strip_prefix(p.as_str()).filter(|rest| rest.starts_with('/'))).unwrap_or(path);
        *out.entry(own.to_string()).or_insert(0.0) += n;
    }
    out
}

/// The pages the keyword room can have built: every seed page and every `ownPages` path of a round on
/// disk (`<state>/discover/<day>/round.json`), with the day of the room's first round. A page that entered
/// the sitemap before that day was built by someone else (the four 2026-11-03 SEO pages, a day before the
/// first round, are indexed without clicks and would close the gate at once).
pub fn room_pages(config: &WatchConfig, state_dir: &str) -> (BTreeSet<String>, Option<NaiveDate>) {
    let days: BTreeSet<NaiveDate> = std::fs::read_dir(format!("{state_dir}/discover")).ok().into_iter().flatten()
        .filter_map(|e| NaiveDate::parse_from_str(&e.ok()?.file_name().into_string().ok()?, "%Y-%m-%d").ok()).collect();
    let own = days.iter().filter_map(|d| read_json(&format!("{state_dir}/discover/{d}/round.json")).ok())
        .flat_map(|r| items(&r["ownPages"]).iter().filter_map(|p| p[1].as_str().map(str::to_string)).collect::<Vec<_>>());
    let seeds = config.discovery.iter().flat_map(|d| d.seeds.iter().map(|s| s.page.clone()));
    (seeds.chain(own).filter(|p| p.len() > 1 && p.matches('/').count() == 1).collect(), days.first().copied())
}

/// The room's pages first seen in the sitemap inside the window and since `room_start`, with their clicks
/// from `by_page`. A page is `待定` while it is younger than `graceDays`, or not crawled yet and younger than
/// the index grace.
pub fn new_pages(site: &str, paths: &BTreeSet<String>, room_start: NaiveDate, state: &WatchState, clicks: &BTreeMap<String, f64>, now: DateTime<FixedOffset>, cfg: &GateConfig) -> Vec<NewPage> {
    let seen = |url: &str| state.first_seen.get(url).and_then(|t| DateTime::parse_from_rfc3339(t).ok());
    let mut out: Vec<NewPage> = paths.iter().cloned().filter_map(|path| {
        let url = format!("{}{path}", site.trim_end_matches('/'));
        let at = seen(&url).filter(|at| at.with_timezone(now.offset()).date_naive() >= room_start && now - *at <= Duration::days(cfg.window_days))?;
        let age = now - at;
        let inspection = state.inspections.get(&url);
        let crawled = inspection.is_some_and(|i| !i.last_crawl.is_empty());
        let status = if age < Duration::days(cfg.grace_days) || (!crawled && age < Duration::days(INDEX_GRACE_DAYS)) { PENDING }
            else if inspection.is_some_and(|i| i.coverage == INDEXED) { IN_INDEX } else { NOT_INDEXED };
        let clicks = clicks.get(&path).copied().unwrap_or(0.0);
        Some(NewPage { path, first_seen: at.with_timezone(now.offset()).format("%Y-%m-%d").to_string(), status: status.into(), clicks })
    }).collect();
    out.sort_by(|a, b| a.first_seen.cmp(&b.first_seen).then(a.path.cmp(&b.path)));
    out
}

/// Open when every judged page has clicks and enough of them are indexed, or when nothing can be judged
/// yet (the first batch); a failed click read closes it.
pub fn decide(pages: Vec<NewPage>, clicks_read: bool, cfg: &GateConfig, max: usize) -> Gate {
    let judged: Vec<&NewPage> = pages.iter().filter(|p| p.status != PENDING).collect();
    let indexed = judged.iter().filter(|p| p.status == IN_INDEX).count();
    let clicked = judged.iter().filter(|p| p.clicks > 0.0).count();
    let rate = if judged.is_empty() { 1.0 } else { indexed as f64 / judged.len() as f64 };
    let open = clicks_read && rate >= cfg.min_index_rate && clicked == judged.len();
    let cap = if open { max } else { cfg.fallback_cap.min(max) };
    let why = if !clicks_read { "Search Console 页面点击读取失败，按不达标处理".to_string() }
        else if judged.is_empty() { "还没有可判定的新页，按首批处理".to_string() }
        else { format!("收录 {indexed}/{}（{:.0}%，门槛 {:.0}%），有点击 {clicked}/{}", judged.len(), rate * 100.0, cfg.min_index_rate * 100.0, judged.len()) };
    let note = format!("新页闸门：近 {} 天新建关键词页 {} 个，其中 {} 个待定（上线不满 {} 天或谷歌还没抓取）；{why} → {}，今天最多新建 {cap} 页",
        cfg.window_days, pages.len(), pages.len() - judged.len(), cfg.grace_days, if open { "达标" } else { "不达标，回落" });
    Gate { cap, open, pages, note }
}

/// Today's gate from the run's state and the Search Console page report.
pub fn gate(config: &WatchConfig, state: &WatchState, state_dir: &str, page_report: &Value, now: DateTime<FixedOffset>) -> Gate {
    let Some(d) = &config.discovery else { return Gate::default() };
    let prefixes: Vec<String> = config.rank.iter().flat_map(|r| r.markets.iter().map(|m| m.prefix.clone())).collect();
    let read = clicks(page_report).map(|c| by_page(&c, &config.site, &prefixes));
    let (paths, start) = room_pages(config, state_dir);
    let pages = start.map_or(Vec::new(), |s| new_pages(&config.site, &paths, s, state, read.as_ref().unwrap_or(&BTreeMap::new()), now, &d.new_page_gate));
    decide(pages, read.is_some(), &d.new_page_gate, d.max_new_pages_per_day)
}

#[cfg(test)]
#[path = "watch_page_gate_tests.rs"]
mod tests;
