//! Search Console reads of the hourly watchdog: hourly totals (`HOURLY_ALL`, the last few days),
//! 28 days of query × page rows and of page totals, the sitemap list and URL Inspection for a rotating slice of URLs.

use super::config::Gsc;
use super::gsc::{access_token, encode_uri_component, post_json, query_url};
use super::watch_config::Inspection;
use serde_json::{Value, json};

const INSPECT_URL: &str = "https://searchconsole.googleapis.com/v1/urlInspection/index:inspect";

/// Raw Search Console answers of one run; `errors` lists every call that failed.
#[derive(Default)]
pub struct GscSnapshot {
    pub hourly: Value,
    pub query_page: Value,
    /// 28 days by page: unlike the query × page rows it keeps the clicks of anonymized queries.
    pub pages: Value,
    /// 28 days by date × page: the only split that keeps anonymized impressions per day.
    pub day_page: Value,
    pub sitemaps: Value,
    pub inspections: Vec<(String, Inspection)>,
    pub errors: Vec<String>,
}

pub fn hourly_body(start: &str, end: &str) -> Value {
    json!({"startDate": start, "endDate": end, "dimensions": ["HOUR"], "dataState": "HOURLY_ALL", "rowLimit": 500})
}

pub fn query_page_body(start: &str, end: &str) -> Value {
    json!({"startDate": start, "endDate": end, "dimensions": ["query", "page"], "dataState": "ALL", "rowLimit": 1000})
}

pub fn page_body(start: &str, end: &str) -> Value {
    json!({"startDate": start, "endDate": end, "dimensions": ["page"], "dataState": "ALL", "rowLimit": 1000})
}

pub fn day_page_body(start: &str, end: &str) -> Value {
    json!({"startDate": start, "endDate": end, "dimensions": ["date", "page"], "dataState": "ALL", "rowLimit": 5000})
}

/// Reads the fields the watchdog keeps from one `index:inspect` answer.
pub fn parse_inspection(answer: &Value, checked_at: &str) -> Inspection {
    let status = &answer["inspectionResult"]["indexStatusResult"];
    let field = |key: &str| status[key].as_str().unwrap_or_default().to_string();
    Inspection {
        verdict: field("verdict"),
        coverage: field("coverageState"),
        last_crawl: field("lastCrawlTime"),
        google_canonical: field("googleCanonical"),
        checked_at: checked_at.to_string(),
    }
}

fn get_json(url: &str, bearer: &str) -> Result<Value, String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let response = reqwest::Client::new().get(url).bearer_auth(bearer).send().await.map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let text = response.text().await.map_err(|e| e.to_string())?;
        if !(200..300).contains(&status) { return Err(format!("HTTP {status}")); }
        serde_json::from_str(&text).map_err(|e| e.to_string())
    })
}

fn post(url: &str, bearer: &str, body: &Value) -> Result<Value, String> {
    let (status, text) = post_json(url, bearer, &body.to_string())?;
    if !(200..300).contains(&status) { return Err(format!("HTTP {status}")); }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// `today` and the date windows are Pacific dates, Search Console's own day boundary.
pub fn collect(gsc: &Gsc, today: &str, hourly_from: &str, month_from: &str, inspect: &[String], checked_at: &str) -> GscSnapshot {
    let mut snapshot = GscSnapshot::default();
    let token = match access_token(gsc) {
        Ok(token) => token,
        Err(error) => { snapshot.errors.push(format!("token: {error}")); return snapshot; }
    };
    let query = query_url(&gsc.property);
    match post(&query, &token, &hourly_body(hourly_from, today)) {
        Ok(value) => snapshot.hourly = value,
        Err(error) => snapshot.errors.push(format!("hourly: {error}")),
    }
    match post(&query, &token, &query_page_body(month_from, today)) {
        Ok(value) => snapshot.query_page = value,
        Err(error) => snapshot.errors.push(format!("query×page: {error}")),
    }
    match post(&query, &token, &page_body(month_from, today)) {
        Ok(value) => snapshot.pages = value,
        Err(error) => snapshot.errors.push(format!("pages: {error}")),
    }
    match post(&query, &token, &day_page_body(month_from, today)) {
        Ok(value) => snapshot.day_page = value,
        Err(error) => snapshot.errors.push(format!("date×page: {error}")),
    }
    let sitemaps = format!("https://www.googleapis.com/webmasters/v3/sites/{}/sitemaps", encode_uri_component(&gsc.property));
    match get_json(&sitemaps, &token) {
        Ok(value) => snapshot.sitemaps = value,
        Err(error) => snapshot.errors.push(format!("sitemaps: {error}")),
    }
    for url in inspect {
        match post(INSPECT_URL, &token, &json!({"inspectionUrl": url, "siteUrl": gsc.property})) {
            Ok(answer) => snapshot.inspections.push((url.clone(), parse_inspection(&answer, checked_at))),
            Err(error) => snapshot.errors.push(format!("inspect {url}: {error}")),
        }
    }
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hourly_query_asks_for_hourly_all_by_hour() {
        let body = hourly_body("2026-10-30", "2026-11-02");
        assert_eq!(body["dimensions"], json!(["HOUR"]));
        assert_eq!(body["dataState"], "HOURLY_ALL");
    }

    #[test]
    fn inspection_keeps_verdict_coverage_crawl_and_canonical() {
        let answer = json!({"inspectionResult": {"indexStatusResult": {"verdict": "PASS", "coverageState": "Submitted and indexed",
            "lastCrawlTime": "2026-10-31T08:15:14Z", "googleCanonical": "https://example.com/"}}});
        let inspection = parse_inspection(&answer, "2026-11-02T06:00:00Z");
        assert_eq!(inspection.coverage, "Submitted and indexed");
        assert_eq!(inspection.google_canonical, "https://example.com/");
        assert_eq!(parse_inspection(&json!({}), "t").verdict, "");
    }
}
