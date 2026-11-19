//! Real-browser page loads in the production load balancer log: the witness that tells a silent
//! night (nobody but crawlers visited) from a broken release (people loaded the SDK, PostHog stored nothing).
//! posthog-js drops events from bot user agents and from requests with no user agent, so crawler
//! page loads fetch `config.js` and never send a capture.

use super::config::{Finding, VisitorLog};
use chrono::{DateTime, SecondsFormat, Utc};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::process::Command;

const BOT_UA: &str = r"(?i)bot|crawl|spider|slurp|headless|lighthouse|pingdom|uptime|monitor|preview|inspection|googleother|facebookexternalhit|curl|wget|python|go-http|java/|okhttp|axios|node-fetch|phantom|puppeteer|playwright|selenium";

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VisitorLoads {
    /// Start of the inspected window (UTC, RFC 3339).
    pub since: String,
    /// SDK loads (`GET …/config.js`) from a browser user agent outside the ignored addresses.
    pub human_loads: usize,
    /// Capture requests (`POST /ingest/e/`, `/ingest/i/v0/e/`) from those same clients.
    pub human_captures: usize,
    /// SDK loads from a bot user agent or with no user agent.
    pub bot_loads: usize,
}

fn is_capture(url: &str) -> bool {
    let path = url.split('?').next().unwrap_or(url);
    path.ends_with("/ingest/e/") || path.ends_with("/ingest/i/v0/e/")
}

/// Counts load-balancer entries (`httpRequest` objects as `gcloud logging read --format=json` prints them).
pub fn classify(entries: &[Value], ignore_ip_prefixes: &[String], since: &str) -> VisitorLoads {
    let bot = Regex::new(BOT_UA).expect("bot regex");
    let mut out = VisitorLoads { since: since.into(), ..Default::default() };
    for entry in entries {
        let req = &entry["httpRequest"];
        let (method, url) = (req["requestMethod"].as_str().unwrap_or(""), req["requestUrl"].as_str().unwrap_or(""));
        let ip = req["remoteIp"].as_str().unwrap_or("");
        if ignore_ip_prefixes.iter().any(|p| ip.starts_with(p.as_str())) { continue; }
        let ua = req["userAgent"].as_str().unwrap_or("").trim();
        let human = !ua.is_empty() && !bot.is_match(ua);
        let load = method == "GET" && url.contains("/config.js");
        match (load, human) {
            (true, true) => out.human_loads += 1,
            (true, false) => out.bot_loads += 1,
            (false, true) if method == "POST" && is_capture(url) => out.human_captures += 1,
            _ => {}
        }
    }
    out
}

/// Reads the production `/ingest` SDK loads and captures since `since` from Cloud Logging.
pub fn visitor_loads(log: &VisitorLog, since: DateTime<Utc>) -> Result<VisitorLoads, String> {
    let since = since.to_rfc3339_opts(SecondsFormat::Secs, true);
    let filter = format!(
        "resource.type=\"http_load_balancer\" AND httpRequest.requestUrl:\"{}/ingest/\" AND (httpRequest.requestUrl:\"/config.js\" OR httpRequest.requestUrl:\"/e/\") AND timestamp>=\"{since}\"",
        log.origin);
    let out = Command::new("gcloud")
        .args(["logging", "read", &filter, "--project", &log.project, "--limit", "20000"])
        .arg("--format=json(httpRequest.requestMethod,httpRequest.requestUrl,httpRequest.userAgent,httpRequest.remoteIp)")
        .output().map_err(|e| format!("gcloud: {e}"))?;
    if !out.status.success() {
        return Err(format!("gcloud logging read: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or_default()));
    }
    let entries: Vec<Value> = serde_json::from_slice(&out.stdout).map_err(|e| format!("gcloud json: {e}"))?;
    Ok(classify(&entries, &log.ignore_ip_prefixes, &since))
}

/// A silent-hours P0 with no browser page load in the window becomes a quiet P2: nobody came.
pub fn judge(silent: Finding, loads: Option<&VisitorLoads>) -> Finding {
    match loads {
        Some(v) if v.human_loads == 0 && v.human_captures == 0 => Finding::new("watch:silent-no-visitors", "P2",
            format!("{}，但负载均衡日志里同期没有真人页面加载（只有 {} 次爬虫加载，posthog-js 按设计丢弃）", silent.title, v.bot_loads),
            "没人走到，不是埋点故障；有真人加载而 PostHog 无事件时才会升为 P0"),
        Some(v) => Finding { title: format!("{}；同期有 {} 次真人页面加载、{} 次 capture 请求", silent.title, v.human_loads, v.human_captures), ..silent },
        None => silent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(method: &str, url: &str, ua: Option<&str>, ip: &str) -> Value {
        json!({"httpRequest": {"requestMethod": method, "requestUrl": url, "userAgent": ua, "remoteIp": ip}})
    }

    const LOAD: &str = "https://example.com/ingest/array/phc_x/config.js";
    const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 27_2 like Mac OS X) AppleWebKit/605.1.15";

    #[test]
    fn crawlers_and_ignored_addresses_are_not_visitors() {
        let entries = vec![
            entry("GET", LOAD, None, "34.66.114.108"),
            entry("GET", LOAD, Some("Mozilla/5.0 (compatible; AhrefsBot/7.0; +http://ahrefs.com/robot/)"), "202.8.4.1"),
            entry("GET", LOAD, Some("AdsBot-Google (+http://www.google.com/adsbot.html)"), "74.125.1.1"),
            entry("GET", LOAD, Some("Mozilla/5.0 (Macintosh) Chrome/156.0.0.0 Safari/537.36"), "61.13.236.78"),
            entry("GET", LOAD, Some(IPHONE), "45.78.1.1"),
            entry("POST", "https://example.com/ingest/e/?ip=0&ver=1", Some(IPHONE), "45.78.1.1"),
            entry("POST", "https://example.com/ingest/flags/?v=2", Some(IPHONE), "45.78.1.1"),
        ];
        let v = classify(&entries, &["61.13.236.78".into()], "t");
        assert_eq!((v.human_loads, v.human_captures, v.bot_loads), (1, 1, 3));
    }

    #[test]
    fn a_crawler_only_window_turns_the_silence_p0_into_a_p2() {
        let silent = Finding::new("health:silent-hours", "P0", "生产最近 2 小时没有任何事件（前 24 小时共 1109 个）", "x");
        let quiet = VisitorLoads { bot_loads: 800, ..Default::default() };
        assert_eq!(judge(silent.clone(), Some(&quiet)).key, "watch:silent-no-visitors");
        let broken = VisitorLoads { human_loads: 4, ..Default::default() };
        let f = judge(silent.clone(), Some(&broken));
        assert_eq!((f.key.as_str(), f.severity.as_str()), ("health:silent-hours", "P0"));
        assert!(f.title.contains("4 次真人页面加载"));
        assert_eq!(judge(silent.clone(), None), silent);
    }
}
