//! Volume surge and weekly acceleration of Trends risers (founder 2026-11-07: compare a word's
//! latest volume with its 12-month level to find fast risers). Monthly volume comes from Google Ads
//! (one DataForSEO call for every kept riser, cached 7 days per word) and lags a month, so the top
//! unjudged risers also get a 12-month weekly Trends curve, one keyword per call (a shared call
//! normalises small words to 0-1), cached per day. The numbers rank words for the hourly retro;
//! a judgment still decides, nothing is dropped by them.

use super::files::{at, items, read_json, stringify, text, write_text};
use super::monid::{MonidCall, run_all};
use super::watch_trends::{Pull, Rise, TrendsConfig, needs_judgment};
use super::watch_trends_market::{file_key, location, volume_key};
use super::watch_trends_ledger::Label;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const NEW: &str = "新出现";
pub const RISING: &str = "快速上升";
pub const SEASONAL: &str = "季节回潮";
/// The month still says rising but the last two weeks are under half the weekly baseline.
pub const FADED: &str = "已回落";
/// Weekly interest (0-100) the last two weeks need over a near-zero baseline to count as new.
const WEEKLY_NEW: f64 = 10.0;
const VOLUME_DAYS: i64 = 7;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SurgeConfig {
    /// Latest over baseline at or above this is `快速上升`.
    pub min_ratio: f64,
    /// Latest monthly volume any flag needs.
    pub min_volume: f64,
    /// Mean monthly volume of the 11 months before, under which a word is `新出现`.
    pub new_baseline: f64,
    /// Unjudged risers per pull that get a weekly curve (one paid Trends call each).
    pub weekly_top: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Monthly {
    /// `YYYY-MM` of the latest published month.
    pub month: String,
    pub latest: f64,
    /// Mean of the up to 11 months before it.
    pub base: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Weekly {
    /// Mean of the last 2 complete weeks (0-100).
    pub recent: f64,
    /// Mean of the complete weeks before the last 4.
    pub base: f64,
    /// Mean of the first 2 weeks of the window, about a year before `recent`.
    pub year_ago: f64,
    /// The partial current week (0 when absent): a news word that exists only this week shows here alone.
    pub current: f64,
}

fn mean(v: &[f64]) -> f64 { if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 } }

/// One Google Ads `monthly_searches` list (any order) as latest month against the 11 before it.
pub fn monthly(item: &Value) -> Option<Monthly> {
    let mut months: Vec<(i64, i64, f64)> = items(at(item, &["monthly_searches"])).iter()
        .filter_map(|m| Some((m["year"].as_i64()?, m["month"].as_i64()?, m["search_volume"].as_f64().unwrap_or(0.0)))).collect();
    months.sort_by_key(|m| (m.0, m.1));
    let (last, before) = months.split_last()?;
    if before.is_empty() { return None; }
    let prev: Vec<f64> = before[before.len().saturating_sub(11)..].iter().map(|m| m.2).collect();
    Some(Monthly { month: format!("{}-{:02}", last.0, last.1), latest: last.2, base: mean(&prev) })
}

/// A `past_12_months` Trends graph of one keyword; the partial current week (`missing_data`) is kept
/// apart as `current`, gaps count as 0.
pub fn weekly(json: &Value) -> Option<Weekly> {
    let first = super::monid::first(json);
    let data: Vec<&Value> = items(at(first, &["items"])).iter().filter(|i| text(at(i, &["type"])) == "google_trends_graph")
        .flat_map(|i| items(at(i, &["data"])).iter()).collect();
    let value = |p: &&Value| p["values"][0].as_f64().unwrap_or(0.0);
    let partial = |p: &&&Value| p["missing_data"].as_bool().unwrap_or(false);
    let points: Vec<f64> = data.iter().filter(|p| !partial(p)).map(value).collect();
    let current = data.iter().filter(partial).map(value).next_back().unwrap_or(0.0);
    let n = points.len();
    if n < 12 { return None; }
    Some(Weekly { recent: mean(&points[n - 2..]), base: mean(&points[..n - 4]), year_ago: mean(&points[..2]), current })
}

pub fn ratio(latest: f64, base: f64) -> f64 { latest / base.max(1.0) }

/// The flag of one word: the weekly curve, when there is one, overrides the lagging month.
pub fn flag(m: Option<&Monthly>, w: Option<&Weekly>, cfg: &SurgeConfig) -> &'static str {
    if m.is_some_and(|m| m.latest < cfg.min_volume) { return ""; }
    let by_month = m.map_or("", |m| if m.base < cfg.new_baseline { NEW } else if ratio(m.latest, m.base) >= cfg.min_ratio { RISING } else { "" });
    let Some(w) = w else { return by_month };
    let new = w.base < 1.0 && (w.recent >= WEEKLY_NEW || (w.recent < 1.0 && w.current >= WEEKLY_NEW));
    let rising = new || ratio(w.recent, w.base) >= cfg.min_ratio;
    match () {
        _ if rising && w.recent > 0.0 && w.year_ago >= 0.5 * w.recent => SEASONAL,
        _ if new || (rising && by_month == NEW) => NEW,
        _ if rising => RISING,
        _ if !by_month.is_empty() && ratio(w.recent, w.base) < 0.5 => FADED,
        _ => by_month,
    }
}

