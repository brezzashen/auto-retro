//! Real visitors by session-entry source and visitor segment over fixed whole-hour windows that end at the
//! run's hour, so the hourly retro tells new visitors from returning ones and existing users by what they pay,
//! and compares with the previous hour, the same hour yesterday and the previous day without its own queries.
//! Account segments read the person properties `analytics-sync` sets on every account (`account_created_at`,
//! `customer_type`; `mars-crm-server` `routes/analytics_people.rs`); anonymous visitors are new when their
//! first production page view falls inside the window. Production tracking starts 2026-11-02 22:38 SGT,
//! so a visitor whose earlier visits predate it reads as new.

use crate::skills::conversion_analytics::posthog::{num, text, PostHog};
use serde::Serialize;

/// `(length, offset)` in hours: the window covers `[END - offset - length, END - offset)`. Offset 0 is the
/// current hour and day; 1 the previous hour; 24 the same hour yesterday and the previous day.
pub const WINDOWS: [(u32, u32); 5] = [(1, 0), (24, 0), (1, 1), (1, 24), (24, 24)];

/// The end of every window: the start of the current UTC hour, which is also a Singapore hour boundary.
pub const END: &str = "toStartOfHour(now())";

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct SegmentRow {
    pub window_hours: u32,
    /// Hours between the window's end and `END`; 0 for the current window.
    pub offset_hours: u32,
    pub source: String,
    /// The page label of `SEGMENT`: new visitor, returning visitor, new sign-up, existing user by
    /// customer type, or a signed-in account whose person properties the sync has not set yet.
    pub segment: String,
    pub visitors: f64,
    pub cta: f64,
    pub signups: f64,
    pub senders: f64,
}

/// Real visitors of the last day sharing one OS, browser and time zone (at least three): a scripted
/// burst or crawler shows as one group with near-equal times and mostly page-view-only visitors.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct BatchRow {
    pub os: String,
    pub browser: String,
    pub timezone: String,
    pub visitors: f64,
    /// Visitors who sent nothing but page views.
    pub pageview_only: f64,
    /// Singapore time of the group's first and last page view.
    pub first_seen: String,
    pub last_seen: String,
    /// Up to six distinct `$ip` /16 prefixes (`110.40.*`).
    pub ips: String,
    /// Up to four distinct landing paths.
    pub landings: String,
}

/// The visitor segment as HogQL over a per-visitor subquery (`ws`), the person row (`created`, `ct`, `fp`) and the first
/// page view (`fv`). Person properties come from the `persons` table: this project stamps person properties on each event
/// at ingestion (`person_id_override_properties_on_events`), so `events.person.properties` misses later `$set`s.
/// HogQL types `account_created_at` as `DateTime64(6, 'Asia/Singapore')` (`ca`) and renders it as text with a space
/// (`created`), so the window test compares `ca` as a time; a text compare with a UTC `T` string misread same-day sign-ups.
const SEGMENT: &str = "multiIf(created != '' and ca >= ws, '新注册', \
     created != '', concat('老用户·', multiIf(ct = 'paying', '付费', ct = 'granted', '赠送', ct = 'lapsed', '曾付费', '免费')), \
     fp != '', '已登录·未同步', fv >= ws, '新访客', '回访访客')";

/// A start click that can end in a web sign-up; the hero App Store button and the WeChat download banner leave for the store.
pub const WEB_START: &str = "event = 'cta_clicked' and coalesce(toString(properties.position), '') not in ('hero_app_store', 'wechat_app_banner')";
/// Sign-in finished on the web, by code or by Google or Apple.
const SIGNED_IN: &str = "event in ('auth_otp_verified', 'auth_oauth_completed')";

fn span(hours: u32, offset: u32, end: &str) -> String {
    format!("timestamp >= {end} - interval {} hour and timestamp < {end} - interval {offset} hour", hours + offset)
}

pub fn segments_sql(hours: u32, offset: u32, end: &str, source: &str, own: &str) -> String {
    format!(
        "select src, {SEGMENT} seg, uniq(w.p) visitors, uniqIf(w.p, cta > 0), uniqIf(w.p, otp > 0), uniqIf(w.p, msg > 0) from \
         (select person_id p, {end} - interval {} hour ws, argMinIf({source}, timestamp, event = '$pageview') src, \
          countIf({WEB_START}) cta, countIf({SIGNED_IN}) otp, countIf(event = 'message_sent') msg \
          from events where properties.mars_env = 'production' and not {own} and {} \
          group by p having countIf(event = '$pageview') > 0) w \
         left join (select id, coalesce(toString(properties.account_created_at), '') created, properties.account_created_at ca, \
          coalesce(toString(properties.customer_type), '') ct, \
          coalesce(toString(properties.first_platform), '') fp from persons) pp on pp.id = w.p \
         left join (select person_id p, min(timestamp) fv from events where event = '$pageview' and properties.mars_env = 'production' \
          and timestamp > now() - interval 180 day group by p) f on f.p = w.p \
         group by src, seg order by visitors desc limit 200",
        hours + offset, span(hours, offset, end)
    )
}

