//! `.claude/skills/seo-competitor/scripts/analyze.ts` `gscSummary`: the target's Search Console rows,
//! summed per week and cut to the top queries and pages. Null when the raw files are absent.

use super::files::{at, items, read_json};
use chrono::{Datelike, Duration, NaiveDate};
use crate::js_wire::js;
use serde_json::{Value, json};
use std::collections::BTreeMap;

struct GscRow {
    key: String,
    clicks: f64,
    impressions: f64,
    position: f64,
}

/// `(read(gsc_<dimension>.json)).rows ?? []`.
fn rows(raw: &str, dimension: &str) -> Result<Vec<GscRow>, String> {
    let json = read_json(&format!("{raw}/gsc_{dimension}.json"))?;
    Ok(items(at(&json, &["rows"]))
        .iter()
        .map(|r| GscRow {
            key: r["keys"][0].as_str().unwrap_or_default().to_string(),
            clicks: at(r, &["clicks"]).as_f64().unwrap_or(0.0),
            impressions: at(r, &["impressions"]).as_f64().unwrap_or(0.0),
            position: at(r, &["position"]).as_f64().unwrap_or(0.0),
        })
        .collect())
}

/// The Sunday on or before a `YYYY-MM-DD` day, as `setUTCDate(getUTCDate() - getUTCDay())` gives it.
fn week_start(day: &str) -> Result<String, String> {
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").map_err(|_| format!("RangeError: Invalid time value ({day})"))?;
    let sunday = date - Duration::days(i64::from(date.weekday().num_days_from_sunday()));
    Ok(sunday.format("%Y-%m-%d").to_string())
}

/// `gscSummary()`: totals, weekly series, top queries and pages of the target's Search Console data.
pub fn gsc_summary(raw: &str) -> Result<Value, String> {
    if !std::path::Path::new(&format!("{raw}/gsc_query.json")).exists() {
        return Ok(Value::Null);
    }
    let by_date = rows(raw, "date")?;
    let mut weeks: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for r in &by_date {
        let entry = weeks.entry(week_start(&r.key)?).or_insert((0.0, 0.0));
        entry.0 += r.clicks;
        entry.1 += r.impressions;
    }
    let clicks = by_date.iter().fold(0.0, |acc, r| acc + r.clicks);
    let impressions = by_date.iter().fold(0.0, |acc, r| acc + r.impressions);
    let weeks: Vec<Value> = weeks.iter().map(|(week, (c, i))| json!({"week": week, "clicks": c, "impressions": i})).collect();
    let queries: Vec<Value> = rows(raw, "query")?
        .iter()
        .map(|r| json!({"query": r.key, "clicks": r.clicks, "impressions": r.impressions, "position": js::round(r.position * 10.0) / 10.0}))
        .collect();
    let pages: Vec<Value> = rows(raw, "page")?.iter().map(|r| json!({"page": r.key, "clicks": r.clicks, "impressions": r.impressions})).collect();
    Ok(json!({"clicks": clicks, "impressions": impressions, "weeks": weeks, "queries": queries, "pages": pages}))
}
