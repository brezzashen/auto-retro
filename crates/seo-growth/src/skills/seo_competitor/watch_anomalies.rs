//! Multi-dimensional SEO, search, indexing and funnel anomaly detection for the hourly watchdog.
//! Complements `watch_analyze` by catching cross-metric decoupling, Page-1 CTR leaks, keyword
//! cannibalization, sharp rank drops, SERP-vs-GSC divergence, canonical hijacks, CTA-to-signup
//! funnel breaks, suspected bot batches and Google-vs-Bing visibility gaps, each paired with a
//! conventional hypothesis (`hypothesis_a`) and an adversarial counter-hypothesis (`hypothesis_b`).

use super::watch_analyze::{Finding, QueryRow, Window};
use super::watch_bing::BingView;
use super::watch_config::WatchState;
use super::watch_machine_queries::{machine_mix, MachineQueryConfig};
use super::watch_rank::{base_path, in_serp_depth, normalize, production_path};
use super::watch_rank_history::{gap_by_day, previous_line};
use super::watch_rank_run::Ranks;
use super::watch_traffic::Traffic;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const SECTION: &str = "异常指标捕捉";
pub const CATEGORIES: [&str; 6] = [
    "搜索量价背离",
    "关键词内耗",
    "排名异动",
    "收录与规范页",
    "转化与异常批次",
    "跨引擎断层",
];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Anomaly {
    pub key: String,
    pub category: String,
    pub severity: String,
    pub subject: String,
    pub metric: String,
    pub current: String,
    pub baseline: String,
    pub hypothesis_a: String,
    pub hypothesis_b: String,
}

pub struct AnomalyInput<'a> {
    pub urls: &'a [String],
    pub state: &'a WatchState,
    pub gsc_pages: &'a Value,
    pub query_rows: &'a [QueryRow],
    pub machine_queries: &'a MachineQueryConfig,
    pub ranks: Option<&'a Ranks>,
    pub traffic: Option<&'a Traffic>,
    pub bing: Option<&'a BingView>,
    pub last: &'a Window,
    pub previous: &'a Window,
    pub site: &'a str,
    pub today: NaiveDate,
}

#[allow(clippy::too_many_arguments)]
fn item(key: String, cat: &str, sev: &str, subject: String, metric: &str, cur: String, base: String, ha: String, hb: String) -> Anomaly {
    Anomaly {
        key, category: cat.into(), severity: sev.into(), subject, metric: metric.into(),
        current: cur, baseline: base, hypothesis_a: ha, hypothesis_b: hb,
    }
}

fn gsc_window_anomalies(last: &Window, prev: &Window) -> Vec<Anomaly> {
    let mut out = Vec::new();
    if prev.impressions >= 20.0 && last.impressions < prev.impressions * 0.5 {
        out.push(item("anomaly:gsc-drop".into(), CATEGORIES[0], "P1", "GSC 24h 展示".into(), "展示数",
            format!("{:.0} 次（点击 {:.0}）", last.impressions, last.clicks), format!("前 24h {:.0} 次", prev.impressions),
            "主力页面掉出索引、规范页冲突或核心词跌出首页".into(),
            "前一日偶发热点词自然回落，或 Search Console 小时窗口刚跨过高峰时段".into()));
    } else if last.impressions >= 50.0 && prev.impressions >= 10.0 && last.impressions >= prev.impressions * 1.5 {
        out.push(item("anomaly:gsc-surge".into(), CATEGORIES[0], "P2", "GSC 24h 展示突增".into(), "展示数",
            format!("{:.0} 次（点击 {:.0}）", last.impressions, last.clicks), format!("前 24h {:.0} 次", prev.impressions),
            "新页面或新词进入前 20，趁势补强首段与 FAQ 转化点击".into(),
            "泛词低名次曝光或爬虫刷展示，点击并未同步放大，不宜盲目改标题".into()));
    }
    if last.impressions >= 80.0 && last.impressions >= prev.impressions * 1.25 && last.clicks <= prev.clicks {
        out.push(item("anomaly:ctr-decouple".into(), CATEGORIES[0], "P2", "全站展示涨点击平".into(), "量价关系",
            format!("展示 {:.0} / 点击 {:.0}", last.impressions, last.clicks), format!("前 24h {:.0} / {:.0}", prev.impressions, prev.clicks),
            "新增曝光页标题与摘要吸引力弱，需优化首屏钩子".into(),
            "新增展示集中在 30 名后长尾词或 AI Overview 零点击词，改标题无效".into()));
    }
    out
}

