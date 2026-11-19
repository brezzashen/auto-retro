//! The `排名收敛` section of the seo-watch page: the weighted gap to position 1 over time, the
//! words per rank band and one row per target keyword, largest share of the gap first.

use super::watch_rank::{BANDS, RankRow, band, weighted_gap};
use super::watch_rank_history::gap_by_day;
use super::watch_rank_run::Ranks;
use chrono::NaiveDate;

pub const TITLE: &str = "排名收敛（目标：每个词趋近第 1）";

fn cell(text: &str) -> String { text.replace('|', "\\|") }

fn label(text: &str) -> String { format!("\"{}\"", text.replace('"', "'")) }

fn number(v: Option<f64>) -> String { v.map_or("—".into(), |v| format!("{v:.1}")) }

pub fn section(ranks: Option<&Ranks>, today: NaiveDate) -> String {
    let mut md = format!("## {TITLE}\n\n");
    let Some(ranks) = ranks else { return md + "配置里没有 rank 一节，未计算。\n\n" };
    let measured: Vec<&RankRow> = ranks.rows.iter().filter(|r| r.measured()).collect();
    let gap = weighted_gap(measured.iter().map(|r| (r.rank, r.weight())));
    let count = |b: &str| measured.iter().filter(|r| band(r.rank) == b).count();
    md.push_str("口径：名次取当天 Google 自然结果名次（DataForSEO SERP，前 100，英文页按美国、`/ja` 按日本、`/zh-TW` 按台湾），SERP 前 100 没有我们或当天没有 SERP 时取 Search Console 近 28 天生产页（只算 example.com 目标页及其语言版本、搜索词精确匹配）的平均排名。\
加权距第一差距 = Σ 月搜索量 × (名次 − 1) ÷ Σ 月搜索量（无搜索量按 10），0 表示全部第 1。\n\n");
    md.push_str(&format!("当前加权距第一差距 **{gap:.1}**；第 1 名 {} 个，前 3 {} 个，前 10 {} 个；已测 {} / {} 个词。\n\n",
        count(BANDS[0]), count(BANDS[0]) + count(BANDS[1]), count(BANDS[0]) + count(BANDS[1]) + count(BANDS[2]), measured.len(), ranks.rows.len()));
    let days = gap_by_day(&ranks.history, today, 28);
    if !days.is_empty() {
        let max = days.iter().map(|(_, g)| *g).fold(1.0_f64, f64::max).ceil();
        md.push_str(&format!("```mermaid\nxychart-beta\ntitle \"加权距第一差距（越低越好）\"\nx-axis [{}]\ny-axis \"名次差距\" 0 --> {max}\nline [{}]\n```\n\n",
            days.iter().map(|(d, _)| label(d.get(5..).unwrap_or(d))).collect::<Vec<_>>().join(", "),
            days.iter().map(|(_, g)| format!("{g:.1}")).collect::<Vec<_>>().join(", ")));
    }
    let bars: Vec<usize> = BANDS.iter().map(|b| count(b)).collect();
    md.push_str(&format!("```mermaid\nxychart-beta\ntitle \"各名次段的词数\"\nx-axis [{}]\ny-axis \"词数\" 0 --> {}\nbar [{}]\n```\n\n",
        BANDS.iter().map(|b| label(b)).collect::<Vec<_>>().join(", "), bars.iter().max().copied().unwrap_or(1).max(1),
        bars.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")));
    let mut rows: Vec<&RankRow> = ranks.rows.iter().collect();
    rows.sort_by(|a, b| (b.measured(), b.weight() * (b.rank - 1.0)).partial_cmp(&(a.measured(), a.weight() * (a.rank - 1.0))).unwrap_or(std::cmp::Ordering::Equal));
    md.push_str("| 词 | 页面 | 名次段 | SERP 名次 | GSC 平均 | 7 天变化 | 停滞天数 | 月搜索量 | 第一名 |\n|---|---|---|---|---|---|---|---|---|\n");
    for r in rows {
        let serp = match r.serp { Some(s) if s > 100.0 => "未进前 100".into(), s => number(s) };
        let gsc = if r.gsc_position > 0.0 { format!("{:.1}（{} 次）", r.gsc_position, r.impressions) } else { "—".into() };
        let delta = r.delta7.map_or("—".into(), |d| if d > 0.0 { format!("↑{d:.1}") } else if d < 0.0 { format!("↓{:.1}", -d) } else { "0".into() });
        md.push_str(&format!("| {} | {} | {} | {serp} | {gsc} | {delta} | {} | {} | {} |\n", cell(&r.keyword), r.page,
            if r.measured() { band(r.rank) } else { "无数据" }, r.stalled_days, r.volume.map_or("—".into(), |v| format!("{v:.0}")), cell(&r.leader)));
    }
    md + "\n"
}
