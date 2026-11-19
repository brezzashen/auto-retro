//! PostHog Cloud API client: HogQL queries and the insight endpoints. The personal API key is read
//! from the macOS Keychain per run and never written to a file or log.

use crate::http;
use serde_json::{Value, json};
use std::process::Command;
use std::time::Duration;

pub struct PostHog {
    host: String,
    project: u64,
    key: String,
    /// Longest wait for the project's hourly query budget before one retry; `None` fails at once.
    budget_wait: Option<Duration>,
    /// The `name` PostHog's query log records for `query` (`conversion-watch` unless `with_label` changes it).
    label: &'static str,
}

/// Seconds PostHog asks a client to wait when the project's hourly query-read budget is spent
/// (HTTP 429 "... Expected available in <n> seconds."); `None` for any other error.
pub fn budget_wait_secs(error: &str) -> Option<u64> {
    if !error.contains("HTTP 429") { return None; }
    let tail = error.split("Expected available in ").nth(1)?;
    tail.split_whitespace().next()?.parse().ok()
}

impl PostHog {
    pub fn from_keychain(host: &str, project: u64, service: &str) -> Result<Self, String> {
        let read = || Command::new("security").args(["find-generic-password", "-s", service, "-w"]).output().map_err(|e| format!("security: {e}"));
        let mut out = read()?;
        // One retry: a read can fail transiently while other sessions hit the Keychain (2026-11-18 16:43).
        if !out.status.success() { std::thread::sleep(Duration::from_secs(3)); out = read()?; }
        if !out.status.success() { return Err(format!("Keychain entry {service} not readable (security exit {:?})", out.status.code())); }
        let key = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(Self { host: host.trim_end_matches('/').into(), project, key, budget_wait: None, label: "conversion-watch" })
    }

    /// Waits up to `max` once for a spent hourly query budget instead of failing the query. The
    /// budget is shared by every session querying the project, so an unattended run uses this.
    pub fn with_budget_wait(mut self, max: Duration) -> Self {
        self.budget_wait = Some(max);
        self
    }

    /// `METHOD /api/projects/<project>/<path>` with an optional JSON body; the parsed JSON answer.
    pub fn api(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let url = format!("{}/api/projects/{}/{path}", self.host, self.project);
        let headers = [("authorization", format!("Bearer {}", self.key)), ("content-type", "application/json".to_string())];
        let payload = body.map(|b| b.to_string().into_bytes());
        let response = http::send(method, &url, &headers, payload, Some(Duration::from_secs(120)))?;
        let parsed: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
        if !response.ok() {
            let detail = parsed["detail"].as_str().or(parsed["error"].as_str()).unwrap_or("no detail");
            return Err(format!("PostHog {path}: HTTP {} {detail}", response.status));
        }
        Ok(parsed)
    }

    /// Tags this client's `query` calls in PostHog's query log, so its read volume is attributed to its caller.
    pub fn with_label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
    }

    /// The result rows of one HogQL query.
    pub fn query(&self, hogql: &str) -> Result<Vec<Vec<Value>>, String> {
        self.query_named(hogql, self.label).map(|(_, rows)| rows)
    }

    /// The column names and result rows of one HogQL query, tagged `name` in PostHog's query log.
    pub fn query_named(&self, hogql: &str, name: &str) -> Result<(Vec<String>, Vec<Vec<Value>>), String> {
        let body = json!({ "query": { "kind": "HogQLQuery", "query": hogql }, "name": name });
        let answer = match self.api(reqwest::Method::POST, "query/", Some(&body)) {
            Err(error) => match (budget_wait_secs(&error), self.budget_wait) {
                (Some(secs), Some(max)) if Duration::from_secs(secs) <= max => {
                    eprintln!("[conversion-analytics] PostHog query budget spent; waiting {secs} s, then retrying once");
                    std::thread::sleep(Duration::from_secs(secs + 5));
                    self.api(reqwest::Method::POST, "query/", Some(&body))?
                }
                _ => return Err(error),
            },
            ok => ok?,
        };
        if let Some(error) = answer["error"].as_str() { return Err(format!("HogQL: {error}")); }
        let columns = answer["columns"].as_array().into_iter().flatten().map(|c| c.as_str().unwrap_or("").to_string()).collect();
        let rows = answer["results"].as_array().into_iter().flatten().map(|row| row.as_array().cloned().unwrap_or_default()).collect();
        Ok((columns, rows))
    }
}

/// A numeric cell (`0` when null or not a number).
pub fn num(row: &[Value], i: usize) -> f64 {
    row.get(i).and_then(|v| v.as_f64().or_else(|| v.as_bool().map(f64::from)).or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0.0)
}

/// A text cell (`""` when null).
pub fn text(row: &[Value], i: usize) -> String {
    match row.get(i) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_read_numbers_booleans_and_nulls() {
        let row = vec![json!(3), json!(true), Value::Null, json!("2026-11-03")];
        assert_eq!(num(&row, 0), 3.0);
        assert_eq!(num(&row, 1), 1.0);
        assert_eq!(num(&row, 2), 0.0);
        assert_eq!(text(&row, 2), "");
        assert_eq!(text(&row, 3), "2026-11-03");
    }

    #[test]
    fn only_a_spent_query_budget_yields_a_wait() {
        let spent = "PostHog query/: HTTP 429 This project used its hourly budget of data read by API queries. Expected available in 509 seconds.";
        assert_eq!(budget_wait_secs(spent), Some(509));
        assert_eq!(budget_wait_secs("PostHog query/: HTTP 429 Request was throttled."), None);
        assert_eq!(budget_wait_secs("PostHog query/: HTTP 500 Expected available in 9 seconds."), None);
    }
}
