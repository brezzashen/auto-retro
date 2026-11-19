//! Abandoned-checkout recovery (`checkout-recovery` CronJob, `crm.checkout_recovery_delivery`): how many first
//! checkouts were abandoned and mailed, how many readers came back through the email's coupon link
//! (`payment_exit_recall_clicked` with `coupon`) and how many paid with the coupon, plus every email sent in the
//! last 24 hours with a masked recipient. Read-only: `mars canary sql` on the shared database and `kubectl get`.

use super::config::{CronJobTarget, Finding};
use super::health::CronStatus;
use super::posthog::{num, PostHog};
use super::probes::cron_status;
use crate::skills::seo_competitor::watch_hour_db::{query, Row};
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;

const JOB: &str = "checkout-recovery";
/// The CronJob runs every 15 minutes; two missed passes are stale.
const MAX_AGE_HOURS: f64 = 0.75;

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryWatch {
    /// `None` while production has no `checkout-recovery` CronJob yet (before promotion).
    pub cron: Option<CronStatus>,
    pub counts: Row,
    pub recall_clicks_7d: Option<f64>,
    pub sent_24h: Vec<Row>,
}

const COUNTS: &str = "with starts as (
    select user_id, created_at started_at, coalesce(payment_expires_at, created_at + interval '2 hours') + interval '1 hour' due_at
      from crm.payment_order where status='pending' and gateway in ('stripe','alipay') and order_type='subscribe'
    union all
    select user_id, started_at, started_at + interval '3 hours' from crm.purchase_start where environment='Production'
  )
  select
    (select count(distinct s.user_id) from starts s where s.started_at > now() - interval '7 days' and s.due_at <= now()
       and not exists (select 1 from crm.payment_order p where p.user_id=s.user_id and p.status='paid' and p.paid_at >= s.started_at))::int abandoned_7d,
    (select count(*) from crm.checkout_recovery_delivery where sent_at > now() - interval '24 hours')::int mailed_24h,
    (select count(*) from crm.checkout_recovery_delivery where sent_at > now() - interval '7 days')::int mailed_7d,
    (select count(*) from crm.checkout_recovery_delivery)::int mailed_total,
    (select count(*) from crm.checkout_recovery_delivery where status in ('failed','uncertain') and last_error_code is distinct from 'no_longer_eligible'
       and coalesce(attempted_at, created_at) > now() - interval '24 hours')::int delivery_failed_24h,
    (select count(*) from crm.checkout_recovery_delivery where status='pending' and created_at < now() - interval '1 hour')::int stuck_pending,
    (select count(*) from crm.checkout_recovery_delivery d join crm.payment_order o on o.user_id=d.user_id and o.coupon_code=d.coupon_code
       and o.status='paid')::int recovered_total";

const SENT: &str = "select to_char(d.sent_at at time zone 'Asia/Singapore', 'MM-DD HH24:MI') sent_sgt, left(d.user_id::text, 8) user8,
    left(d.recipient, 1) || '***@' || split_part(d.recipient, '@', 2) recipient, d.gateway, d.plan, d.currency,
    d.coupon_price::text coupon_price, d.language,
    exists (select 1 from crm.payment_order o where o.user_id=d.user_id and o.coupon_code=d.coupon_code and o.status='paid') recovered
  from crm.checkout_recovery_delivery d where d.sent_at > now() - interval '24 hours' order by d.sent_at";

const RECALLS: &str = "select uniq(person_id) from events where event = 'payment_exit_recall_clicked'
  and properties.coupon = true and properties.mars_env = 'production' and timestamp > now() - interval 7 day";

/// Reads the CronJob, the ledger and the recall clicks; each failing source adds an error line.
pub fn collect(base: &CronJobTarget, ph: &PostHog, errors: &mut Vec<String>) -> RecoveryWatch {
    let target = CronJobTarget { name: JOB.into(), max_age_hours: MAX_AGE_HOURS, ..base.clone() };
    let cron = match cron_status(&target, Utc::now()) {
        Ok(status) => Some(status),
        Err(error) if error.contains("NotFound") || error.contains("not found") => None,
        Err(error) => { errors.push(format!("{JOB} CronJob: {error}")); None }
    };
    let counts = query(COUNTS).map(|rows| rows.into_iter().next().unwrap_or_default()).unwrap_or_else(|error| { errors.push(format!("recovery counts: {error}")); Row::new() });
    let sent_24h = query(SENT).unwrap_or_else(|error| { errors.push(format!("recovery sent: {error}")); Vec::new() });
    let recall_clicks_7d = match ph.query(RECALLS) {
        Ok(rows) => Some(rows.first().map_or(0.0, |row| num(row, 0))),
        Err(error) => { errors.push(format!("recovery recalls: {error}")); None }
    };
    RecoveryWatch { cron, counts, recall_clicks_7d, sent_24h }
}

fn count(row: &Row, key: &str) -> f64 {
    match row.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn field(row: &Row, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => if *b { "是".into() } else { "否".into() },
        Some(other) if !other.is_null() => other.to_string(),
        _ => String::new(),
    }
}

