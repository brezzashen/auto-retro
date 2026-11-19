//! The `异常上升词（Google Trends）` section of the watchdog page and its findings: flagged words
//! (`新出现`, `快速上升`) first, within a flag the words waiting for a judgment, then judged
//! opportunities and watches, focus and brand movers, and the interest of every focus topic per day.

use super::watch_analyze::Finding;
use super::watch_trends::{BRAND, FOCUS, Heat, Pull, Rise, TrendsConfig, needs_judgment};
use super::watch_trends_ledger::{Label, Ledger, labels, ledger};
use super::watch_trends_market::{DEFAULT, geo, topic_pulls};
use super::watch_trends_surge::rise_rank;
use std::collections::BTreeMap;

pub const SECTION: &str = "异常上升词（Google Trends）";
const MAX_ROWS: usize = 30;
/// Google shows a rise at or above this value as Breakout.
const BREAKOUT: f64 = 5000.0;
const HIDDEN: [&str; 2] = ["噪声", "无关"];

fn cell(text: &str) -> String { text.replace('|', "/").replace('\n', " ") }

fn kind_label(r: &Rise) -> &'static str {
    match r.kind.as_str() {
        FOCUS if r.source.contains("similarweb:") => "竞品关注词",
        FOCUS if r.source.contains("topic:") => "关注领域词",
        FOCUS => "关注词上升",
        BRAND => "品牌/竞品",
        _ if r.source.contains("similarweb:") => "竞品异常词",
        _ => "未知新词",
    }
}

fn rise_text(value: f64) -> String {
    if value >= BREAKOUT { format!("+{value}%（Breakout）") } else { format!("+{value}%") }
}

fn amount(v: Option<f64>) -> String {
    v.map_or("—".into(), |v| if v >= 100.0 { format!("{v:.0}") } else { format!("{v:.1}").trim_end_matches(".0").to_string() })
}

fn times(v: Option<f64>) -> String { v.map_or("—".into(), |v| format!("{v:.1}×")) }

/// Weekly acceleration; a word with no complete week yet is new this week.
fn accel_text(r: &Rise) -> String {
    if r.accel == Some(0.0) && r.flag == super::watch_trends_surge::NEW { "本周才出现".into() } else { times(r.accel) }
}

/// A Google Trends explore link for the last 7 days in the riser's market (`US`, `JP`, ...), for a human look.
pub fn explore_link(query: &str, geo: &str) -> String {
    let encoded: String = query.bytes().map(|b| match b {
        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
        _ => format!("%{b:02X}"),
    }).collect();
    format!("[看趋势](https://trends.google.com/trends/explore?date=now%207-d&geo={geo}&q={encoded})")
}

/// Row order within a flag: waiting for a judgment, `机会`, `观察`, other judged, seed focus, brand;
/// `噪声` and `无关` stay hidden.
fn rank(r: &Rise, label: Option<&Label>) -> Option<u8> {
    match (label.map(|l| l.label.as_str()), r.kind.as_str()) {
        (Some(l), _) if HIDDEN.contains(&l) => None,
        (None, _) if needs_judgment(r) => Some(0),
        (Some("机会"), _) => Some(1),
        (Some("观察"), _) => Some(2),
        (Some(_), _) => Some(3),
        (None, FOCUS) => Some(4),
        _ => Some(5),
    }
}

/// One interest chart per market, the default market first.
fn heat_charts(heat: &[Heat]) -> String {
    let mut markets: Vec<&str> = heat.iter().map(|h| h.market.as_str()).collect();
    markets.dedup();
    markets.iter().map(|m| heat_chart(&heat.iter().filter(|h| h.market == *m).cloned().collect::<Vec<_>>(), if m.is_empty() { DEFAULT } else { m })).collect()
}

fn heat_chart(heat: &[Heat], market: &str) -> String {
    let Some(base) = heat.iter().max_by_key(|h| h.daily.len()) else { return String::new() };
    let days: Vec<&String> = base.daily.iter().map(|(d, _)| d).collect();
    let lines: Vec<String> = heat.iter().map(|h| {
        let by_day: BTreeMap<&String, f64> = h.daily.iter().map(|(d, v)| (d, *v)).collect();
        format!("line [{}]", days.iter().map(|d| format!("{}", by_day.get(d).copied().unwrap_or(0.0))).collect::<Vec<_>>().join(", "))
    }).collect();
    format!("### Focus topic interest per day ({market})\n\n```mermaid\nxychart-beta\ntitle \"Google Trends interest per day ({market}, 0-100)\"\n%% series: {}\nx-axis [{}]\ny-axis \"Interest\" 0 --> 100\n{}\n```\n\n",
        heat.iter().map(|h| h.topic.as_str()).collect::<Vec<_>>().join(", "),
        days.iter().map(|d| format!("\"{}\"", d.get(5..).unwrap_or(d))).collect::<Vec<_>>().join(", "), lines.join("\n"))
}

