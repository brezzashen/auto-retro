//! Flags of the keyword pool: low-cost, high-return words first (founder 2026-11-18), then waste and dead words.
//! Every rule compares a word with its own campaign over the same window (Ads Last 7 days plus today), and needs a
//! minimum sample, because at $35/day most words see a handful of clicks.

use super::cpc_pool::{Entry, Pool, Stats, row_stats};
use super::files::{items, text};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

pub const GOLDEN: &str = "低投入高收益";
pub const CHEAP_CONVERSION: &str = "低成本转化";
pub const TERM_GOLDEN: &str = "搜索词高效：建议加为精确词";
pub const TERM_CONVERTS: &str = "搜索词带来转化：建议加为精确词";
pub const PLANNER_CHEAP: &str = "便宜有量：优先测试";
pub const WASTE: &str = "花费无转化";
pub const TERM_NEGATIVE: &str = "建议加否定词";
pub const DEAD: &str = "长期零展示：建议退役";
pub const LOW_CTR: &str = "点击率低：检查广告相关性";

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Thresholds {
    /// A live word needs this many clicks before its CPC and CTR count.
    pub min_clicks: f64,
    /// Golden when its CPC is at most this share of its campaign's CPC ...
    pub cheap_cpc_ratio: f64,
    /// ... and its CTR at least this multiple of the campaign's CTR (or it converted).
    pub ctr_ratio: f64,
    /// Cheap conversion: cost per conversion at most this share of the account's, once the account has 2 conversions.
    pub cheap_conversion_ratio: f64,
    /// A search term needs this many clicks before it is proposed as a word.
    pub term_min_clicks: f64,
    /// US$ spent in the window with no conversion that flags waste.
    pub waste_cost: f64,
    /// Days live with no impression before a word is proposed for retirement.
    pub dead_days: i64,
    /// Impressions before CTR is judged, and the CTR below which it is low.
    pub low_ctr_impressions: f64,
    pub low_ctr: f64,
    /// Untested candidate: planner volume over all markets at least this ...
    pub planner_min_volume: f64,
    /// ... with the highest top-of-page bid in any market at most this many US$.
    pub planner_cheap_bid: f64,
}

/// Campaign name → its window numbers, from the Ads campaign tables (Last 7 days plus today).
pub fn campaigns(tabs: &Value) -> BTreeMap<String, Stats> {
    let mut out: BTreeMap<String, Stats> = BTreeMap::new();
    for tab in ["campaigns_7d", "campaigns_today"] {
        for row in items(&tabs[tab]) {
            let name = text(&row["name"]).to_string();
            if name.is_empty() { continue; }
            let sum = out.entry(name).or_default();
            *sum = sum.add(&row_stats(row));
        }
    }
    out
}

fn account(campaigns: &BTreeMap<String, Stats>) -> Stats {
    campaigns.values().fold(Stats::default(), |sum, s| sum.add(s))
}

/// The flags one word earns now.
pub fn flags(e: &Entry, campaign: &Stats, account: &Stats, t: &Thresholds, today: chrono::NaiveDate) -> Vec<&'static str> {
    let w = &e.window;
    let mut out = Vec::new();
    let cheaper = |ratio: f64| match (w.cpc(), campaign.cpc()) { (Some(mine), Some(theirs)) => mine <= ratio * theirs, _ => false };
    let sharper = |ratio: f64| match (w.ctr(), campaign.ctr()) { (Some(mine), Some(theirs)) => mine >= ratio * theirs, _ => false };
    match e.status.as_str() {
        "live" => {
            if w.clicks >= t.min_clicks && cheaper(t.cheap_cpc_ratio) && (sharper(t.ctr_ratio) || w.conversions > 0.0) { out.push(GOLDEN); }
            if w.conversions > 0.0 {
                let cheap = account.conversions < 2.0 || (w.cost / w.conversions) <= t.cheap_conversion_ratio * (account.cost / account.conversions);
                if cheap { out.push(CHEAP_CONVERSION); }
            }
            if w.cost >= t.waste_cost && w.conversions == 0.0 { out.push(WASTE); }
            let age = e.first_live.as_deref().and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).map(|d| (today - d).num_days());
            if age.is_some_and(|days| days >= t.dead_days) && w.impressions == 0.0 { out.push(DEAD); }
            if w.impressions >= t.low_ctr_impressions && w.ctr().is_some_and(|ctr| ctr < t.low_ctr) { out.push(LOW_CTR); }
        }
        "search-term" => {
            if w.conversions > 0.0 { out.push(TERM_CONVERTS); }
            else if w.clicks >= t.term_min_clicks && cheaper(1.0) && sharper(1.0) { out.push(TERM_GOLDEN); }
            if w.cost >= t.waste_cost && w.conversions == 0.0 { out.push(TERM_NEGATIVE); }
        }
        "blocked" => if w.clicks > 0.0 { out.push(TERM_NEGATIVE); },
        "candidate" => {
            let volume: f64 = e.markets.values().map(|m| m.volume).sum();
            let bid = e.markets.values().map(|m| m.bid_high).fold(0.0, f64::max);
            if volume >= t.planner_min_volume && bid <= t.planner_cheap_bid { out.push(PLANNER_CHEAP); }
        }
        _ => {}
    }
    out
}

/// Re-flags every word and keeps the date each flag first appeared.
pub fn judge(pool: &mut Pool, tabs: &Value, t: &Thresholds, today: chrono::NaiveDate) {
    let campaigns = campaigns(tabs);
    let account = account(&campaigns);
    let day = today.format("%Y-%m-%d").to_string();
    for e in pool.words.values_mut() {
        let campaign = e.slots.first().and_then(|s| campaigns.get(&s.campaign)).cloned().unwrap_or_default();
        let now: Vec<String> = flags(e, &campaign, &account, t, today).into_iter().map(String::from).collect();
        e.flagged_since.retain(|flag, _| now.contains(flag));
        for flag in &now { e.flagged_since.entry(flag.clone()).or_insert_with(|| day.clone()); }
        e.flags = now;
    }
}
