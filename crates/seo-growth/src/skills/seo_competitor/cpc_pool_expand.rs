//! Once a day the pool fills in Google Ads planner numbers (volume, top-of-page bid range, competition) per market
//! for words that have none, and grows from Google Ads keyword ideas around its best words: golden live words and
//! search terms first, then untested cheap candidates, then seed words. About 2 × markets DataForSEO calls a day.

use super::cpc_pool::{Market, Pool, PoolConfig, blocked, normalize, theme};
use super::cpc_pool_judge::{GOLDEN, CHEAP_CONVERSION, PLANNER_CHEAP, TERM_CONVERTS, TERM_GOLDEN};
use super::files::{items, num_or, text};
use super::monid::{MonidCall, run_all};
use super::watch_trends_surge::ads_keyword;
use serde_json::{Value, json};

/// New words one day's keyword ideas may add, best volume first.
const MAX_NEW_PER_DAY: usize = 60;
/// Google Ads keyword ideas take at most 20 seed words per request.
const MAX_SEEDS_PER_CALL: usize = 20;
const MAX_VOLUME_WORDS_PER_CALL: usize = 700;

/// A short content hash for cache file names: the same day may ask for different words after a pool reset.
pub(super) fn tag(words: &[String]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    words.hash(&mut h);
    format!("{:08x}", h.finish() as u32)
}

fn call(endpoint: &str, body: Value, out: String) -> MonidCall {
    MonidCall { provider: "dataforseo".into(), endpoint: endpoint.into(), body, out }
}

fn market(item: &Value, day: &str) -> Market {
    Market { volume: num_or(&item["search_volume"], 0.0), bid_low: num_or(&item["low_top_of_page_bid"], 0.0),
        bid_high: num_or(&item["high_top_of_page_bid"], 0.0), competition: text(&item["competition"]).into(), fetched: day.into() }
}

/// Words that still lack planner numbers, in the form Google Ads accepts.
pub fn unmeasured(pool: &Pool) -> Vec<String> {
    pool.words.iter().filter(|(_, e)| e.status != "blocked" && e.markets.is_empty())
        .filter_map(|(word, _)| ads_keyword(word).filter(|k| k == word)).collect()
}

/// Seeds for keyword ideas: flagged words first, then seed-list candidates by volume; never one already used.
pub fn seeds(pool: &Pool, limit: usize) -> Vec<String> {
    let rank = |flags: &[String]| -> u8 {
        if flags.iter().any(|f| f == GOLDEN || f == CHEAP_CONVERSION || f == TERM_CONVERTS) { 0 }
        else if flags.iter().any(|f| f == TERM_GOLDEN) { 1 } else if flags.iter().any(|f| f == PLANNER_CHEAP) { 2 } else { 3 }
    };
    let mut ranked: Vec<(u8, f64, &String)> = pool.words.iter()
        .filter(|(word, e)| e.status != "blocked" && e.theme != "other" && !pool.expanded_from.contains(word) && ads_keyword(word).as_ref() == Some(*word))
        .filter(|(_, e)| rank(&e.flags) < 3 || e.source == "seed")
        .map(|(word, e)| (rank(&e.flags), e.markets.values().map(|m| m.volume).sum::<f64>(), word)).collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
    ranked.into_iter().take(limit.min(MAX_SEEDS_PER_CALL)).map(|(_, _, w)| w.clone()).collect()
}

/// Adds the ideas of one market: unknown business words become candidates, known words get this market's numbers.
pub fn absorb_ideas(pool: &mut Pool, config: &PoolConfig, location: u32, result: &Value, day: &str) -> Vec<(f64, String)> {
    let mut fresh = Vec::new();
    for item in items(result) {
        let word = normalize(text(&item["keyword"]));
        if word.is_empty() { continue; }
        if let Some(e) = pool.words.get_mut(&word) { e.markets.insert(location.to_string(), market(item, day)); continue; }
        if blocked(config, &word) || theme(config, &word) == "other" { continue; }
        fresh.push((num_or(&item["search_volume"], 0.0), word));
    }
    fresh
}

