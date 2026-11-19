//! Production traffic by source from PostHog for the SEO watchdog: real visitors per Singapore day
//! (our own machines and known machine traffic excluded), visitors per source over the last days, and a
//! spike finding that opens a source-attribution tab.

use super::watch_analyze::Finding;
use super::watch_fingerprints::{excluded_filter, machine_filter, own_filter};
use super::watch_segments::{self, BatchRow, SegmentRow};
use crate::skills::conversion_analytics::posthog::{PostHog, num, text};
use serde::{Deserialize, Serialize};

const TZ: &str = "'Asia/Singapore'";

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct TrafficConfig {
    pub host: String,
    pub project: u64,
    pub keychain_service: String,
    /// Public IPs of our own machines; this host's current public IP is added at run time.
    pub own_ips: Vec<String>,
    /// `$ip` prefixes of known machine traffic (crawlers, scripted checks), left out of real visitors
    /// and counted on their own; each row is attributed in `references/traffic-sources.md`.
    pub machine_ip_prefixes: Vec<MachinePrefix>,
    pub days: u32,
    /// Days of per-source rows on the page and in the spike finding.
    pub source_days: u32,
    /// A day is a spike when real visitors reach `spike_ratio` × the median of the earlier days and at least `spike_min`.
    pub spike_ratio: f64,
    pub spike_min: f64,
    /// Persons matched by a device fingerprint, read once per run (`watch_fingerprints::fingerprinted_sql`).
    #[serde(skip)]
    pub fingerprinted: Vec<String>,
}