pub(super) fn page_ctr_anomalies(gsc_pages: &Value, rows: &[QueryRow], machine: &MachineQueryConfig, site: &str) -> Vec<Anomaly> {
    let mut out = Vec::new();
    for row in gsc_pages["rows"].as_array().into_iter().flatten() {
        let Some(url) = row["keys"][0].as_str() else { continue };
        let Some(path) = production_path(url, site) else { continue };
        let page = (row["impressions"].as_f64().unwrap_or(0.0), row["clicks"].as_f64().unwrap_or(0.0), row["position"].as_f64().unwrap_or(0.0));
        let mix = machine_mix(rows, url, machine);
        if let Some(m) = &mix {
            out.push(item(format!("anomaly:machine-queries:{path}"), CATEGORIES[0], "P2", path.into(), "机器长尾占比",
                format!("可见展示 {:.0} 中机器长尾 {:.0}；其余 {:.0} 展示 {:.0} 点击，均名 {:.1}", m.visible_impressions, m.machine_impressions, m.human.0, m.human.1, m.human.2),
                format!("页面行 {:.0} 展示 / {:.0} 点击，均名 {:.1}", page.0, page.1, page.2),
                "按真人查询判断首页 CTR，不为机器长尾改标题".into(),
                "若机器长尾是真实用户的新问法，应为其补 H2 而非剔除".into()));
        }
        let (impr, clicks, pos) = mix.map_or(page, |m| m.human);
        if impr < 50.0 || pos <= 0.0 { continue; }
        let ctr = clicks / impr;
        if pos <= 12.0 && ctr < 0.015 {
            out.push(item(format!("anomaly:page1-ctr:{path}"), CATEGORIES[0], "P1", path.into(), "首页附近 CTR",
                format!("CTR {:.2}%（{clicks:.0}/{impr:.0}，均名 {pos:.1}）", ctr * 100.0), "同位预期 ≥ 2.0%".into(),
                format!("重写 {path} 标题与描述，前置高频主查询词与免费试用卖点"),
                "标题若改于 7 天内则仍在混算旧数据（禁动）；或 SERP 被 AI Overview / 论坛贴截流".into()));
        } else if impr >= 80.0 && clicks == 0.0 {
            out.push(item(format!("anomaly:zero-click:{path}"), CATEGORIES[0], "P2", path.into(), "高展零点",
                format!("0 点击 / {impr:.0} 展示（均名 {pos:.1}）"), "≥ 1 点击".into(),
                format!("补充 {path} 对主查询词的直接回答与结构化 FAQ"),
                "展示分散在意图错位的泛词上，应查 query×page 分布而非盲目改页头".into()));
        }
    }
    out.truncate(8);
    out
}

fn locale_tag(path: &str) -> &'static str {
    ["/zh-CN", "/zh-TW", "/ja", "/ko", "/th", "/fr", "/it", "/es"].into_iter()
        .find(|p| path == *p || path.starts_with(&format!("{p}/"))).unwrap_or("en")
}

pub(super) fn cannibal_anomalies(rows: &[QueryRow], site: &str) -> Vec<Anomaly> {
    let mut grouped: BTreeMap<(String, String), BTreeMap<String, (f64, f64)>> = BTreeMap::new();
    for r in rows {
        let q = normalize(&r.query);
        if q.is_empty() || q.contains("mars") { continue; }
        let Some(path) = production_path(&r.page, site) else { continue };
        let entry = grouped.entry((locale_tag(path).into(), q)).or_default().entry(base_path(path).into()).or_default();
        entry.0 += r.impressions;
        entry.1 += r.position * r.impressions;
    }
    let mut hits: Vec<(f64, Anomaly)> = grouped.into_iter().filter_map(|((loc, q), pages)| {
        let mut active: Vec<(String, f64, f64)> = pages.into_iter()
            .filter(|(_, (impr, _))| *impr >= 5.0).map(|(p, (impr, w))| (p, impr, w / impr)).collect();
        if active.len() < 2 { return None; }
        active.sort_by(|a, b| b.1.total_cmp(&a.1));
        let total: f64 = active.iter().map(|x| x.1).sum();
        if total < 15.0 { return None; }
        let detail: Vec<String> = active.iter().take(3).map(|(p, i, pos)| format!("{p}({i:.0}次/{pos:.1}名)")).collect();
        Some((total, item(format!("anomaly:cannibal:{loc}:{q}"), CATEGORIES[1], "P2", format!("「{q}」({loc})"), "多页分流",
            detail.join(" vs "), "单一主力页承接".into(),
            format!("将次要页指向 {} 的锚文本与 H2 收敛，消除自相竞争", active[0].0),
            "两页若分别承接总览与细分场景，Google 正在测意图匹配，强行删词反而掉总展示".into())))
    }).collect();
    hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    hits.into_iter().take(6).map(|(_, a)| a).collect()
}