pub fn batches_sql(end: &str, own: &str) -> String {
    format!(
        "select os, browser, tz, uniq(p) visitors, uniqIf(p, other = 0) pageview_only, \
         formatDateTime(toTimeZone(min(f), 'Asia/Singapore'), '%m-%d %H:%i'), formatDateTime(toTimeZone(max(f), 'Asia/Singapore'), '%m-%d %H:%i'), \
         arrayStringConcat(arraySlice(groupUniqArray(ipp), 1, 6), ' '), arrayStringConcat(arraySlice(groupUniqArray(landing), 1, 4), ' ') from \
         (select person_id p, any(properties.$os) os, any(properties.$browser) browser, any(properties.$timezone) tz, minIf(timestamp, event = '$pageview') f, \
          any(concat(arrayStringConcat(arraySlice(splitByChar('.', coalesce(properties.$ip, '')), 1, 2), '.'), '.*')) ipp, \
          argMinIf(properties.$pathname, timestamp, event = '$pageview') landing, countIf(event != '$pageview') other \
          from events where properties.mars_env = 'production' and not {own} and {} \
          group by p having countIf(event = '$pageview') > 0) \
         group by os, browser, tz having visitors >= 3 order by visitors desc limit 12",
        span(24, 0, end)
    )
}

pub fn collect(ph: &PostHog, source: &str, own: &str) -> Result<Vec<SegmentRow>, String> {
    let mut rows = Vec::new();
    for (hours, offset) in WINDOWS {
        rows.extend(ph.query(&segments_sql(hours, offset, END, source, own))?.iter().map(|r| SegmentRow {
            window_hours: hours, offset_hours: offset, source: text(r, 0), segment: text(r, 1),
            visitors: num(r, 2), cta: num(r, 3), signups: num(r, 4), senders: num(r, 5),
        }));
    }
    Ok(rows)
}

pub fn batches(ph: &PostHog, own: &str) -> Result<Vec<BatchRow>, String> {
    Ok(ph.query(&batches_sql(END, own))?.iter().map(|r| BatchRow {
        os: text(r, 0), browser: text(r, 1), timezone: text(r, 2), visitors: num(r, 3), pageview_only: num(r, 4),
        first_seen: text(r, 5), last_seen: text(r, 6), ips: text(r, 7), landings: text(r, 8),
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_accounts_by_customer_type_and_anonymous_visitors_by_first_visit() {
        let sql = segments_sql(24, 0, END, "SRC", "OWN");
        assert!(sql.contains("toStartOfHour(now()) - interval 24 hour ws") && sql.contains("not OWN") && sql.contains("argMinIf(SRC"));
        assert!(sql.contains("from persons) pp on pp.id = w.p") && !sql.contains("person.properties") && sql.contains("ct = 'paying', '付费'") && sql.contains("ct = 'lapsed', '曾付费'"));
        assert!(sql.contains("fv >= ws, '新访客', '回访访客'"));
        assert!(sql.find("'新注册'").unwrap() < sql.find("'老用户·'").unwrap(), "an account created in the window is new, not existing");
        assert!(sql.contains("created != '' and ca >= ws, '新注册'") && sql.contains("properties.account_created_at ca"), "creation time compares as a time");
        assert!(!sql.contains("%Y-%m-%dT"), "no text compare against a UTC string");
    }

    #[test]
    fn start_clicks_leave_out_the_app_store_button_and_sign_ins_count_oauth() {
        let sql = segments_sql(24, 0, END, "SRC", "OWN");
        assert!(sql.contains("countIf(event = 'cta_clicked' and coalesce(toString(properties.position), '') not in ('hero_app_store', 'wechat_app_banner')) cta"));
        assert!(sql.contains("countIf(event in ('auth_otp_verified', 'auth_oauth_completed')) otp"));
    }

    #[test]
    fn windows_are_whole_hours_ending_at_the_run_hour_with_offsets() {
        let yesterday = segments_sql(1, 24, "E", "SRC", "OWN");
        assert!(yesterday.contains("timestamp >= E - interval 25 hour and timestamp < E - interval 24 hour"));
        assert!(yesterday.contains("E - interval 25 hour ws"), "new visitors and sign-ups count from the window start");
        assert_eq!(WINDOWS.iter().filter(|(_, offset)| *offset == 0).count(), 2, "the page reads the current hour and day");
    }

    #[test]
    fn batches_group_the_last_day_by_device_and_keep_groups_of_three() {
        let sql = batches_sql("E", "OWN");
        assert!(sql.contains("timestamp >= E - interval 24 hour and timestamp < E - interval 0 hour") && sql.contains("not OWN"));
        assert!(sql.contains("group by os, browser, tz having visitors >= 3") && sql.contains("uniqIf(p, other = 0) pageview_only"));
    }
}
