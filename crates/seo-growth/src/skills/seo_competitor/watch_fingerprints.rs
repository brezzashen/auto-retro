//! Machine traffic matched by device fingerprint instead of IP or User-Agent: crawlers that rotate
//! proxy IPs across several ranges within one burst and send an ordinary desktop Chrome User-Agent.
//! Each fingerprint is attributed in `.claude/skills/seo-competitor/references/traffic-sources.md`.
//! Also the filters every traffic query applies to tell real visitors from our own and machine traffic.

use super::watch_traffic::{MachinePrefix, MACHINE_AGENTS};
use crate::skills::conversion_analytics::posthog::{text, PostHog};

/// HogQL condition on one event, and the name shown on the watchdog page.
pub const MACHINE_FINGERPRINTS: &[(&str, &str)] = &[
    // 2026-11-03..11 bursts from 5.183.*, 195.64.*, 194.26.*, 113.30.*, 172.58.*, 40.223.* and more:
    // several fresh persons within seconds, a Google referrer and one page each.
    ("properties.$os = 'Linux' and properties.$timezone = 'America/New_York' \
      and properties.$session_entry_referring_domain = 'www.google.com' and properties.$screen_width = 1920",
     "Google 来路代理爬虫（Linux·纽约时区·1920 px）"),
    // Headless checks on Google Cloud, AWS, LogicWeb, EGIHosting and DigitalOcean (2026-11-03..10).
    ("properties.$os = 'Linux' and properties.$timezone = 'UTC'", "Linux UTC 无头检查"),
];

/// Cloud ranges (whois 2026-11-15: Tencent Cloud, Huawei Cloud `HWCSNET`, ACE/Tencent Singapore) of a crawler that
/// fetches one public page about every 50 minutes as a new person, rotating range, OS and time zone (12 persons on
/// 2026-11-15, every one a single `$pageview` with no referrer). The Google Cloud blocks (whois 2026-11-16: `GOOGL-46`,
/// Google LLC `34.64.0.0/10`, `GOOGLE-CLOUD`) hold a page-check batch of 9 fresh persons in 3 minutes on 2026-11-16 15:03,
/// one `$pageview` each on `/intro` or a Wiki guide. Carrier ranges never belong here.
pub const IDC_RANGES: &[&str] = &[
    "49.232.0.0/14", "82.156.0.0/15", "116.204.0.0/18", "119.28.0.0/15", "101.32.104.0/21", "129.226.96.0/19", "129.226.128.0/20", "43.134.0.0/16",
    "136.112.0.0/12", "34.64.0.0/10", "2600:1900::/28",
];
/// Name of the cloud single-hit rule on the watchdog page.
pub const IDC_SINGLE_HIT: &str = "云机房单页抓取（只有一次 $pageview、无来路）";

/// Days of a fingerprinted person's history read; longer than every traffic window of the watchdog.
const LOOKBACK_DAYS: u32 = 35;
/// PostHog's row limit per query; a list that fills it would be incomplete.
const MAX_PERSONS: usize = 50_000;

/// The persons who hit a fingerprint and sent nothing but page views and page leaves in the lookback, so a
/// matching visitor who clicks, signs up or chats stays a real visitor; plus the persons whose whole lookback is
/// one direct `$pageview` from an `IDC_RANGES` address, so a second page, a page leave or any event keeps them.
pub fn fingerprinted_sql() -> String {
    let any = MACHINE_FINGERPRINTS.iter().map(|(condition, _)| format!("({condition})")).collect::<Vec<_>>().join(" or ");
    let idc = IDC_RANGES.iter().map(|cidr| format!("isIPAddressInRange(ifNull(properties.$ip, ''), '{cidr}')")).collect::<Vec<_>>().join(" or ");
    format!("select toString(person_id) p from events where properties.mars_env = 'production' \
     and timestamp > now() - interval {LOOKBACK_DAYS} day group by person_id \
     having (countIf({any}) > 0 and countIf(event not in ('$pageview', '$pageleave')) = 0) \
     or (count() = 1 and countIf(event = '$pageview' and ifNull(properties.$session_entry_referring_domain, '$direct') = '$direct' \
     and ifNull(properties.$session_entry_utm_source, '') = '' and ({idc})) = 1) limit {MAX_PERSONS}")
}

