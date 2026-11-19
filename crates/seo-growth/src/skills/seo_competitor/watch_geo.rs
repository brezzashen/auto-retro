//! AI recommendation watch (founder 2026-11-12): every watchdog hour asks web-searching ChatGPT, Claude and Gemini
//! (through AIsa) a few unbranded questions of the geo-test bank and records whether the answer recommends Mars.
//! Records live in `<state>/geo/records.jsonl` with every cited URL and search query, answer texts in
//! `<run>/geo/answers.jsonl`, the rotation per market and the day's spend (Singapore day, US$) in `<state>/geo/state.json`.

use super::files::{stringify, write_text};
use super::watch_geo_aisa::{self as aisa, Answer, Verdict};
use super::watch_geo_ark::{self as ark, Price};
pub use super::watch_geo_bank::{Question, read_bank, rotate};
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Command;

/// The market of an engine without one.
pub const GLOBAL: &str = "global";

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GeoEngine {
    pub id: String,
    pub label: String,
    pub model: String,
    /// `openai-search`, `anthropic-search`, `gemini-search` (AIsa) or `ark-search` (Volcengine Ark, `watch_geo_ark`).
    pub kind: String,
    /// Keychain item of the engine's own key; absent means the config's AIsa key.
    #[serde(default)]
    pub keychain_service: Option<String>,
    #[serde(default)]
    pub keychain_account: Option<String>,
    /// US$ per million tokens, for an API that bills without returning the amount (`ark-search`).
    #[serde(default)]
    pub price_input_per_m_usd: f64,
    #[serde(default)]
    pub price_output_per_m_usd: f64,
    /// Bank market this engine is asked (`global` when absent); Doubao asks `cn` questions.
    #[serde(default)]
    pub market: Option<String>,
}

impl GeoEngine {
    pub fn market(&self) -> &str {
        self.market.as_deref().unwrap_or(GLOBAL)
    }
}

