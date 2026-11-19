//! The search-engine push watchdog's configuration (`references/push.config.json`) and its persisted
//! state (`<state>/state.json`): which sitemap URLs each engine (Baidu, IndexNow, Bing URL Submission) has
//! accepted and today's quota.

use super::watch_config::ReportTarget;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PushSite {
    /// Origin whose `/sitemap.xml` lists the URLs to push, e.g. `https://example.cn`.
    pub site: String,
    /// File holding the Baidu Search Resource Platform push token (page `普通收录`, API submission); `~` expands to HOME.
    pub baidu_token_file: Option<String>,
    /// Push the sitemap URLs to IndexNow (Bing, Yandex, Seznam, Naver).
    pub indexnow: bool,
    /// Submit the sitemap URLs through the Bing Webmaster URL Submission API (needs `bingApiKeyFile`).
    #[serde(default)]
    pub bing_submit: bool,
    /// Paths an engine needs to keep the site verified; each must answer HTTP 200.
    pub keep_files: Vec<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PushConfig {
    pub sites: Vec<PushSite>,
    /// IndexNow key served at `<site>/indexnow-<key>.txt` on every site.
    pub indexnow_key: String,
    /// File holding the Bing Webmaster API key (Settings → API access); `~` expands to HOME.
    pub bing_api_key_file: Option<String>,
    /// Minimum hours between two repair tabs.
    pub repair_cooldown_hours: f64,
    /// Minimum hours between two tabs asking for the Baidu sitemap web submission.
    pub baidu_sitemap_retry_hours: f64,
    pub report: ReportTarget,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct EngineState {
    /// URL → RFC 3339 time the engine accepted it.
    pub pushed: BTreeMap<String, String>,
    /// URL → sitemap `lastmod` date (YYYY-MM-DD) it carried when the engine last accepted it.
    #[serde(default)]
    pub pushed_lastmod: BTreeMap<String, String>,
    /// URL → head signature (title, description, canonical) it had when the engine last accepted it.
    #[serde(default)]
    pub pushed_head: BTreeMap<String, String>,
    /// `(Beijing date, URLs left today)` from the engine's last answer.
    pub remain: Option<(String, i64)>,
    /// Beijing date on which the engine answered `over quota`.
    pub exhausted_day: Option<String>,
    /// One-line outcome of the last request, shown on the page.
    pub last_result: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SiteState {
    pub baidu: EngineState,
    pub indexnow: EngineState,
    #[serde(default)]
    pub bing: EngineState,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PushState {
    pub sites: BTreeMap<String, SiteState>,
    /// Failure keys already handed to a repair tab; a key leaves when it heals, so a relapse alerts again.
    pub handled: Vec<String>,
    pub last_repair_at: Option<String>,
    pub last_sitemap_tab_at: Option<String>,
    pub last_session: Option<String>,
}

pub fn read_config(path: &str) -> Result<PushConfig, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

pub fn read_state(path: &str) -> PushState {
    std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// Reads a token file, expanding a leading `~/`; `None` when it is missing or empty.
pub fn read_token(path: &str, home: &str) -> Option<String> {
    let full = path.strip_prefix("~/").map_or_else(|| path.to_string(), |rest| format!("{home}/{rest}"));
    std::fs::read_to_string(full).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}