fn rank_anomalies(ranks: Option<&Ranks>, today: NaiveDate) -> Vec<Anomaly> {
    let Some(r) = ranks else { return Vec::new() };
    let mut out = Vec::new();
    let gaps = gap_by_day(&r.history, today, 7);
    if let [.., prev, cur] = gaps.as_slice() && cur.1 - prev.1 >= 3.0 {
        out.push(item("anomaly:gap-regress".into(), CATEGORIES[2], "P1", "全站加权距第一差距".into(), "收敛差距",
            format!("{:.1}（{}）", cur.1, cur.0), format!("{:.1}（{}）", prev.1, prev.0),
            "高搜索量目标词集体退步，优先排查掉位词对应页面改动与收录".into(),
            "新加入 targets 的高搜量新词初始未进前 100 拉高均值，属分母结构变化而非旧页退化".into()));
    }
    let day = today.format("%Y-%m-%d").to_string();
    for row in r.rows.iter().filter(|x| x.measured()) {
        // A drop counts only when both days read the same source: a word that enters the SERP depth
        // at 24 after a Search Console fallback of 4 has improved, not fallen.
        let same_source = previous_line(&r.history, &row.keyword, &day).is_none_or(|l| in_serp_depth(l.serp) == in_serp_depth(row.serp));
        if let Some(prev) = row.previous && prev <= 30.0 && row.rank - prev >= 5.0 && same_source {
            out.push(item(format!("anomaly:rank-drop:{}", row.keyword), CATEGORIES[2], "P1", format!("「{}」({})", row.keyword, row.page),
                "单日骤降", format!("第 {:.1} 名", row.rank), format!("前次 {:.1} 名", prev),
                format!("核查 {} 近期标题/H2 改动与反链，对标榜首 {}", row.page, row.leader),
                "单日 SERP 个性化或地域抖动，对比 GSC 7 天趋势再决定是否回滚文案".into()));
        } else if row.gsc_position > 0.0 && row.gsc_position <= 12.0 && row.serp.is_some_and(|s| s > 50.0) {
            out.push(item(format!("anomaly:serp-gsc:{}", row.keyword), CATEGORIES[2], "P2", format!("「{}」({})", row.keyword, row.page),
                "SERP/GSC 背离", format!("SERP >50 vs GSC {:.1}", row.gsc_position), format!("28d 展示 {:.0}", row.impressions),
                "最近几天刚发生掉位而 28 天均值尚未反映，需尽早补内链与用例".into(),
                "GSC 曝光来自其他英语地区（英/加/新）或个性化结果，美国主市场本就未进前 50".into()));
        }
    }
    out.truncate(8);
    out
}

pub(super) fn canonical_anomalies(urls: &[String], state: &WatchState, site: &str) -> Vec<Anomaly> {
    urls.iter().filter_map(|url| {
        let insp = state.inspections.get(url)?;
        let chosen = insp.google_canonical.trim_end_matches('/');
        if chosen.is_empty() || chosen == url.trim_end_matches('/') { return None; }
        let path = production_path(url, site).unwrap_or(url.as_str());
        Some(item(format!("anomaly:canonical:{path}"), CATEGORIES[3], "P1", path.into(), "Google 规范页偏离",
            format!("被归并至 {chosen}（{}）", insp.coverage), format!("声明 {url}"),
            format!("加厚 {path} 独有正文与差异化 H1/H2，消除模板骨架重复后重提索引"),
            "该页与目标页本就是高度重叠的薄内容，与其硬保收录不如合并 301 或精简公用外框".into()))
    }).take(6).collect()
}

