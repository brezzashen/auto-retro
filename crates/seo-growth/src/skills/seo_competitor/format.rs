//! `.claude/skills/seo-competitor/scripts/format.ts`: number and label formatting for the report builders.

use super::files::{at, num, show, truthy};
use crate::js_wire::js;
use serde_json::Value;
use std::sync::LazyLock;

static NAME_NOTE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new("（.*）").expect("regex"));
static SIGNAL_LATKA: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new("latka").expect("regex"));
static SIGNAL_CUSTOMERS: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new("customers").expect("regex"));
static SIGNAL_THIRD: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new("adapty|screensdesign").expect("regex"));

/// `usd(n)`: `$1.2M`, `$3K` or `$450`.
pub fn usd(n: f64) -> String {
    if n >= 1e6 {
        format!("${}M", js::to_fixed(n / 1e6, 1))
    } else if n >= 1000.0 {
        format!("${}K", js::number_string(js::round(n / 1000.0)))
    } else {
        format!("${}", js::number_string(js::round(n)))
    }
}

/// `int(n)`: `Math.round(n).toLocaleString("en-US")`.
pub fn int(n: f64) -> String {
    let rounded = js::round(n);
    if !rounded.is_finite() {
        return js::number_string(rounded);
    }
    if rounded == 0.0 {
        return "0".to_string();
    }
    let digits = format!("{:.0}", rounded.abs());
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if rounded < 0.0 { format!("-{grouped}") } else { grouped }
}

/// `pct(x, signed)`: a whole percentage, `—` for null.
pub fn pct(x: Option<f64>, signed: bool) -> String {
    match x {
        None => "—".to_string(),
        Some(x) => {
            let sign = if signed && x > 0.0 { "+" } else { "" };
            format!("{sign}{}%", js::number_string(js::round(x * 100.0)))
        }
    }
}

/// `pct` of a JSON field: null or a missing field is `—`.
pub fn pct_of(value: &Value, signed: bool) -> String {
    pct(num(value), signed)
}

/// `shortName(name)`: the name without its full-width parenthesised note.
pub fn short_name(name: &str) -> String {
    NAME_NOTE.replace(name, "").into_owned()
}

/// Node's `new URL(url).pathname`, shortened to 36 units and an ellipsis when longer than 38.
pub fn url_path(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(parsed) => {
            let path = parsed.path();
            if js::len(path) > 38 { format!("{}…", js::prefix_lossy(path, 36)) } else { path.to_string() }
        }
        Err(_) => url.to_string(),
    }
}

/// `mrrRange(m)`: `$1.2K（$900–$1.5K）`, or N/A.
pub fn mrr_range(m: &Value) -> String {
    if m.is_null() || truthy(at(m, &["na"])) {
        return "N/A".to_string();
    }
    let (base, low, high) = (num_at(m, "base"), num_at(m, "low"), num_at(m, "high"));
    format!("{}（{}–{}）", usd(base), usd(low), usd(high))
}

/// A numeric field of an MRR object (0 when absent).
pub fn num_at(value: &Value, key: &str) -> f64 {
    at(value, &[key]).as_f64().unwrap_or(0.0)
}

/// `growthText(g)`: the year-over-year change, `新站` for a series that starts at zero, or `—`.
pub fn growth_text(g: &Value) -> String {
    let yoy = at(g, &["yoy"]);
    if !yoy.is_null() {
        return pct_of(yoy, true);
    }
    let points = at(g, &["points"]).as_array();
    let last_etv = points.and_then(|p| p.last()).map(|p| at(p, &["etv"]).clone());
    if last_etv.is_some_and(|v| truthy(&v)) { "新站".to_string() } else { "—".to_string() }
}

/// `CONFIDENCE[label]`; an unknown label prints as `undefined`, as the TypeScript does.
pub fn confidence(label: &str) -> &'static str {
    match label {
        "high" => "高",
        "medium" => "中",
        "low" => "低",
        "very low" => "很低",
        _ => "undefined",
    }
}

/// `TIER[tier] ?? tier`: the Chinese name of an evidence tier, or the tier itself.
pub fn tier_name(tier: &str) -> String {
    let name = match tier {
        "third_party_estimate" => "第三方估算",
        "derived_public" => "公开数据推算",
        "company_disclosure" => "公司披露",
        "reputable_press" => "媒体报道",
        "market_prior" => "同类先验",
        "third_party_verified" => "第三方验证",
        _ => tier,
    };
    name.to_string()
}

/// `signalName(id)`: the readable name of a revenue signal.
pub fn signal_name(id: &str) -> String {
    let name = if id == "similarweb-traffic-funnel" {
        "流量漏斗（Similarweb 访问 × 统一转化率 × 定价页客单价）"
    } else if id == "app-store-rating-heuristic" {
        "App Store 美区评分推算付费用户"
    } else if SIGNAL_LATKA.is_match(id) {
        "Getlatka 收入估算"
    } else if SIGNAL_CUSTOMERS.is_match(id) {
        "客户数 × 客单价"
    } else if SIGNAL_THIRD.is_match(id) {
        "第三方月收入估算"
    } else {
        id
    };
    name.to_string()
}


/// `${x}` of an optional JSON field, `undefined` when absent.
pub fn label_of(value: &Value, key: &str) -> String {
    let field = at(value, &[key]);
    if field.is_null() { "undefined".to_string() } else { show(field) }
}