/// Sort rank of a riser (founder 2026-11-07, report seo-trends-surge-20261107 item 1 B): flagged words
/// with a Google Ads month (`新出现`, then `快速上升`) first, flagged words without one (news that
/// exists only this week) after them, then the rest.
pub fn rise_rank(r: &Rise) -> u8 {
    match (r.flag.as_str(), r.volume.is_some()) { (NEW, true) => 0, (RISING, true) => 1, (RISING, false) => 2, (NEW, false) => 3, _ => 4 }
}

/// The query as Google Ads accepts it (letters, digits, Thai vowel and tone marks, spaces and `'&+.-`,
/// at most 10 words and 80 characters); a comma anywhere fails the whole batch.
pub fn ads_keyword(query: &str) -> Option<String> {
    let thai = |c: char| ('\u{0E00}'..='\u{0E7F}').contains(&c);
    let kept: String = query.to_lowercase().chars().map(|c| if c.is_alphanumeric() || thai(c) || " '&+.-".contains(c) { c } else { ' ' }).collect();
    let words: Vec<&str> = kept.split_whitespace().collect();
    let out = words.join(" ");
    (!out.is_empty() && words.len() <= 10 && out.chars().count() <= 80).then_some(out)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Cached { fetched: String, monthly: Option<Monthly> }

/// Google Ads monthly volume of every riser in its own market: cached words younger than 7 days, the
/// rest in one call per market written to `dir`. Returns the volumes by riser query.
fn volumes(rises: &[Rise], trends: &TrendsConfig, dir: &str, state_dir: &str, today: chrono::NaiveDate, pull: &mut Pull) -> BTreeMap<String, Monthly> {
    let cache_path = format!("{state_dir}/trends/volume.json");
    let mut cache: BTreeMap<String, Cached> = read_json(&cache_path).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let fresh = |c: &Cached| chrono::NaiveDate::parse_from_str(&c.fetched, "%Y-%m-%d").is_ok_and(|d| (today - d).num_days() < VOLUME_DAYS);
    let mut asks: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut asked = false;
    for r in rises {
        let Some(k) = ads_keyword(&r.query) else { continue };
        if !cache.get(&volume_key(&r.market, &k)).is_some_and(fresh) { asks.entry(r.market.clone()).or_default().push(k); }
    }
    for (market, mut ask) in asks {
        asked = true;
        ask.sort();
        ask.dedup();
        let (location_code, language_code) = location(trends, &market);
        let out = if market.is_empty() { format!("{dir}/volume.json") } else { format!("{dir}/volume_{}.json", market.to_lowercase()) };
        let call = MonidCall { provider: "dataforseo".into(), endpoint: "/keywords/google-ads-search-volume".into(),
            body: json!({"keywords": ask, "location_code": location_code, "language_code": language_code}), out: out.clone() };
        *pull.calls.entry("volume".into()).or_default() += 1;
        match run_all(vec![call], 1).and_then(|_| read_json(&out)) {
            Ok(Value::Array(list)) => {
                let got: BTreeMap<String, Option<Monthly>> = list.iter().map(|i| (text(&i["keyword"]).to_lowercase(), monthly(i))).collect();
                for k in ask { cache.insert(volume_key(&market, &k), Cached { fetched: today.to_string(), monthly: got.get(&k).cloned().flatten() }); }
            }
            Ok(other) => pull.errors.push(format!("trends volume {market}: {}", text(&other["tasks"][0]["status_message"]))),
            Err(e) => pull.errors.push(format!("trends volume {market}: {e}")),
        }
    }
    if asked && let Err(e) = stringify(&cache).and_then(|t| write_text(&cache_path, &t)) { pull.errors.push(format!("trends volume: {e}")); }
    rises.iter().filter_map(|r| Some((r.query.clone(), cache.get(&volume_key(&r.market, &ads_keyword(&r.query)?))?.monthly.clone()?))).collect()
}

/// Adds volume, surge, weekly acceleration and a flag to every riser of `pull`. Weekly curves go to
/// the `weekly_top` risers still waiting for a judgment (flagged first), cached in `<state>/trends/weekly/<day>`.
pub fn enrich(pull: &mut Pull, cfg: &SurgeConfig, trends: &TrendsConfig, dir: &str, state_dir: &str, today: chrono::NaiveDate, judged: &BTreeMap<String, Label>) {
    let rises = pull.rising.clone();
    let months = volumes(&rises, trends, dir, state_dir, today, pull);
    for r in pull.rising.iter_mut() {
        let Some(m) = months.get(&r.query) else { continue };
        let prior_flag = r.flag.clone();
        (r.volume, r.base, r.surge) = (Some(m.latest), Some(m.base), Some((ratio(m.latest, m.base) * 10.0).round() / 10.0));
        let by_ads = flag(Some(m), None, cfg);
        r.flag = if by_ads.is_empty() && r.source.contains("similarweb:") && m.latest >= cfg.min_volume && !prior_flag.is_empty() {
            prior_flag
        } else {
            by_ads.into()
        };
        pull.month = pull.month.clone().max(m.month.clone());
    }
    let waiting = |r: &&mut Rise| needs_judgment(r) && judged.get(&r.query).is_none_or(|l| l.label == "观察") && ads_keyword(&r.query).is_some()
        && r.volume.is_none_or(|v| v >= cfg.min_volume);
    let mut picked: Vec<&mut Rise> = pull.rising.iter_mut().filter(|r| waiting(r)).collect();
    picked.sort_by(|a, b| rise_rank(a).cmp(&rise_rank(b)).then(b.value.total_cmp(&a.value)));
    picked.truncate(cfg.weekly_top);
    let week_dir = format!("{state_dir}/trends/weekly/{today}");
    let path = |r: &Rise| if r.market.is_empty() { format!("{week_dir}/{}.json", file_key(&r.query)) } else { format!("{week_dir}/{}_{}.json", r.market.to_lowercase(), file_key(&r.query)) };
    let calls: Vec<MonidCall> = picked.iter().filter(|r| !std::path::Path::new(&path(r)).exists()).map(|r| MonidCall {
        provider: "dataforseo".into(), endpoint: "/keywords/google-trends-explore".into(), out: path(r),
        body: json!({"keywords": [ads_keyword(&r.query)], "location_code": location(trends, &r.market).0, "time_range": "past_12_months", "item_types": ["google_trends_graph"]}) }).collect();
    let paid = calls.len();
    if let Err(e) = run_all(calls, 4) { pull.errors.push(format!("trends weekly: {e}")); }
    for r in picked {
        let Some(w) = read_json(&path(r)).ok().as_ref().and_then(weekly) else { continue };
        let prior_flag = r.flag.clone();
        r.accel = Some((ratio(w.recent, w.base) * 10.0).round() / 10.0);
        let m = months.get(&r.query);
        let by_week = flag(m, Some(&w), cfg);
        r.flag = if by_week.is_empty() && r.source.contains("similarweb:") && !prior_flag.is_empty() {
            prior_flag
        } else {
            by_week.into()
        };
    }
    if paid > 0 { *pull.calls.entry("weekly".into()).or_default() += paid; }
}

#[cfg(test)]
#[path = "watch_trends_surge_tests.rs"]
mod tests;
