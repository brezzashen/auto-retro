//! Runs every query and probe of one watchdog run. A failing source becomes an error line on the
//! page, never a missing run: the rest of the analysis still publishes.

use super::alerts::{AlertRow, Window, evaluate};
use super::config::{Finding, WatchConfig};
use super::funnel::{FunnelDay, Leak, funnel_days, locate};
use super::hourly::{HourRow, fill, hours};
use super::health::{Health, Witness, volume_tables};
use super::posthog::{PostHog, num, text};
use super::probes::{cron_status, project_token, relay_status, replay_enabled};
use super::visitors::{VisitorLoads, visitor_loads};
use super::queries::{self, DIMENSIONS, STEPS};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Breakdown {
    pub label: String,
    pub hogql: String,
    pub rows: Vec<Vec<Value>>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    pub health: Health,
    pub funnel: Vec<FunnelDay>,
    pub funnel_hogql: String,
    pub leak: Option<Leak>,
    pub breakdowns: Vec<Breakdown>,
    pub activation: Vec<Vec<Value>>,
    pub retention: Vec<Vec<Value>>,
    pub methods: Vec<Vec<Value>>,
    pub failures: Vec<Vec<Value>>,
    pub sources: Vec<Vec<Value>>,
    pub errors: Vec<String>,
    /// Production days fully tracked before today (the alert-threshold baseline).
    pub baseline_days: i64,
    /// Smallest group a breakdown suggestion is drawn from.
    pub min_sample: f64,
    pub alerts: Vec<AlertRow>,
    pub alert_findings: Vec<Finding>,
    pub alerts_decided: String,
    pub hourly: Vec<HourRow>,
    /// Browser page loads in the silent window; read only when the silence rule fires.
    pub visitors: Option<VisitorLoads>,
    /// Abandoned-checkout recovery; `None` when PostHog could not be read.
    pub recovery: Option<super::recovery::RecoveryWatch>,
    /// Production `$exception` volume and top errors; `None` when PostHog could not be read.
    pub exceptions: Option<super::exceptions::ExceptionWatch>,
}

fn expected_events(config: &WatchConfig, root: &str) -> Result<Vec<(String, &'static str)>, String> {
    let path = format!("{root}/{}", config.event_source);
    let source = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let web = super::config::union_events(&source);
    if web.is_empty() { return Err(format!("{path}: no AnalyticsEvent union found")); }
    let tagged = |list: &[String], tag: &'static str| list.iter().map(|e| (e.clone(), tag)).collect::<Vec<_>>();
    Ok([tagged(&config.sdk_events, "sdk"), tagged(&web, "web"), tagged(&config.server_events, "server")].concat())
}

