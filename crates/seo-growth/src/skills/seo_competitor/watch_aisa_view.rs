//! The `AIsa 搜索信号` section of the seo-watch page, drawn from one run's `aisaSeo` block.

use super::watch_aisa::AisaRun;
use super::watch_aisa_parse::AiCite;
use std::collections::BTreeMap;

pub const SECTION: &str = "AIsa 搜索信号";
/// Keyword difficulty at or below this counts as easy (the skill's "winnable" threshold).
const EASY_KD: f64 = 20.0;

fn market(location: u32) -> &'static str {
    match location { 2840 => "美国", 2392 => "日本", 2410 => "韩国", 2764 => "泰国", 2158 => "台湾", 2702 => "新加坡", _ => "其他" }
}

fn top_domains(rows: &[AiCite], n: usize) -> String {
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for d in rows.iter().flat_map(|r| r.domains.iter()) { *count.entry(d).or_default() += 1; }
    let mut list: Vec<(&str, usize)> = count.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    list.iter().take(n).map(|(d, c)| format!("{d}（{c}）")).collect::<Vec<_>>().join("、")
}

fn cite_line(label: &str, rows: &[AiCite]) -> String {
    let shown: Vec<&AiCite> = rows.iter().filter(|r| r.present).collect();
    let deferred = shown.iter().filter(|r| r.deferred).count();
    let mars = shown.iter().filter(|r| r.mars_cited).count();
    let cited: Vec<AiCite> = shown.iter().map(|r| (*r).clone()).collect();
    format!("{label}：{} 个目标词中 {} 个出现 AI 答案（{deferred} 个延迟加载、看不到来源），example.com 被引用 {mars} 次。被引用最多的域名：{}。\n\n",
        rows.len(), shown.len(), if cited.is_empty() { "无".into() } else { top_domains(&cited, 8) })
}

pub fn section(run: Option<&AisaRun>) -> String {
    let Some(run) = run else { return String::new() };
    let mut md = format!("## {SECTION}\n\n经 AIsa 调 DataForSEO：AI 概览来自当天排名用的 Google 自然结果页（不另收费），AI 模式每轮轮换查几个目标词，意图与难度缓存 30 天，外链增减每天查一次。\n\n");
    md.push_str(&cite_line("Google AI 概览（今天）", &run.overview));
    md.push_str(&cite_line("Google AI 模式（每个词最近一次）", &run.ai_mode));
    if !run.ai_mode_run.is_empty() {
        md.push_str("本轮 AI 模式：\n\n| 关键词 | 市场 | AI 答案 | 引用 Mars | 引用来源（前 4） |\n|---|---|---|---|---|\n");
        for r in &run.ai_mode_run {
            let answer = if r.error.is_some() { "失败" } else if r.present { "有" } else { "无" };
            md.push_str(&format!("| {} | {} | {answer} | {} | {} |\n", r.keyword, market(r.location), if r.mars_cited { "是" } else { "否" },
                r.domains.iter().take(4).cloned().collect::<Vec<_>>().join("、")));
        }
        md.push('\n');
    }
    if !run.props.is_empty() {
        let mut intents: BTreeMap<&str, usize> = BTreeMap::new();
        for p in run.props.values() { *intents.entry(p.intent.as_deref().unwrap_or("未知")).or_default() += 1; }
        let easy = run.props.values().filter(|p| p.difficulty.is_some_and(|d| d <= EASY_KD)).count();
        md.push_str(&format!("关键词意图（{} 个目标词）：{}；难度 ≤ {EASY_KD} 的易做词 {easy} 个。\n\n", run.props.len(),
            intents.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join("、")));
    }
    if !run.links.is_empty() {
        let week = &run.links[run.links.len().saturating_sub(7)..];
        let sum = |f: fn(&super::watch_aisa_parse::LinkDay) -> i64| week.iter().map(f).sum::<i64>();
        md.push_str(&format!("外链增减（近 7 天，DataForSEO 外链库）：新增外链 {}、丢失 {}；新增来源域 {}、丢失 {}。\n\n",
            sum(|d| d.new_backlinks), sum(|d| d.lost_backlinks), sum(|d| d.new_referring_domains), sum(|d| d.lost_referring_domains)));
    }
    md.push_str(&format!("今日 AIsa 搜索信号花费 ${:.2} / 上限 ${:.2}。", run.spent_today_usd, run.daily_budget_usd));
    if !run.skipped.is_empty() { md.push_str(&format!("未调用：{}。", run.skipped.join("；"))); }
    md.push_str("\n\n");
    md
}
