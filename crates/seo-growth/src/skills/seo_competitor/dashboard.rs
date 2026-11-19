//! `.claude/skills/seo-competitor/scripts/dashboard.ts`: chart-first overview cases (MRR, growth,
//! keyword share, win rate) and the master table. Competitors arrive as analysis.json objects.

use super::charts::{Quadrant, bars, log_scale, quadrant};
use super::files::{at, items, num, num_or, show, text, truthy};
use super::format::{growth_text, int, label_of, mrr_range, pct, pct_of, short_name};
use super::metrics::WINNABLE_KD;
use crate::js_wire::js;
use serde_json::{Value, json};
use std::collections::HashMap;

/// MRR objects by competitor domain, as `new Map(mrr.json rows)`.
pub type Mrr = HashMap<String, Value>;

const WAIVER: &str = "证据是 API 与公开网页数据，每家的来源链接在竞对卡片的关键词明细中。";

/// `mrr.get(domain)`: a missing row reads as null.
pub fn m_of<'a>(mrr: &'a Mrr, domain: &str) -> &'a Value {
    mrr.get(domain).unwrap_or(&super::files::NULL)
}

/// `mrr.get(domain)?.base ?? 0`.
fn base_of(mrr: &Mrr, domain: &str) -> f64 {
    num_or(at(m_of(mrr, domain), &["base"]), 0.0)
}

fn domain_of(c: &Value) -> &str {
    text(at(c, &["domain"]))
}

fn name_of(c: &Value) -> &str {
    text(at(c, &["name"]))
}

fn short_of(c: &Value) -> String {
    short_name(name_of(c))
}

/// The last point's etv of a competitor's history, 0 when there is none.
fn last_etv(c: &Value) -> f64 {
    items(at(c, &["growth", "points"])).last().map_or(0.0, |p| num_or(at(p, &["etv"]), 0.0))
}

fn last3_of(c: &Value) -> Option<f64> {
    num(at(c, &["growth", "last3"]))
}

