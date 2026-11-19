//! `.claude/skills/seo-competitor/scripts/mrrEstimate.ts`: adds the uniform Similarweb traffic-funnel
//! signal to each `<run>/mrr/<domain>.json` evidence file and writes `<run>/mrr.json` with one MRR
//! range per competitor. `mrrEstimate <run-dir>`.

use super::config::{RunConfig, load_config, raw_dir};
use super::estimator::{Signal, estimate};
use super::files::{at, fail, items, num_or, read_json, show, stringify, text, to_pretty_one, truthy, write_text};
use crate::js_wire::js;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::path::Path;
use std::sync::LazyLock;

static ARPA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"ARPA:\s*low=([0-9.]+)\s*base=([0-9.]+)\s*high=([0-9.]+)").expect("regex"));

/// Paying accounts as a share of monthly visits, by business kind (low, base, high).
pub fn conversion(kind: &str) -> Option<[f64; 3]> {
    match kind {
        "saas" => Some([0.002, 0.006, 0.015]),
        "consumer" => Some([0.001, 0.003, 0.008]),
        _ => None,
    }
}

/// `ARPA: low=<n> base=<n> high=<n>` from the notes of an evidence file.
fn parse_arpa(notes: &[Value]) -> Option<[f64; 3]> {
    let joined = notes.iter().map(|n| if n.is_null() { String::new() } else { show(n) }).collect::<Vec<_>>().join("\n");
    let caps = ARPA.captures(&joined)?;
    Some([js::to_number(&caps[1]), js::to_number(&caps[2]), js::to_number(&caps[3])])
}

fn range_json(low: f64, base: f64, high: f64) -> Value {
    json!({"low": low, "base": base, "high": high})
}

/// The traffic-funnel signal built from the Similarweb visits and the pricing-page ARPA.
fn funnel_signal(domain: &str, name: &str, kind: &str, visits: f64, arpa: [f64; 3], history_to: &str, publisher: &str) -> Result<Value, String> {
    let conv = conversion(kind).ok_or_else(|| "bad number undefined".to_string())?;
    Ok(json!({
        "id": "similarweb-traffic-funnel",
        "dimension": "product_traction",
        "metric": "traffic_funnel",
        "monthly_visits": range_json(js::round(visits * 0.8), visits, js::round(visits * 1.2)),
        "paid_conversion_rate": range_json(conv[0], conv[1], conv[2]),
        "monthly_arpa": range_json(arpa[0], arpa[1], arpa[2]),
        "independence_group": "similarweb-traffic",
        "source": {
            "publisher": publisher,
            "url": format!("https://www.similarweb.com/website/{domain}/"),
            "date": history_to,
            "tier": "third_party_estimate",
        },
        "supporting_sources": [{
            "dimension": "pricing",
            "publisher": name,
            "url": format!("https://{domain}/"),
            "date": history_to,
            "notes": "ARPA from the evidence file's pricing note.",
        }],
        "notes": "Uniform assumption: paying accounts as a share of monthly visits, times the pricing-page ARPA.",
    }))
}

/// One signal as the output lists it: optional fields only when the source had them.
fn signal_json(s: &Signal) -> Value {
    let mut out = Map::new();
    if let Some(id) = &s.id {
        out.insert("id".into(), id.clone());
    }
    out.insert("metric".into(), json!(s.metric));
    out.insert("tier".into(), json!(s.tier));
    if let Some(url) = &s.url {
        out.insert("url".into(), url.clone());
    }
    if let Some(date) = &s.date {
        out.insert("date".into(), date.clone());
    }
    out.insert("mrr".into(), range_json(s.mrr.low, s.mrr.base, s.mrr.high));
    Value::Object(out)
}