/// An engine the watch names but cannot query, shown with its reason instead of a number.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Unmeasured {
    pub label: String,
    pub reason: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GeoConfig {
    pub keychain_service: String,
    pub keychain_account: String,
    /// The geo-test question bank (JSON lines), relative to the repository root.
    pub bank: String,
    pub questions_per_run: usize,
    /// Spend cap per Singapore day in US$; a run that would pass it is skipped.
    pub daily_budget_usd: f64,
    /// Expected cost of one question across every engine, for the cap check.
    pub est_question_usd: f64,
    pub judge_model: String,
    pub engines: Vec<GeoEngine>,
    #[serde(default)]
    pub unmeasured: Vec<Unmeasured>,
}

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GeoState {
    /// Rotation cursor of the `global` market.
    pub cursor: usize,
    pub day: String,
    pub spent_usd: f64,
    /// Rotation cursors of the other markets.
    #[serde(default)]
    pub market_cursors: BTreeMap<String, usize>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GeoRecord {
    pub at: String,
    pub hour: String,
    pub qid: String,
    pub lang: String,
    pub engine: String,
    pub model: String,
    /// The answer arrived whole and the judge read it; only these count in a rate.
    pub ok: bool,
    pub error: Option<String>,
    pub mentioned: bool,
    pub rank: Option<u32>,
    pub products: Vec<String>,
    pub cited: usize,
    pub mars_cited: bool,
    pub cost_usd: f64,
    /// The question's bucket (`tool`, `advice`, `dating`); empty in records older than buckets, read as `tool`.
    #[serde(default)]
    pub bucket: String,
    /// Cited URLs (Gemini: source titles, its URLs are redirects).
    #[serde(default)]
    pub cited_urls: Vec<String>,
    /// Search queries the engine ran, where its API returns them.
    #[serde(default)]
    pub fan_out: Vec<String>,
}

/// The `geo` block of `analysis.json`.
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GeoRun {
    pub hour: String,
    pub records: Vec<GeoRecord>,
    pub spent_today_usd: f64,
    pub daily_budget_usd: f64,
    pub skipped: Option<String>,
    pub unmeasured: Vec<Unmeasured>,
}

fn keychain(service: &str, account: &str) -> Result<String, String> {
    let out = Command::new("security").args(["find-generic-password", "-s", service, "-a", account, "-w"]).output().map_err(|e| format!("security: {e}"))?;
    if !out.status.success() { return Err(format!("Keychain {service}/{account} not readable")); }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn record(base: GeoRecord, answer: Result<Answer, String>, verdict: Option<Result<Verdict, String>>) -> GeoRecord {
    let base = match &answer { Ok(a) => GeoRecord { cited_urls: a.cited.clone(), fan_out: a.fan_out.clone(), ..base }, Err(_) => base };
    match (answer, verdict) {
        (Err(error), _) => GeoRecord { error: Some(error), ..base },
        (Ok(a), _) if a.incomplete.is_some() => GeoRecord { error: a.incomplete.clone(), cost_usd: a.cost_usd, cited: a.cited.len(), ..base },
        (Ok(a), Some(Ok(v))) => GeoRecord { ok: true, mentioned: v.mentioned, rank: v.rank, products: v.products, cited: a.cited.len(),
            mars_cited: a.cited.iter().any(|c| aisa::is_ours(c)), cost_usd: a.cost_usd + v.cost_usd, ..base },
        (Ok(a), Some(Err(error))) => GeoRecord { error: Some(error), cost_usd: a.cost_usd, cited: a.cited.len(), ..base },
        (Ok(a), None) => GeoRecord { error: Some("not judged".into()), cost_usd: a.cost_usd, ..base },
    }
}

fn ask(key: &str, engine: &GeoEngine, question: &str) -> Result<Answer, String> {
    if engine.kind == "ark-search" {
        return ark::ask(key, &engine.model, question, Price { input_per_m: engine.price_input_per_m_usd, output_per_m: engine.price_output_per_m_usd });
    }
    aisa::ask(key, &engine.kind, &engine.model, question)
}

fn read_state(path: &str) -> GeoState {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// One hour of the watch; failures land in `errors` and never stop the watchdog run.
pub fn collect(config: Option<&GeoConfig>, state_dir: &str, run_dir: &str, now: DateTime<FixedOffset>, dry_run: bool, errors: &mut Vec<String>) -> Option<GeoRun> {
    let config = config?;
    let state_path = format!("{state_dir}/geo/state.json");
    let mut state = read_state(&state_path);
    let today = now.format("%Y-%m-%d").to_string();
    if state.day != today { state = GeoState { day: today, spent_usd: 0.0, ..state }; }
    let hour = now.format("%Y-%m-%dT%H").to_string();
    let mut run = GeoRun { hour: hour.clone(), daily_budget_usd: config.daily_budget_usd, unmeasured: config.unmeasured.clone(), ..Default::default() };
    let mut by_market: BTreeMap<String, (Vec<Question>, usize)> = BTreeMap::new();
    for market in config.engines.iter().map(|e| e.market().to_string()) {
        if by_market.contains_key(&market) { continue; }
        let bank = match read_bank(&config.bank, &market) { Ok(b) => b, Err(e) => { errors.push(format!("geo: {e}")); return Some(run); } };
        let cursor = if market == GLOBAL { state.cursor } else { state.market_cursors.get(&market).copied().unwrap_or(0) };
        by_market.insert(market, rotate(&bank, cursor, config.questions_per_run));
    }
    let questions_per_engine = by_market.values().map(|(q, _)| q.len()).max().unwrap_or(0);
    let estimate = config.est_question_usd * questions_per_engine as f64;
    run.spent_today_usd = state.spent_usd;
    if state.spent_usd + estimate > config.daily_budget_usd {
        run.skipped = Some(format!("今日已花 ${:.2}，再跑约 ${estimate:.2} 会超过每日上限 ${:.2}", state.spent_usd, config.daily_budget_usd));
        return Some(run);
    }
    if dry_run { run.skipped = Some(format!("演练：将问 {questions_per_engine} 题 × {} 个引擎", config.engines.len())); return Some(run); }
    let key = match keychain(&config.keychain_service, &config.keychain_account) { Ok(k) => k, Err(e) => { errors.push(format!("geo: {e}")); return Some(run); } };
    let engine_keys: Vec<Result<String, String>> = config.engines.iter().map(|e| match (&e.keychain_service, &e.keychain_account) {
        (Some(s), Some(a)) => keychain(s, a), _ => Ok(key.clone()) }).collect();
    let pairs: Vec<(&Question, &GeoEngine, &Result<String, String>)> = config.engines.iter().zip(&engine_keys)
        .flat_map(|(e, k)| by_market[e.market()].0.iter().map(move |q| (q, e, k))).collect();
    let answered: Vec<(GeoRecord, Option<(String, String)>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = pairs.iter().map(|(q, e, engine_key)| { let (key, hour) = (&key, &hour); scope.spawn(move || {
            let base = GeoRecord { at: now.to_rfc3339(), hour: hour.clone(), qid: q.id.clone(), lang: q.lang.clone(), engine: e.id.clone(), model: e.model.clone(), bucket: q.bucket().to_string(), ..Default::default() };
            let answer = match engine_key { Ok(k) => ask(k, e, &q.question), Err(err) => Err(err.clone()) };
            let verdict = answer.as_ref().ok().filter(|a| a.incomplete.is_none()).map(|a| aisa::judge(key, &config.judge_model, &q.question, &a.text));
            let text = answer.as_ref().ok().map(|a| (q.id.clone(), a.text.clone()));
            (record(base, answer, verdict), text)
        }) }).collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    let mut answers = String::new();
    for (rec, text) in &answered {
        if let Some((qid, text)) = text { answers.push_str(&stringify(&serde_json::json!({"qid": qid, "engine": rec.engine, "answer": text})).unwrap_or_default()); answers.push('\n'); }
        if let Some(error) = &rec.error { errors.push(format!("geo {} {}: {error}", rec.engine, rec.qid)); }
    }
    run.records = answered.into_iter().map(|(r, _)| r).collect();
    let spent: f64 = run.records.iter().map(|r| r.cost_usd).sum();
    state.spent_usd += spent;
    for (market, (_, cursor)) in by_market {
        if market == GLOBAL { state.cursor = cursor; } else { state.market_cursors.insert(market, cursor); }
    }
    run.spent_today_usd = state.spent_usd;
    let _ = std::fs::create_dir_all(format!("{run_dir}/geo")).map(|_| write_text(&format!("{run_dir}/geo/answers.jsonl"), &answers));
    if let Err(e) = append_records(state_dir, &run.records).and_then(|_| write_text(&state_path, &stringify(&state)?)) { errors.push(format!("geo: {e}")); }
    Some(run)
}

fn append_records(state_dir: &str, records: &[GeoRecord]) -> Result<(), String> {
    let path = format!("{state_dir}/geo/records.jsonl");
    std::fs::create_dir_all(format!("{state_dir}/geo")).map_err(|e| e.to_string())?;
    let prior = std::fs::read_to_string(&path).unwrap_or_default();
    let lines: String = records.iter().filter_map(|r| stringify(r).ok()).map(|l| l + "\n").collect();
    write_text(&path, &format!("{prior}{lines}"))
}

/// Every stored record, oldest first.
pub fn read_records(state_dir: &str) -> Vec<GeoRecord> {
    std::fs::read_to_string(format!("{state_dir}/geo/records.jsonl")).unwrap_or_default().lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter_map(|v| serde_json::from_value(v).ok()).collect()
}

#[cfg(test)]
#[path = "watch_geo_tests.rs"]
mod tests;
