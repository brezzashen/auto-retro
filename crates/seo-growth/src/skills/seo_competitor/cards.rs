//! `.claude/skills/seo-competitor/scripts/cards.ts`: one chart card plus one keyword-detail section per
//! competitor. The detail section repeats the keyword table and the MRR evidence rows.

use super::charts::{bars, line, pie};
use super::dashboard::{Mrr, m_of, sorted_by_mrr};
use super::files::{at, items, num_or, show, text, truthy};
use super::format::{confidence, growth_text, int, mrr_range, pct, pct_of, short_name, signal_name, tier_name, url_path, usd};
use super::metrics::WINNABLE_KD;
use crate::js_wire::js;
use serde_json::{Value, json};

pub const CARD_PREFIX: &str = "竞对卡片 · ";
pub const DETAIL_SUFFIX: &str = " · 关键词明细";

/// The MRR line of a card's body.
fn mrr_line(m: &Value) -> String {
    if truthy(m) && !truthy(at(m, &["na"])) {
        format!("MRR {}，置信度 {}（{}）", mrr_range(m), show(at(m, &["score"])), confidence(text(at(m, &["label"]))))
    } else {
        "MRR N/A".to_string()
    }
}

/// The chart card: the link line, then the traffic line, the keyword-type pie and the top-8 bars.
fn chart_card(c: &Value, m: &Value) -> Value {
    let name = short_name(text(at(c, &["name"])));
    let url = text(at(c, &["url"]));
    let points = items(at(c, &["growth", "points"]));
    let months: Vec<String> = points.iter().map(|p| js::slice_lossy(text(at(p, &["month"])), 2, None)).collect();
    let etvs: Vec<f64> = points.iter().map(|p| num_or(at(p, &["etv"]), 0.0)).collect();
    let top: Vec<&Value> = items(at(c, &["keywords"])).iter().take(8).collect();
    let body = format!(
        "[{url}]({url}) · {} · {} · 美国 SEO {}/月（12 个月 {}） · {}",
        text(at(c, &["positioning"])),
        mrr_line(m),
        int(num_or(at(c, &["usEtv"]), 0.0)),
        growth_text(at(c, &["growth"])),
        at(c, &["background"]).as_str().unwrap_or(""),
    );
    let shares = |key: &str| num_or(at(c, &["shares", key]), 0.0);
    let keyword_names: Vec<String> = top.iter().map(|k| text(at(k, &["keyword"])).to_string()).collect();
    let keyword_etv: Vec<f64> = top.iter().map(|k| num_or(at(k, &["etv"]), 0.0)).collect();
    json!({
        "title": format!("{CARD_PREFIX}{}", text(at(c, &["name"]))),
        "body": [body],
        "diagrams": [
            {
                "title": format!("{name}：美国 SEO 流量走势"),
                "mermaid": line("US SEO visits per month", &months, &etvs, "Visits"),
                "caption": format!("近 3 个月 {}。", pct_of(at(c, &["growth", "last3"]), true)),
            },
            {
                "title": format!("{name}：SEO 流量来自哪类词"),
                "mermaid": pie("Traffic share by keyword type", &[
                    ("品牌词".to_string(), shares("brand") * 100.0),
                    (format!("可抢（非品牌 KD≤{WINNABLE_KD}）"), shares("winnable") * 100.0),
                    ("难抢（非品牌 KD 高）".to_string(), shares("hard") * 100.0),
                ]),
                "caption": format!("可抢 {}：这部分流量 Mars 有机会拿走。", pct(Some(shares("winnable")), false)),
            },
            {
                "title": format!("{name}：预估流量前 8 的关键词"),
                "mermaid": bars("Top keywords by estimated US visits", &keyword_names, &[keyword_etv], "Visits", None),
                "caption": "词表、排名和落地页见下一节。",
            },
        ],
    })
}