/// Competitors sorted by MRR base, largest first (stable).
pub fn sorted_by_mrr(comps: &[Value], mrr: &Mrr) -> Vec<Value> {
    let mut out = comps.to_vec();
    out.sort_by(|a, b| base_of(mrr, domain_of(b)).partial_cmp(&base_of(mrr, domain_of(a))).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Signed log scale so a +1000% outlier does not flatten the rest; 0.5 is no change.
fn growth_y(g: Option<f64>, max: f64) -> f64 {
    match g {
        None => 0.5,
        Some(g) => 0.5 + (0.45 * js::sign(g) * g.abs().ln_1p()) / max.ln_1p(),
    }
}

fn growth_bars(comps: &[Value]) -> String {
    let mut sorted: Vec<&Value> = comps.iter().filter(|c| last3_of(c).is_some()).collect();
    sorted.sort_by(|a, b| last3_of(b).partial_cmp(&last3_of(a)).unwrap_or(std::cmp::Ordering::Equal));
    let names: Vec<String> = sorted.iter().map(|c| short_of(c)).collect();
    let values: Vec<f64> = sorted.iter().map(|c| js::max(-100.0, js::min(300.0, last3_of(c).unwrap_or(0.0) * 100.0))).collect();
    bars("US SEO traffic change over the last 3 months (%)", &names, &[values], "%", Some(300.0)).replacen("0 --> 300", "-100 --> 300", 1)
}

/// The MRR overview: median bars, then the low / base / high bars per competitor.
pub fn mrr_case(comps: &[Value], mrr: &Mrr) -> Value {
    let ranked: Vec<Value> = sorted_by_mrr(&comps.iter().filter(|c| truthy(m_of(mrr, domain_of(c))) && !truthy(at(m_of(mrr, domain_of(c)), &["na"]))).cloned().collect::<Vec<_>>(), mrr);
    let names: Vec<String> = ranked.iter().map(short_of).collect();
    let column = |field: &str| -> Vec<f64> { ranked.iter().map(|c| num_or(at(m_of(mrr, domain_of(c)), &[field]), 0.0) / 1000.0).collect() };
    let na: Vec<&str> = comps.iter().filter(|c| !truthy(m_of(mrr, domain_of(c))) || truthy(at(m_of(mrr, domain_of(c)), &["na"]))).map(name_of).collect();
    let top: Vec<String> = ranked.iter().take(3).map(|c| format!("{} {}", short_of(c), mrr_range(m_of(mrr, domain_of(c))))).collect();
    let na_note = if na.is_empty() { String::new() } else { format!("，{} 记为 N/A", na.join("、")) };
    json!({
        "title": "竞对 MRR 估算",
        "verdict": "NOT-VERIFIED",
        "reason": format!("没有一家公开 MRR；{} 家给出估算区间{na_note}。", ranked.len()),
        "expected": ["这个竞对大概 MRR 是多少"],
        "actual": [format!("中位数前三：{}。", top.join("、"))],
        "shotWaiver": WAIVER,
        "diagrams": [
            {
                "title": "MRR 估算中位数（千美元/月）",
                "mermaid": bars("Estimated MRR, base case (USD K per month)", &names, &[column("base")], "USD K", None),
                "caption": "按中位数排序；置信度见总表。",
            },
            {
                "title": "MRR 区间：每家三根柱依次是低 / 中 / 高",
                "mermaid": bars("MRR range per competitor: low, base, high (USD K)", &names, &[column("low"), column("base"), column("high")], "USD K", None),
                "caption": "柱越分散，证据越弱。",
            },
        ],
    })
}

/// The growth overview: 3-month change bars and the size × growth quadrant.
pub fn growth_case(comps: &[Value], target: &Value) -> Value {
    let mut sorted = comps.to_vec();
    sorted.sort_by(|a, b| last_etv(b).partial_cmp(&last_etv(a)).unwrap_or(std::cmp::Ordering::Equal));
    let max_growth = comps.iter().fold(1.0_f64, |acc, c| js::max(acc, last3_of(c).unwrap_or(0.0).abs()));
    let points = items(at(target, &["growth", "points"]));
    let window = format!("{} → {}", points.first().map_or("undefined".to_string(), |p| label_of(p, "month")), points.last().map_or("undefined".to_string(), |p| label_of(p, "month")));
    let rising: Vec<String> = sorted.iter().filter(|c| last3_of(c).unwrap_or(0.0) > 0.3).map(|c| format!("{} {}", short_of(c), pct_of(at(c, &["growth", "last3"]), true))).collect();
    let quad = Quadrant {
        title: "Size vs 3-month growth".to_string(),
        x_axis: ["Small".to_string(), "Large".to_string()],
        y_axis: ["Shrinking".to_string(), "Growing".to_string()],
        quadrants: ["Big and rising".to_string(), "Small but rising".to_string(), "Small and fading".to_string(), "Big but fading".to_string()],
        points: sorted.iter().map(|c| (short_of(c), log_scale(last_etv(c), 100.0, 50000.0), growth_y(last3_of(c), max_growth))).collect(),
    };
    json!({
        "title": "竞对增长：美国 SEO 流量趋势",
        "verdict": "PASS",
        "reason": format!("用 DataForSEO 月度历史比较 {window} 的美国 SEO 预估流量。"),
        "expected": ["对方的增长"],
        "actual": [format!("近 3 个月增长超过 30%：{}。", if rising.is_empty() { "无".to_string() } else { rising.join("、") })],
        "shotWaiver": WAIVER,
        "diagrams": [
            {"title": "近 3 个月美国 SEO 流量增速（%，超过 300% 截断为 300）", "mermaid": growth_bars(comps), "caption": "向右是增长，向左是下滑。"},
            {"title": "规模 × 近 3 个月增速", "caption": "右上角 = 已经大且还在涨，是最需要盯的对手。", "mermaid": quadrant(&quad)},
        ],
    })
}

/// Organic share by keyword type, and global against US organic visits.
pub fn share_case(comps: &[Value]) -> Value {
    let mut sorted = comps.to_vec();
    sorted.sort_by(|a, b| share_of(b, "winnable").partial_cmp(&share_of(a, "winnable")).unwrap_or(std::cmp::Ordering::Equal));
    let names: Vec<String> = sorted.iter().map(short_of).collect();
    let global_value = |c: &Value| num_or(at(c, &["visits"]), 0.0) * num_or(at(c, &["organicShare"]), 0.0);
    let mut global = comps.to_vec();
    global.sort_by(|a, b| global_value(b).partial_cmp(&global_value(a)).unwrap_or(std::cmp::Ordering::Equal));
    let top: Vec<String> = sorted.iter().take(3).map(|c| format!("{} {}", short_of(c), pct(Some(share_of(c, "winnable")), false))).collect();
    json!({
        "title": "竞对关键词占比与预估流量",
        "verdict": "PASS",
        "reason": format!("每家前 200 个美国排名词按流量分成三类：品牌词、可抢词（非品牌且 KD ≤ {WINNABLE_KD}）、难抢词。"),
        "expected": ["这个竞对打了哪些词，这些词预估有多少流量", "把竞对的官网链接发给我"],
        "actual": [format!("可抢流量占比最高：{}。每家的词表、落地页和官网在竞对卡片里。", top.join("、"))],
        "shotWaiver": WAIVER,
        "diagrams": [
            {
                "title": format!("每家两根柱：前 = 品牌词流量占比，后 = 可抢词（非品牌，KD ≤ {WINNABLE_KD}）流量占比"),
                "mermaid": bars("Share of SEO traffic: brand terms vs winnable terms (%)", &names, &[sorted.iter().map(|c| share_of(c, "brand") * 100.0).collect(), sorted.iter().map(|c| share_of(c, "winnable") * 100.0).collect()], "%", Some(100.0)),
                "caption": "后一根越长，这家越值得抢词；前一根长说明它靠品牌，抢不走。",
            },
            {
                "title": "自然搜索访问：前 = Similarweb 全球，后 = DataForSEO 美国",
                "mermaid": bars("Organic search visits per month: global vs US", &global.iter().map(short_of).collect::<Vec<_>>(), &[global.iter().map(global_value).collect(), global.iter().map(|c| num_or(at(c, &["usEtv"]), 0.0)).collect()], "Visits", None),
                "caption": "前后差很多，说明流量主要不在美国。",
            },
        ],
    })
}

fn share_of(c: &Value, key: &str) -> f64 {
    num_or(at(c, &["shares", key]), 0.0)
}

/// The win-rate overview: now against after the link plan, and win rate against volume.
pub fn win_case(keywords: &[Value], target: &Value, planned_rank: f64) -> Value {
    let mut sorted = keywords.to_vec();
    sorted.sort_by(|a, b| num_or(&b["winNow"], 0.0).partial_cmp(&num_or(&a["winNow"], 0.0)).unwrap_or(std::cmp::Ordering::Equal));
    let best: Vec<String> = sorted.iter().filter(|k| num_or(&k["winNow"], 0.0) >= 0.5).map(|k| format!("{}（{}）", text(&k["keyword"]), pct_of(&k["winNow"], false))).collect();
    let rank = show(at(target, &["rank"]));
    let quad = Quadrant {
        title: "Win rate vs search volume".to_string(),
        x_axis: ["Low win rate".to_string(), "High win rate".to_string()],
        y_axis: ["Low volume".to_string(), "High volume".to_string()],
        quadrants: ["Do first".to_string(), "Worth it but hard".to_string(), "Skip".to_string(), "Easy long tail".to_string()],
        points: sorted.iter().map(|k| (text(&k["keyword"]).to_string(), (num_or(&k["winNow"], 0.0) - 0.2) / 0.6, log_scale(num_or(&k["volume"], 0.0), 50.0, 40000.0))).collect(),
    };
    let names: Vec<String> = sorted.iter().map(|k| text(&k["keyword"]).to_string()).collect();
    json!({
        "title": "我们的胜率：Mars 能进前 10 的概率",
        "verdict": "NOT-VERIFIED",
        "reason": format!("估算公式：70% × 前 10 名里外链实力不高于 Mars+10 的站点占比 + 30% × (1 − KD)。Mars 现在外链评分 {rank}，计划补到 {}。", js::number_string(planned_rank)),
        "expected": ["我们的胜率"],
        "actual": [format!("现在胜率 ≥ 50% 的词：{}。", if best.is_empty() { "无".to_string() } else { best.join("、") })],
        "shotWaiver": WAIVER,
        "diagrams": [
            {
                "title": format!("每个词两根柱：前 = 现在（评分 {rank}），后 = 外链补到评分 {} 后", js::number_string(planned_rank)),
                "mermaid": bars("Win rate per target keyword: now vs after links (%)", &names, &[sorted.iter().map(|k| num_or(&k["winNow"], 0.0) * 100.0).collect(), sorted.iter().map(|k| num_or(&k["winPlanned"], 0.0) * 100.0).collect()], "%", Some(100.0)),
                "caption": "前一根高的词现在就能做；后一根明显变高的词要先补外链。",
            },
            {"title": "胜率 × 搜索量", "caption": "右上角 = 胜率高且搜索量大，优先做。", "mermaid": quadrant(&quad)},
        ],
    })
}

/// The master table: one row per competitor, largest MRR first.
pub fn master_table(comps: &[Value], mrr: &Mrr) -> Value {
    let rows: Vec<Value> = sorted_by_mrr(comps, mrr).iter().map(|c| master_row(c, mrr)).collect();
    json!({
        "title": "竞对总表",
        "columns": ["竞品", "官网", "MRR 估算（中位，区间）", "美国 SEO 流量/月", "12 个月增长", "品牌词占比", "可抢流量占比", "流量最大的非品牌词"],
        "rows": rows,
        "note": "MRR 为估算；SEO 流量、增长、占比来自 DataForSEO（美国 Google）。可抢 = 非品牌且 KD ≤ 20。",
    })
}

fn master_row(c: &Value, mrr: &Mrr) -> Value {
    let keywords = items(at(c, &["keywords"]));
    let nb = keywords.iter().find(|k| text(at(k, &["group"])) != "brand");
    let domain = domain_of(c);
    let last = nb.map_or("—".to_string(), |k| format!("{}（第 {}，{}/月）", text(at(k, &["keyword"])), show(at(k, &["position"])), int(num_or(at(k, &["etv"]), 0.0))));
    json!([
        format!("{}｜{}", short_of(c), text(at(c, &["positioning"]))),
        format!("[{domain}]({})", text(at(c, &["url"]))),
        mrr_range(m_of(mrr, domain)),
        int(num_or(at(c, &["usEtv"]), 0.0)),
        growth_text(at(c, &["growth"])),
        pct_of(at(c, &["shares", "brand"]), false),
        pct_of(at(c, &["shares", "winnable"]), false),
        last,
    ])
}
