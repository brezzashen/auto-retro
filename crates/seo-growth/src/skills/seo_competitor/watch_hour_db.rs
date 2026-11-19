//! This hour's sign-ups, orders and subscription changes read from the production database through
//! `mars canary sql` (read-only), so the hourly retro checks each order without writing its own SQL.
//! Windows are whole UTC hours ending at the run hour, which are also Singapore hour boundaries.

use chrono::{DateTime, Duration, DurationRound, Utc};
use serde::Serialize;
use serde_json::{Map, Value};
use std::io::Write;
use std::process::{Command, Stdio};

pub type Row = Map<String, Value>;

#[derive(Serialize, Clone, Debug, Default)]
pub struct HourDb {
    /// `[end - 1 h, end)` in Singapore time, `YYYY-MM-DD HH:MM`.
    pub from: String,
    pub to: String,
    /// `signups_h0/h1/h24` (`crm.user_account`) and `paid_h0/h1/h24` (`crm.payment_order.paid_at`):
    /// this hour, the previous hour and the same hour yesterday.
    pub counts: Row,
    /// Orders created or paid this hour; `environment` = `Sandbox` is an Apple test order, not revenue.
    /// `email_paid_accounts` counts accounts that paid with the same checkout email (above 1 = repeat buyer).
    pub orders: Vec<Row>,
    pub subscription_changes: Vec<Row>,
    /// The previous hour's sign-ups with their chats and payments up to this run (`watch_hour_followup`).
    pub followup: super::watch_hour_followup::FollowUp,
}

pub(crate) fn at(t: DateTime<Utc>) -> String { format!("timestamptz '{}'", t.format("%Y-%m-%d %H:%M:%S+00")) }

fn within(column: &str, end: DateTime<Utc>, offset: i64) -> String {
    format!("{column} >= {} and {column} < {}", at(end - Duration::hours(offset + 1)), at(end - Duration::hours(offset)))
}

pub fn counts_sql(end: DateTime<Utc>) -> String {
    let n = |table: &str, column: &str, offset: i64, name: &str| format!("(select count(*) from {table} where {}) {name}", within(column, end, offset));
    let mut parts = Vec::new();
    for (offset, tag) in [(0, "h0"), (1, "h1"), (24, "h24")] {
        parts.push(n("crm.user_account", "created_at", offset, &format!("signups_{tag}")));
        parts.push(n("crm.payment_order", "paid_at", offset, &format!("paid_{tag}")));
    }
    format!("select {}", parts.join(", "))
}

const SGT: &str = "at time zone 'Asia/Singapore', 'MM-DD HH24:MI')";

pub fn orders_sql(end: DateTime<Utc>) -> String {
    format!(
        "select o.order_no, o.status, o.environment, o.gateway, o.order_type, o.plan, o.amount, o.currency, o.platform, o.coupon_code, \
         to_char(o.created_at {SGT} created, to_char(o.paid_at {SGT} paid, \
         regexp_replace(coalesce(o.checkout_email, ''), '^(.).*(@.*)$', '\\1***\\2') email, left(o.user_id::text, 8) user8, \
         to_char(u.created_at at time zone 'Asia/Singapore', 'YYYY-MM-DD') account_created, \
         (select count(distinct o2.user_id) from crm.payment_order o2 where o2.checkout_email = o.checkout_email and o2.paid_at is not null) email_paid_accounts \
         from crm.payment_order o left join crm.user_account u on u.id = o.user_id \
         where ({}) or ({}) order by coalesce(o.paid_at, o.created_at) limit 50",
        within("o.created_at", end, 0), within("o.paid_at", end, 0)
    )
}

pub fn changes_sql(end: DateTime<Utc>) -> String {
    format!(
        "select to_char(created_at {SGT} at, action, from_plan, to_plan, payment_method, left(user_id::text, 8) user8, \
         to_char(period_ends_at at time zone 'Asia/Singapore', 'YYYY-MM-DD') period_ends \
         from crm.subscription_change where {} order by created_at limit 50",
        within("created_at", end, 0)
    )
}

/// Runs one read-only statement; `mars` prints banner lines before the JSON array.
pub(crate) fn query(sql: &str) -> Result<Vec<Row>, String> {
    let mut child = Command::new("mars").args(["canary", "sql", "--format=json", "--limit=200", "-"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("mars canary sql: {e}"))?;
    child.stdin.take().ok_or("mars canary sql: no stdin")?.write_all(sql.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let reason: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("[mars]") && !l.contains("://")).collect();
        return Err(format!("mars canary sql failed: {}", reason.join(" | ")));
    }
    parse(&stdout)
}

pub fn parse(stdout: &str) -> Result<Vec<Row>, String> {
    let start = if stdout.starts_with('[') { Some(0) } else { stdout.find("\n[").map(|i| i + 1) };
    let start = start.ok_or("mars canary sql: no JSON array")?;
    let rows: Vec<Value> = serde_json::from_str(stdout[start..].trim()).map_err(|e| format!("mars canary sql JSON: {e}"))?;
    Ok(rows.into_iter().filter_map(|r| r.as_object().cloned()).collect())
}

pub fn collect(now: DateTime<Utc>) -> Result<HourDb, String> {
    let end = now.duration_trunc(Duration::hours(1)).map_err(|e| e.to_string())?;
    let sgt = |t: DateTime<Utc>| (t + Duration::hours(8)).format("%Y-%m-%d %H:%M").to_string();
    let counts = query(&counts_sql(end))?.into_iter().next().unwrap_or_default();
    Ok(HourDb { from: sgt(end - Duration::hours(1)), to: sgt(end), counts, orders: query(&orders_sql(end))?, subscription_changes: query(&changes_sql(end))?, followup: super::watch_hour_followup::collect(end)? })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end() -> DateTime<Utc> { DateTime::parse_from_rfc3339("2026-11-13T07:00:00Z").unwrap().with_timezone(&Utc) }

    #[test]
    fn counts_cover_this_hour_the_previous_hour_and_the_same_hour_yesterday() {
        let sql = counts_sql(end());
        assert!(sql.contains("created_at >= timestamptz '2026-11-13 06:00:00+00' and created_at < timestamptz '2026-11-13 07:00:00+00') signups_h0"));
        assert!(sql.contains("paid_at >= timestamptz '2026-11-13 05:00:00+00' and paid_at < timestamptz '2026-11-13 06:00:00+00') paid_h1"));
        assert!(sql.contains("paid_at >= timestamptz '2026-11-12 06:00:00+00' and paid_at < timestamptz '2026-11-12 07:00:00+00') paid_h24"));
    }

    #[test]
    fn orders_mask_the_email_and_flag_repeat_buyers() {
        let sql = orders_sql(end());
        assert!(sql.contains("'^(.).*(@.*)$', '\\1***\\2') email") && !sql.contains(", o.checkout_email, ") && sql.contains("email_paid_accounts"));
        assert!(sql.contains("o.environment") && sql.contains("o.paid_at >= timestamptz '2026-11-13 06:00:00+00'"));
        assert!(changes_sql(end()).contains("from crm.subscription_change where created_at >= timestamptz '2026-11-13 06:00:00+00'"));
    }

    #[test]
    fn the_json_array_is_read_after_banner_lines() {
        let rows = parse("canary (GKE): read-only\n[\n  {\"n\": 1}\n]\n").unwrap();
        assert_eq!(rows[0]["n"], 1);
        assert!(parse("[]").unwrap().is_empty() && parse("no rows").is_err());
    }
}