pub fn collect(config: &WatchConfig, root: &str, now: DateTime<FixedOffset>) -> Analysis {
    let mut a = Analysis { min_sample: config.min_sample, ..Analysis::default() };
    let today = now.date_naive();
    let start = NaiveDate::parse_from_str(&config.tracking_start[..10.min(config.tracking_start.len())], "%Y-%m-%d").unwrap_or(today);
    a.baseline_days = (today - start).num_days() - 1;
    let from_day = (today - Duration::days(config.funnel_days)).max(start).format("%Y-%m-%d").to_string();
    let from = queries::date_sql(&from_day);
    let since = config.tracking_start.as_str();

    let mut relay = relay_status(&config.site);
    let token = std::fs::read_to_string(format!("{root}/{}", config.event_source)).ok().and_then(|source| project_token(&source));
    relay.replay = token.and_then(|token| replay_enabled(&config.site, &token));
    a.health.relay = Some(relay);
    match cron_status(&config.cron_job, Utc::now()) {
        Ok(status) => a.health.cron = Some(status),
        Err(error) => a.errors.push(format!("CronJob: {error}")),
    }
    let ph = match PostHog::from_keychain(&config.host, config.project, &config.keychain_service) {
        Ok(ph) => ph.with_budget_wait(std::time::Duration::from_secs(config.budget_wait_secs)),
        Err(error) => { mark_unread(&mut a, error); return a; }
    };
    a.recovery = Some(super::recovery::collect(&config.cron_job, &ph, &mut a.errors));
    a.exceptions = super::exceptions::collect(&ph, now, &mut a.errors);
    a.health.failure_events = config.failure_events.clone();
    let mut failed = Vec::new();
    if !config.witnesses.is_empty() {
        let conditions: Vec<String> = config.witnesses.values().cloned().collect();
        // Queried outside `run`: a failed query must leave no rows, not zero counts.
        match ph.query(&queries::witness_counts(&conditions)) {
            Ok(rows) => a.health.witnesses = config.witnesses.iter().enumerate().map(|(i, (event, condition))| Witness {
                event: event.clone(), condition: condition.clone(), count: rows.first().map_or(0.0, |r| num(r, i)),
            }).collect(),
            Err(error) => failed.push(format!("witnesses: {error}")),
        }
    }
    // Also outside `run`: a failed volume query leaves the tables unknown, never all zero.
    match (ph.query(&queries::volume()), expected_events(config, root)) {
        (Ok(volume), Ok(expected)) => (a.health.envs, a.health.events) = volume_tables(&volume, &expected),
        (Err(error), _) => { failed.push(format!("volume: {error}")); a.health.volume_error = Some(error); }
        (_, Err(error)) => a.errors.push(error),
    }
    let mut run = |label: &str, hogql: &str| ph.query(hogql).unwrap_or_else(|error| { failed.push(format!("{label}: {error}")); Vec::new() });
    for row in run("resends", &queries::resends(&config.server_events)) {
        if let Some(e) = a.health.events.iter_mut().find(|e| e.name == text(&row, 0)) { e.last_resend_hours = Some(num(&row, 1)); }
    }
    a.health.unlabelled = run("unlabelled", &queries::unlabelled()).iter().map(|r| (text(r, 0), text(r, 1), text(r, 2), num(r, 3))).collect();
    if let Some(r) = run("stitching", &queries::stitching(since)).first() { a.health.stitching = (num(r, 0), num(r, 1), num(r, 2)); }

    a.funnel_hogql = queries::funnel(since, &from);
    let funnel_rows = run("funnel", &a.funnel_hogql);
    a.funnel = funnel_days(&funnel_rows, today);
    a.leak = locate(&a.funnel, config.baseline_days, config.min_sample);
    if let Some(leak) = &a.leak {
        let window = leak.day.as_deref().map_or(from.clone(), queries::date_sql);
        for dim in DIMENSIONS.iter().filter(|d| d.key != "method" || leak.step >= 1) {
            let hogql = queries::breakdown(since, &window, leak.step, dim);
            let rows = run(dim.key, &hogql);
            a.breakdowns.push(Breakdown { label: format!("{}：{} → {}", dim.label, STEPS[leak.step].label, STEPS[leak.step + 1].label), hogql, rows });
        }
    }
    a.activation = run("activation", &queries::activation(since));
    a.retention = run("retention", &queries::retention(since));
    a.methods = run("signin", &queries::signin_methods(&from));
    a.failures = run("failures", &queries::signin_failures(&from));
    a.sources = run("sources", &queries::signup_sources(since));
    let rates = run("rates", &queries::daily_rates(since));
    a.hourly = fill(hours(&run("hourly", &queries::hourly())), now);
    if let (Some(log), Some(_)) = (&config.visitor_log, super::hourly::silence_finding(&a.hourly, now, config.silence_hours, config.silence_min_hourly)) {
        match visitor_loads(log, Utc::now() - Duration::hours(config.silence_hours)) {
            Ok(loads) => a.visitors = Some(loads),
            Err(error) => failed.push(format!("visitors: {error}")),
        }
    }
    if let Some(relay) = a.health.relay.as_mut() { relay.events_last_hour = a.hourly.iter().rev().take(2).map(|h| h.events).sum(); }
    let first_day = if config.tracking_start.ends_with("00:00:00") { start } else { start + Duration::days(1) };
    (a.alerts, a.alert_findings) = evaluate(&rates, &a.activation, &config.alerts, &Window { today, first_day, days: config.threshold_baseline_days });
    a.alerts_decided = config.alerts.decided.clone();
    a.errors.extend(failed);
    a
}

/// No PostHog client (the Keychain key could not be read, 2026-11-18 16:43): every PostHog source is
/// unread, so the volume is unknown, never zero; without this the empty tables raised a false
/// "production silent" P0.
fn mark_unread(a: &mut Analysis, error: String) {
    a.health.volume_error = Some(error.clone());
    a.errors.push(format!("volume: {error}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreadable_key_is_unknown_volume_not_a_silent_production() {
        let mut a = Analysis::default();
        mark_unread(&mut a, "Keychain entry mars-crm-posthog-personal-api-key not readable".into());
        assert!(super::super::health::findings(&a.health, 3.0).iter().all(|f| f.key != "health:production-silent"));
        let unread = super::super::sources::unread_finding(&a.errors, None).expect("unread finding");
        assert!(unread.title.contains("生产是否静默") && unread.title.contains("volume"));
    }
}
