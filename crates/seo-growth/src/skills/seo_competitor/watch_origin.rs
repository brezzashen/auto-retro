//! Origin check of slow pages: a page whose only problem is `slow` is judged by the HTTP load
//! balancer's own `httpRequest.latency` for this run's probe requests (Cloud Logging), because the
//! probe host's proxy path stalls for seconds while the origin answers in milliseconds.

use super::gsc::post_json;
use super::watch_probe::{PageProbe, only_slow};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Load-balancer latency, in seconds, under which a slow probe is the probe path, not the page.
const ORIGIN_SLOW_SECONDS: f64 = 1.0;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OriginConfig {
    /// GCP project whose `http_load_balancer` logs serve the site.
    pub project: String,
    /// gcloud account allowed to read those logs.
    pub account: String,
}

/// Clears `slow` from every slow-only probe whose load-balancer requests since `since` (RFC 3339)
/// all stayed under [`ORIGIN_SLOW_SECONDS`]; a page without log entries stays slow. Returns how many
/// probes were cleared.
pub fn clear_probe_path_slowness(probes: &mut [PageProbe], origin: &OriginConfig, since: &str) -> Result<usize, String> {
    let slow: Vec<String> = probes.iter().filter(|p| only_slow(p)).map(|p| p.url.clone()).collect();
    if slow.is_empty() { return Ok(0); }
    let worst = origin_worst_latency(origin, &slow, since)?;
    let mut cleared = 0;
    for probe in probes.iter_mut().filter(|p| only_slow(p)) {
        if worst.get(&probe.url).is_some_and(|s| *s < ORIGIN_SLOW_SECONDS) {
            probe.problems.clear();
            cleared += 1;
        }
    }
    Ok(cleared)
}

fn origin_worst_latency(origin: &OriginConfig, urls: &[String], since: &str) -> Result<BTreeMap<String, f64>, String> {
    let token = std::process::Command::new("gcloud")
        .args(["auth", "print-access-token", "--account", &origin.account])
        .output().map_err(|e| e.to_string())?;
    if !token.status.success() { return Err(format!("gcloud token for {} failed", origin.account)); }
    let token = String::from_utf8_lossy(&token.stdout).trim().to_string();
    let body = json!({"resourceNames": [format!("projects/{}", origin.project)], "filter": filter(urls, since), "pageSize": 1000});
    let (status, text) = post_json("https://logging.googleapis.com/v2/entries:list", &token, &body.to_string())?;
    if !(200..300).contains(&status) { return Err(format!("logging entries:list HTTP {status}")); }
    Ok(worst_by_url(&serde_json::from_str(&text).map_err(|e| e.to_string())?))
}

fn filter(urls: &[String], since: &str) -> String {
    let any = urls.iter().map(|u| format!("httpRequest.requestUrl=\"{u}\"")).collect::<Vec<_>>().join(" OR ");
    format!("resource.type=\"http_load_balancer\" AND httpRequest.userAgent:\"MarsSeoWatch\" AND timestamp>=\"{since}\" AND ({any})")
}

/// The slowest `httpRequest.latency` (`"0.008s"`) per `requestUrl` in an `entries:list` response.
fn worst_by_url(response: &Value) -> BTreeMap<String, f64> {
    let mut worst = BTreeMap::new();
    for entry in response["entries"].as_array().into_iter().flatten() {
        let request = &entry["httpRequest"];
        let (Some(url), Some(latency)) = (request["requestUrl"].as_str(), request["latency"].as_str()) else { continue };
        let Ok(seconds) = latency.trim_end_matches('s').parse::<f64>() else { continue };
        let slot = worst.entry(url.to_string()).or_insert(0.0_f64);
        *slot = slot.max(seconds);
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worst_latency_is_kept_per_url_and_bad_rows_are_skipped() {
        let response = json!({"entries": [
            {"httpRequest": {"requestUrl": "https://example.com/a", "latency": "0.008s"}},
            {"httpRequest": {"requestUrl": "https://example.com/a", "latency": "0.210s"}},
            {"httpRequest": {"requestUrl": "https://example.com/b", "latency": "2.5s"}},
            {"httpRequest": {"requestUrl": "https://example.com/c"}},
        ]});
        let worst = worst_by_url(&response);
        assert_eq!(worst["https://example.com/a"], 0.21);
        assert_eq!(worst["https://example.com/b"], 2.5);
        assert!(!worst.contains_key("https://example.com/c"));
    }

    #[test]
    fn filter_names_every_slow_url_and_the_probe_agent() {
        let f = filter(&["https://example.com/a".into(), "https://example.com/b".into()], "2026-11-11T06:17:00Z");
        assert!(f.contains("userAgent:\"MarsSeoWatch\""));
        assert!(f.contains("timestamp>=\"2026-11-11T06:17:00Z\""));
        assert!(f.contains("requestUrl=\"https://example.com/a\" OR httpRequest.requestUrl=\"https://example.com/b\""));
    }
}