/// The topic as shown: `恋愛（JP）` outside the default market.
fn heat_name(h: &Heat) -> String { if h.market.is_empty() { h.topic.clone() } else { format!("{}（{}）", h.topic, h.market) } }

fn hot<'a>(pull: &'a Pull, cfg: &TrendsConfig) -> Vec<&'a Heat> {
    pull.heat.iter().filter(|h| h.prior > 0.0 && h.last24 >= h.prior * cfg.heat_ratio).collect()
}

fn surge_note(p: &Pull, cfg: &TrendsConfig) -> String {
    let Some(s) = &cfg.surge else { return String::new() };
    let flagged = |f: &str| p.rising.iter().filter(|r| r.flag == f).count();
    format!("体量对比（课程算法）：倍数 = Google Ads 最新月（{}）搜索量 ÷ 前 11 个月均量；前 11 月均量 < {} 且当月 ≥ {} 标「新出现」，倍数 ≥ {} 且当月 ≥ {} 标「快速上升」。月度数据晚一个月，所以待判断的前 {} 个词再看 12 个月周曲线：周加速 = 最近 2 个完整周 ÷ 之前的周均值，一年前同期 ≥ 最近一半标「季节回潮」，月度说涨而最近两周不到周均一半标「已回落」。本轮 新出现 {}、快速上升 {}；付费调用 {}。数字只排序，判断仍由复盘标签做。\n\n",
        if p.month.is_empty() { "—" } else { &p.month }, s.new_baseline, s.min_volume, s.min_ratio, s.min_volume, s.weekly_top,
        flagged(super::watch_trends_surge::NEW), flagged(super::watch_trends_surge::RISING),
        if p.calls.is_empty() { "无（缓存）".into() } else { p.calls.iter().map(|(k, n)| format!("{k} {n}")).collect::<Vec<_>>().join("、") })
}

