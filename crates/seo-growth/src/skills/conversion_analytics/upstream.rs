//! A zero the funnel above it explains is a quiet P2, not a broken-tracking P0:
//! - a server event downstream of sign-up (`person_created`, `order_paid`) while the
//!   `analytics-sync` pass is healthy and sign-ups fell below half their prior mean (no
//!   first-of-source rows to send); with sign-ups at their usual level the zero stays P0, since a
//!   wrong-but-valid SQL never fails the pass;
//! - a screen-only event (`config.witnesses`, e.g. `onboarding_step_viewed`) while its witness condition (the
//!   web `/onboarding` pageview, a phone-line source answer on web or iOS) matched nothing either: nobody
//!   reached the screen. With witnesses present, or the witness query failed, the zero stays P0;
//! - a failure event (`config.failure_events`, e.g. `auth_failed`) for fewer than `FAILURE_ZERO_DAYS`
//!   windows in a row: nobody failed. Its call site is guarded by tests; a longer run stays P0.

use super::config::Finding;
use super::health::{CronStatus, Health, VolumeRow};

const SIGNUP: &str = "user_signed_up";
/// Consecutive zero 24-hour windows after which a failure event's silence is a P0 again.
pub const FAILURE_ZERO_DAYS: usize = 3;

fn cron_healthy(cron: Option<&CronStatus>) -> bool {
    cron.is_some_and(|c| !c.suspended && c.failed_24h == 0 && c.age_hours.is_some_and(|h| h <= c.max_age_hours))
}

/// The P2 that replaces `health:zero:<event>` when the step above explains the zero.
pub fn explained_zero(health: &Health, event: &VolumeRow) -> Option<Finding> {
    if event.source == "server" {
        return server_zero(health, event);
    }
    if health.failure_events.contains(&event.name) {
        return failure_zero(event);
    }
    witnessed_zero(health, event)
}

fn server_zero(health: &Health, event: &VolumeRow) -> Option<Finding> {
    if event.name == SIGNUP || !cron_healthy(health.cron.as_ref()) {
        return None;
    }
    let signups = health.events.iter().find(|e| e.name == SIGNUP)?;
    if signups.prior_mean <= 0.0 || signups.last24 >= signups.prior_mean / 2.0 {
        return None;
    }
    Some(Finding::new(
        format!("watch:quiet-upstream:{}", event.name),
        "P2",
        format!(
            "`{}` 24 小时为 0（此前日均 {:.1}），但同期注册只有 {}（此前日均 {:.1}），analytics-sync 每小时正常：多为新账号没人走到这一步",
            event.name, event.prior_mean, signups.last24, signups.prior_mean
        ),
        "无需修复；用 mars canary sql 查窗口内按来源的首条 crm.person / 已付订单为 0 即确认。注册回升后仍为 0 再按 P0 查",
    ))
}

fn failure_zero(event: &VolumeRow) -> Option<Finding> {
    if event.zero_days >= FAILURE_ZERO_DAYS {
        return None;
    }
    Some(Finding::new(
        format!("watch:quiet-failure:{}", event.name),
        "P2",
        format!("失败事件 `{}` 24 小时为 0（此前日均 {:.1}）：没人失败，不是埋点坏了", event.name, event.prior_mean),
        format!("无需修复；调用点有单测守护。连续 {FAILURE_ZERO_DAYS} 天为 0 时升为 P0，再用 Supabase 认证日志核对是否真有失败"),
    ))
}

