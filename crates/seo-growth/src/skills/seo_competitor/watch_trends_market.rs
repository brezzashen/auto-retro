//! Extra Google Trends markets beside the default one (`trends.locationCode`, the US). Each market
//! pulls its own focus topics in its own language; its risers carry the market code so volume and
//! weekly curves are read in that market, and their sources read `topic:<code>:<keyword>`.

use super::monid::slug;
use super::watch_trends::{Topic, TrendsConfig};
use serde::Deserialize;

/// The market code of the default location.
pub const DEFAULT: &str = "US";

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Market {
    /// Upper-case ISO country code, e.g. `JP`, or country plus language when one country has several
    /// (`SG-EN`, `SG-TW`); the country part is the Trends `geo`.
    pub code: String,
    /// Chinese display name, e.g. `日本`.
    pub label: String,
    pub location_code: u32,
    /// Google Ads `language_code`, e.g. `ja`.
    pub language_code: String,
    pub topics: Vec<Topic>,
    /// Risers kept per topic of this market; absent uses `trends.maxPerSource`.
    #[serde(default)]
    pub max_per_source: Option<usize>,
}

/// One topic pull: `(market code, empty for the default market)`, location and topic.
pub struct TopicPull<'a> {
    pub market: String,
    pub location_code: u32,
    pub topic: &'a Topic,
}

/// Every topic of the default market, then every topic of each extra market.
pub fn topic_pulls(cfg: &TrendsConfig) -> Vec<TopicPull<'_>> {
    let default = cfg.topics.iter().map(|t| TopicPull { market: String::new(), location_code: cfg.location_code, topic: t });
    let extra = cfg.markets.iter().flat_map(|m| m.topics.iter().map(|t| TopicPull { market: m.code.clone(), location_code: m.location_code, topic: t }));
    default.chain(extra).collect()
}

/// `topic:<keyword>` for the default market, `topic:<code>:<keyword>` (code lower-case) for another.
pub fn topic_source(market: &str, keyword: &str) -> String {
    if market.is_empty() { format!("topic:{keyword}") } else { format!("topic:{}:{keyword}", market.to_lowercase()) }
}

/// The market code a source names (`topic:jp:恋愛` -> `JP`, `topic:sg-en:dating` -> `SG-EN`), empty for
/// the default market.
pub fn market_of(source: &str) -> String {
    let Some(rest) = source.strip_prefix("topic:") else { return String::new() };
    let letters = |p: &str| p.len() == 2 && p.chars().all(|c| c.is_ascii_lowercase());
    match rest.split_once(':') {
        Some((code, _)) if code.split('-').count() <= 2 && code.split('-').all(letters) => code.to_uppercase(),
        _ => String::new(),
    }
}

/// `(location_code, language_code)` of a market code, the default location for an empty or unknown one.
pub fn location(cfg: &TrendsConfig, market: &str) -> (u32, String) {
    cfg.markets.iter().find(|m| m.code == market).map_or((cfg.location_code, "en".into()), |m| (m.location_code, m.language_code.clone()))
}

/// Risers kept per source: the market's own cap for a market topic, else `trends.maxPerSource`.
pub fn cap(cfg: &TrendsConfig, source: &str) -> usize {
    let market = market_of(source);
    cfg.markets.iter().find(|m| m.code == market).and_then(|m| m.max_per_source).unwrap_or(cfg.max_per_source)
}

/// The Trends `geo` of a riser's market: its country part.
pub fn geo(market: &str) -> &str { if market.is_empty() { DEFAULT } else { market.split('-').next().unwrap_or(market) } }

/// A file name stem for any keyword: the ASCII slug, with an FNV-1a hash appended when the keyword
/// holds other letters (Japanese, Korean or Thai slug to nothing and would share one file).
pub fn file_key(text: &str) -> String {
    let s = slug(text);
    if text.is_ascii() && !s.is_empty() { return s; }
    let hash = text.bytes().fold(0x811c_9dc5_u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
    if s.is_empty() { format!("{hash:08x}") } else { format!("{s}-{hash:08x}") }
}

/// The file stem of one topic pull, market-prefixed outside the default market.
pub fn topic_file(market: &str, keyword: &str) -> String {
    if market.is_empty() { format!("topic_{}", file_key(keyword)) } else { format!("topic_{}_{}", market.to_lowercase(), file_key(keyword)) }
}

/// The volume cache key of a Google Ads keyword in a market.
pub fn volume_key(market: &str, keyword: &str) -> String {
    if market.is_empty() { keyword.to_string() } else { format!("{}:{keyword}", market.to_lowercase()) }
}

#[cfg(test)]
#[path = "watch_trends_market_tests.rs"]
mod tests;
