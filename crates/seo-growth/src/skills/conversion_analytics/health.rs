//! Tracking health: per-environment and per-event volume of the last 24 h against the prior days,
//! plus the `analytics-sync` CronJob, the `/ingest` relay, unlabelled events and identity stitching.
//! Pure functions over the query rows, so every finding rule is unit-tested.

use super::config::Finding;
use super::posthog::{num, text};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct VolumeRow {
    pub name: String,
    /// `web` (the `AnalyticsEvent` union), `sdk`, `server`, or `env` for an environment row.
    pub source: String,
    pub last24: f64,
    /// Mean per 24 h over the prior windows (up to 7) in which production received any event.
    pub prior_mean: f64,
    /// Stored rows per unique uuid in the last 24 h; above 1 the raw `count()` double counts.
    pub duplicates: f64,
    /// Hours since the newest resent copy of a last-24-hour server row (`queries::resends`);
    /// `None` when no row was stored twice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_resend_hours: Option<f64>,
    /// Consecutive 24-hour windows, newest first, in which production stored none of this event.
    pub zero_days: usize,
}

/// A resend older than this is residue the 24-hour window still holds, not an active resend: the
/// hourly pass runs at :07, so an active defect shows a resend at most about an hour old.
pub const RESEND_ACTIVE_HOURS: f64 = 2.0;

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CronStatus {
    pub last_success: Option<String>,
    pub age_hours: Option<f64>,
    pub failed_24h: usize,
    pub suspended: bool,
    pub max_age_hours: f64,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RelayStatus {
    pub asset_status: u16,
    pub capture_status: u16,
    pub ok: bool,
    /// Probe rounds run (a failed round is retried).
    pub attempts: u32,
    /// Transport error of the last round, when a request got no HTTP answer at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Production events PostHog stored in the current and previous Singapore hour, which only
    /// arrive through the relay: with both probes unanswered, a nonzero count means this host's
    /// path to example.com failed, not the relay.
    pub events_last_hour: f64,
    /// Session replay switch PostHog serves to a production page through the relay
    /// (`probes::replay_enabled`); `None` when the probe got no answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay: Option<bool>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub envs: Vec<VolumeRow>,
    pub events: Vec<VolumeRow>,
    /// `(host, path, event, count)` of last-24-hour events without `mars_env`.
    pub unlabelled: Vec<(String, String, String, f64)>,
    /// Production sign-ups since tracking start, those with any web event, those with an auth event.
    pub stitching: (f64, f64, f64),
    pub cron: Option<CronStatus>,
    pub relay: Option<RelayStatus>,
    /// Screen-only events with the last-24-hour production events matching their witness condition
    /// (`config.witnesses`); empty when the query failed, which keeps their zero a P0.
    pub witnesses: Vec<Witness>,
    /// Events that record a failure (`config.failure_events`): a day without them is good news.
    pub failure_events: Vec<String>,
    /// The volume query's error: `envs` and `events` are then unknown, not zero, and no silence
    /// or zero rule may read them (2026-11-13: a spent PostHog query budget raised a false P0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_error: Option<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Witness {
    pub event: String,
    pub condition: String,
    pub count: f64,
}

/// Turns `queries::volume` rows into environment rows and one row per expected event.
pub fn volume_tables(rows: &[Vec<Value>], expected: &[(String, &str)]) -> (Vec<VolumeRow>, Vec<VolumeRow>) {
    let mut by_env: BTreeMap<String, [f64; 8]> = BTreeMap::new();
    let mut prod: BTreeMap<String, ([f64; 8], f64, f64)> = BTreeMap::new();
    for row in rows {
        let (env, event, ago) = (text(row, 0), text(row, 1), num(row, 2) as usize);
        if ago > 7 { continue; }
        let (stored, unique) = (num(row, 3), num(row, 4));
        by_env.entry(env.clone()).or_default()[ago] += unique;
        if env == "production" {
            let entry = prod.entry(event).or_default();
            entry.0[ago] += unique;
            if ago == 0 { entry.1 += stored; entry.2 += unique; }
        }
    }
    let active: Vec<usize> = (1..8).filter(|&i| by_env.get("production").is_some_and(|w| w[i] > 0.0)).collect();
    let mean = |w: &[f64; 8]| if active.is_empty() { 0.0 } else { active.iter().map(|&i| w[i]).sum::<f64>() / active.len() as f64 };
    let envs = by_env.iter().map(|(env, w)| {
        let own: Vec<usize> = (1..8).filter(|&i| w[i] > 0.0).collect();
        let prior = if own.is_empty() { 0.0 } else { own.iter().map(|&i| w[i]).sum::<f64>() / own.len() as f64 };
        VolumeRow { name: env.clone(), source: "env".into(), last24: w[0], prior_mean: prior, duplicates: 1.0, ..Default::default() }
    }).collect();
    let events = expected.iter().map(|(event, source)| {
        let (w, stored, unique) = prod.get(event).cloned().unwrap_or(([0.0; 8], 0.0, 0.0));
        let zero_days = w.iter().take_while(|&&count| count == 0.0).count();
        VolumeRow { name: event.clone(), source: (*source).into(), last24: w[0], prior_mean: mean(&w), duplicates: if unique > 0.0 { stored / unique } else { 1.0 }, last_resend_hours: None, zero_days }
    }).collect();
    (envs, events)
}

/// Every health finding; `min_daily` separates a broken event from a naturally quiet one.
pub fn findings(health: &Health, min_daily: f64) -> Vec<Finding> {
    let mut out = Vec::new();
    let production = health.envs.iter().find(|e| e.name == "production");
    if health.volume_error.is_none() && production.is_none_or(|p| p.last24 == 0.0) {
        out.push(Finding::new("health:production-silent", "P0", "生产 24 小时内没有收到任何 PostHog 事件", "检查 example.com 是否加载 posthog-js、/ingest 中转与项目 token"));
    }
    let mut never = Vec::new();
    for e in &health.events {
        if e.last24 == 0.0 && e.prior_mean >= min_daily {
            if let Some(quiet) = super::upstream::explained_zero(health, e) {
                out.push(quiet);
                continue;
            }
            out.push(Finding::new(format!("health:zero:{}", e.name), "P0", format!("`{}` 生产 24 小时归零（此前日均 {:.1}）", e.name, e.prior_mean),
                "找到该事件的调用点或 analytics-sync 查询，确认最近发布是否删改了它，修复并经 beta 交付到 Canary"));
        } else if e.last24 == 0.0 && e.prior_mean > 0.0 {
            out.push(Finding::new(format!("watch:quiet:{}", e.name), "P2", format!("`{}` 今日为 0，但此前日均仅 {:.1}，低量波动", e.name, e.prior_mean), "继续观察，连续 3 天为 0 再查"));
        } else if e.last24 == 0.0 && e.prior_mean == 0.0 {
            never.push(format!("`{}`", e.name));
        }
        if e.source == "server" && e.duplicates > 1.05 && e.last_resend_hours.is_some_and(|h| h > RESEND_ACTIVE_HOURS) {
            out.push(Finding::new(format!("watch:duplicates-residue:{}", e.name), "P2",
                format!("服务端事件 `{}` 近 24 小时同一 uuid 存了 {:.2} 份，但最后一次重发在 {:.1} 小时前，此后每小时的 pass 都没有重发：是修复前留下的残留", e.name, e.duplicates, e.last_resend_hours.unwrap_or_default()),
                "无需修复：残留滑出 24 小时窗口后归 1；计数继续用 uniq(uuid)"));
        } else if e.source == "server" && e.duplicates > 1.05 {
            out.push(Finding::new(format!("health:duplicates:{}", e.name), "P1", format!("服务端事件 `{}` 近 24 小时同一 uuid 存了 {:.2} 份（重叠的 3 小时窗口重发，PostHog 未合并），count() 与仪表板总数会重复计数", e.name, e.duplicates),
                "让 analytics-sync 不重发已发过的行，或把仪表板的服务端系列改为按人去重；本页已按 uuid 去重"));
        }
    }
    if !never.is_empty() {
        out.push(Finding::new("watch:never", "P2", format!("生产从未收到：{}", never.join("、")), "多为还没人走到这一步；若同期有对应行为（如服务端注册而无验证码事件），再按埋点缺陷处理"));
    }
    let mut total = 0.0;
    let mut items: Vec<String> = Vec::new();
    for (host, path, event, count) in &health.unlabelled {
        total += count;
        let item = format!("{host}{path} `{event}`");
        if !items.contains(&item) { items.push(item); }
    }
    if !items.is_empty() {
        out.push(Finding::new("health:unlabelled", "P1", format!("近 24 小时有 {total} 条事件没有 mars_env（{}），按环境过滤时会丢掉它们", items.join("、")),
            "mars_env 目前在 analyticsConfig 的 loaded 回调里才 register（web/src/lib/analytics.ts），可能晚于首个 $pageview；改为在 before_send 里补上，或在 init 前注册"));
    }
    let (signups, with_web, _) = health.stitching;
    if signups > with_web {
        out.push(Finding::new("watch:unstitched", "P2", format!("追踪开始后的 {signups} 个新注册里 {} 个没有任何网页事件", signups - with_web), "多为 iOS 注册（iOS SDK 未接入）；若都来自网页，则是 identify 断了"));
    }
    match &health.cron {
        Some(c) if c.suspended => out.push(Finding::new("health:cronjob-suspended", "P0", "analytics-sync CronJob 被暂停", "恢复生产 GKE overlay 的 suspend: false")),
        Some(c) if c.age_hours.is_none_or(|h| h > c.max_age_hours) => out.push(Finding::new("health:cronjob-stale", "P0",
            format!("analytics-sync 上次成功是 {}", c.last_success.as_deref().unwrap_or("从未")), "查看 Job 日志（mars canary logs）与 /internal/v1/analytics/sync 的错误")),
        Some(c) if c.failed_24h > 0 => out.push(Finding::new("health:cronjob-failed", "P0", format!("analytics-sync 24 小时内失败 {} 次", c.failed_24h), "查看失败 Job 的日志并修复")),
        _ => {}
    }
    if health.relay.as_ref().is_some_and(|r| !r.ok && r.asset_status == 0 && r.capture_status == 0 && r.events_last_hour > 0.0) {
        let r = health.relay.as_ref().expect("checked above");
        out.push(Finding::new("watch:relay-probe", "P2",
            format!("本机探测 example.com/ingest {} 次都没有得到 HTTP 应答（{}），但生产最近一小时仍有 {} 个事件经中转到达：是本机到 example.com 的网络问题，不是中转故障", r.attempts, r.error.as_deref().unwrap_or("无错误文本"), r.events_last_hour),
            "连续多次出现再查本机网络（Surge、DNS）；中转本身以生产网关日志的 /ingest 请求为准"));
    } else if health.relay.as_ref().is_some_and(|r| !r.ok) {
        out.push(Finding::new("health:relay", "P0", "example.com/ingest 中转没有正常转发到 PostHog", "检查 api gateway/posthog_proxy.rs 与生产网关日志"));
    }
    if health.relay.as_ref().is_some_and(|r| r.replay == Some(false)) {
        out.push(Finding::new("health:replay-off", "P1", "PostHog 经 example.com/ingest 返回 sessionRecording: false，生产会话回放不会录制",
            "查项目设置 session_recording_opt_in 与 recording_domains，以及中转是否把页面 Origin 传给 PostHog（posthog_proxy.rs page_origin）"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rows() -> Vec<Vec<Value>> {
        vec![
            vec![json!("production"), json!("$pageview"), json!(0), json!(50), json!(50)],
            vec![json!("production"), json!("$pageview"), json!(1), json!(40), json!(40)],
            vec![json!("production"), json!("cta_clicked"), json!(1), json!(6), json!(6)],
            vec![json!("production"), json!("auth_failed"), json!(1), json!(1), json!(1)],
            vec![json!("production"), json!("user_signed_up"), json!(0), json!(4), json!(2)],
        ]
    }

    #[test]
    fn a_busy_event_at_zero_is_p0_a_quiet_one_is_p2_and_duplicates_are_p1() {
        let expected = [("$pageview", "sdk"), ("cta_clicked", "web"), ("auth_failed", "web"), ("checkout_started", "web"), ("user_signed_up", "server")]
            .map(|(e, s)| (e.to_string(), s));
        let (envs, events) = volume_tables(&rows(), &expected);
        assert_eq!(envs[0].last24, 52.0);
        assert_eq!(events.iter().map(|e| e.zero_days).collect::<Vec<_>>(), vec![0, 1, 1, 8, 0]);
        let health = Health { envs, events, stitching: (3.0, 3.0, 3.0), ..Default::default() };
        let keys: Vec<(String, String)> = findings(&health, 3.0).into_iter().map(|f| (f.key, f.severity)).collect();
        assert!(keys.contains(&("health:zero:cta_clicked".into(), "P0".into())));
        assert!(keys.contains(&("watch:quiet:auth_failed".into(), "P2".into())));
        assert!(keys.contains(&("watch:never".into(), "P2".into())));
        assert!(keys.contains(&("health:duplicates:user_signed_up".into(), "P1".into())));
        assert!(!keys.iter().any(|(k, _)| k == "health:production-silent"));
    }

    #[test]
    fn an_unread_volume_is_never_a_silent_production() {
        let health = Health { volume_error: Some("PostHog query/: HTTP 429".into()), ..Default::default() };
        assert!(findings(&health, 3.0).is_empty());
    }

    #[test]
    fn a_stalled_cronjob_and_a_broken_relay_are_p0() {
        let health = Health { envs: vec![VolumeRow { name: "production".into(), last24: 9.0, ..Default::default() }],
            cron: Some(CronStatus { age_hours: Some(5.0), max_age_hours: 2.0, ..Default::default() }), relay: Some(RelayStatus { ok: false, ..Default::default() }), ..Default::default() };
        let keys: Vec<String> = findings(&health, 3.0).into_iter().map(|f| f.key).collect();
        assert_eq!(keys, vec!["health:cronjob-stale", "health:relay"]);
    }

    #[test]
    fn replay_switched_off_is_p1_and_an_unanswered_replay_probe_is_silent() {
        let relay = |replay| Health { envs: vec![VolumeRow { name: "production".into(), last24: 9.0, ..Default::default() }],
            relay: Some(RelayStatus { ok: true, asset_status: 200, capture_status: 401, replay, ..Default::default() }), ..Default::default() };
        let keys = |h: &Health| findings(h, 0.0).into_iter().map(|f| (f.key, f.severity)).collect::<Vec<_>>();
        assert_eq!(keys(&relay(Some(false))), vec![("health:replay-off".into(), "P1".into())]);
        assert!(keys(&relay(Some(true))).is_empty());
        assert!(keys(&relay(None)).is_empty());
    }

    #[test]
    fn an_unanswered_probe_while_events_arrive_is_a_host_network_p2_but_a_wrong_answer_stays_p0() {
        let relay = |asset_status, events_last_hour| Health { envs: vec![VolumeRow { name: "production".into(), last24: 9.0, ..Default::default() }],
            relay: Some(RelayStatus { asset_status, events_last_hour, attempts: 3, error: Some("asset: timeout".into()), ..Default::default() }), ..Default::default() };
        let keys = |h: &Health| findings(h, 3.0).into_iter().map(|f| (f.key, f.severity)).collect::<Vec<_>>();
        assert_eq!(keys(&relay(0, 25.0)), vec![("watch:relay-probe".into(), "P2".into())]);
        assert_eq!(keys(&relay(0, 0.0)), vec![("health:relay".into(), "P0".into())]);
        assert_eq!(keys(&relay(502, 25.0)), vec![("health:relay".into(), "P0".into())]);
    }

    #[test]
    fn duplicates_whose_last_resend_is_old_are_residue_and_recent_ones_stay_p1() {
        let dup = |hours| Health { envs: vec![VolumeRow { name: "production".into(), last24: 9.0, ..Default::default() }],
            events: vec![VolumeRow { name: "person_created".into(), source: "server".into(), last24: 3.0, prior_mean: 3.0, duplicates: 1.67, last_resend_hours: hours, ..Default::default() }], ..Default::default() };
        let keys = |h: &Health| findings(h, 3.0).into_iter().map(|f| (f.key, f.severity)).collect::<Vec<_>>();
        assert_eq!(keys(&dup(Some(4.5))), vec![("watch:duplicates-residue:person_created".into(), "P2".into())]);
        assert_eq!(keys(&dup(Some(0.9))), vec![("health:duplicates:person_created".into(), "P1".into())]);
        assert_eq!(keys(&dup(None)), vec![("health:duplicates:person_created".into(), "P1".into())]);
    }
}
