//! `.claude/skills/seo-competitor/scripts/selfCase.ts`: the target's own search performance from
//! Search Console, as one case with a weekly line and a top-queries bar chart.

use super::charts::{bars, line};
use super::files::{at, items, num_or, show, text};
use super::format::int;
use crate::js_wire::js;
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

static BRAND: LazyLock<Regex> = LazyLock::new(|| Regex::new("(?i)mars").expect("regex"));

/// `selfCase(target, siteName)`: null when the target has no Search Console data.
pub fn self_case(target: &Value, site_name: &str) -> Option<Value> {
    let g = at(target, &["gsc"]);
    if g.is_null() {
        return None;
    }
    let queries = items(at(g, &["queries"]));
    let non_brand: Vec<&Value> = queries.iter().filter(|q| !BRAND.is_match(text(at(q, &["query"])))).collect();
    let top: Vec<&Value> = queries.iter().take(10).collect();
    let weeks = items(at(g, &["weeks"]));
    let week_names: Vec<String> = weeks.iter().map(|w| js::slice_lossy(text(at(w, &["week"])), 5, None)).collect();
    let week_impressions: Vec<f64> = weeks.iter().map(|w| num_or(at(w, &["impressions"]), 0.0)).collect();
    let peak = week_impressions.iter().fold(0.0_f64, |acc, v| js::max(acc, *v));
    let non_brand_clicks = non_brand.iter().fold(0.0, |acc, q| acc + num_or(at(q, &["clicks"]), 0.0));
    let mut diagrams = vec![json!({
        "title": "每周展示量（Google）",
        "mermaid": line("Google impressions per week", &week_names, &week_impressions, "Impressions"),
        "caption": format!("每周最多 {} 次展示，量级仍然很小。", js::number_string(peak)),
    })];
    if !top.is_empty() {
        let names: Vec<String> = top.iter().map(|q| text(at(q, &["query"])).to_string()).collect();
        let impressions: Vec<f64> = top.iter().map(|q| num_or(at(q, &["impressions"]), 0.0)).collect();
        diagrams.push(json!({
            "title": "展示量最多的搜索词",
            "mermaid": bars("Top queries by impressions", &names, &[impressions], "Impressions", None),
            "caption": "带 mars 的都是品牌词；非品牌词是 SEO 要做的方向。",
        }));
    }
    let actual: Vec<String> = top
        .iter()
        .take(5)
        .map(|q| format!("{}（{} 次展示，平均第 {}）", text(at(q, &["query"])), show(at(q, &["impressions"])), show(at(q, &["position"]))))
        .collect();
    let actual_text = if actual.is_empty() { "无".to_string() } else { actual.join("、") };
    Some(json!({
        "title": format!("我们自己的搜索表现（GSC）：{site_name}"),
        "verdict": if non_brand.iter().any(|q| num_or(at(q, &["clicks"]), 0.0) > 0.0) { "PASS" } else { "FAIL" },
        "reason": format!(
            "Search Console API 只读读取：{} 次点击、{} 次展示；非品牌词带来 {} 次点击。",
            int(num_or(at(g, &["clicks"]), 0.0)),
            int(num_or(at(g, &["impressions"]), 0.0)),
            int(non_brand_clicks),
        ),
        "expected": ["非品牌词带来点击"],
        "actual": [format!("展示最多的词：{actual_text}。")],
        "shotWaiver": "证据是 Search Console API 返回的数据，原始 JSON 保存在本地运行目录。",
        "diagrams": diagrams,
    }))
}
