//! The daily rank sources of every target keyword: one DataForSEO Google organic SERP per word and
//! Singapore day (`<state>/rank/<day>/serp_<key>.json`, reused by the day's later runs), and the
//! Google Ads volume of words the keyword ledger lacks (`<state>/rank/volume.json`, 30 days).

use super::files::{at, items, read_json, serp_domain, stringify, text, write_text};
use super::monid::{MonidCall, first, run_all};
use super::watch_config::TargetKeyword;
use super::watch_trends_market::file_key;
use super::watch_trends_surge::ads_keyword;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

const VOLUME_DAYS: i64 = 30;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RankMarket {
    /// Locale prefix of the target page, e.g. `/ja`.
    pub prefix: String,
    pub location_code: u32,
    /// SERP language (`zh-TW`); `ads_language_code` when Google Ads spells it differently (`zh_TW`).
    pub language_code: String,
    #[serde(default)]
    pub ads_language_code: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RankConfig {
    /// SERP results fetched per word (DataForSEO bills per 10).
    pub serp_depth: u32,
    pub location_code: u32,
    pub language_code: String,
    #[serde(default)]
    pub markets: Vec<RankMarket>,
    /// Most keyword work items handed to one optimization tab.
    pub max_work_per_tab: usize,
}

impl RankConfig {
    /// `(location, SERP language, Ads language)` of a target page by its locale prefix.
    pub fn market(&self, page: &str) -> (u32, String, String) {
        self.markets.iter().find(|m| page == m.prefix || page.starts_with(&format!("{}/", m.prefix)))
            .map(|m| (m.location_code, m.language_code.clone(), m.ads_language_code.clone().unwrap_or_else(|| m.language_code.clone())))
            .unwrap_or_else(|| (self.location_code, self.language_code.clone(), self.language_code.clone()))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Serp {
    /// Our first organic position, `None` when absent from the fetched depth.
    pub position: Option<f64>,
    pub breadcrumb: String,
    /// Host and title of the first organic result.
    pub leader: String,
}

/// Reads one SERP response: our first organic result on `host` and the leader.
pub fn parse(serp: &serde_json::Value, host: &str) -> Option<Serp> {
    let task = first(serp);
    let organic: Vec<&serde_json::Value> = items(at(task, &["items"])).iter().filter(|r| text(at(r, &["type"])) == "organic").collect();
    if organic.is_empty() && at(task, &["keyword"]).is_null() { return None; }
    let ours = organic.iter().find(|r| serp_domain(r) == host);
    let leader = organic.first().map(|r| format!("{}（{}）", serp_domain(r), text(at(r, &["title"])))).unwrap_or_default();
    Some(Serp { position: ours.and_then(|r| r["rank_group"].as_f64()), breadcrumb: ours.map(|r| text(at(r, &["breadcrumb"])).to_string()).unwrap_or_default(), leader })
}

/// The day's SERP file of a target under `day_dir`.
pub fn serp_file(cfg: &RankConfig, t: &TargetKeyword, day_dir: &str) -> String {
    format!("{day_dir}/serp_{}.json", file_key(&format!("{} {}", t.keyword, cfg.market(&t.page).0)))
}

/// Today's SERP of every target, fetching only the files the day does not have yet.
pub fn fetch(cfg: &RankConfig, targets: &[TargetKeyword], host: &str, day_dir: &str, errors: &mut Vec<String>) -> BTreeMap<String, Serp> {
    let out = |t: &TargetKeyword| serp_file(cfg, t, day_dir);
    let calls: Vec<MonidCall> = targets.iter().filter(|t| !std::path::Path::new(&out(t)).exists()).map(|t| {
        let (location, language, _) = cfg.market(&t.page);
        MonidCall { provider: "dataforseo".into(), endpoint: "/serp/google-organic".into(),
            body: json!({"keyword": t.keyword, "location_code": location, "language_code": language, "depth": cfg.serp_depth}), out: out(t) }
    }).collect();
    if !calls.is_empty() && let Err(e) = run_all(calls, 6) { errors.push(format!("rank serp: {e}")); }
    targets.iter().filter_map(|t| {
        let parsed = read_json(&out(t)).ok().and_then(|v| parse(&v, host));
        // A task error (`Internal SE Server Error`) is saved like a result; drop it so the next run retries.
        if parsed.is_none() { let _ = std::fs::remove_file(out(t)); }
        Some((t.keyword.clone(), parsed?))
    }).collect()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Cached { fetched: String, volume: Option<f64> }

/// Monthly search volume of each target: the ledger's, else Google Ads (cached `VOLUME_DAYS`).
pub fn volumes(cfg: &RankConfig, targets: &[TargetKeyword], ledger: &BTreeMap<String, f64>, state_dir: &str, today: chrono::NaiveDate, errors: &mut Vec<String>) -> BTreeMap<String, f64> {
    let cache_path = format!("{state_dir}/rank/volume.json");
    let mut cache: BTreeMap<String, Cached> = read_json(&cache_path).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let key = |t: &TargetKeyword| format!("{}:{}", cfg.market(&t.page).0, t.keyword.to_lowercase());
    let fresh = |c: &Cached| chrono::NaiveDate::parse_from_str(&c.fetched, "%Y-%m-%d").is_ok_and(|d| (today - d).num_days() < VOLUME_DAYS);
    let mut asks: BTreeMap<(u32, String), Vec<(String, String)>> = BTreeMap::new();
    for t in targets.iter().filter(|t| !ledger.contains_key(&t.keyword.to_lowercase()) && !cache.get(&key(t)).is_some_and(fresh)) {
        let (location, _, ads) = cfg.market(&t.page);
        if let Some(k) = ads_keyword(&t.keyword) { asks.entry((location, ads)).or_default().push((t.keyword.to_lowercase(), k.to_lowercase())); }
    }
    for ((location, language), ask) in &asks {
        let out = format!("{state_dir}/rank/volume_{location}_{}.json", today.format("%Y%m%d"));
        let call = MonidCall { provider: "dataforseo".into(), endpoint: "/keywords/google-ads-search-volume".into(),
            body: json!({"keywords": ask.iter().map(|(_, k)| k).collect::<Vec<_>>(), "location_code": location, "language_code": language}), out: out.clone() };
        match run_all(vec![call], 1).and_then(|_| read_json(&out)) {
            Ok(serde_json::Value::Array(list)) => {
                let got: BTreeMap<String, Option<f64>> = list.iter().map(|i| (text(&i["keyword"]).to_lowercase(), i["search_volume"].as_f64())).collect();
                for (word, k) in ask { cache.insert(format!("{location}:{word}"), Cached { fetched: today.to_string(), volume: got.get(k).copied().flatten() }); }
            }
            Ok(other) => {
                errors.push(format!("rank volume {location}: {}", text(&other["tasks"][0]["status_message"])));
                let _ = std::fs::remove_file(&out);
            }
            Err(e) => errors.push(format!("rank volume {location}: {e}")),
        }
    }
    if !asks.is_empty() && let Err(e) = stringify(&cache).and_then(|t| write_text(&cache_path, &t)) { errors.push(format!("rank volume: {e}")); }
    targets.iter().filter_map(|t| {
        let k = t.keyword.to_lowercase();
        ledger.get(&k).copied().filter(|v| *v > 0.0).or_else(|| cache.get(&key(t))?.volume).map(|v| (t.keyword.clone(), v))
    }).collect()
}
