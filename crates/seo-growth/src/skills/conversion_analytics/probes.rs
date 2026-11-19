//! Live probes outside PostHog: the production `analytics-sync` CronJob (read-only `kubectl get`
//! with an explicit context and namespace), the first-party `/ingest` relay and the session replay
//! switch PostHog serves through it.

use super::config::CronJobTarget;
use super::health::{CronStatus, RelayStatus};
use crate::http;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::process::Command;
use std::time::Duration;

const RELAY_ATTEMPTS: u32 = 3;
const RELAY_RETRY_PAUSE: Duration = Duration::from_secs(10);

fn kubectl(target: &CronJobTarget, args: &[&str]) -> Result<Value, String> {
    let out = Command::new("kubectl").env("MARS_RAW_KUBECTL", "1")
        .args(["--context", &target.context, "-n", &target.namespace, "--request-timeout", "30s"]).args(args).args(["-o", "json"])
        .output().map_err(|e| format!("kubectl: {e}"))?;
    if !out.status.success() {
        return Err(format!("kubectl {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or_default()));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("kubectl json: {e}"))
}

/// Last success, its age and the failed Jobs started in the last 24 h.
pub fn cron_status(target: &CronJobTarget, now: DateTime<Utc>) -> Result<CronStatus, String> {
    let cron = kubectl(target, &["get", "cronjob", &target.name])?;
    let jobs = kubectl(target, &["get", "jobs", "-l", &format!("app.kubernetes.io/name={}", target.name)])?;
    let last_success = cron["status"]["lastSuccessfulTime"].as_str().map(String::from);
    let age_hours = last_success.as_deref().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| (now - t.with_timezone(&Utc)).num_minutes() as f64 / 60.0);
    let failed_24h = jobs["items"].as_array().into_iter().flatten().filter(|job| {
        let recent = job["status"]["startTime"].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).is_some_and(|t| now - t.with_timezone(&Utc) <= chrono::Duration::hours(24));
        recent && job["status"]["failed"].as_u64().unwrap_or(0) > 0
    }).count();
    Ok(CronStatus { last_success, age_hours, failed_24h, suspended: cron["spec"]["suspend"].as_bool().unwrap_or(false), max_age_hours: target.max_age_hours })
}

/// The asset path must load the SDK bundle; the capture path must reach PostHog itself, which
/// answers a probe carrying a bogus token with its own 401 (a dead upstream gives the relay's 502).
/// A failed probe is retried `RELAY_ATTEMPTS` times, because one transport blip on this host's path
/// to example.com (2026-11-03 22:27: neither request reached the gateway while real traffic flowed)
/// must not read as a broken relay; the last transport error is kept for the page.
pub fn relay_status(site: &str) -> RelayStatus {
    let mut status = RelayStatus::default();
    for attempt in 1..=RELAY_ATTEMPTS {
        if attempt > 1 { std::thread::sleep(RELAY_RETRY_PAUSE); }
        status = relay_once(site);
        status.attempts = attempt;
        if status.ok { break; }
    }
    status
}

/// The project token (`POSTHOG_PROJECT_TOKEN = 'phc_…'`) declared in the web analytics module.
pub fn project_token(source: &str) -> Option<String> {
    let rest = &source[source.find("POSTHOG_PROJECT_TOKEN")?..];
    let start = rest.find("phc_")?;
    let token: String = rest[start..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    (token.len() > 4).then_some(token)
}

/// Whether PostHog turns session replay on for a page of `site`: the remote config the SDK fetches
/// through the relay, sent with the page `Referer` a browser sends. PostHog answers
/// `sessionRecording: false` for an origin outside the project's recording domains, and the relay must
/// pass that origin on (2026-11-15: it did not, and no replay was recorded). `None` when unanswered.
pub fn replay_enabled(site: &str, token: &str) -> Option<bool> {
    let url = format!("{site}/ingest/array/{token}/config");
    let reply = http::get(&url, &[("referer", format!("{site}/"))], Some(Duration::from_secs(20))).ok().filter(|r| r.ok())?;
    let body: Value = serde_json::from_str(&reply.text()).ok()?;
    Some(body.get("sessionRecording").is_some_and(Value::is_object))
}

fn relay_once(site: &str) -> RelayStatus {
    let timeout = Some(Duration::from_secs(20));
    let asset = http::get(&format!("{site}/ingest/static/array.js"), &[], timeout);
    let capture = http::send(reqwest::Method::POST, &format!("{site}/ingest/flags/?v=2"), &[("content-type", "application/json".into())],
        Some(br#"{"token":"conversion-watch-probe","distinct_id":"conversion-watch-probe"}"#.to_vec()), timeout);
    let asset_status = asset.as_ref().map_or(0, |r| r.status);
    let capture_status = capture.as_ref().map_or(0, |r| r.status);
    let error = [asset.as_ref().err().map(|e| format!("asset: {e}")), capture.as_ref().err().map(|e| format!("capture: {e}"))]
        .into_iter().flatten().collect::<Vec<_>>().join("; ");
    let posthog_answered = capture.is_ok_and(|r| (r.ok() || r.status == 401) && r.text().contains("\""));
    RelayStatus { asset_status, capture_status, ok: asset_status == 200 && posthog_answered, error: (!error.is_empty()).then_some(error), ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::project_token;

    #[test]
    fn reads_the_project_token_from_the_web_module() {
        let source = "export const POSTHOG_PROJECT_TOKEN = 'phc_AbC123'\nconst other = 'phc_zzz'";
        assert_eq!(project_token(source).as_deref(), Some("phc_AbC123"));
        assert_eq!(project_token("const other = 'phc_zzz'"), None);
    }
}
