//! Follow-up of the previous hour's sign-ups (founder 2026-11-15): one run later, read from the
//! production database whether each account has chatted and paid so far, so the hourly retro reports
//! what sign-ups actually did instead of stopping at the sign-up count.

use super::watch_hour_db::{at, query, Row};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

#[derive(Serialize, Clone, Debug, Default)]
pub struct FollowUp {
    /// The sign-up hour `[end - 2 h, end - 1 h)` in Singapore time, `YYYY-MM-DD HH:MM`.
    pub from: String,
    pub to: String,
    pub signups: usize,
    /// Accounts with at least one user message in any conversation, up to the run hour.
    pub chatted: usize,
    /// Accounts with a paid order that is not an Apple Sandbox test order, up to the run hour.
    pub paid: usize,
    /// One row per account: `user8`, `created`, `threads`, `user_messages`, `first_message`,
    /// `last_message`, `checkouts`, `paid_orders`, `paid` (plan amount currency per order), `sandbox_paid`.
    pub accounts: Vec<Row>,
}

/// `agent.mastra_messages."createdAt"` is a UTC `timestamp without time zone`.
const MSG_SGT: &str = "(m.\"createdAt\" at time zone 'UTC') at time zone 'Asia/Singapore', 'MM-DD HH24:MI')";

pub fn followup_sql(end: DateTime<Utc>) -> String {
    let user_msgs = "from agent.mastra_messages m where m.\"resourceId\" = u.id::text and m.role = 'user'";
    let real = "o.environment is distinct from 'Sandbox'";
    format!(
        "select left(u.id::text, 8) user8, to_char(u.created_at at time zone 'Asia/Singapore', 'MM-DD HH24:MI') created, \
         (select count(*) from agent.mastra_threads t where t.\"resourceId\" = u.id::text) threads, \
         (select count(*) {user_msgs}) user_messages, \
         (select to_char(min{MSG_SGT} {user_msgs}) first_message, \
         (select to_char(max{MSG_SGT} {user_msgs}) last_message, \
         (select count(*) from crm.payment_order o where o.user_id = u.id and {real}) checkouts, \
         (select count(*) from crm.payment_order o where o.user_id = u.id and o.paid_at is not null and {real}) paid_orders, \
         (select string_agg(concat_ws(' ', o.plan, o.amount, o.currency), '; ' order by o.paid_at) from crm.payment_order o \
          where o.user_id = u.id and o.paid_at is not null and {real}) paid, \
         (select count(*) from crm.payment_order o where o.user_id = u.id and o.paid_at is not null and o.environment = 'Sandbox') sandbox_paid \
         from crm.user_account u where u.created_at >= {} and u.created_at < {} order by u.created_at limit 200",
        at(end - Duration::hours(2)), at(end - Duration::hours(1))
    )
}

fn positive(row: &Row, key: &str) -> bool {
    row.get(key).and_then(|v| v.as_str().and_then(|s| s.parse::<i64>().ok()).or_else(|| v.as_i64())).unwrap_or(0) > 0
}

pub fn summarize(from: String, to: String, accounts: Vec<Row>) -> FollowUp {
    let chatted = accounts.iter().filter(|r| positive(r, "user_messages")).count();
    let paid = accounts.iter().filter(|r| positive(r, "paid_orders")).count();
    FollowUp { from, to, signups: accounts.len(), chatted, paid, accounts }
}

pub fn collect(end: DateTime<Utc>) -> Result<FollowUp, String> {
    let sgt = |t: DateTime<Utc>| (t + Duration::hours(8)).format("%Y-%m-%d %H:%M").to_string();
    Ok(summarize(sgt(end - Duration::hours(2)), sgt(end - Duration::hours(1)), query(&followup_sql(end))?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end() -> DateTime<Utc> { DateTime::parse_from_rfc3339("2026-11-13T07:00:00Z").unwrap().with_timezone(&Utc) }

    #[test]
    fn the_cohort_is_the_previous_hour_and_activity_runs_up_to_now() {
        let sql = followup_sql(end());
        assert!(sql.contains("u.created_at >= timestamptz '2026-11-13 05:00:00+00' and u.created_at < timestamptz '2026-11-13 06:00:00+00'"));
        assert!(!sql.contains("m.\"createdAt\" <") && !sql.contains("o.paid_at <"), "chats and payments count up to the run, not only inside the sign-up hour");
    }

    #[test]
    fn messages_are_user_turns_read_as_utc_and_sandbox_orders_are_apart() {
        let sql = followup_sql(end());
        assert!(sql.contains("m.role = 'user'") && sql.contains("(m.\"createdAt\" at time zone 'UTC') at time zone 'Asia/Singapore'"));
        assert!(sql.contains("o.environment is distinct from 'Sandbox'") && sql.contains("o.environment = 'Sandbox') sandbox_paid"));
    }

    #[test]
    fn the_summary_counts_accounts_that_chatted_and_paid() {
        let row = |m: &str, p: &str| serde_json::json!({"user_messages": m, "paid_orders": p}).as_object().unwrap().clone();
        let f = summarize("a".into(), "b".into(), vec![row("2", "0"), row("0", "0"), row("5", "1")]);
        assert_eq!((f.signups, f.chatted, f.paid), (3, 2, 1));
    }
}
