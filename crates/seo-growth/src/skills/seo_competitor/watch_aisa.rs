//! AIsa search signals of every watchdog run (block `aisaSeo` of `analysis.json`, section `AIsa 搜索信号`):
//! AI Overview citations read from the day's rank SERPs at no cost, Google AI Mode answers for a rotating
//! slice of targets, keyword intent and difficulty (30-day cache) and daily new/lost backlinks.
//! State and caches live under `<state>/aisa/`; spend stops at `dailyBudgetUsd` per Singapore day.

use super::aisa::dataforseo_tool;
use super::files::{read_json, stringify};
use super::watch_aisa_parse::{self as parse, AiCite, KeywordProps, LinkDay};
use super::watch_config::{TargetKeyword, WatchConfig};
use super::watch_rank_serp::{RankConfig, serp_file};
use chrono::{DateTime, Duration, FixedOffset};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

const AI_MODE_TOOL: &str = "post_dataforseo_serp_google_ai_mode_live";
const INTENT_TOOL: &str = "post_dataforseo_labs_google_search_intent_live";
const DIFFICULTY_TOOL: &str = "post_dataforseo_labs_google_bulk_keyword_difficulty_live";
const LINKS_TOOL: &str = "post_dataforseo_backlinks_timeseries_new_lost_live";
const PROPS_DAYS: i64 = 30;
const LINK_DAYS: i64 = 30;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AisaSeoConfig {
    /// Targets asked of Google AI Mode per run (≈ $0.004 each), in rotation.
    pub ai_mode_per_run: usize,
    /// Spend cap per Singapore day in US$; a call that would pass it is skipped.
    pub daily_budget_usd: f64,
}

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
struct AisaState { cursor: usize, day: String, spent_usd: f64 }

/// The `aisaSeo` block of `analysis.json`.
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AisaRun {
    /// Today's AI Overview of every target whose rank SERP is on disk.
    pub overview: Vec<AiCite>,
    /// AI Mode rows fetched by this run.
    pub ai_mode_run: Vec<AiCite>,
    /// Latest AI Mode row of every target ever asked.
    pub ai_mode: Vec<AiCite>,
    /// Target keyword → intent and difficulty.
    pub props: BTreeMap<String, KeywordProps>,
    /// Daily new/lost links to our domain, oldest first.
    pub links: Vec<LinkDay>,
    pub spent_today_usd: f64,
    pub daily_budget_usd: f64,
    pub skipped: Vec<String>,
}

/// `stopped` turns true once a dry run, the cap or a missing key forbids further calls this run.
struct Spend<'a> { state: &'a mut AisaState, cap: f64, dry_run: bool, stopped: bool, skipped: &'a mut Vec<String> }

impl Spend<'_> {
    /// Runs one paid call unless the day's cap or a dry run forbids it.
    fn call(&mut self, what: &str, tool: &str, body: serde_json::Value, errors: &mut Vec<String>) -> Option<serde_json::Value> {
        if self.stopped { return None; }
        if self.dry_run { self.stop(format!("{what}：演练不调用")); return None; }
        if self.state.spent_usd >= self.cap { self.stop(format!("{what}：今日已花 ${:.2}，达到上限 ${:.2}", self.state.spent_usd, self.cap)); return None; }
        match dataforseo_tool(tool, body) {
            None => { self.stop(format!("{what}：没有 AIsa 密钥")); None }
            Some(Err(e)) => { errors.push(format!("aisa {what}: {e}")); None }
            Some(Ok((result, cost))) => { self.state.spent_usd += cost; Some(result) }
        }
    }

    fn stop(&mut self, why: String) { self.stopped = true; self.skipped.push(why); }
}

fn default_rank() -> RankConfig {
    RankConfig { serp_depth: 10, location_code: 2840, language_code: "en".into(), markets: Vec::new(), max_work_per_tab: 0 }
}

