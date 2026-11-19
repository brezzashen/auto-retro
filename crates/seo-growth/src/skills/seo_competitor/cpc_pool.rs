//! The Google Ads keyword candidate pool (founder 2026-11-18): every word we bid on, every real search term the ads
//! matched and every candidate from the seed list or keyword ideas, with its planner volume and bids per market and
//! its measured clicks and cost. `<state>/cpc-pool/pool.json` persists it; `cpc_pool_judge` flags low-cost,
//! high-return words, `cpc_pool_expand` grows it once a day and `cpc_pool_view` writes the review and the proposals.

use super::files::{items, num_or, read_json, stringify, text, write_text};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PoolConfig {
    /// Curated seed words, `references/cpc-pool.seed.json` (`[{keyword, theme?, source?}]`).
    pub seed: String,
    /// Planner markets as DataForSEO location codes (Singapore 2702, Malaysia 2458, Philippines 2608, Hong Kong 2344, Taiwan 2158).
    pub markets: Vec<u32>,
    /// Ordered `[theme, regex]`: the first pattern a word matches names its theme; no match leaves it `other`.
    pub themes: Vec<(String, String)>,
    /// Words that never become paid candidates (relationship and dating intents stay organic, founder 2026-11-18).
    pub blocked: String,
    /// Theme → where a proposed word goes: `{en: [campaign, ad group, final URL], zh: [...]}`.
    pub targets: BTreeMap<String, BTreeMap<String, (String, String, String)>>,
    pub thresholds: super::cpc_pool_judge::Thresholds,
    /// Seed words sent to Google Ads keyword ideas per day; each costs one call per market.
    #[serde(default = "default_expand")]
    pub expand_seeds_per_day: usize,
}

fn default_expand() -> usize { 3 }

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub impressions: f64,
    pub clicks: f64,
    pub cost: f64,
    pub conversions: f64,
}

impl Stats {
    pub fn add(&self, other: &Stats) -> Stats {
        Stats { impressions: self.impressions + other.impressions, clicks: self.clicks + other.clicks, cost: self.cost + other.cost, conversions: self.conversions + other.conversions }
    }
    pub fn cpc(&self) -> Option<f64> { (self.clicks > 0.0).then(|| self.cost / self.clicks) }
    pub fn ctr(&self) -> Option<f64> { (self.impressions > 0.0).then(|| self.clicks / self.impressions) }
}

/// Google Ads planner numbers of one market: 12-month mean volume and top-of-page bid range in US$.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Market {
    pub volume: f64,
    pub bid_low: f64,
    pub bid_high: f64,
    pub competition: String,
    pub fetched: String,
}

/// Where a word runs in the account: campaign, ad group, match type, max CPC and its own Ads status.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Slot {
    pub campaign: String,
    pub ad_group: String,
    pub match_type: String,
    pub bid: String,
    pub status: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub theme: String,
    /// `seed`, `keyword-ideas:<seed>`, `search-term`, `live`.
    pub source: String,
    /// `live` (a keyword in the account), `search-term` (matched a live keyword, not one itself), `candidate`, `blocked`.
    pub status: String,
    pub first_seen: String,
    #[serde(default)]
    pub first_live: Option<String>,
    #[serde(default)]
    pub slots: Vec<Slot>,
    #[serde(default)]
    pub markets: BTreeMap<String, Market>,
    /// Completed days (Ads "Last 7 days") plus today, summed over the word's slots or search-term rows.
    #[serde(default)]
    pub window: Stats,
    /// Singapore date → that day's numbers as last read (the final hourly read of a day stands for the day).
    #[serde(default)]
    pub days: BTreeMap<String, Stats>,
    #[serde(default)]
    pub flags: Vec<String>,
    /// Flag → Singapore date it first appeared, so the hourly review reports only new flags.
    #[serde(default)]
    pub flagged_since: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Pool {
    #[serde(default)]
    pub words: BTreeMap<String, Entry>,
    #[serde(default)]
    pub last_expand_day: Option<String>,
    #[serde(default)]
    pub expanded_from: Vec<String>,
    /// RFC 3339 time of the last run that read both search-term tables (`watch_ads` reads them first when it is old).
    #[serde(default)]
    pub last_terms_read: Option<String>,
}

pub fn path(state_dir: &str) -> String { format!("{state_dir}/cpc-pool/pool.json") }

pub fn read(state_dir: &str) -> Pool {
    read_json(&path(state_dir)).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

pub fn write(state_dir: &str, pool: &Pool) -> Result<(), String> {
    std::fs::create_dir_all(format!("{state_dir}/cpc-pool")).map_err(|e| e.to_string())?;
    write_text(&path(state_dir), &stringify(pool)?)
}

/// `[camcard]` / `"camcard"` / `camcard` → `camcard`.
pub fn normalize(keyword: &str) -> String {
    keyword.trim().trim_matches(|c| c == '[' || c == ']' || c == '"' || c == '+').trim().to_lowercase()
}

pub fn theme(config: &PoolConfig, word: &str) -> String {
    config.themes.iter().find(|(_, pattern)| regex::Regex::new(pattern).is_ok_and(|r| r.is_match(word)))
        .map(|(name, _)| name.clone()).unwrap_or_else(|| "other".into())
}

pub fn blocked(config: &PoolConfig, word: &str) -> bool {
    regex::Regex::new(&config.blocked).is_ok_and(|r| r.is_match(word))
}

/// `$1,234.50` / `1.00` / `—` → number.
pub fn money(value: &Value) -> f64 {
    text(value).replace(['$', ',', '%'], "").trim().parse().unwrap_or_else(|_| num_or(value, 0.0))
}

pub fn row_stats(row: &Value) -> Stats {
    Stats { impressions: money(&row["stats.impressions"]), clicks: money(&row["stats.clicks"]), cost: money(&row["stats.cost"]), conversions: money(&row["stats.conversions"]) }
}

fn entry<'a>(pool: &'a mut Pool, config: &PoolConfig, word: &str, source: &str, today: &str) -> &'a mut Entry {
    pool.words.entry(word.to_string()).or_insert_with(|| Entry {
        theme: theme(config, word), source: source.into(), status: "candidate".into(), first_seen: today.into(), ..Default::default()
    })
}

