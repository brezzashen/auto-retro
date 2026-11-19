//! The watchdog page section of the AI recommendation watch: per engine, this hour and the rolling 24 hours and
//! 7 days of answers that recommend Mars (`watch_geo.rs`). The headline rate counts `tool` questions only; `advice`
//! and `dating` questions get their own 7-day columns (report geo-optimization-20261112).

use super::watch_geo::{GeoConfig, GeoRecord, GeoRun};
use super::watch_geo_bank::TOOL;
use chrono::{DateTime, Duration, FixedOffset};
use serde::Serialize;
use std::collections::BTreeMap;

pub const SECTION: &str = "AI 推荐率";

fn bucket(r: &GeoRecord) -> &str {
    if r.bucket.is_empty() { TOOL } else { &r.bucket }
}

/// Counts over the answers that arrived whole and were judged (`ok`); `failed` counts the rest.
#[derive(Serialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rate {
    pub answers: usize,
    pub mentioned: usize,
    pub first: usize,
    pub mars_cited: usize,
    pub failed: usize,
}

impl Rate {
    fn add(&mut self, r: &GeoRecord) {
        if !r.ok { self.failed += 1; return; }
        self.answers += 1;
        self.mentioned += r.mentioned as usize;
        self.first += (r.rank == Some(1)) as usize;
        self.mars_cited += r.mars_cited as usize;
    }

    fn share(&self) -> String {
        if self.answers == 0 { return "—".into(); }
        format!("{}/{}（{:.0}%）", self.mentioned, self.answers, 100.0 * self.mentioned as f64 / self.answers as f64)
    }
}

/// Engine id → (this hour, 24 hours, 7 days) over the `tool` questions.
pub fn rates(records: &[GeoRecord], hour: &str, now: DateTime<FixedOffset>) -> BTreeMap<String, (Rate, Rate, Rate)> {
    let mut out: BTreeMap<String, (Rate, Rate, Rate)> = BTreeMap::new();
    for r in records.iter().filter(|r| bucket(r) == TOOL) {
        let Ok(at) = DateTime::parse_from_rfc3339(&r.at) else { continue };
        let age = now - at;
        if age > Duration::days(7) || age < Duration::zero() { continue; }
        let entry = out.entry(r.engine.clone()).or_default();
        entry.2.add(r);
        if age <= Duration::hours(24) { entry.1.add(r); }
        if r.hour == hour { entry.0.add(r); }
    }
    out
}

/// Engine id → 7 days of answers to questions of `wanted` bucket.
pub fn bucket_rates(records: &[GeoRecord], wanted: &str, now: DateTime<FixedOffset>) -> BTreeMap<String, Rate> {
    let mut out: BTreeMap<String, Rate> = BTreeMap::new();
    for r in records.iter().filter(|r| bucket(r) == wanted) {
        let Ok(at) = DateTime::parse_from_rfc3339(&r.at) else { continue };
        let age = now - at;
        if age <= Duration::days(7) && age >= Duration::zero() { out.entry(r.engine.clone()).or_default().add(r); }
    }
    out
}

/// Domains the last 24 hours of answers cited most often (GPT, Claude and Doubao URLs; Gemini gives source titles).
pub fn top_domains(records: &[GeoRecord], now: DateTime<FixedOffset>) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for r in records.iter().filter(|r| DateTime::parse_from_rfc3339(&r.at).is_ok_and(|at| now - at <= Duration::hours(24))) {
        for url in &r.cited_urls {
            let host = url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or_default().trim_start_matches("www.");
            if !host.is_empty() { *counts.entry(host.to_string()).or_default() += 1; }
        }
    }
    let mut sorted: Vec<_> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    sorted.into_iter().take(10).map(|(d, n)| format!("{d} {n}")).collect::<Vec<_>>().join("、")
}

/// Products the last 24 hours of answers named most often, Mars included when named.
fn top_products(records: &[GeoRecord], now: DateTime<FixedOffset>) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for r in records.iter().filter(|r| r.ok && DateTime::parse_from_rfc3339(&r.at).is_ok_and(|at| now - at <= Duration::hours(24))) {
        for p in &r.products { *counts.entry(p.trim().to_string()).or_default() += 1; }
    }
    let mut sorted: Vec<_> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    sorted.into_iter().take(8).map(|(p, n)| format!("{p} {n}")).collect::<Vec<_>>().join("、")
}

/// The page section; empty when the watch is off.
pub fn section(config: Option<&GeoConfig>, run: Option<&GeoRun>, records: &[GeoRecord], now: DateTime<FixedOffset>) -> String {
    let (Some(config), Some(run)) = (config, run) else { return String::new() };
    let table = rates(records, &run.hour, now);
    let mut md = format!("## {SECTION}\n\n");
    let (advice, dating) = (bucket_rates(records, "advice", now), bucket_rates(records, "dating", now));
    md.push_str("每小时轮换问 GPT、Claude、Gemini（开启联网搜索，经 AIsa；只有 GPT 能指定新加坡位置，Claude 拒绝 SG、Gemini 不支持指定位置）和豆包（火山方舟联网，问题库 market cn 的题）几道不提品牌的问题（geo-test 题库 branded false），由判定模型读答案：推荐率 = 答案里把 Mars 作为产品推荐的次数 ÷ 完整答案数，只算要工具的题（bucket tool）；纯建议题（advice）和恋爱题（dating）单列 7 天。首推 = Mars 排第一。样本少于 30 个时 10 个百分点以内的变化是噪声，看 24 小时与 7 天。\n\n");
    md.push_str("| 引擎 | 模型 | 本小时 | 近 24 小时 | 近 7 天 | 7 天首推 | 7 天引用 example.com | 7 天建议题 | 7 天恋爱题 | 7 天失败 |\n|---|---|---|---|---|---|---|---|---|---|\n");
    for engine in &config.engines {
        let (hour, day, week) = table.get(&engine.id).cloned().unwrap_or_default();
        let (a, d) = (advice.get(&engine.id).cloned().unwrap_or_default(), dating.get(&engine.id).cloned().unwrap_or_default());
        md.push_str(&format!("| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n", engine.label, engine.model, hour.share(), day.share(), week.share(), week.first, week.mars_cited, a.share(), d.share(), week.failed + a.failed + d.failed));
    }
    for u in &run.unmeasured { md.push_str(&format!("| {} | — | 未测量 | 未测量 | 未测量 | — | — | — | — | — |\n", u.label)); }
    md.push('\n');
    for u in &run.unmeasured { md.push_str(&format!("{}未测量：{}\n\n", u.label, u.reason)); }
    if let Some(skipped) = &run.skipped { md.push_str(&format!("本小时未提问：{skipped}。\n\n")); }
    let asked: Vec<String> = run.records.iter().map(|r| r.qid.clone()).fold(Vec::new(), |mut v, q| { if !v.contains(&q) { v.push(q); } v });
    if !asked.is_empty() { md.push_str(&format!("本小时题目：{}。", asked.join("、"))); }
    md.push_str(&format!("今日 AIsa 花费 ${:.2} / 上限 ${:.2}。\n\n", run.spent_today_usd, run.daily_budget_usd));
    let top = top_products(records, now);
    if !top.is_empty() { md.push_str(&format!("近 24 小时答案里最常被推荐的产品：{top}。\n\n")); }
    let domains = top_domains(records, now);
    if !domains.is_empty() { md.push_str(&format!("近 24 小时被引用最多的域名：{domains}。\n\n")); }
    md
}