/// One run; `None` when the config has no `aisaSeo` block.
pub fn collect(config: &WatchConfig, state_dir: &str, now: DateTime<FixedOffset>, dry_run: bool, errors: &mut Vec<String>) -> Option<AisaRun> {
    let cfg = config.aisa_seo.as_ref()?;
    let rank = config.rank.clone().unwrap_or_else(default_rank);
    let dir = format!("{state_dir}/aisa");
    if !dry_run && let Err(e) = std::fs::create_dir_all(&dir) { errors.push(format!("aisa cache dir: {}", e.kind())); }
    let today = now.format("%Y-%m-%d").to_string();
    let state_path = format!("{dir}/state.json");
    let mut state: AisaState = read_json(&state_path).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    if state.day != today { state = AisaState { day: today.clone(), spent_usd: 0.0, ..state }; }
    let mut run = AisaRun { daily_budget_usd: cfg.daily_budget_usd, ..Default::default() };
    run.overview = overview(&rank, &config.targets, &format!("{state_dir}/rank/{today}"), &today);
    let mut skipped = Vec::new();
    let mut spend = Spend { state: &mut state, cap: cfg.daily_budget_usd, dry_run, stopped: false, skipped: &mut skipped };
    run.ai_mode_run = ai_mode(&mut spend, &rank, &config.targets, cfg.ai_mode_per_run, now, errors);
    run.ai_mode = merge_ai_mode(&format!("{dir}/ai_mode.json"), &run.ai_mode_run, dry_run, errors);
    run.props = props(&mut spend, &rank, &config.targets, &format!("{dir}/keyword_props.json"), now, errors);
    run.links = links(&mut spend, &config.site, &format!("{dir}/links_{today}.json"), now, errors);
    run.skipped = skipped;
    run.spent_today_usd = state.spent_usd;
    if !dry_run { save(&state_path, &state, "state", errors); }
    Some(run)
}

/// Writes one cache file. The error names only `aisa/<file>`: an absolute home path on the page trips the
/// report publication guard (`users/<name>/…`) and the whole watch page stops publishing.
fn save<T: Serialize>(path: &str, value: &T, what: &str, errors: &mut Vec<String>) {
    if let Err(e) = stringify(value).and_then(|t| std::fs::write(path, t).map_err(|e| e.kind().to_string())) {
        let file = path.rsplit('/').next().unwrap_or(path);
        errors.push(format!("aisa {what} (aisa/{file}): {e}"));
    }
}

fn overview(rank: &RankConfig, targets: &[TargetKeyword], day_dir: &str, today: &str) -> Vec<AiCite> {
    targets.iter().filter_map(|t| {
        let serp = read_json(&serp_file(rank, t, day_dir)).ok()?;
        Some(parse::ai_cite(&serp, &t.keyword, &t.page, rank.market(&t.page).0, today))
    }).collect()
}

fn ai_mode(spend: &mut Spend, rank: &RankConfig, targets: &[TargetKeyword], per_run: usize, now: DateTime<FixedOffset>, errors: &mut Vec<String>) -> Vec<AiCite> {
    if targets.is_empty() { return Vec::new(); }
    let take = per_run.min(targets.len());
    let start = spend.state.cursor % targets.len();
    let mut rows = Vec::new();
    for t in (0..take).map(|i| &targets[(start + i) % targets.len()]) {
        let (location, language, _) = rank.market(&t.page);
        let body = json!({"keyword": t.keyword, "location_code": location, "language_code": language});
        let at = now.to_rfc3339();
        match spend.call(&format!("AI Mode {}", t.keyword), AI_MODE_TOOL, body, errors) {
            Some(result) => rows.push(parse::ai_cite(&result, &t.keyword, &t.page, location, &at)),
            None if spend.stopped => break,
            None => rows.push(AiCite { keyword: t.keyword.clone(), page: t.page.clone(), location, at, error: Some("AIsa 调用失败，见 errors".into()), ..Default::default() }),
        }
    }
    if !rows.is_empty() { spend.state.cursor = (start + rows.len()) % targets.len(); }
    rows
}