/// Seed words enter as candidates; a word already known keeps its state. Any untested candidate the current
/// `blocked` pattern matches (a seed or an earlier keyword idea) is blocked, so a widened pattern also clears old ideas.
pub fn merge_seed(pool: &mut Pool, config: &PoolConfig, seed: &Value, today: &str) {
    for item in items(seed) {
        let word = normalize(text(&item["keyword"]));
        if word.is_empty() { continue; }
        let source = if text(&item["source"]).trim().is_empty() { "seed" } else { text(&item["source"]) };
        let e = entry(pool, config, &word, source, today);
        if let Some(t) = item["theme"].as_str() { e.theme = t.into(); }
    }
    for (word, e) in pool.words.iter_mut() {
        if e.status == "candidate" && blocked(config, word) { e.status = "blocked".into(); }
    }
}

/// Text of a table control or total row (`expand_more`, `Total: ...`) rather than a search term.
fn ui_label(word: &str) -> bool {
    word.starts_with("expand_") || word.starts_with("total") || word.starts_with("other search terms")
}

/// Both the Today and the Last 7 days table of `kind` (`keywords`, `search_terms`) were read this run.
pub fn read_both(tabs: &Value, kind: &str) -> bool {
    tabs[format!("{kind}_7d")].is_array() && tabs[format!("{kind}_today")].is_array()
}

/// Live keywords and real search terms from the watchdog's `googleAds.tabs`; the window is Last 7 days plus today.
pub fn merge_ads(pool: &mut Pool, config: &PoolConfig, tabs: &Value, today: &str) -> Option<String> {
    let (mut keywords, terms) = (read_both(tabs, "keywords"), read_both(tabs, "search_terms"));
    let note = keywords.then(|| filtered_read(pool, tabs)).flatten();
    if note.is_some() { keywords = false; }
    for e in pool.words.values_mut() {
        if (keywords && e.status == "live") || (terms && e.status == "search-term") { e.slots.clear(); e.window = Stats::default(); }
    }
    let mut today_stats: BTreeMap<String, Stats> = BTreeMap::new();
    for (tab, is_today) in [("keywords_7d", false), ("keywords_today", true)].into_iter().filter(|_| keywords) {
        for row in items(&tabs[tab]) {
            let word = normalize(text(&row["keyword_text"]));
            if word.is_empty() { continue; }
            let stats = row_stats(row);
            let e = entry(pool, config, &word, "live", today);
            e.status = "live".into();
            e.first_live.get_or_insert_with(|| today.to_string());
            e.window = e.window.add(&stats);
            let slot = Slot { campaign: text(&row["campaign_name"]).into(), ad_group: text(&row["ad_group_name"]).into(),
                match_type: text(&row["match_type"]).into(), bid: text(&row["bid_config.effective_cpc_bid"]).into(), status: text(&row["primary_status"]).into() };
            if !e.slots.contains(&slot) { e.slots.push(slot); }
            if is_today { let t = today_stats.entry(word).or_default(); *t = t.add(&stats); }
        }
    }
    for (tab, is_today) in [("search_terms_7d", false), ("search_terms_today", true)].into_iter().filter(|_| terms) {
        for row in items(&tabs[tab]) {
            let word = normalize(text(&row["query"]));
            if word.is_empty() || ui_label(&word) || pool.words.get(&word).is_some_and(|e| e.status == "live") { continue; }
            let stats = row_stats(row);
            let block = blocked(config, &word);
            let e = entry(pool, config, &word, "search-term", today);
            e.status = if block { "blocked".into() } else { "search-term".into() };
            e.window = e.window.add(&stats);
            let slot = Slot { campaign: text(&row["campaign_name"]).into(), ad_group: text(&row["ad_group_name"]).into(), match_type: text(&row["query_match_type"]).into(), ..Default::default() };
            if !e.slots.contains(&slot) { e.slots.push(slot); }
            if is_today { let t = today_stats.entry(word).or_default(); *t = t.add(&stats); }
        }
    }
    for (word, stats) in today_stats { if let Some(e) = pool.words.get_mut(&word) { e.days.insert(today.into(), stats); } }
    for e in pool.words.values_mut().filter(|_| keywords) { if e.status == "live" && e.slots.is_empty() { e.status = "removed".into(); } }
    note
}

/// A keyword table that lacks over a quarter of the live words is a filtered view (2026-11-19 11:17: a filter left in
/// the shared Ads UI cut 95 rows to 16 and marked card scanner and camcard removed); real removals come a few at a time.
fn filtered_read(pool: &Pool, tabs: &Value) -> Option<String> {
    let live = pool.words.values().filter(|e| e.status == "live").count();
    let read: std::collections::BTreeSet<String> = ["keywords_7d", "keywords_today"].iter()
        .flat_map(|tab| items(&tabs[*tab])).map(|row| normalize(text(&row["keyword_text"]))).filter(|w| !w.is_empty()).collect();
    (live >= 8 && (read.len() as f64) < 0.75 * live as f64)
        .then(|| format!("cpc pool: the keyword table listed {} words but {live} are live, so it was read filtered; keyword numbers kept", read.len()))
}

#[cfg(test)]
#[path = "cpc_pool_tests.rs"]
mod tests;