pub(super) fn traffic_anomalies(traffic: Option<&Traffic>) -> Vec<Anomaly> {
    let Some(t) = traffic else { return Vec::new() };
    let mut out = Vec::new();
    let sum_seg = |offset: u32, pred: fn(&str) -> bool| -> (f64, f64, f64) {
        t.segments.iter().filter(|s| s.window_hours == 24 && s.offset_hours == offset && pred(&s.segment))
            // An account created in the window after a start click signed up, whichever sign-in method it used.
            .fold((0.0, 0.0, 0.0), |(v, c, u), r| (v + r.visitors, c + r.cta, u + if r.segment == "新注册" { r.signups.max(r.cta) } else { r.signups }))
    };
    let (new_v, new_cta, new_reg) = sum_seg(0, |s| s == "新访客" || s == "回访访客" || s == "新注册");
    if (new_cta >= 8.0 && new_reg == 0.0) || (new_cta >= 10.0 && new_reg / new_cta < 0.25) {
        out.push(item("anomaly:funnel-leak".into(), CATEGORIES[4], "P1", "新访客 CTA→注册漏斗".into(), "注册转化率",
            format!("访客 {new_v:.0} · 点 CTA {new_cta:.0} · 注册 {new_reg:.0}（{:.0}%）", new_reg / new_cta * 100.0), "点 CTA 后注册率 ≥ 25%".into(),
            "排查移动端/内置浏览器（微信、X、旧安卓内核）点击开始后未进引导或验证码受阻".into(),
            "私域分享页（/c/…）或未识别脚本访客点击页头 CTA 虚增分母，先按 PostHog 入口与设备拆真伪".into()));
    }
    let (all_now, _, _) = sum_seg(0, |_| true);
    let (all_prev, _, _) = sum_seg(24, |_| true);
    if all_prev >= 30.0 && all_now <= all_prev * 0.5 {
        out.push(item("anomaly:traffic-drop24h".into(), CATEGORIES[4], "P2", "近 24h 真实访客".into(), "滚动 24h 访客",
            format!("{all_now:.0} 人"), format!("前 24h {all_prev:.0} 人"),
            "检查自然搜索或主要引荐渠道是否断流、落地页是否异常".into(),
            "前 24h 含私域/社媒单帖脉冲移出时间窗，拆自然搜索子集若平稳则无需干预 SEO".into()));
    }
    for b in t.batches.iter().filter(|b| b.visitors >= 5.0 && (b.pageview_only >= 0.8 * b.visitors || b.browser.contains("Headless") || (b.os == "Linux" && b.timezone == "UTC"))).take(3) {
        out.push(item(format!("anomaly:bot-batch:{}:{}:{}", b.os, b.browser, b.timezone), CATEGORIES[4], "P2",
            format!("{} / {} ({})", b.os, b.browser, b.timezone), "疑似未过滤机器批次",
            format!("{:.0} 人（只看一页 {:.0}，IP {}）", b.visitors, b.pageview_only, b.ips), "已过滤机器 = 0".into(),
            "核实后把固定机房 IP 段或特征 UA 写入 machineIpPrefixes / MACHINE_AGENTS".into(),
            "社媒内置浏览器（X/微信）真实读者也有高只看一页率，先核停留与事件再封禁".into()));
    }
    out
}

pub(super) fn engine_gap_anomalies(ranks: Option<&Ranks>, bing: Option<&BingView>) -> Vec<Anomaly> {
    let (Some(r), Some(b)) = (ranks, bing) else { return Vec::new() };
    let g_top10: Vec<&str> = r.rows.iter().filter(|x| x.measured() && x.rank <= 10.0
        && b.ranks.get(&x.keyword).is_some_and(|br| br.position.is_none())).map(|x| x.keyword.as_str()).collect();
    let uncrawled = b.crawled.values().filter(|d| d.is_none()).count();
    if g_top10.is_empty() && uncrawled < 5 { return Vec::new(); }
    vec![item("anomaly:engine-gap".into(), CATEGORIES[5], "P2", "Google vs Bing/DDG".into(), "跨引擎收录与排名",
        format!("Google 前 10 而 Bing 首页外 {} 词；Bing 未抓取 {uncrawled} 页", g_top10.len()), "双引擎同步收录".into(),
        format!("对未抓取目标页重推 IndexNow 与 Bing SubmitUrlBatch（代表词：{}）", if g_top10.is_empty() { "—" } else { g_top10[0] }),
        "Bing 对低外链新站不靠主动提交发现内页，根因是外部高权重引用缺口而非页面文案".into())]
}

