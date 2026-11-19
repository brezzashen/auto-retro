//! Rank convergence toward position 1 for every target keyword (founder 2026-11-09, report
//! `seo-rank-standard-20261109`): the production-only Search Console position, the daily SERP rank,
//! the volume-weighted gap to position 1, one action per rank band and stall escalation.

use super::watch_analyze::{Finding, QueryRow, TargetRow};
use super::watch_config::TargetKeyword;
use serde::{Deserialize, Serialize};

/// Rank of a keyword whose page is absent from the fetched SERP depth or has no data at all.
pub const UNRANKED: f64 = 101.0;
/// Days without a new best rank (one position or more) before the action escalates.
pub const STALL_DAYS: i64 = 14;
/// Weight of a keyword without a known monthly search volume.
const MIN_WEIGHT: f64 = 10.0;
const LOCALES: [&str; 8] = ["/zh-CN", "/zh-TW", "/ja", "/ko", "/th", "/fr", "/it", "/es"];

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RankRow {
    pub keyword: String,
    pub page: String,
    /// Today's organic SERP position of our page; `UNRANKED` beyond the fetched depth; None without a SERP.
    pub serp: Option<f64>,
    /// Breadcrumb of our ranking result (which page Google shows).
    pub serp_page: String,
    /// Host and title of the first organic result: the page to beat.
    pub leader: String,
    /// Production-only, impression-weighted Search Console position over 28 days; 0 without impressions.
    pub gsc_position: f64,
    pub impressions: f64,
    pub volume: Option<f64>,
    /// The rank every judgment uses: the SERP rank within its depth, else the Search Console position, else `UNRANKED`.
    pub rank: f64,
    /// Rank on the last earlier day in the history.
    pub previous: Option<f64>,
    /// Positions gained over 7 days (positive is better).
    pub delta7: Option<f64>,
    /// Days since the keyword last reached a new best rank.
    pub stalled_days: i64,
}

impl RankRow {
    /// Whether any source measured the keyword; an unmeasured one only means the SERP fetch failed.
    pub fn measured(&self) -> bool { self.serp.is_some() || self.gsc_position > 0.0 }
    pub fn weight(&self) -> f64 { weight_of(self.volume) }
}

/// Gap weight of a word: its monthly search volume, at least `MIN_WEIGHT`.
pub fn weight_of(volume: Option<f64>) -> f64 { volume.unwrap_or(0.0).max(MIN_WEIGHT) }

/// Lowercase, quotes removed, whitespace collapsed: `"Personal AI"` and `personal ai` are one query.
pub fn normalize(query: &str) -> String {
    query.to_lowercase().replace(['"', '\u{201c}', '\u{201d}'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The path without its locale prefix, so `/zh-TW/x`, `/ja/x` and `/x` are one page family.
pub fn base_path(path: &str) -> &str {
    for prefix in LOCALES {
        if path == prefix { return "/"; }
        if let Some(rest) = path.strip_prefix(prefix).filter(|r| r.starts_with('/')) { return rest; }
    }
    path
}

/// The path of a production URL; None for any other host (Canary included).
pub fn production_path<'a>(url: &'a str, site: &str) -> Option<&'a str> {
    let rest = url.strip_prefix(site.trim_end_matches('/'))?;
    if !(rest.is_empty() || rest.starts_with('/')) { return None; }
    let path = rest.split(['?', '#']).next().unwrap_or_default().trim_end_matches('/');
    Some(if path.is_empty() { "/" } else { path })
}

/// Search Console rows of each target: the exact normalized query on the production page family.
pub fn gsc_targets(rows: &[QueryRow], targets: &[TargetKeyword], site: &str) -> Vec<TargetRow> {
    targets.iter().map(|target| {
        let want = normalize(&target.keyword);
        let family = base_path(&target.page);
        let hits: Vec<&QueryRow> = rows.iter().filter(|row| normalize(&row.query) == want
            && production_path(&row.page, site).is_some_and(|p| base_path(p) == family)).collect();
        let impressions: f64 = hits.iter().map(|row| row.impressions).sum::<f64>() + 0.0;
        let weighted: f64 = hits.iter().map(|row| row.position * row.impressions).sum();
        TargetRow {
            keyword: target.keyword.clone(), page: target.page.clone(), impressions,
            clicks: hits.iter().map(|row| row.clicks).sum::<f64>() + 0.0,
            position: if impressions > 0.0 { weighted / impressions } else { 0.0 },
        }
    }).collect()
}

/// Whether the SERP itself gave the rank; otherwise `effective_rank` fell back to Search Console.
pub fn in_serp_depth(serp: Option<f64>) -> bool { serp.is_some_and(|s| s <= 100.0) }

/// A page beyond the SERP depth keeps a real Search Console position: impressions prove it ranks
/// elsewhere (another country or personalised results).
pub fn effective_rank(serp: Option<f64>, gsc_position: f64) -> f64 {
    match serp {
        Some(s) if s <= 100.0 => s,
        _ if gsc_position > 0.0 => gsc_position,
        _ => UNRANKED,
    }
}

/// Volume-weighted mean distance to position 1: Σ w × (rank − 1) / Σ w; 0 means every word is first.
pub fn weighted_gap<'a>(rows: impl Iterator<Item = (f64, f64)> + 'a) -> f64 {
    let (sum, weights) = rows.fold((0.0, 0.0), |(s, w), (rank, weight)| (s + weight * (rank - 1.0).max(0.0), w + weight));
    if weights > 0.0 { sum / weights } else { 0.0 }
}

