//! Google Trends watch of our focus areas (founder 2026-11-07: a breakout name like "nano banana"
//! under "AI" must reach a judgment instead of being filtered out as off-topic). Every `everyHours`
//! it pulls each focus topic's interest graph and rising queries, adds the rising queries of the
//! keyword room's seed pulls, sorts every query into a kind (`watch_trends_ledger` keeps them).
//! The hourly retro labels unknown words in `<state>/trend-labels.jsonl` (`机会` with a page feeds the
//! keyword room).

use super::files::{at, items, num_or, read_json, text};
use super::monid::{MonidCall, first, run_all, slug};
use super::watch_config::{Seed, WatchConfig};
use super::watch_discover::{Candidate, rivals};
use super::watch_judge::{brands, fit_page, is_brand};
use super::watch_trends_market::{TopicPull, cap, market_of, topic_file, topic_pulls, topic_source};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Shares a stem with a keyword-room seed: a lead for the keyword room.
pub const FOCUS: &str = "focus";
/// A competitor or platform name on the move.
pub const BRAND: &str = "brand";
/// Neither: needs a judgment of what it is.
pub const UNKNOWN: &str = "unknown";
/// The label that turns a judged word into a keyword-room candidate.
pub const OPPORTUNITY: &str = "机会";

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Topic {
    pub keyword: String,
    pub area: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrendsConfig {
    pub every_hours: f64,
    pub location_code: u32,
    /// DataForSEO `time_range`, e.g. `past_7_days`.
    pub time_range: String,
    pub topics: Vec<Topic>,
    /// Smallest rising value (percent growth; Google shows 5000 and above as Breakout) that counts.
    pub min_rise: f64,
    pub max_per_source: usize,
    /// Last-24-hour interest over the earlier mean that raises a topic heat finding.
    pub heat_ratio: f64,
    /// Volume surge and weekly acceleration; absent skips both (no extra calls).
    #[serde(default)]
    pub surge: Option<super::watch_trends_surge::SurgeConfig>,
    /// Words judged `机会` handed to a same-hour `SEO异常词跟进` tab per Singapore day; absent turns it off.
    #[serde(default)]
    pub follow_up_per_day: Option<usize>,
    /// Similarweb competitor keyword snapshots from the `pro.similarweb.com` subscription over HTTP; absent skips it.
    #[serde(default)]
    pub similarweb: Option<super::watch_trends_similarweb::SimilarwebConfig>,
    /// Markets pulled beside the default location, each with topics in its own language.
    #[serde(default)]
    pub markets: Vec<super::watch_trends_market::Market>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rise {
    pub query: String,
    pub value: f64,
    /// `topic:<keyword>`, `topic:<market>:<keyword>` or `seed:<slug>`, comma-joined when several sources carry the query.
    pub source: String,
    pub kind: String,
    /// The seed page a `focus` query fits.
    pub page: String,
    /// Market code (`JP`, `KR`, `TH`) of a riser from an extra market; empty for the default one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub market: String,
    /// Google Ads volume of the latest published month.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
    /// Mean Google Ads volume of the 11 months before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<f64>,
    /// `volume / max(base, 1)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surge: Option<f64>,
    /// Weekly Trends: mean of the last 2 complete weeks over `max(mean of the weeks before the last 4, 1)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accel: Option<f64>,
    /// `新出现`, `快速上升`, `季节回潮`, `已回落` or empty (`watch_trends_surge::flag`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub flag: String,
}

/// True when the hourly retro must judge the word before anything uses it: unknown words, Similarweb
/// competitor risers, and focus words that only a topic pull raised ("relationship" brings celebrity gossip).
pub fn needs_judgment(r: &Rise) -> bool {
    r.kind == UNKNOWN || r.source.contains("similarweb:") || (r.kind == FOCUS && r.source.contains("topic:"))
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Heat {
    pub topic: String,
    pub area: String,
    /// Market code of an extra market; empty for the default one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub market: String,
    /// `(YYYY-MM-DD UTC, mean interest 0-100)`, oldest first.
    pub daily: Vec<(String, f64)>,
    pub last24: f64,
    pub prior: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Pull {
    pub at: String,
    pub heat: Vec<Heat>,
    pub rising: Vec<Rise>,
    /// Queries seen under two or more seeds within 7 days, none of which they fit (Trends filler such
    /// as "best time to visit maldives").
    pub noise: usize,
    #[serde(default)]
    pub filler: Vec<String>,
    /// The Google Ads month behind `Rise::volume` (`YYYY-MM`).
    #[serde(default)]
    pub month: String,
    /// Paid calls this pull made for volume (`volume`) and weekly curves (`weekly`).
    #[serde(default)]
    pub calls: BTreeMap<String, usize>,
    pub errors: Vec<String>,
}


/// What we already know: seed stems, target keywords and brand names.
pub struct Lens {
    pub seeds: Vec<Seed>,
    pub targets: Vec<String>,
    pub brands: Vec<String>,
}

impl Lens {
    pub fn new(config: &WatchConfig) -> Lens {
        let discovery = config.discovery.as_ref();
        Lens {
            seeds: discovery.map(|d| d.seeds.clone()).unwrap_or_default(),
            targets: config.targets.iter().map(|t| t.keyword.to_lowercase()).collect(),
            brands: discovery.and_then(|d| rivals(&d.registry).ok().map(|r| brands(&r, d))).unwrap_or_default(),
        }
    }

    /// `(kind, page)` of one rising query.
    pub fn kind(&self, query: &str) -> (&'static str, String) {
        if query.contains("mars") || is_brand(query, &self.brands) { return (BRAND, String::new()); }
        let probe = Candidate { keyword: query.into(), ..Default::default() };
        match fit_page(&probe, &self.seeds) {
            Some(seed) => (FOCUS, seed.page.clone()),
            None if self.targets.iter().any(|t| t == query) => (FOCUS, String::new()),
            None => (UNKNOWN, String::new()),
        }
    }

    /// Trends filler: a non-brand query under two or more seeds (`slug(seed.keyword)`) that fits none of them.
    pub fn filler(&self, query: &str, seeds: &BTreeSet<String>) -> bool {
        if seeds.len() < 2 || self.kind(query).0 == BRAND { return false; }
        let probe = Candidate { keyword: query.into(), ..Default::default() };
        !self.seeds.iter().filter(|s| seeds.contains(&slug(&s.keyword))).any(|s| fit_page(&probe, std::slice::from_ref(s)).is_some())
    }
}

/// `query -> seed slugs` of every `trends_<slug>.json` seed pull in `dirs` (the last 7 days).
pub fn seed_history(dirs: &[String]) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (source, queries) in dirs.iter().flat_map(|d| seed_sources(d)) {
        for (q, _) in queries { out.entry(q).or_default().insert(source.trim_start_matches("seed:").to_string()); }
    }
    out
}

/// `(seed:<slug>, rising queries)` of every seed pull in one keyword-room day directory.
fn seed_sources(dir: &str) -> Vec<(String, Vec<(String, f64)>)> {
    let names = std::fs::read_dir(dir).ok().into_iter().flatten().filter_map(|e| e.ok()?.file_name().into_string().ok());
    names.filter(|n| n.starts_with("trends_") && n.ends_with(".json")).collect::<BTreeSet<_>>().into_iter()
        .filter_map(|n| Some((format!("seed:{}", n.trim_start_matches("trends_").trim_end_matches(".json")), rising(&read_json(&format!("{dir}/{n}")).ok()?))))
        .collect()
}

/// `(query, value)` of every rising query in one Trends explore answer.
pub fn rising(json: &Value) -> Vec<(String, f64)> {
    items(at(first(json), &["items"])).iter().filter(|i| text(at(i, &["type"])) == "google_trends_queries_list")
        .flat_map(|i| items(at(i, &["data", "rising"])).iter())
        .map(|q| (text(at(q, &["query"])).trim().to_lowercase(), num_or(at(q, &["value"]), 0.0)))
        .filter(|(q, _)| !q.is_empty()).collect()
}

fn mean(values: &[f64]) -> f64 { if values.is_empty() { 0.0 } else { values.iter().sum::<f64>() / values.len() as f64 } }

/// The interest graph of one topic: daily means plus the last 24 hours against the hours before.
pub fn heat(json: &Value, topic: &Topic) -> Option<Heat> {
    let points: Vec<(i64, String, f64)> = items(at(first(json), &["items"])).iter()
        .filter(|i| text(at(i, &["type"])) == "google_trends_graph").flat_map(|i| items(at(i, &["data"])).iter())
        .filter(|p| !p["missing_data"].as_bool().unwrap_or(false))
        .filter_map(|p| Some((p["timestamp"].as_i64()?, text(at(p, &["date_from"])).to_string(), p["values"][0].as_f64()?))).collect();
    let end = points.iter().map(|p| p.0).max()?;
    let (recent, before): (Vec<_>, Vec<_>) = points.iter().partition(|p| p.0 > end - 86_400);
    let mut days: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for (_, day, v) in &points { days.entry(day.clone()).or_default().push(*v); }
    let round = |v: f64| (v * 10.0).round() / 10.0;
    Some(Heat { topic: topic.keyword.clone(), area: topic.area.clone(), market: String::new(),
        daily: days.into_iter().map(|(d, v)| (d, round(mean(&v)))).collect(),
        last24: round(mean(&recent.iter().map(|p| p.2).collect::<Vec<_>>())), prior: round(mean(&before.iter().map(|p| p.2).collect::<Vec<_>>())) })
}

/// Every source's rising queries at or above `min_rise`, kinded, capped per source and merged by query
/// (highest value wins, sources joined); returns them with the filler queries left out. `history` is
/// `seed_history` of the last 7 days: filler often shows under one seed a day but several in a week.
pub fn classify(sources: &[(String, Vec<(String, f64)>)], history: &BTreeMap<String, BTreeSet<String>>, lens: &Lens, cfg: &TrendsConfig) -> (Vec<Rise>, Vec<String>) {
    let mut seeds_with = history.clone();
    for (source, queries) in sources.iter().filter(|(s, _)| s.starts_with("seed:")) {
        for (q, _) in queries { seeds_with.entry(q.clone()).or_default().insert(source.trim_start_matches("seed:").to_string()); }
    }
    let (mut merged, mut noise): (BTreeMap<String, Rise>, BTreeSet<String>) = Default::default();
    for (source, queries) in sources {
        let mut kept: Vec<Rise> = queries.iter().filter(|(_, v)| *v >= cfg.min_rise).filter_map(|(q, v)| {
            if seeds_with.get(q).is_some_and(|s| lens.filler(q, s)) { noise.insert(q.clone()); return None; }
            let (kind, page) = lens.kind(q);
            Some(Rise { query: q.clone(), value: *v, source: source.clone(), kind: kind.into(), page, market: market_of(source), ..Default::default() })
        }).collect();
        kept.sort_by(|a, b| b.value.total_cmp(&a.value));
        for r in kept.into_iter().take(cap(cfg, source)) {
            let entry = merged.entry(r.query.clone()).or_insert_with(|| Rise { source: String::new(), ..r.clone() });
            entry.source = if entry.source.is_empty() { r.source.clone() } else { format!("{}, {}", entry.source, r.source) };
            if r.value > entry.value { entry.value = r.value; }
        }
    }
    let mut rises: Vec<Rise> = merged.into_values().collect();
    rises.sort_by(|a, b| b.value.total_cmp(&a.value));
    (rises, noise.into_iter().collect())
}

/// Pulls every focus topic into `dir` (Monid caches each file), reads the seed pulls in `seed_dir` and
/// judges filler against the seed pulls in `history` (the last 7 days' keyword-room directories).
pub fn pull(cfg: &TrendsConfig, lens: &Lens, dir: &str, seed_dir: Option<&str>, history: &[String], at: &str) -> Pull {
    let topics = topic_pulls(cfg);
    let path = |p: &TopicPull| format!("{dir}/{}.json", topic_file(&p.market, &p.topic.keyword));
    let calls = topics.iter().map(|p| MonidCall { provider: "dataforseo".into(), endpoint: "/keywords/google-trends-explore".into(),
        body: json!({"keywords": [p.topic.keyword], "location_code": p.location_code, "time_range": cfg.time_range,
            "item_types": ["google_trends_graph", "google_trends_queries_list"]}), out: path(p) }).collect();
    let mut out = Pull { at: at.into(), ..Default::default() };
    if let Err(e) = run_all(calls, 4) { out.errors.push(format!("trends: {e}")); }
    let mut sources = Vec::new();
    for p in &topics {
        let Ok(json) = read_json(&path(p)) else { continue };
        out.heat.extend(heat(&json, p.topic).map(|h| Heat { market: p.market.clone(), ..h }));
        sources.push((topic_source(&p.market, &p.topic.keyword), rising(&json)));
    }
    sources.extend(seed_dir.map(seed_sources).unwrap_or_default());
    (out.rising, out.filler) = classify(&sources, &seed_history(history), lens, cfg);
    out.noise = out.filler.len();
    out
}

#[cfg(test)]
#[path = "watch_trends_tests.rs"]
mod tests;
