//! The hourly SEO watchdog's configuration (`references/watch.config.json`) and its persisted state
//! (`<state>/state.json`).

use super::config::Gsc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TargetKeyword {
    pub keyword: String,
    /// Unprefixed path of the landing page meant to rank for the keyword.
    pub page: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Dispatch {
    /// Minimum hours between two optimization tabs.
    pub optimize_cooldown_hours: f64,
    /// Minimum hours between two tabs for the same site-health failure.
    pub health_cooldown_hours: f64,
    /// Singapore hour (0-23) of the daily review tab.
    pub retro_hour_sgt: u32,
    /// Report-only rule for over-edited keyword pages; absent turns it off.
    #[serde(default)]
    pub copy_cap: Option<super::watch_copy_cap::CopyCap>,
    /// Report-only rule for words whose page lacks only referring domains; absent turns it off.
    #[serde(default)]
    pub link_gap: Option<super::watch_link_gap::LinkGap>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ReportTarget {
    pub slug: String,
    pub title: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WatchConfig {
    /// Production origin, e.g. `https://example.com`.
    pub site: String,
    /// Canary origin serving the next release; its sitemap lists production URLs.
    pub canary: Option<String>,
    pub gsc: Gsc,
    pub targets: Vec<TargetKeyword>,
    /// URLs passed to the URL Inspection API per run (daily quota 2,000 per property).
    pub inspect_per_run: usize,
    pub dispatch: Dispatch,
    pub report: ReportTarget,
    /// PostHog production traffic by source; defaults to project 704719.
    #[serde(default)]
    pub traffic: super::watch_traffic::TrafficConfig,
    /// Daily keyword discovery; absent turns it off.
    #[serde(default)]
    pub discovery: Option<DiscoveryConfig>,
    /// Google Trends watch of the focus areas and their unusual risers; absent turns it off.
    #[serde(default)]
    pub trends: Option<super::watch_trends::TrendsConfig>,
    /// Daily SERP rank and convergence toward position 1 of every target; absent turns it off.
    #[serde(default)]
    pub rank: Option<super::watch_rank_serp::RankConfig>,
    /// Bing Webmaster API and Bing first-page ranks each run; defaults apply when absent.
    #[serde(default)]
    pub bing: super::watch_bing::BingConfig,
    /// Weekly Baidu ranks of mainland China words; no `words` turns it off.
    #[serde(default)]
    pub baidu: super::watch_baidu::BaiduConfig,
    /// Load-balancer logs that judge slow-only probes by origin latency; absent keeps probe timing.
    #[serde(default)]
    pub origin: Option<super::watch_origin::OriginConfig>,
    /// Machine-generated query phrasings left out of the Page-1 CTR check; defaults apply when absent.
    #[serde(default)]
    pub machine_queries: super::watch_machine_queries::MachineQueryConfig,
    /// Hourly AI recommendation watch (GPT, Claude, Gemini through AIsa); absent turns it off.
    #[serde(default)]
    pub geo: Option<super::watch_geo::GeoConfig>,
    /// AIsa search signals (AI Overview and AI Mode citations, keyword intent and difficulty, new/lost links); absent turns it off.
    #[serde(default)]
    pub aisa_seo: Option<super::watch_aisa::AisaSeoConfig>,
    /// Sponsored creator posts read hourly through AIsa (Douyin, WeChat Channels, Xiaohongshu); absent turns it off.
    #[serde(default)]
    pub creator_posts: Option<super::watch_creator::CreatorConfig>,
    /// Google Ads numbers of account 594-270-3856 read hourly from the Ads script's spreadsheet; absent turns it off.
    #[serde(default)]
    pub google_ads: Option<super::watch_ads::AdsConfig>,
}

/// One landing page the scout expands: its main keyword and the word stems a candidate must share
/// with it to count as on-topic for that page.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Seed {
    pub keyword: String,
    pub page: String,
    pub fit: Vec<String>,
    /// Also ask Google Trends for this seed's rising queries.
    #[serde(default)]
    pub trends: bool,
    /// Steady-demand topic: a keyword of this page meeting the rule is `做` without a rising trend.
    #[serde(default)]
    pub evergreen: Option<Evergreen>,
    /// Core-market page (counselling, win-back, spotting a high-value man): a keyword that fits it is
    /// never dropped for navigational intent, because a `near me` or city search is that market.
    #[serde(default)]
    pub core_market: bool,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Evergreen {
    pub min_volume: f64,
    pub max_kd: f64,
}

/// The daily keyword room (founder 2026-11-04, report `seo-keyword-room-plan-20261104`).
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryConfig {
    /// Singapore hour of the daily round.
    pub hour_sgt: u32,
    pub location_code: u32,
    pub language_code: String,
    /// Our own domain, for the backlink rank the win rate compares against.
    pub domain: String,
    /// Competitor registry with `groups[].competitors[].{domain, brand}`.
    pub registry: String,
    pub seeds: Vec<Seed>,
    /// Brand names beyond the registry (tools, platforms) that make a keyword navigational for us.
    pub extra_brands: Vec<String>,
    /// Words that make a keyword unsafe for the brand.
    pub blocked: Vec<String>,
    pub min_volume: f64,
    /// A keyword at most this hard can be done now; harder up to `drop_kd` is watched.
    pub max_kd: f64,
    pub drop_kd: f64,
    pub min_win_rate: f64,
    /// SERPs fetched per round for the best pre-judged keywords.
    pub serp_per_day: usize,
    pub max_per_day: usize,
    pub max_new_pages_per_day: usize,
    /// Monthly searches from which a `做` word whose need no page covers gets its own page (G8).
    #[serde(default = "own_page_volume")]
    pub own_page_volume: f64,
    /// Index and click gate on `max_new_pages_per_day` (G12).
    #[serde(default)]
    pub new_page_gate: super::watch_page_gate::GateConfig,
    /// Gefei's method (report `gefei-keyword-method-20261110`); absent turns its paid probes off.
    #[serde(default)]
    pub method: Option<MethodConfig>,
}

fn own_page_volume() -> f64 { 1000.0 }

/// Paid probes of Gefei's keyword method, each capped per daily round.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MethodConfig {
    /// A stable word with a known Google Ads volume that scales Trends interest into searches (G2).
    pub reference: TrendsReference,
    /// Candidates that get a 5-year and a 30-day Trends curve (birth date, season, scaled volume).
    pub trends_per_day: usize,
    /// SEO sites from the top 3 of `做` words read the next day for their new keywords (G5).
    pub loop_sites: usize,
    /// New words checked for freshly registered domains holding them (G6).
    pub domain_checks_per_day: usize,
    /// Registrations in the last 30 days at which the word's window counts as closed.
    pub max_fresh_domains: usize,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrendsReference {
    pub keyword: String,
    /// Monthly Google Ads searches of `keyword` in the discovery market.
    pub volume: f64,
}

/// The last URL Inspection answer for one URL.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub verdict: String,
    pub coverage: String,
    pub last_crawl: String,
    pub google_canonical: String,
    pub checked_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct WatchState {
    /// Next index into the sitemap URL list for URL Inspection.
    pub inspect_cursor: usize,
    pub inspections: BTreeMap<String, Inspection>,
    /// First run (RFC 3339) that saw each sitemap URL.
    pub first_seen: BTreeMap<String, String>,
    /// First run (RFC 3339) that held a rank finding of a target page no sitemap lists yet (`watch_new_page_grace`).
    #[serde(default)]
    pub unlisted_since: BTreeMap<String, String>,
    /// Finding keys (`kind:subject`) already handed to an optimization tab.
    pub handled: Vec<String>,
    pub last_optimize_at: Option<String>,
    pub last_health_at: Option<String>,
    /// Singapore date (YYYY-MM-DD) of the last daily review tab.
    pub last_retro_day: Option<String>,
    /// Singapore date of the last keyword discovery round.
    #[serde(default)]
    pub last_discover_day: Option<String>,
    /// Singapore hour (YYYY-MM-DDTHH) of the last hourly retro tab.
    #[serde(default)]
    pub last_hourly_retro: Option<String>,
    /// RFC 3339 time of the last Google Trends pull.
    #[serde(default)]
    pub last_trends_at: Option<String>,
    /// Singapore date of the last re-send of unsent reindex requests (`watch_reindex_resend`).
    #[serde(default)]
    pub last_resend_day: Option<String>,
    /// Target word → live product-listing links to its page when `watch_link_gap` last held or handed it over.
    #[serde(default)]
    pub link_gap: BTreeMap<String, i64>,
    pub last_session: Option<String>,
}

pub fn read_config(path: &str) -> Result<WatchConfig, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

pub fn read_state(path: &str) -> WatchState {
    std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}