/// The result row of one competitor with a fresh estimate.
fn estimated_row(domain: &str, input: &Value) -> Result<Value, String> {
    let r = estimate(input)?;
    Ok(json!({
        "domain": domain,
        "low": r.low,
        "base": r.base,
        "high": r.high,
        "status": r.status,
        "score": r.score,
        "label": r.label,
        "contradiction": r.contradiction,
        "signals": r.signals.iter().map(signal_json).collect::<Vec<_>>(),
    }))
}

/// One competitor: its evidence file, the funnel signal, and the estimate (or N/A).
fn competitor_row(run_dir: &str, config: &RunConfig, competitor: &super::config::Competitor) -> Result<Value, String> {
    let domain = competitor.domain.as_str();
    let path = format!("{run_dir}/mrr/{domain}.json");
    if !Path::new(&path).exists() {
        return Ok(json!({"domain": domain, "na": true, "reason": "no evidence file"}));
    }
    let mut doc = read_json(&path)?;
    let arpa = parse_arpa(items(at(&doc, &["notes"])));
    let sw_file = read_json(&format!("{}/sw/sw_{domain}.json", raw_dir(run_dir)))?;
    let sw = at(&sw_file, &["data"]).clone();
    // A snapshot names its own source; one without it was collected through Monid.
    let publisher = sw_file["source"].as_str().unwrap_or("Similarweb via Monid");
    let mut signals: Vec<Value> = items(at(&doc, &["signals"])).iter().filter(|s| text(at(s, &["metric"])) != "traffic_funnel").cloned().collect();
    let visits = num_or(at(&sw, &["monthlyVisits"]), 0.0);
    if let Some(arpa) = arpa.filter(|a| a[1] > 0.0 && visits != 0.0 && truthy(at(&sw, &["monthlyVisits"]))) {
        signals.push(funnel_signal(domain, &competitor.name, &competitor.kind, visits, arpa, &config.history_to, publisher)?);
    }
    if let Some(object) = doc.as_object_mut() {
        object.insert("signals".into(), Value::Array(signals.clone()));
    }
    if signals.is_empty() {
        return Ok(json!({"domain": domain, "na": true, "reason": "no recurring pricing or revenue evidence"}));
    }
    let mut input = doc.clone();
    let as_of = if truthy(at(&doc, &["as_of"])) { at(&doc, &["as_of"]).clone() } else { json!(config.history_to) };
    if let Some(object) = input.as_object_mut() {
        object.insert("as_of".into(), as_of);
    }
    estimated_row(domain, &input)
}

fn run(run_dir: &str) -> Result<String, String> {
    let (_raw, config) = load_config(run_dir)?;
    let mut results = Vec::with_capacity(config.competitors.len());
    for competitor in &config.competitors {
        results.push(competitor_row(run_dir, &config, competitor)?);
    }
    let estimated = results.iter().filter(|r| !r["na"].as_bool().unwrap_or(false)).count();
    let na = results.len() - estimated;
    let path = format!("{run_dir}/mrr.json");
    write_text(&path, &to_pretty_one(&Value::Array(results))?)?;
    stringify(&json!({"mrr": path, "estimated": estimated, "na": na}))
}

/// `bun mrrEstimate.ts <run-dir>`.
pub fn main(argv: &[String]) -> i32 {
    let Some(run_dir) = argv.first().filter(|d| !d.is_empty()) else {
        eprintln!("usage: bun mrrEstimate.ts <run-dir>");
        return 2;
    };
    match run(run_dir) {
        Ok(line) => {
            println!("{line}");
            0
        }
        Err(message) => fail(&message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn funnel_signal_names_the_snapshot_source_as_publisher() {
        let signal = funnel_signal("attio.com", "Attio", "saas", 1000.0, [10.0, 20.0, 30.0], "2026-10-01", "Similarweb Pro via pro.similarweb.com").unwrap();
        assert_eq!(signal["source"]["publisher"], "Similarweb Pro via pro.similarweb.com");
        assert_eq!(signal["monthly_visits"]["base"], 1000.0);
    }
}
