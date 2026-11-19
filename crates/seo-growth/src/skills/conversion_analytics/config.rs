//! The conversion watchdog's configuration (`references/watch.config.json`), its persisted state
//! (`<state>/state.json`) and the finding every check produces.

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CronJobTarget {
    pub context: String,
    pub namespace: String,
    pub name: String,
    /// Hours since the last successful run before the job counts as stalled.
    pub max_age_hours: f64,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Dispatch {
    /// Minimum hours between two tabs when a new P0 is present.
    pub health_cooldown_hours: f64,
    /// Minimum hours between two tabs for P1 tracking defects alone.
    pub fix_cooldown_hours: f64,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Alerts {
    /// Who set these values and when; shown on the page.
    pub decided: String,
    /// Last day's sign-ups below this share of the prior days' median.
    pub signup_median_ratio: f64,
    /// Sign-in code success this many points (0-1) below the pooled baseline.
    pub otp_drop_points: f64,
    pub otp_min_sent: f64,
    /// Checkout success below this share of the pooled baseline.
    pub checkout_ratio: f64,
    pub checkout_min_started: f64,
    /// 24-hour activation below this share of the pooled baseline.
    pub activation_ratio: f64,
    pub activation_min_signups: f64,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct VisitorLog {
    /// GCP project holding the production load balancer's request log.
    pub project: String,
    /// Production origin whose `/ingest` requests are counted, e.g. `https://example.com`.
    pub origin: String,
    /// Client address prefixes that are not visitors (this host, attributed machine ranges).
    #[serde(default)]
    pub ignore_ip_prefixes: Vec<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ReportTarget {
    pub slug: String,
    pub title: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WatchConfig {
    pub host: String,
    pub project: u64,
    pub keychain_service: String,
    pub dashboard: u64,
    /// Production origin whose `/ingest` relay is probed.
    pub site: String,
    /// Singapore time of the first production event, `YYYY-MM-DD HH:MM:SS`.
    pub tracking_start: String,
    /// File holding the `AnalyticsEvent` union, relative to the repository root.
    pub event_source: String,
    pub sdk_events: Vec<String>,
    pub server_events: Vec<String>,
    /// Screen-only events (`event` → HogQL condition on production events that happen only when that screen
    /// was shown, e.g. its route's `$pageview`): at zero while the condition matched nothing in 24 h either,
    /// nobody reached the screen, so the zero is a quiet P2.
    #[serde(default)]
    pub witnesses: std::collections::BTreeMap<String, String>,
    /// Events that record a failure (`auth_failed`): a zero day means nobody failed, a quiet P2, until
    /// `upstream::FAILURE_ZERO_DAYS` such days in a row make it a P0 (the call site may be gone).
    #[serde(default)]
    pub failure_events: Vec<String>,
    /// Prior daily mean an event needs before a silent day is a P0 rather than a quiet low-volume day.
    pub zero_min_daily: f64,
    pub funnel_days: i64,
    pub baseline_days: usize,
    /// Smallest step population a conversion rate is compared on.
    pub min_sample: f64,
    /// Production days of baseline before alert thresholds are proposed for confirmation.
    pub threshold_baseline_days: usize,
    /// Hours without any production event that make a P0 after a busy day.
    pub silence_hours: i64,
    /// Mean events per hour over the last 24 h below which the silence rule stays quiet.
    pub silence_min_hourly: f64,
    /// Load balancer log read when the silence rule fires; without it the silence stays a P0.
    #[serde(default)]
    pub visitor_log: Option<VisitorLog>,
    /// Hours after dispatch before a still-present handed-over finding is flagged in the retro.
    pub stale_hours: f64,
    /// Longest wait for PostHog's hourly query-read budget, shared with every session querying the
    /// project, before a query is retried once (PostHog states the wait in its HTTP 429).
    #[serde(default = "default_budget_wait_secs")]
    pub budget_wait_secs: u64,
    pub alerts: Alerts,
    pub cron_job: CronJobTarget,
    pub dispatch: Dispatch,
    pub report: ReportTarget,
}

fn default_budget_wait_secs() -> u64 { 900 }

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub key: String,
    pub handed_at: String,
    pub resolved_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WatchState {
    /// Finding keys already handed to a tab; a key leaves once its finding is gone.
    pub handled: Vec<String>,
    /// When each handled key was handed over (RFC 3339).
    pub handed_at: std::collections::BTreeMap<String, String>,
    /// Handled keys whose finding disappeared, newest first.
    pub resolved: Vec<Resolved>,
    pub last_fix_at: Option<String>,
    pub last_session: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// Stable across runs (`kind:subject`), so a tab receives it once.
    pub key: String,
    /// `P0` tracking broken, `P1` tracking defect, `P2` trend or watch item (report only).
    pub severity: String,
    pub title: String,
    pub action: String,
}

impl Finding {
    pub fn new(key: impl Into<String>, severity: &str, title: impl Into<String>, action: impl Into<String>) -> Self {
        Self { key: key.into(), severity: severity.into(), title: title.into(), action: action.into() }
    }
}

pub fn read_config(path: &str) -> Result<WatchConfig, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

pub fn read_state(path: &str) -> WatchState {
    std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// The event names of the `export type AnalyticsEvent = | '…'` union in `source`.
pub fn union_events(source: &str) -> Vec<String> {
    let Some(start) = source.find("export type AnalyticsEvent") else { return Vec::new() };
    let body = &source[start..];
    let end = body.find("\n\n").unwrap_or(body.len());
    let re = regex::Regex::new(r"\|\s*'([a-z_$]+)'").expect("union regex");
    re.captures_iter(&body[..end]).map(|c| c[1].to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_union_parser_reads_every_member_and_stops_at_the_type() {
        let source = "x\nexport type AnalyticsEvent =\n  | 'cta_clicked'\n  | 'auth_failed'\n\nexport type Other = | 'nope'\n";
        assert_eq!(union_events(source), vec!["cta_clicked", "auth_failed"]);
    }
}