/// Reads the fingerprinted persons once per run. The list used to be a subquery inside every traffic query, and
/// each copy rescanned 35 days of event properties (720 MB of the project's hourly PostHog read budget, 2026-11-14).
pub fn fingerprinted(ph: &PostHog) -> Result<Vec<String>, String> {
    let persons: Vec<String> = ph.query(&fingerprinted_sql())?.iter().map(|r| text(r, 0)).filter(|p| !p.is_empty()).collect();
    if persons.len() >= MAX_PERSONS {
        return Err(format!("fingerprinted persons filled a PostHog page ({MAX_PERSONS} rows)"));
    }
    Ok(persons)
}

/// Matches the events of the persons `fingerprinted` listed.
pub fn fingerprint_filter(persons: &[String]) -> String {
    if persons.is_empty() {
        return "false".into();
    }
    let list = persons.iter().map(|p| format!("'{}'", p.replace(['\'', '\\'], ""))).collect::<Vec<_>>().join(", ");
    format!("toString(person_id) in ({list})")
}

/// Our own machines and LAN addresses.
pub fn own_filter(ips: &[String]) -> String {
    let list = ips.iter().map(|ip| format!("'{}'", ip.replace('\'', ""))).collect::<Vec<_>>().join(", ");
    format!("(properties.$ip in ({list}) or properties.$ip like '192.168.%' or properties.$ip like '10.%')")
}

/// Known machine traffic. Events without a User-Agent (server events) read as not matching, never as null.
pub fn machine_filter(prefixes: &[MachinePrefix], fingerprinted: &[String]) -> String {
    let ips = prefixes.iter().map(|m| format!("startsWith(properties.$ip, '{}')", m.prefix.replace('\'', "")));
    let agents = MACHINE_AGENTS.iter().map(|(pattern, _)| format!("ifNull(properties.$raw_user_agent, '') like '{pattern}'"));
    let fingerprints = std::iter::once(fingerprint_filter(fingerprinted));
    format!("({})", ips.chain(agents).chain(fingerprints).collect::<Vec<_>>().join(" or "))
}

/// Everything that is not a real visitor: our own machines, LAN addresses and known machine traffic.
pub fn excluded_filter(ips: &[String], prefixes: &[MachinePrefix], fingerprinted: &[String]) -> String {
    format!("({} or {})", own_filter(ips), machine_filter(prefixes, fingerprinted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_needs_a_fingerprint_hit_and_page_views_only() {
        let sql = fingerprinted_sql();
        assert!(sql.starts_with("select toString(person_id) p from events"));
        assert!(sql.contains("properties.$timezone = 'America/New_York'") && sql.contains("properties.$timezone = 'UTC'"));
        assert!(sql.contains("countIf(event not in ('$pageview', '$pageleave')) = 0"));
        assert!(sql.contains("interval 35 day"));
    }

    #[test]
    fn a_cloud_range_person_counts_only_with_one_direct_page_view_and_nothing_else() {
        let sql = fingerprinted_sql();
        let idc = &sql[sql.find("or (count() = 1").expect("cloud single-hit clause")..];
        assert!(idc.contains("event = '$pageview'") && idc.contains("= '$direct'") && idc.contains("$session_entry_utm_source, '') = ''"), "{idc}");
        for cidr in IDC_RANGES {
            assert!(idc.contains(&format!("isIPAddressInRange(ifNull(properties.$ip, ''), '{cidr}')")), "{cidr}");
        }
        assert!(!IDC_RANGES.iter().any(|cidr| cidr.starts_with("218.60.")), "UNICOM-LN is a carrier, not a cloud range");
    }

    #[test]
    fn traffic_filters_carry_the_list_instead_of_a_subquery() {
        let persons = vec!["p-1".to_string(), "p'2".to_string()];
        assert_eq!(fingerprint_filter(&persons), "toString(person_id) in ('p-1', 'p2')");
        assert_eq!(fingerprint_filter(&[]), "false");
        let excluded = excluded_filter(&["61.13.236.78".into()], &[], &persons);
        assert!(excluded.contains("toString(person_id) in ('p-1', 'p2')") && !excluded.contains("select"), "{excluded}");
    }
}