pub fn detect(input: &AnomalyInput) -> Vec<Anomaly> {
    let mut out = Vec::new();
    out.extend(gsc_window_anomalies(input.last, input.previous));
    out.extend(page_ctr_anomalies(input.gsc_pages, input.query_rows, input.machine_queries, input.site));
    out.extend(cannibal_anomalies(input.query_rows, input.site));
    out.extend(rank_anomalies(input.ranks, input.today));
    out.extend(canonical_anomalies(input.urls, input.state, input.site));
    out.extend(traffic_anomalies(input.traffic));
    out.extend(engine_gap_anomalies(input.ranks, input.bing));
    out.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.key.cmp(&b.key)));
    out
}

/// Actionable findings derived from anomalies, skipping keys `watch_analyze` already emitted.
pub fn findings(anomalies: &[Anomaly], existing: &[Finding]) -> Vec<Finding> {
    let keys: BTreeSet<&str> = existing.iter().map(|f| f.key.as_str()).collect();
    anomalies.iter().filter(|a| a.key != "anomaly:gsc-drop" || !keys.contains("trend:drop"))
        .filter(|a| a.severity == "P1" || a.category == CATEGORIES[1])
        .map(|a| Finding {
            key: a.key.clone(), severity: a.severity.clone(),
            title: format!("[{}] {}：{}（对照 {}）", a.category, a.subject, a.current, a.baseline),
            action: format!("常规假设：{}；对抗假设（先证伪再动手）：{}", a.hypothesis_a, a.hypothesis_b),
        }).collect()
}

fn cell(s: &str) -> String { s.replace('|', "/").replace('\n', " ") }

pub fn section(anomalies: &[Anomaly]) -> String {
    let mut md = format!("## {SECTION}\n\n");
    if anomalies.is_empty() {
        return md + "本小时六个异常维度（量价背离、关键词内耗、排名异动、收录规范页、转化与异常批次、跨引擎断层）均无超阈值异常。\n\n";
    }
    let counts: Vec<usize> = CATEGORIES.iter().map(|c| anomalies.iter().filter(|a| a.category == *c).count()).collect();
    let summary: Vec<String> = CATEGORIES.iter().zip(&counts).filter(|(_, n)| **n > 0).map(|(c, n)| format!("{c} {n}")).collect();
    md.push_str(&format!("本小时捕捉到 **{}** 项指标异常（{}）。每项同时给出常规动作假设与反方证伪假设，供主导会话与 Gemini 4 Carbon 对抗复盘裁决。\n\n",
        anomalies.len(), summary.join("、")));
    let max = counts.iter().copied().max().unwrap_or(1).max(1);
    md.push_str(&format!("```mermaid\nxychart-beta\ntitle \"各维度捕捉到的异常指标数\"\nx-axis [{}]\ny-axis \"异常数\" 0 --> {max}\nbar [{}]\n```\n\n",
        CATEGORIES.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", "),
        counts.iter().map(usize::to_string).collect::<Vec<_>>().join(", ")));
    md.push_str("| 级别 | 维度 | 对象 | 当前指标 | 基线/对照 | 假设 A（常规动作） | 假设 B（反方证伪视角） |\n|---|---|---|---|---|---|---|\n");
    for a in anomalies {
        md.push_str(&format!("| {} | {} | {} | {} | {} | {} | {} |\n",
            a.severity, a.category, cell(&a.subject), cell(&a.current), cell(&a.baseline), cell(&a.hypothesis_a), cell(&a.hypothesis_b)));
    }
    md + "\n"
}

#[cfg(test)]
#[path = "watch_anomalies_tests.rs"]
mod tests;
