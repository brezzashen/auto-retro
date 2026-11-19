//! Readers of the AIsa (DataForSEO) answers behind the `aisaSeo` block: AI citations in an organic SERP's
//! AI Overview or a Google AI Mode answer, keyword intent and difficulty, and daily new/lost backlinks.

use super::files::{items, text};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Our host; a citation of it or a subdomain counts as Mars cited.
pub const OUR_HOST: &str = "example.com";

/// One target keyword's AI answer: Google's AI Overview on the organic SERP, or the AI Mode answer.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AiCite {
    pub keyword: String,
    pub page: String,
    pub location: u32,
    /// Whether Google showed an AI answer at all.
    pub present: bool,
    /// The AI Overview loads after the page (DataForSEO `asynchronous_ai_overview`), so its sources are unknown.
    #[serde(default)]
    pub deferred: bool,
    pub mars_cited: bool,
    /// Cited domains in answer order, without Google's own.
    pub domains: Vec<String>,
    /// RFC 3339 time the answer was fetched (AI Mode) or the SERP's day (AI Overview).
    pub at: String,
    /// Why the answer could not be read; such a row proves nothing either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn is_ours(domain: &str) -> bool { domain == OUR_HOST || domain.ends_with(&format!(".{OUR_HOST}")) }

/// Every `references[].domain` under `value`, in order, once each, without google.com.
pub fn cited_domains(value: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                for r in map.get("references").map(items).unwrap_or(&[]) {
                    let d = text(&r["domain"]).trim_start_matches("www.").to_string();
                    if !d.is_empty() && !d.ends_with("google.com") && !out.contains(&d) { out.push(d); }
                }
                for (k, child) in map { if k != "references" { walk(child, out); } }
            }
            Value::Array(list) => list.iter().for_each(|c| walk(c, out)),
            _ => {}
        }
    }
    walk(value, &mut out);
    out
}

/// The first AI answer item of a DataForSEO SERP result (`result` array or its first task result).
fn ai_item(result: &Value) -> Option<&Value> {
    let first = if result.is_array() { &result[0] } else { result };
    items(&first["items"]).iter().find(|i| text(&i["type"]) == "ai_overview")
}

/// Reads an organic SERP (AI Overview) or an AI Mode result into one row.
pub fn ai_cite(result: &Value, keyword: &str, page: &str, location: u32, at: &str) -> AiCite {
    let base = AiCite { keyword: keyword.into(), page: page.into(), location, at: at.into(), ..Default::default() };
    let Some(item) = ai_item(result) else { return base };
    let domains = cited_domains(item);
    let mentioned = text(&item["markdown"]).to_lowercase().contains(OUR_HOST);
    AiCite { present: true, deferred: item["asynchronous_ai_overview"].as_bool() == Some(true) && domains.is_empty(),
        mars_cited: mentioned || domains.iter().any(|d| is_ours(d)), domains, ..base }
}

/// A keyword's intent and difficulty, cached 30 days (a failed lookup is retried the next day).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeywordProps {
    /// Singapore date of the lookup.
    pub fetched: String,
    pub intent: Option<String>,
    pub probability: Option<f64>,
    pub secondary: Option<String>,
    /// DataForSEO keyword difficulty, 0–100.
    pub difficulty: Option<f64>,
}

/// `keyword (lowercase) → (main intent, its probability, first secondary intent)` of a search_intent result.
pub fn intents(result: &Value) -> BTreeMap<String, (String, f64, Option<String>)> {
    items(&result[0]["items"]).iter().filter_map(|i| {
        let label = text(&i["keyword_intent"]["label"]);
        (!label.is_empty()).then(|| (text(&i["keyword"]).to_lowercase(), (label.to_string(), i["keyword_intent"]["probability"].as_f64().unwrap_or(0.0),
            items(&i["secondary_keyword_intents"]).first().map(|s| text(&s["label"]).to_string()))))
    }).collect()
}

/// `keyword (lowercase) → difficulty` of a bulk_keyword_difficulty result.
pub fn difficulties(result: &Value) -> BTreeMap<String, f64> {
    items(&result[0]["items"]).iter().filter_map(|i| Some((text(&i["keyword"]).to_lowercase(), i["keyword_difficulty"].as_f64()?))).collect()
}

/// One day of new and lost links to our domain.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LinkDay {
    /// YYYY-MM-DD (UTC, as DataForSEO groups it).
    pub date: String,
    pub new_backlinks: i64,
    pub lost_backlinks: i64,
    pub new_referring_domains: i64,
    pub lost_referring_domains: i64,
}

/// The days of a timeseries_new_lost_summary result, oldest first.
pub fn link_days(result: &Value) -> Vec<LinkDay> {
    let n = |i: &Value, k: &str| i[k].as_i64().unwrap_or(0);
    items(&result[0]["items"]).iter().map(|i| LinkDay {
        date: text(&i["date"]).chars().take(10).collect(), new_backlinks: n(i, "new_backlinks"), lost_backlinks: n(i, "lost_backlinks"),
        new_referring_domains: n(i, "new_referring_domains"), lost_referring_domains: n(i, "lost_referring_domains"),
    }).collect()
}