/// The keyword table of the first twelve keywords, as Markdown.
fn keyword_table(c: &Value) -> String {
    let group_name = |group: &str| match group {
        "brand" => "品牌",
        "winnable" => "可抢",
        "hard" => "难抢",
        _ => "undefined",
    };
    let mut lines = vec!["| 关键词 | 类型 | 月搜索量 | KD | 排名 | 预估流量/月 | 落地页 |".to_string(), "|---|---|---:|---:|---:|---:|---|".to_string()];
    for k in items(at(c, &["keywords"])).iter().take(12) {
        let kd = at(k, &["kd"]);
        let kd_text = if kd.is_null() { "—".to_string() } else { show(kd) };
        let url = text(at(k, &["url"]));
        lines.push(format!(
            "| {} | {} | {} | {kd_text} | {} | {} | [{}]({url}) |",
            text(at(k, &["keyword"])),
            group_name(text(at(k, &["group"]))),
            int(num_or(at(k, &["volume"]), 0.0)),
            show(at(k, &["position"])),
            int(num_or(at(k, &["etv"]), 0.0)),
            url_path(url),
        ));
    }
    lines.join("\n")
}

/// One MRR evidence row: the signal name, its range, the tier, the source link and its date.
fn evidence_row(s: &Value) -> String {
    let base = num_or(at(s, &["mrr", "base"]), 0.0);
    let low = num_or(at(s, &["mrr", "low"]), 0.0);
    let high = num_or(at(s, &["mrr", "high"]), 0.0);
    let tier = text(at(s, &["tier"]));
    format!(
        "- {}：{}/月（{}–{}），{}，[来源]({})，{}",
        signal_name(&show(at(s, &["id"]))),
        usd(base),
        usd(low),
        usd(high),
        tier_name(tier),
        show(at(s, &["url"])),
        show(at(s, &["date"])),
    )
}

/// The keyword-detail section: the table, the top pages, the MRR evidence and the site figures.
fn detail_section(c: &Value, m: &Value) -> Value {
    let evidence: Vec<String> = if truthy(m) && !truthy(at(m, &["na"])) {
        items(at(m, &["signals"])).iter().map(evidence_row).collect()
    } else {
        vec!["- 没有找到订阅定价或收入证据，MRR 记为 N/A。".to_string()]
    };
    let pages: Vec<String> = items(at(c, &["pages"]))
        .iter()
        .take(3)
        .map(|p| {
            let url = text(at(p, &["url"]));
            format!("[{}]({url}) {}", url_path(url), int(num_or(at(p, &["etv"]), 0.0)))
        })
        .collect();
    let countries: Vec<String> = items(at(c, &["topCountries"])).iter().map(show).collect();
    let rank = at(c, &["rank"]);
    let referring = at(c, &["referringDomains"]);
    let mut body = vec![
        keyword_table(c),
        format!("**流量最多的页面**：{}", pages.join("；")),
        "**MRR 证据**：".to_string(),
    ];
    body.extend(evidence);
    body.push(format!(
        "**全站流量**：Similarweb 月访问 {}（{}），自然搜索 {}，付费搜索 {}；外链评分 {}，引用域名 {}。",
        int(num_or(at(c, &["visits"]), 0.0)),
        countries.join("、"),
        pct_of(at(c, &["organicShare"]), false),
        pct_of(at(c, &["paidShare"]), false),
        if rank.is_null() { "—".to_string() } else { show(rank) },
        int(num_or(referring, 0.0)),
    ));
    json!({
        "title": format!("{}{DETAIL_SUFFIX}", short_name(text(at(c, &["name"])))),
        "body": body,
        "vizWaiver": "关键词与证据明细表；对应图表在上一节卡片中。",
    })
}

/// One chart card and one detail section per competitor, largest MRR first.
pub fn competitor_sections(comps: &[Value], mrr: &Mrr) -> Vec<Value> {
    sorted_by_mrr(comps, mrr)
        .iter()
        .flat_map(|c| {
            let m = m_of(mrr, text(at(c, &["domain"])));
            [chart_card(c, m), detail_section(c, m)]
        })
        .collect()
}