/// The day's planner refresh and keyword ideas; returns a one-line note for the review.
pub fn daily(pool: &mut Pool, config: &PoolConfig, state_dir: &str, day: &str, dry_run: bool, errors: &mut Vec<String>) -> String {
    if dry_run || pool.last_expand_day.as_deref() == Some(day) { return String::new(); }
    let raw = format!("{state_dir}/cpc-pool/raw/{day}");
    let missing = unmeasured(pool);
    let seeds = seeds(pool, config.expand_seeds_per_day);
    let mut calls = Vec::new();
    for &m in &config.markets {
        for chunk in missing.chunks(MAX_VOLUME_WORDS_PER_CALL) {
            calls.push(call("/keywords/google-ads-search-volume", json!({"keywords": chunk, "location_code": m}), format!("{raw}/volume_{m}_{}.json", tag(chunk))));
        }
        if !seeds.is_empty() {
            calls.push(call("/keywords/google-ads-keywords-for-keywords", json!({"keywords": seeds, "location_code": m, "sort_by": "search_volume"}), format!("{raw}/ideas_{m}_{}.json", tag(&seeds))));
        }
    }
    if calls.is_empty() { pool.last_expand_day = Some(day.into()); return "词池今天没有需要补数的词，也没有可扩展的种子词".into(); }
    if let Err(error) = run_all(calls, 4) { errors.push(format!("cpc pool: {error}")); }
    let mut fresh: Vec<(f64, String, u32)> = Vec::new();
    for &m in &config.markets {
        for chunk in missing.chunks(MAX_VOLUME_WORDS_PER_CALL) {
            let Ok(result) = super::files::read_json(&format!("{raw}/volume_{m}_{}.json", tag(chunk))) else { continue };
            for item in items(&result) {
                if let Some(e) = pool.words.get_mut(&normalize(text(&item["keyword"]))) { e.markets.insert(m.to_string(), market(item, day)); }
            }
        }
        if let Ok(result) = super::files::read_json(&format!("{raw}/ideas_{m}_{}.json", tag(&seeds))) {
            fresh.extend(absorb_ideas(pool, config, m, &result, day).into_iter().map(|(v, w)| (v, w, m)));
        }
    }
    let added = add_fresh(pool, config, fresh, &seeds, &raw, day);
    pool.expanded_from.extend(seeds.iter().cloned());
    pool.last_expand_day = Some(day.into());
    format!("词池今天补了 {} 个词的规划师数据，从 {} 个种子词（{}）扩出 {added} 个新候选", missing.len(), seeds.len(), seeds.join("、"))
}

/// Keeps the best `MAX_NEW_PER_DAY` new words and fills every market's numbers from the idea files already on disk.
fn add_fresh(pool: &mut Pool, config: &PoolConfig, mut fresh: Vec<(f64, String, u32)>, seeds: &[String], raw: &str, day: &str) -> usize {
    fresh.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut chosen: Vec<String> = Vec::new();
    for (_, word, _) in &fresh { if !chosen.contains(word) && chosen.len() < MAX_NEW_PER_DAY { chosen.push(word.clone()); } }
    for word in &chosen {
        let e = pool.words.entry(word.clone()).or_default();
        e.theme = theme(config, word);
        e.source = format!("keyword-ideas:{}", seeds.join(", "));
        e.status = "candidate".into();
        e.first_seen = day.into();
    }
    for &m in &config.markets {
        let Ok(result) = super::files::read_json(&format!("{raw}/ideas_{m}_{}.json", tag(seeds))) else { continue };
        for item in items(&result) {
            if let Some(e) = pool.words.get_mut(&normalize(text(&item["keyword"]))) { e.markets.insert(m.to_string(), market(item, day)); }
        }
    }
    chosen.len()
}
