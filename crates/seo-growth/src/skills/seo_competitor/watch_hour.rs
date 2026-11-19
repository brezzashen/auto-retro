//! The run hour in detail for the hourly SEO retro: one row per real visitor of the current whole
//! hour (PostHog) and this hour's sign-ups, orders and subscription changes from the database
//! (`watch_hour_db`). Conversation retro 85832046819103478997 (2026-11-13): 125 of 134 hourly retros
//! wrote their own HogQL for these visitor rows (576 queries) and 57 wrote `mars canary sql` for the
//! orders (297 calls), so the watchdog collects both once and the retro reads `traffic.hour`.

use super::watch_hour_db::{self, HourDb};
use super::watch_segments::{END, WEB_START};
use crate::skills::conversion_analytics::posthog::{num, text, PostHog};
use serde::Serialize;

/// How long a watch run waits, once per query, when PostHog's hourly read budget (shared by every
/// session) is spent; the 15:17, 16:17 and 17:17 runs of 2026-11-13 lost all traffic data to it.
pub const POSTHOG_BUDGET_WAIT: std::time::Duration = std::time::Duration::from_secs(900);

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct HourVisitor {
    /// First 8 characters of the PostHog person id.
    pub person: String,
    /// Session-entry source of the first page view (same rule as `traffic.sources`).
    pub source: String,
    pub os: String,
    pub browser: String,
    pub timezone: String,
    /// `$ip` with its last part masked (`34.31.195.*`; IPv6 keeps four groups).
    pub ip: String,
    pub screen_width: f64,
    /// Singapore `HH:MM` of the first and last event in the hour.
    pub first_seen: String,
    pub last_seen: String,
    pub landing: String,
    /// Up to five distinct page paths, space separated.
    pub pages: String,
    pub pageviews: f64,
    /// Events other than page views and page leaves (clicks, vitals, sign-in steps).
    pub other: f64,
    pub cta: f64,
    pub signed_in: f64,
    pub messages: f64,
    /// Raw User-Agent, first 180 characters (`MicroMessenger`, in-app browsers, headless names).
    pub user_agent: String,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Hour {
    /// Real visitors of `[END - 1 hour, END)`, own machines and known machine traffic left out.
    pub visitors: Vec<HourVisitor>,
    pub visitors_error: Option<String>,
    pub db: Option<HourDb>,
    pub db_error: Option<String>,
}

const MASKED_IP: &str = "if(position(coalesce(properties.$ip, ''), ':') > 0, \
     concat(arrayStringConcat(arraySlice(splitByChar(':', properties.$ip), 1, 4), ':'), ':*'), \
     concat(arrayStringConcat(arraySlice(splitByChar('.', coalesce(properties.$ip, '')), 1, 3), '.'), '.*'))";

pub fn visitors_sql(end: &str, source: &str, excluded: &str) -> String {
    format!(
        "select substring(toString(person_id), 1, 8), argMinIf({source}, timestamp, event = '$pageview'), any(properties.$os), any(properties.$browser), \
         any(properties.$timezone), any({MASKED_IP}), max(toFloat(coalesce(properties.$screen_width, 0))), \
         formatDateTime(toTimeZone(min(timestamp), 'Asia/Singapore'), '%H:%i'), formatDateTime(toTimeZone(max(timestamp), 'Asia/Singapore'), '%H:%i'), \
         argMinIf(properties.$pathname, timestamp, event = '$pageview'), \
         arrayStringConcat(arraySlice(groupUniqArrayIf(properties.$pathname, event = '$pageview'), 1, 5), ' '), \
         countIf(event = '$pageview'), countIf(event not in ('$pageview', '$pageleave')), countIf({WEB_START}), \
         countIf(event in ('auth_otp_verified', 'auth_oauth_completed')), countIf(event = 'message_sent'), \
         substring(coalesce(any(properties.$raw_user_agent), ''), 1, 180) \
         from events where properties.mars_env = 'production' and not {excluded} \
         and timestamp >= {end} - interval 1 hour and timestamp < {end} \
         group by person_id having countIf(event = '$pageview') > 0 order by min(timestamp) limit 120"
    )
}

fn visitors(ph: &PostHog, source: &str, excluded: &str) -> Result<Vec<HourVisitor>, String> {
    Ok(ph.query(&visitors_sql(END, source, excluded))?.iter().map(|r| HourVisitor {
        person: text(r, 0), source: text(r, 1), os: text(r, 2), browser: text(r, 3), timezone: text(r, 4), ip: text(r, 5),
        screen_width: num(r, 6), first_seen: text(r, 7), last_seen: text(r, 8), landing: text(r, 9), pages: text(r, 10),
        pageviews: num(r, 11), other: num(r, 12), cta: num(r, 13), signed_in: num(r, 14), messages: num(r, 15), user_agent: text(r, 16),
    }).collect())
}

/// Never fails: each half keeps its own error so the rest of the run stands.
pub fn collect(ph: &PostHog, source: &str, excluded: &str) -> Hour {
    let (visitors, visitors_error) = match visitors(ph, source, excluded) { Ok(v) => (v, None), Err(e) => (Vec::new(), Some(e)) };
    let (db, db_error) = match watch_hour_db::collect(chrono::Utc::now()) { Ok(d) => (Some(d), None), Err(e) => (None, Some(e)) };
    Hour { visitors, visitors_error, db, db_error }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_the_real_visitors_of_the_current_whole_hour() {
        let sql = visitors_sql("E", "SRC", "EXCL");
        assert!(sql.contains("timestamp >= E - interval 1 hour and timestamp < E") && sql.contains("not EXCL"));
        assert!(sql.contains("argMinIf(SRC, timestamp, event = '$pageview')") && sql.contains("having countIf(event = '$pageview') > 0"));
        assert!(sql.contains("countIf(event = 'cta_clicked' and") && sql.contains("'auth_oauth_completed'") && sql.contains("'message_sent'"));
    }

    #[test]
    fn the_ip_is_masked_and_the_user_agent_cut() {
        let sql = visitors_sql("E", "SRC", "EXCL");
        assert!(sql.contains("splitByChar('.', coalesce(properties.$ip, '')), 1, 3), '.'), '.*')") && sql.contains("':*'"));
        assert!(!sql.contains("any(properties.$ip)"), "the full IP never reaches analysis.json");
        assert!(sql.contains("substring(coalesce(any(properties.$raw_user_agent), ''), 1, 180)"));
    }
}