/// P1 when the CronJob fails or goes stale, or when mail delivery failed in the last 24 hours.
pub fn findings(watch: &RecoveryWatch) -> Vec<Finding> {
    let mut out = Vec::new();
    match &watch.cron {
        Some(c) if c.suspended => out.push(Finding::new("recovery:cronjob-suspended", "P1", "checkout-recovery CronJob 被暂停", "确认是否有意暂停；否则恢复生产 GKE overlay 的 suspend: false")),
        Some(c) if c.failed_24h > 0 => out.push(Finding::new("recovery:cronjob-failed", "P1", format!("checkout-recovery 24 小时内失败 {} 次", c.failed_24h), "查看失败 Job 的日志（mars canary logs）与 /internal/v1/email/checkout-recovery 的错误")),
        Some(c) if c.age_hours.is_none_or(|h| h > c.max_age_hours) => out.push(Finding::new("recovery:cronjob-stale", "P1",
            format!("checkout-recovery 上次成功是 {}", c.last_success.as_deref().unwrap_or("从未")), "检查 CronJob 是否在调度、crm 是否挂载了该路由（需要 SES 凭据 ses.json）")),
        _ => {}
    }
    let failed = count(&watch.counts, "delivery_failed_24h");
    if failed > 0.0 {
        out.push(Finding::new("recovery:mail-failed", "P1", format!("漏单补单邮件 24 小时内投递失败或结果未知 {failed} 封"), "查 crm.checkout_recovery_delivery.last_error_code 与 SES 退信"));
    }
    let stuck = count(&watch.counts, "stuck_pending");
    if stuck > 0.0 {
        out.push(Finding::new("recovery:mail-stuck", "P1", format!("漏单补单邮件有 {stuck} 封排队超过 1 小时未发出"), "查 checkout-recovery Job 日志与 SMTP 临时失败"));
    }
    out
}

pub fn section(watch: Option<&RecoveryWatch>) -> String {
    let mut md = String::from("## 漏单补单\n\n首单开始结账但没付款（Stripe、支付宝、App Store 购买开始）后约 3 小时，自动给已验证邮箱发一封首单 5 折个人券邮件，每个账号终身一次。\n\n");
    let Some(w) = watch else { md.push_str("本轮未读取。\n\n"); return md };
    match &w.cron {
        None => md.push_str("- `checkout-recovery` CronJob：生产尚未部署（等待上线到 example.com），不会发信\n"),
        Some(c) => md.push_str(&format!("- `checkout-recovery` CronJob：上次成功 {}（{:.1} 小时前），24 小时内失败 {} 次{}\n",
            c.last_success.as_deref().unwrap_or("从未"), c.age_hours.unwrap_or(f64::NAN), c.failed_24h, if c.suspended { "，**已暂停**" } else { "" })),
    }
    let c = &w.counts;
    let recalls = w.recall_clicks_7d.map_or("未知".to_string(), |n| format!("{n}"));
    md.push_str(&format!(
        "- 近 7 天漏单（开始结账、过期 1 小时后仍未付款的账号）{}；近 7 天发出 {}（24 小时 {}，累计 {}）；近 7 天点券链接回来 {} 人；累计用券付款 {}\n- 24 小时投递失败或未知 {}，排队超 1 小时 {}\n\n",
        count(c, "abandoned_7d"), count(c, "mailed_7d"), count(c, "mailed_24h"), count(c, "mailed_total"), recalls,
        count(c, "recovered_total"), count(c, "delivery_failed_24h"), count(c, "stuck_pending")));
    md.push_str("近 24 小时发出的邮件（收件人已打码）：\n\n| 发送（新加坡） | 账号 | 收件人 | 来源 | 套餐 | 券后价 | 语言 | 已用券付款 |\n|---|---|---|---|---|---|---|---|\n");
    if w.sent_24h.is_empty() { md.push_str("| 无 | | | | | | | |\n"); }
    for row in &w.sent_24h {
        let cells = ["sent_sgt", "user8", "recipient", "gateway", "plan"].map(|key| field(row, key));
        md.push_str(&format!("| {} | {} {} | {} | {} |\n", cells.join(" | "), field(row, "currency"), field(row, "coupon_price"), field(row, "language"), field(row, "recovered")));
    }
    md.push('\n');
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn watch(counts: Value, cron: Option<CronStatus>) -> RecoveryWatch {
        RecoveryWatch { cron, counts: counts.as_object().cloned().unwrap(), recall_clicks_7d: Some(1.0), sent_24h: vec![] }
    }

    #[test]
    fn a_missing_cronjob_before_promotion_is_reported_without_a_finding() {
        let w = watch(json!({"abandoned_7d": 3, "mailed_7d": 0}), None);
        assert!(findings(&w).is_empty());
        assert!(section(Some(&w)).contains("生产尚未部署"));
    }

    #[test]
    fn failed_jobs_and_failed_mail_are_p1() {
        let cron = CronStatus { last_success: Some("2026-11-14T10:00:00Z".into()), age_hours: Some(0.2), failed_24h: 2, suspended: false, max_age_hours: MAX_AGE_HOURS };
        let w = watch(json!({"delivery_failed_24h": "1", "stuck_pending": 0}), Some(cron));
        let keys: Vec<_> = findings(&w).into_iter().map(|f| (f.key, f.severity)).collect();
        assert_eq!(keys, vec![("recovery:cronjob-failed".to_string(), "P1".to_string()), ("recovery:mail-failed".to_string(), "P1".to_string())]);
    }
}