impl Default for TrafficConfig {
    fn default() -> Self {
        Self { host: "https://us.posthog.com".into(), project: 704719, keychain_service: "mars-crm-posthog-personal-api-key".into(),
            own_ips: vec!["61.13.236.78".into()], machine_ip_prefixes: default_machine_prefixes(),
            days: 14, source_days: 3, spike_ratio: 3.0, spike_min: 100.0, fingerprinted: Vec::new() }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MachinePrefix {
    /// Leading part of `$ip`, ending in a dot (`38.34.15.`).
    pub prefix: String,
    /// Dated attribution, shown on the page.
    pub reason: String,
}

fn default_machine_prefixes() -> Vec<MachinePrefix> {
    [("38.34.15.", "2026-11-05 Cogent Los Angeles, Linux Chrome 141, Asia/Shanghai: fresh persons on /intro clicking the header CTA, 0 sign-ups"),
     ("192.178.15.", "2026-11-03 Google Ads landing-page crawler (Google-AdWords-Express), $pageview only"),
     ("168.100.149.", "2026-11-06 Ahrefs Site Audit weekly crawl (AS140577, Fridays 14:00 SGT), Lighthouse moto g power UA, $pageview only, 0 sign-ups")]
        .into_iter().map(|(prefix, reason)| MachinePrefix { prefix: prefix.into(), reason: reason.into() }).collect()
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct DayRow {
    pub day: String,
    pub visitors: f64,
    /// Visitors from our own IPs, left out of `visitors`.
    pub own: f64,
    /// Visitors from `machine_ip_prefixes` (and not our own), left out of `visitors`.
    pub machine: f64,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct SourceRow {
    pub day: String,
    /// `utm_source`, else the referring domain (`$direct` for none); an X short link
    /// (`https://t.co/<code>`) stays whole, since each post carries its own code.
    pub source: String,
    pub visitors: f64,
    /// Singapore time of the source's first page view that day.
    pub first_seen: String,
    /// Landing path of that first page view.
    pub landing: String,
    /// Visitors who sent nothing but page views: no page leave, click or web vital. About a third of
    /// real X visitors do (in-app browsers close without a beacon); a scripted burst is nearly all.
    pub pageview_only: f64,
}

impl SourceRow {
    /// At least 10 visitors and 80 % of them page-view-only (2026-11-04: 28 of 30 `www.google.com`
    /// visitors were one Linux Chrome profile on rotating US proxy IPs within 20 seconds).
    pub fn suspected_bot(&self) -> bool {
        self.visitors >= 10.0 && self.pageview_only >= 0.8 * self.visitors
    }
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Traffic {
    pub days: Vec<DayRow>,
    pub sources: Vec<SourceRow>,
    /// Last hour and last day by source and visitor segment (`watch_segments`).
    pub segments: Vec<SegmentRow>,
    /// Why `segments` (or `batches`) is empty when its query failed; the day and source rows still stand.
    pub segments_error: Option<String>,
    /// Same-device groups among the last day's real visitors (`watch_segments::batches`).
    pub batches: Vec<BatchRow>,
    pub payments: Vec<super::watch_payments::PaymentRow>,
    /// Why `payments` is empty when its query failed.
    pub payments_error: Option<String>,
    /// The run hour per visitor and from the database (`watch_hour`), read by the hourly retro.
    pub hour: super::watch_hour::Hour,
    pub own_ips: Vec<String>,
    pub machine_ip_prefixes: Vec<MachinePrefix>,
}

/// Raw User-Agent `like` patterns of machine traffic that changes IP on every hit, with the name shown on
/// the page; each is attributed in `references/traffic-sources.md`.
pub const MACHINE_AGENTS: &[(&str, &str)] = &[
    ("%Nexus 5 Build/MRA58N%MicroMessenger/7.0.1%", "微信链接预览爬虫"),
    ("%Chrome/75.0.3770.156%aweme_%", "抖音服务端链接检测"),
    ("%Google-AdWords-Express%", "Google Ads 落地页爬虫"),
    ("%Google-Ads-Qualify%", "Google Ads 落地页审核"),
    ("%Sogou web spider%", "搜狗爬虫"),
];

pub fn daily_sql(config: &TrafficConfig, ips: &[String]) -> String {
    let (own, machine) = (own_filter(ips), machine_filter(&config.machine_ip_prefixes, &config.fingerprinted));
    format!("select toDate(toTimeZone(timestamp, {TZ})) d, uniqIf(person_id, not {own} and not {machine}) visitors, uniqIf(person_id, {own}) own, \
     uniqIf(person_id, {machine} and not {own}) machine \
     from events where event = '$pageview' and properties.mars_env = 'production' and timestamp > now() - interval {} day group by d order by d", config.days)
}

/// One row per day and source, each visitor counted once a day under the source of their first page
/// view, with how many of them sent nothing but page views (`pageview_only`).
pub fn sources_sql(config: &TrafficConfig, ips: &[String]) -> String {
    let source = super::watch_source::expr();
    format!("select d, src, uniq(p) visitors, formatDateTime(min(f), '%m-%d %H:%i') first_seen, argMin(landing, f) landing, uniqIf(p, other = 0) pageview_only from \
     (select toDate(toTimeZone(timestamp, {TZ})) d, person_id p, argMinIf({source}, timestamp, event = '$pageview') src, \
     minIf(toTimeZone(timestamp, {TZ}), event = '$pageview') f, argMinIf(properties.$session_entry_pathname, timestamp, event = '$pageview') landing, \
     countIf(event != '$pageview') other \
     from events where properties.mars_env = 'production' and not {} and timestamp > now() - interval {} day group by d, p having countIf(event = '$pageview') > 0) \
     group by d, src order by d desc, visitors desc limit 200", excluded_filter(ips, &config.machine_ip_prefixes, &config.fingerprinted), config.source_days)
}

/// This host's public IP, when the lookup answers.
fn public_ip() -> Option<String> {
    let response = crate::http::send(reqwest::Method::GET, "https://api.ipify.org", &[], None, Some(std::time::Duration::from_secs(10))).ok()?;
    let ip = String::from_utf8_lossy(&response.body).trim().to_string();
    (response.ok() && ip.parse::<std::net::IpAddr>().is_ok()).then_some(ip)
}

pub fn collect(config: &TrafficConfig) -> Result<Traffic, String> {
    let ph = PostHog::from_keychain(&config.host, config.project, &config.keychain_service)?.with_budget_wait(super::watch_hour::POSTHOG_BUDGET_WAIT).with_label("seo-watch");
    let config = &TrafficConfig { fingerprinted: super::watch_fingerprints::fingerprinted(&ph)?, ..config.clone() };
    let mut ips = config.own_ips.clone();
    if let Some(ip) = public_ip().filter(|ip| !ips.contains(ip)) { ips.push(ip); }
    let days = ph.query(&daily_sql(config, &ips))?.iter()
        .map(|r| DayRow { day: text(r, 0), visitors: num(r, 1), own: num(r, 2), machine: num(r, 3) }).collect();
    let sources = ph.query(&sources_sql(config, &ips))?.iter()
        .map(|r| SourceRow { day: text(r, 0), source: text(r, 1), visitors: num(r, 2), first_seen: text(r, 3), landing: text(r, 4), pageview_only: num(r, 5) }).collect();
    let source = super::watch_source::expr();
    let excluded = excluded_filter(&ips, &config.machine_ip_prefixes, &config.fingerprinted);
    let (segments, mut segments_error) = match watch_segments::collect(&ph, &source, &excluded) {
        Ok(rows) => (rows, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    let batches = watch_segments::batches(&ph, &excluded).unwrap_or_else(|e| { segments_error.get_or_insert(format!("batches: {e}")); Vec::new() });
    let (payments, payments_error) = match super::watch_payments::collect(&ph) { Ok(rows) => (rows, None), Err(e) => (Vec::new(), Some(e)) };
    let hour = super::watch_hour::collect(&ph, &source, &excluded);
    Ok(Traffic { days, sources, segments, segments_error, batches, payments, payments_error, hour, own_ips: ips, machine_ip_prefixes: config.machine_ip_prefixes.clone() })
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n % 2 == 1 { values[n / 2] } else { (values[n / 2 - 1] + values[n / 2]) / 2.0 }
}

/// One P1 `traffic:spike:<day>` per day in the source window whose real visitors reach the spike
/// threshold against the median of the days before it (a day with no earlier day is never a spike).
pub fn spike_findings(traffic: &Traffic, config: &TrafficConfig) -> Vec<Finding> {
    let recent: Vec<&String> = traffic.sources.iter().map(|s| &s.day).collect();
    traffic.days.iter().enumerate().filter(|(_, d)| recent.contains(&&d.day)).filter_map(|(i, day)| {
        let mut before: Vec<f64> = traffic.days[..i].iter().map(|d| d.visitors).collect();
        if before.is_empty() { return None; }
        let base = median(&mut before);
        if day.visitors < config.spike_min || day.visitors < config.spike_ratio * base.max(1.0) { return None; }
        let top = traffic.sources.iter().filter(|s| s.day == day.day).max_by(|a, b| a.visitors.total_cmp(&b.visitors))?;
        let earliest = traffic.sources.iter().filter(|s| s.source == top.source).min_by(|a, b| a.first_seen.cmp(&b.first_seen)).unwrap_or(top);
        Some(Finding {
            key: format!("traffic:spike:{}", day.day),
            severity: "P1".into(),
            title: format!("{} 真实访客 {}，是之前 {} 天中位数 {} 的 {:.1} 倍；最大来源 {}（{} 人，{} 人只看一页{}；这个来源在近 {} 天最早出现于 {}，落地页 {}）",
                day.day, day.visitors, before.len(), base, day.visitors / base.max(1.0), top.source, top.visitors, top.pageview_only,
                if top.suspected_bot() { "，疑似机器流量" } else { "" }, config.source_days, earliest.first_seen, earliest.landing),
            action: "流量溯源：t.co / x.com 来源到 X 上找首次访问时间前后链接 example.com 的帖子（先查 @example_handle 的提及、引用和转发），其他来源打开 referrer 页面；写明发帖人、帖子链接、发帖时间和带来的访客数".into(),
        })
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(day: &str, visitors: f64) -> DayRow { DayRow { day: day.into(), visitors, ..Default::default() } }
    fn source(day: &str, source: &str, visitors: f64) -> SourceRow {
        SourceRow { day: day.into(), source: source.into(), visitors, first_seen: format!("{} 00:01", &day[5..]), landing: "/intro".into(), pageview_only: 0.0 }
    }

    #[test]
    fn a_source_of_page_view_only_visitors_is_a_suspected_bot() {
        let bot = SourceRow { pageview_only: 28.0, ..source("2026-11-04", "www.google.com", 30.0) };
        let x = SourceRow { pageview_only: 303.0, ..source("2026-11-03", "t.co", 766.0) };
        let small = SourceRow { pageview_only: 4.0, ..source("2026-11-03", "www.google.com", 6.0) };
        assert!(bot.suspected_bot() && !x.suspected_bot() && !small.suspected_bot());
    }

    #[test]
    fn each_visitor_counts_once_a_day_under_their_first_source() {
        let sql = sources_sql(&TrafficConfig::default(), &[]);
        assert!(sql.contains("group by d, p having countIf(event = '$pageview') > 0"));
        assert!(sql.contains("argMinIf(coalesce(") && sql.contains("uniqIf(p, other = 0) pageview_only"));
    }

    #[test]
    fn a_day_far_above_the_earlier_median_is_a_spike_named_by_its_top_source() {
        let traffic = Traffic { days: vec![day("2026-11-02", 132.0), day("2026-11-03", 1193.0), day("2026-11-04", 91.0)],
            sources: vec![source("2026-11-02", "t.co", 116.0), source("2026-11-03", "t.co", 937.0), source("2026-11-03", "$direct", 211.0), source("2026-11-04", "t.co", 47.0)], own_ips: vec![], ..Default::default() };
        let findings = spike_findings(&traffic, &TrafficConfig::default());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].key, "traffic:spike:2026-11-03");
        assert!(findings[0].title.contains("最大来源 t.co（937 人"));
        assert!(findings[0].title.contains("最早出现于 11-02 00:01"));
    }

    #[test]
    fn the_first_day_and_small_days_are_never_spikes() {
        let traffic = Traffic { days: vec![day("2026-11-02", 500.0), day("2026-11-03", 60.0)],
            sources: vec![source("2026-11-02", "t.co", 400.0), source("2026-11-03", "t.co", 50.0)], own_ips: vec![], ..Default::default() };
        assert!(spike_findings(&traffic, &TrafficConfig::default()).is_empty());
    }

    #[test]
    fn an_x_short_link_names_its_post_and_a_bare_t_co_falls_back_to_the_domain() {
        let sql = sources_sql(&TrafficConfig::default(), &[]);
        assert!(sql.contains("properties.$session_entry_referrer != 'https://t.co/', properties.$session_entry_referrer"));
    }

    #[test]
    fn a_source_is_the_session_entry_and_our_own_hosts_are_internal() {
        let sql = sources_sql(&TrafficConfig::default(), &[]);
        assert!(sql.contains("nullIf(properties.$session_entry_utm_source, '')"));
        assert!(sql.contains("'accounts.google.com'") && sql.contains("'(internal)'"));
        assert!(!sql.contains("properties.$referring_domain"));
    }

    #[test]
    fn own_ips_and_private_ranges_are_excluded_from_real_visitors() {
        let sql = daily_sql(&TrafficConfig::default(), &["61.13.236.78".into()]);
        assert!(sql.contains("properties.$ip in ('61.13.236.78')"));
        assert!(sql.contains("uniqIf(person_id, not (properties.$ip in"));
    }

    #[test]
    fn known_machine_prefixes_are_left_out_of_real_visitors_and_counted_apart() {
        let config = TrafficConfig::default();
        let daily = daily_sql(&config, &["61.13.236.78".into()]);
        assert!(daily.contains("startsWith(properties.$ip, '38.34.15.')") && daily.contains("startsWith(properties.$ip, '192.178.15.')"));
        assert!(daily.contains("and not (startsWith(properties.$ip, '38.34.15.')") && daily.contains(" machine "));
        assert!(daily.contains("startsWith(properties.$ip, '168.100.149.')"));
        let sources = sources_sql(&config, &[]);
        assert!(sources.contains("and not ((properties.$ip in") && sources.contains("startsWith(properties.$ip, '38.34.15.')"));
        assert!(sources.contains("startsWith(properties.$ip, '168.100.149.')"));
        let segments = watch_segments::segments_sql(24, 0, watch_segments::END, "SRC", &excluded_filter(&[], &config.machine_ip_prefixes, &[]));
        assert!(segments.contains("not ((properties.$ip in") && segments.contains("startsWith(properties.$ip, '192.178.15.')"));
    }

    #[test]
    fn an_empty_ip_list_still_excludes_the_known_machine_user_agents() {
        let config = TrafficConfig { machine_ip_prefixes: vec![], ..Default::default() };
        let daily = daily_sql(&config, &[]);
        assert!(!daily.contains("startsWith(properties.$ip"));
        assert!(daily.contains("not (ifNull(properties.$raw_user_agent, '') like '%Nexus 5 Build/MRA58N%MicroMessenger/7.0.1%' or ifNull(properties.$raw_user_agent, '') like '%Chrome/75.0.3770.156%aweme_%' or \
            ifNull(properties.$raw_user_agent, '') like '%Google-AdWords-Express%' or ifNull(properties.$raw_user_agent, '') like '%Google-Ads-Qualify%' or \
            ifNull(properties.$raw_user_agent, '') like '%Sogou web spider%' or false)"));
    }
}