fn witnessed_zero(health: &Health, event: &VolumeRow) -> Option<Finding> {
    let witness = health.witnesses.iter().find(|w| w.event == event.name)?;
    if witness.count > 0.0 {
        return None;
    }
    Some(Finding::new(
        format!("watch:quiet-upstream:{}", event.name),
        "P2",
        format!(
            "`{}` 24 小时为 0（此前日均 {:.1}），但同期能证明有人到达这一屏的事件（`{}`）也为 0：没人走到，不是埋点坏了",
            event.name, event.prior_mean, witness.condition
        ),
        "无需修复；见证事件出现而该事件仍为 0 时再按 P0 查。入口流量下降（如手机端点击开始）属产品转化问题，只写建议",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::health::{findings, Witness};

    fn health(signups_last24: f64, cron: Option<CronStatus>) -> Health {
        let row = |name: &str, last24, prior_mean| VolumeRow { name: name.into(), source: "server".into(), last24, prior_mean, duplicates: 1.0, ..Default::default() };
        Health {
            envs: vec![VolumeRow { name: "production".into(), last24: 900.0, ..Default::default() }],
            events: vec![row("user_signed_up", signups_last24, 11.7), row("person_created", 0.0, 3.9)],
            cron,
            ..Default::default()
        }
    }

    fn keys(h: &Health) -> Vec<(String, String)> {
        findings(h, 3.0).into_iter().map(|f| (f.key, f.severity)).collect()
    }

    fn ok_cron() -> Option<CronStatus> {
        Some(CronStatus { age_hours: Some(0.3), max_age_hours: 2.0, ..Default::default() })
    }

    #[test]
    fn a_downstream_zero_after_a_signup_drop_with_a_healthy_sync_is_a_quiet_p2() {
        assert_eq!(keys(&health(2.0, ok_cron())), vec![("watch:quiet-upstream:person_created".into(), "P2".into())]);
    }

    #[test]
    fn the_zero_stays_p0_when_signups_held_or_the_sync_is_unknown_or_failing() {
        assert_eq!(keys(&health(10.0, ok_cron())), vec![("health:zero:person_created".into(), "P0".into())]);
        assert_eq!(keys(&health(2.0, None)), vec![("health:zero:person_created".into(), "P0".into())]);
        let failed = Some(CronStatus { age_hours: Some(0.3), max_age_hours: 2.0, failed_24h: 1, ..Default::default() });
        assert_eq!(keys(&health(2.0, failed)), vec![("health:zero:person_created".into(), "P0".into()), ("health:cronjob-failed".into(), "P0".into())]);
    }

    fn onboarding(witnessed: Option<f64>) -> Health {
        let witness = |count| Witness { event: "onboarding_step_viewed".into(), condition: "properties.$pathname = '/onboarding'".into(), count };
        Health {
            envs: vec![VolumeRow { name: "production".into(), last24: 1359.0, ..Default::default() }],
            events: vec![VolumeRow { name: "onboarding_step_viewed".into(), source: "web".into(), last24: 0.0, prior_mean: 58.6, duplicates: 1.0, ..Default::default() }],
            witnesses: witnessed.map(|count| vec![witness(count)]).unwrap_or_default(),
            ..Default::default()
        }
    }

    fn failure(zero_days: usize) -> Health {
        Health {
            envs: vec![VolumeRow { name: "production".into(), last24: 2252.0, ..Default::default() }],
            events: vec![VolumeRow { name: "auth_failed".into(), source: "web".into(), prior_mean: 14.0, duplicates: 1.0, zero_days, ..Default::default() }],
            failure_events: vec!["auth_failed".into()],
            ..Default::default()
        }
    }

    #[test]
    fn a_failure_event_at_zero_is_a_quiet_p2_until_it_stays_silent_for_three_days() {
        assert_eq!(keys(&failure(1)), vec![("watch:quiet-failure:auth_failed".into(), "P2".into())]);
        assert_eq!(keys(&failure(2)), vec![("watch:quiet-failure:auth_failed".into(), "P2".into())]);
        assert_eq!(keys(&failure(3)), vec![("health:zero:auth_failed".into(), "P0".into())]);
        let unlisted = Health { failure_events: Vec::new(), ..failure(1) };
        assert_eq!(keys(&unlisted), vec![("health:zero:auth_failed".into(), "P0".into())]);
    }

    #[test]
    fn a_screen_only_event_at_zero_with_no_witness_is_a_quiet_p2() {
        assert_eq!(keys(&onboarding(Some(0.0))), vec![("watch:quiet-upstream:onboarding_step_viewed".into(), "P2".into())]);
    }

    #[test]
    fn the_zero_stays_p0_when_a_witness_arrived_or_its_query_failed() {
        assert_eq!(keys(&onboarding(Some(4.0))), vec![("health:zero:onboarding_step_viewed".into(), "P0".into())]);
        assert_eq!(keys(&onboarding(None)), vec![("health:zero:onboarding_step_viewed".into(), "P0".into())]);
    }
}
