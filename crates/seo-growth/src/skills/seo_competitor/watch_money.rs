//! Money signals of the keyword room (G13; report `gefei-keyword-method-20261110` section 8, P3 and P11).
//! Gefei orders words close to money by CPC inside a difficulty and volume filter (WeChat 2023-10-05,
//! 2023-10-15) and reads who already pays for a word: advertisers, a price, an app (2026-07-02). Every
//! signal only raises a word in the order; a low CPC never demotes one (lmlpg7ae96).

use super::files::{at, items, serp_domain, text};
use super::watch_discover::Candidate;
use super::watch_signals::{demand, priced};
use serde_json::Value;

/// Gefei's beginner filter: "我们设置KD小于29，然后按照CPC倒序排列" with searches of 600 or more
/// (2023-10-05) and "cpc大于0.1" (2023-10-15).
const NEAR_KD: f64 = 29.0;
const NEAR_VOLUME: f64 = 600.0;
const NEAR_CPC: f64 = 0.1;
/// CPC (USD) at which a near-money word earns the whole bonus, the priced-click level.
const FULL_CPC: f64 = 3.0;
const CPC_BONUS: f64 = 0.5;
/// Google Ads competition index from which its level reads HIGH (251 Labs words, 2026-11-10: the lowest
/// HIGH was 0.67, the highest MEDIUM 0.66).
const ADS_HIGH: f64 = 0.67;
/// Ordering bonus of each paid-demand signal.
const SIGNAL_BONUS: f64 = 0.1;
const STORES: [&str; 2] = ["apps.apple.com", "play.google.com"];

/// Inside Gefei's beginner filter, where words are ordered by CPC.
pub fn near_money(c: &Candidate) -> bool {
    c.kd.is_some_and(|kd| kd < NEAR_KD) && demand(c) >= NEAR_VOLUME && c.cpc >= NEAR_CPC
}

/// Many advertisers bid on the word in Google Ads.
pub fn advertised(c: &Candidate) -> bool { c.competition >= ADS_HIGH }

/// Paid-demand signals that fired: advertisers, a priced first-page result, an app-store listing.
pub fn paid_signals(c: &Candidate) -> usize {
    [advertised(c), c.price_pages > 0, c.app_listings > 0].iter().filter(|s| **s).count()
}

/// ×1.5 for a priced click; inside the beginner filter the bonus grows with CPC up to the same ×1.5.
fn cpc_bonus(c: &Candidate) -> f64 {
    if priced(c) { 1.0 + CPC_BONUS } else if near_money(c) { 1.0 + CPC_BONUS * (c.cpc / FULL_CPC).min(1.0) } else { 1.0 }
}

/// Ordering bonus of the money signals: the CPC bonus times ×1.1 per paid-demand signal (at most ×1.95).
pub fn weight(c: &Candidate) -> f64 { cpc_bonus(c) * (1.0 + SIGNAL_BONUS * paid_signals(c) as f64) }

fn pricing_link(title: &str) -> bool {
    let t = title.to_lowercase();
    ["pricing", "plans", "subscription"].iter().any(|w| t.contains(w))
}

/// `(results showing a price or a pricing sitelink, app-store listings)` among first-page organic results.
/// The SERP carries no in-app purchase amounts, and DataForSEO returned no `paid` item in 231 cached
/// first pages (2026-11-04 to 2026-11-10), so ads are read from Google Ads competition instead.
pub fn serp_signals(organic: &[&Value]) -> (usize, usize) {
    let shows_price = |r: &&&Value| !at(r, &["price"]).is_null() || items(at(r, &["links"])).iter().any(|l| pricing_link(text(at(l, &["title"]))));
    let store = |r: &&&Value| STORES.contains(&serp_domain(r).as_str());
    (organic.iter().filter(shows_price).count(), organic.iter().filter(store).count())
}

/// The money signals that fired, for the verdict reason.
pub fn notes(c: &Candidate) -> Vec<String> {
    let mut out = Vec::new();
    if near_money(c) && !priced(c) { out.push(format!("离钱近：KD < 29、月搜 ≥ 600、CPC ${:.2}", c.cpc)); }
    if advertised(c) { out.push(format!("Google Ads 竞争度高（{:.2}），有人持续买这个词", c.competition)); }
    out
}

/// What the first page says about payment, for the devil's advocate note; empty when nothing fired.
pub fn serp_note(c: &Candidate) -> String {
    let mut out = Vec::new();
    if c.price_pages > 0 { out.push(format!("首页 {} 个结果带标价或价格页", c.price_pages)); }
    if c.app_listings > 0 { out.push(format!("首页有 {} 个应用商店 App（内购金额未读）", c.app_listings)); }
    if out.is_empty() { String::new() } else { format!("；付费信号：{}", out.join("、")) }
}

#[cfg(test)]
#[path = "watch_money_tests.rs"]
mod tests;