fn merge_ai_mode(path: &str, fresh: &[AiCite], dry_run: bool, errors: &mut Vec<String>) -> Vec<AiCite> {
    let mut latest: BTreeMap<String, AiCite> = read_json(path).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    for row in fresh.iter().filter(|r| r.error.is_none()) { latest.insert(format!("{}:{}", row.location, row.keyword.to_lowercase()), row.clone()); }
    if !dry_run && !fresh.is_empty() { save(path, &latest, "ai mode", errors); }
    latest.into_values().collect()
}

fn props(spend: &mut Spend, rank: &RankConfig, targets: &[TargetKeyword], path: &str, now: DateTime<FixedOffset>, errors: &mut Vec<String>) -> BTreeMap<String, KeywordProps> {
    let today = now.date_naive();
    let mut cache: BTreeMap<String, KeywordProps> = read_json(path).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let key = |t: &TargetKeyword| format!("{}:{}", rank.market(&t.page).0, t.keyword.to_lowercase());
    let fresh = |p: &KeywordProps| chrono::NaiveDate::parse_from_str(&p.fetched, "%Y-%m-%d").is_ok_and(|d| {
        let age = (today - d).num_days();
        if p.intent.is_some() || p.difficulty.is_some() { age < PROPS_DAYS } else { age < 1 }
    });
    let mut asks: BTreeMap<(u32, String), Vec<String>> = BTreeMap::new();
    for t in targets.iter().filter(|t| !cache.get(&key(t)).is_some_and(fresh)) {
        let (location, language, _) = rank.market(&t.page);
        asks.entry((location, language)).or_default().push(t.keyword.to_lowercase());
    }
    let asked = !asks.is_empty();
    for ((location, language), words) in asks {
        let what = format!("关键词意图 {location}");
        let intent = spend.call(&what, INTENT_TOOL, json!({"keywords": words, "language_code": language}), errors).map(|r| parse::intents(&r));
        let kd = spend.call(&format!("关键词难度 {location}"), DIFFICULTY_TOOL, json!({"keywords": words, "location_code": location, "language_code": language}), errors).map(|r| parse::difficulties(&r));
        if intent.is_none() && kd.is_none() && spend.stopped { continue; }
        for w in &words {
            let i = intent.as_ref().and_then(|m| m.get(w).cloned());
            cache.insert(format!("{location}:{w}"), KeywordProps { fetched: today.to_string(), intent: i.as_ref().map(|x| x.0.clone()),
                probability: i.as_ref().map(|x| x.1), secondary: i.and_then(|x| x.2), difficulty: kd.as_ref().and_then(|m| m.get(w).copied()) });
        }
    }
    if asked && !spend.dry_run { save(path, &cache, "keyword props", errors); }
    targets.iter().filter_map(|t| Some((t.keyword.clone(), cache.get(&key(t))?.clone()))).collect()
}

fn links(spend: &mut Spend, site: &str, path: &str, now: DateTime<FixedOffset>, errors: &mut Vec<String>) -> Vec<LinkDay> {
    if let Ok(saved) = read_json(path) { return serde_json::from_value(saved).unwrap_or_default(); }
    let host = site.split_once("://").map_or(site, |(_, r)| r).trim_end_matches('/').trim_start_matches("www.");
    let from = (now.date_naive() - Duration::days(LINK_DAYS)).format("%Y-%m-%d").to_string();
    let body = json!({"target": host, "date_from": from, "date_to": now.format("%Y-%m-%d").to_string(), "group_range": "day"});
    let Some(result) = spend.call("外链增减", LINKS_TOOL, body, errors) else { return Vec::new() };
    let days = parse::link_days(&result);
    save(path, &days, "links", errors);
    days
}

#[cfg(test)]
#[path = "watch_aisa_tests.rs"]
mod tests;