/// The page section for the latest pull, or a line saying none has run.
pub fn section(pull: Option<&Pull>, cfg: &TrendsConfig, known: &Ledger, judged: &BTreeMap<String, Label>) -> String {
    let mut md = format!("## {SECTION}\n\n");
    let Some(p) = pull else { return md + "还没有抓过 Google Trends。\n\n<!-- vizWaiver: 还没有数据 -->\n\n" };
    let count = |kind: &str| p.rising.iter().filter(|r| r.kind == kind).count();
    let sw_count = p.rising.iter().filter(|r| r.source.contains("similarweb:")).count();
    let unjudged = p.rising.iter().filter(|r| needs_judgment(r) && !judged.contains_key(&r.query)).count();
    let warm: Vec<String> = hot(p, cfg).iter().map(|h| format!("{} {:.0} → {:.0}", heat_name(h), h.prior, h.last24)).collect();
    let markets: Vec<String> = std::iter::once(format!("美国 {}", cfg.topics.len())).chain(cfg.markets.iter().map(|m| format!("{} {}", m.label, m.topics.len()))).collect();
    md.push_str(&format!("{} 抓取（每 {} 小时一次，{}，并存 Similarweb（pro.similarweb.com 订阅，最近 28 天）竞品快照 {} 个）：{} 个关注领域（{}）、{} 个上升词，其中未知新词 {}、关注词 {}、品牌/竞品 {}，待判断 {unjudged}（未知新词、Similarweb 竞品异常词与领域拉取来的关注词）；自动排除 7 天内出现在多个种子下的填充词 {} 个。近 24 小时升温领域：{}。待判断的词由每小时复盘标签查清是什么后判 机会 / 观察 / 噪声 / 无关；只有判为机会的词进找词与同小时跟进。\n\n",
        p.at.get(..16).unwrap_or(&p.at).replace('T', " "), cfg.every_hours, cfg.time_range, sw_count, topic_pulls(cfg).len(), markets.join("、"), p.rising.len(), count(super::watch_trends::UNKNOWN), count(FOCUS), count(BRAND),
        p.noise, if warm.is_empty() { "无".into() } else { warm.join("、") }));
    md.push_str(&surge_note(p, cfg));
    md.push_str(&heat_charts(&p.heat));
    if p.heat.is_empty() { md.push_str("<!-- vizWaiver: 本轮没有拿到领域热度曲线，只列上升词清单 -->\n\n"); }
    let mut rows: Vec<(u8, &Rise)> = p.rising.iter().filter_map(|r| rank(r, judged.get(&r.query)).map(|k| (k, r))).collect();
    rows.sort_by(|a, b| rise_rank(a.1).cmp(&rise_rank(b.1)).then(a.0.cmp(&b.0)).then(b.1.value.total_cmp(&a.1.value))
        .then_with(|| b.1.volume.unwrap_or(0.0).total_cmp(&a.1.volume.unwrap_or(0.0))));
    if rows.is_empty() { return md + "本轮没有达到阈值的上升词。\n\n"; }
    md.push_str("| 词 | 类型 | 标记 | 上升 | 前11月均量 | 当月量 | 倍数 | 周加速 | 来源 | 首次出现 | 判断 | 说明 / 落地页 | 亲自看 |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for (_, r) in rows.into_iter().take(MAX_ROWS) {
        let label = judged.get(&r.query);
        let first = known.get(&r.query).map_or(p.at.as_str(), |s| s.first_seen.as_str());
        let note = label.map(|l| if l.page.is_empty() { l.note.clone() } else { format!("{}（{}）", l.note, l.page) }).unwrap_or_else(|| r.page.clone());
        md.push_str(&format!("| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n", cell(&r.query), kind_label(r),
            if r.flag.is_empty() { "—" } else { &r.flag }, rise_text(r.value), amount(r.base), amount(r.volume), times(r.surge), accel_text(r), cell(&r.source),
            first.get(5..16).unwrap_or(first).replace('T', " "), label.map_or("未判断", |l| l.label.as_str()), cell(&note), explore_link(&r.query, geo(&r.market))));
    }
    md + "\n"
}

/// P2 findings: words waiting for a judgment (flagged first) and focus topics heating up.
pub fn findings(pull: &Pull, cfg: &TrendsConfig, judged: &BTreeMap<String, Label>) -> Vec<Finding> {
    let mut waiting: Vec<&Rise> = pull.rising.iter().filter(|r| needs_judgment(r) && !judged.contains_key(&r.query)).collect();
    waiting.sort_by(|a, b| rise_rank(a).cmp(&rise_rank(b)).then(b.value.total_cmp(&a.value)));
    let mut out = Vec::new();
    if !waiting.is_empty() {
        let show = |r: &&Rise| if r.flag.is_empty() { format!("{}（{}）", r.query, rise_text(r.value)) } else { format!("{}（{}）", r.query, r.flag) };
        out.push(Finding { key: "trend:unjudged".into(), severity: "P2".into(),
            title: format!("{} 个异常上升词待判断：{}", waiting.len(), waiting.iter().take(5).map(show).collect::<Vec<_>>().join("、")),
            action: "每小时复盘标签先判断标「新出现」「快速上升」的词，逐个查清是什么，写入 trend-labels.jsonl（机会 / 观察 / 噪声 / 无关）".into() });
    }
    for h in hot(pull, cfg) {
        out.push(Finding { key: format!("trend:heat:{}:{}", heat_name(h), pull.at.get(..10).unwrap_or_default()), severity: "P2".into(),
            title: format!("关注领域「{}」近 24 小时 Google Trends 热度 {:.0}，是之前均值 {:.0} 的 {:.1} 倍", heat_name(h), h.last24, h.prior, h.last24 / h.prior),
            action: "复盘标签说明升温原因（新闻、模型或产品发布、节日）；与 Mars 相关的上升词判为机会".into() });
    }
    out
}

/// The page section and findings for this run, reading the ledger and labels from `state_dir`.
pub fn apply(pull: Option<&Pull>, cfg: Option<&TrendsConfig>, state_dir: &str) -> (String, Vec<Finding>) {
    let Some(cfg) = cfg else { return (String::new(), Vec::new()) };
    let judged = labels(state_dir);
    let found = pull.map(|p| findings(p, cfg, &judged)).unwrap_or_default();
    (section(pull, cfg, &ledger(state_dir), &judged), found)
}