/// The band label a rank falls in, best first.
pub fn band(rank: f64) -> &'static str {
    match rank {
        r if r < 1.5 => "第 1",
        r if r <= 3.0 => "2–3",
        r if r <= 10.0 => "4–10",
        r if r <= 20.0 => "11–20",
        r if r <= 100.0 => "21–100",
        _ => "未进前 100",
    }
}

pub const BANDS: [&str; 6] = ["第 1", "2–3", "4–10", "11–20", "21–100", "未进前 100"];

fn finding(key: String, title: String, action: String) -> Finding {
    // Reaching position 1 is recorded on the page, never dispatched.
    let severity = if key.starts_with("rank:top:") { "P2" } else { "P1" };
    Finding { key, severity: severity.into(), title, action }
}

/// The action of each band, keyed so a word moving to another band is new work.
fn band_finding(r: &RankRow, today: &str) -> Option<Finding> {
    let (k, p, rank) = (&r.keyword, &r.page, r.rank);
    let at = format!("「{k}」排名 {rank:.1}");
    if rank < 1.5 {
        return r.previous.filter(|&prev| prev >= 1.5).map(|prev| finding(format!("rank:top:{k}:{today}"), format!("{at}，从 {prev:.1} 升到第 1"),
            format!("守位：记录 {p} 本轮改动为有效做法，之后每天看是否掉位")));
    }
    if r.previous.is_some_and(|prev| prev < 1.5) {
        return Some(finding(format!("rank:lost-top:{k}:{today}"), format!("{at}，从第 1 掉落"),
            format!("守位：对比当前第一名 {}，查 {p} 最近改动、收录与反链变化，恢复第 1", r.leader)));
    }
    Some(match rank {
        r3 if r3 <= 3.0 => finding(format!("rank:top3:{k}"), format!("{at}，前 3 未到第 1"),
            format!("对标第一名 {}：逐项比较标题、首段对该词的直接回答、H2 覆盖、FAQ、结构化数据与反链，在 {p} 补齐差距", r.leader)),
        r20 if r20 <= 20.0 => finding(format!("rank:{k}"), format!("{at}，接近首页"),
            format!("在 {p} 加强该词：标题、H2 与 FAQ 各写入一次，补一段具体用例")),
        r100 if r100 <= 100.0 => finding(format!("rank:deep:{k}"), format!("{at}，在前 100 但不在前 20"),
            format!("在 {p} 扩写该词的专门一节（用例 + 三个要点 + FAQ），从 Wiki 与相关落地页加内链，并列入外链缺口")),
        _ => finding(format!("rank:absent:{k}"), format!("「{k}」未进前 100（SERP 前 100 无 example.com）"),
            format!("先查 {p} 是否已收录（URL Inspection），再确认页面标题或 H2 含该词；仍无排名则加内链并请求编入索引")),
    })
}

/// The escalation of a word that has not reached a new best rank for `STALL_DAYS` or more.
fn stall_finding(r: &RankRow) -> Option<Finding> {
    if r.rank < 1.5 || r.stalled_days < STALL_DAYS { return None; }
    let level = r.stalled_days / STALL_DAYS;
    let step = match level {
        1 => "换做法：标题与 H1 把该词前置，首段直接回答搜索意图，新增一个具体用例段与 2 条站内链接",
        2 => "加外链：把该页列入导航站与目录站提交（product-listing），并从 backlink 缺口里挑 3 个能拿到的引用",
        _ => "重新评估意图：看 SERP 前 3 的页面类型，必要时为该词建独立页面或合并到更对口的页面",
    };
    Some(finding(format!("stall:{}:{level}", r.keyword), format!("「{}」{} 天没有新的最好排名（当前 {:.1}）", r.keyword, r.stalled_days, r.rank),
        format!("第 {level} 级升级（{}）：{step}", r.page)))
}

/// One finding per measured word plus stall escalations, ordered by each word's share of the gap.
pub fn findings(rows: &[RankRow], today: &str) -> Vec<Finding> {
    let mut ordered: Vec<&RankRow> = rows.iter().filter(|r| r.measured()).collect();
    ordered.sort_by(|a, b| (b.weight() * (b.rank - 1.0)).total_cmp(&(a.weight() * (a.rank - 1.0))));
    ordered.iter().flat_map(|r| [stall_finding(r), band_finding(r, today)].into_iter().flatten()).collect()
}

#[cfg(test)]
#[path = "watch_rank_tests.rs"]
mod tests;
