//! The daily keyword round of the SEO watchdog and its ledger `<state>/keywords.json`: one round a
//! day (scout, judge, verify), at most `maxPerDay` keywords handed to a `SEO找词` tab, and every
//! handed keyword followed until it appears in `targets` (shipped) for the 28-day retro.

use super::files::{read_json, stringify, write_text};
use super::watch_analyze::{Finding, QueryRow};
use super::watch_config::{WatchConfig, WatchState};
use super::watch_discover::{gsc_lookups, rivals, scout};
use super::watch_judge::{DO, DROP, Judged, brands, prejudge, score, verify};
use super::watch_locale_words::{self as locale_words, Word};
use super::watch_page_gate::Gate;
use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, Timelike};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Days a dropped keyword stays out of the pool.
const DROP_DAYS: i64 = 28;
/// Days before a keyword whose SERP was already read is checked again.
const RECHECK_DAYS: i64 = 7;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub verdict: String,
    pub reason: String,
    pub objection: String,
    pub source: String,
    pub page: String,
    pub volume: f64,
    pub kd: Option<f64>,
    pub win_rate: Option<f64>,
    pub first_seen: String,
    pub last_judged: String,
    pub dispatched: Option<String>,
    pub shipped: Option<String>,
}

pub type Ledger = BTreeMap<String, Entry>;

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Round {
    pub day: String,
    /// Candidates per source kind (`related`, `trends`, `competitor-rank`, `competitor-page`, `anomaly`, `gsc`).
    pub pool: BTreeMap<String, usize>,
    pub prejudged_do: usize,
    pub judged: Vec<serde_json::Value>,
    pub picks: Vec<String>,
    pub new_pages: BTreeMap<String, Vec<String>>,
    /// `(keyword, path)` of picks that get their own landing page today (G8).
    #[serde(default)]
    pub own_pages: Vec<(String, String)>,
    /// The day's new-page cap and the index and click evidence behind it (G12).
    #[serde(default)]
    pub gate: Gate,
    pub errors: Vec<String>,
}

pub fn ledger_path(state_dir: &str) -> String { format!("{state_dir}/keywords.json") }

pub fn read_ledger(state_dir: &str) -> Ledger {
    read_json(&ledger_path(state_dir)).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// Keywords the founder's session vetoed (`keyword-vetoes.jsonl`, one `{"keyword","reason"}` per line).
fn vetoes(state_dir: &str) -> BTreeMap<String, String> {
    std::fs::read_to_string(format!("{state_dir}/keyword-vetoes.jsonl")).unwrap_or_default().lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|v| (v["keyword"].as_str().unwrap_or_default().to_lowercase(), v["reason"].as_str().unwrap_or_default().to_string())).collect()
}

/// True when the ledger says this keyword needs no new verdict today.
fn settled(entry: Option<&Entry>, today: NaiveDate) -> bool {
    let Some(e) = entry else { return false };
    if e.dispatched.is_some() || e.shipped.is_some() || e.verdict == "否决" { return true; }
    let judged = NaiveDate::parse_from_str(&e.last_judged, "%Y-%m-%d").unwrap_or(today);
    let age = today - judged;
    (e.verdict == DROP && age < Duration::days(DROP_DAYS)) || (e.win_rate.is_some() && e.verdict != DO && age < Duration::days(RECHECK_DAYS))
}

/// Runs today's round and returns it with the keywords to hand over, best first.
pub fn round(config: &WatchConfig, state_dir: &str, gsc: &[QueryRow], gate: &Gate, now: DateTime<FixedOffset>, ledger: &mut Ledger) -> Result<(Round, Vec<Judged>), String> {
    let cfg = config.discovery.as_ref().ok_or("discovery is not configured")?;
    let cfg = &super::watch_config::DiscoveryConfig { max_new_pages_per_day: gate.cap, ..cfg.clone() };
    let day = now.format("%Y-%m-%d").to_string();
    let today = now.date_naive();
    let root = format!("{state_dir}/discover");
    let dir = format!("{root}/{day}");
    let week = format!("week-{}-W{:02}", now.iso_week().year(), now.iso_week().week());
    let week_dir = format!("{root}/{week}");
    for d in [&dir, &week_dir] { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
    let prior_week = std::fs::read_dir(&root).ok().into_iter().flatten().filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("week-") && n.as_str() < week.as_str()).max().map(|n| format!("{root}/{n}"));
    let rivals = rivals(&cfg.registry)?;
    let brands = brands(&rivals, cfg);
    let extra: Vec<(String, String)> = super::watch_trends_ledger::leads(state_dir).into_iter().chain(gsc_lookups(gsc)).collect();
    let carried = super::watch_site_loop::read(&root, &day);
    let snapshot = super::watch_similarweb_words::latest(state_dir);
    let fresh_sw = snapshot.as_ref().map(super::watch_similarweb_words::new_words).unwrap_or_default();
    let extra: Vec<(String, String)> = extra.into_iter().chain(carried.leads).chain(fresh_sw).collect();
    let mut found = scout(cfg, &rivals, &extra, &carried.sites, &dir, &week_dir, prior_week.as_deref(), &format!("{root}/sitemaps.json"));
    if let Some(s) = &snapshot { super::watch_similarweb_words::blend(&mut found.candidates, s); }
    let targeted: Vec<String> = config.targets.iter().map(|t| t.keyword.to_lowercase()).collect();
    let mut errors = found.errors;
    let mut pool = BTreeMap::new();
    let mut judged: Vec<Judged> = found.candidates.iter()
        .inspect(|c| *pool.entry(c.source.split(':').next().unwrap_or_default().to_string()).or_insert(0) += 1)
        .filter(|c| !targeted.contains(&c.keyword) && !settled(ledger.get(&c.keyword), today))
        .map(|c| prejudge(c, cfg, &brands)).collect();
    if let Some(m) = &cfg.method {
        super::watch_method::ads_volume(&mut judged, cfg, &dir, &mut errors);
        for j in judged.iter_mut().filter(|j| j.verdict != DROP) { *j = prejudge(&j.candidate, cfg, &brands); }
        super::watch_method::probe(&mut judged, cfg, m, &brands, &dir, today, &mut errors);
    }
    let prejudged_do = judged.iter().filter(|j| j.verdict == DO).count();
    verify(&mut judged, cfg, &dir, &mut errors);
    if let Some(m) = &cfg.method {
        super::watch_method::domain_window(&mut judged, m, &dir, today, &mut errors);
        super::watch_persist::rival_domains(&mut judged, m, &dir, today, &mut errors);
        super::watch_synonyms::merge(&mut judged, cfg, &dir, &mut errors);
        let next = super::watch_site_loop::Loop { day: day.clone(), sites: super::watch_site_loop::next_sites(&judged, &cfg.domain, m.loop_sites),
            leads: super::watch_site_loop::unbranded(&judged, &brands) };
        if let Err(e) = super::watch_site_loop::write(&root, &next) { errors.push(format!("site loop: {e}")); }
    }
    judged.sort_by(|a, b| (b.verdict == DO).cmp(&(a.verdict == DO)).then(score(&b.candidate).total_cmp(&score(&a.candidate))));
    let mut picks: Vec<Judged> = judged.iter().filter(|j| j.verdict == DO).take(cfg.max_per_day).cloned().collect();
    let own_pages = super::watch_own_page::reserve(&judged, &mut picks, cfg, &config.targets);
    for j in &judged {
        let e = ledger.entry(j.candidate.keyword.clone()).or_insert_with(|| Entry { first_seen: day.clone(), ..Default::default() });
        *e = Entry { verdict: j.verdict.clone(), reason: j.reason.clone(), objection: j.objection.clone(), source: j.candidate.source.clone(),
            page: j.candidate.page.clone(), volume: j.candidate.volume, kd: j.candidate.kd, win_rate: j.win_rate, last_judged: day.clone(), ..e.clone() };
    }
    let round = Round { day, pool, prejudged_do, picks: picks.iter().map(|p| p.candidate.keyword.clone()).collect(), new_pages: found.new_pages, errors,
        own_pages, gate: gate.clone(),
        judged: judged.iter().filter(|j| j.verdict != DROP).take(40).chain(judged.iter().filter(|j| j.verdict == DROP).take(40))
            .filter_map(|j| serde_json::to_value(j).ok()).collect() };
    write_text(&format!("{dir}/round.json"), &stringify(&round)?)?;
    Ok((round, picks))
}

/// Marks dispatched keywords that now appear in `targets` as shipped, and applies vetoes.
pub fn follow(ledger: &mut Ledger, config: &WatchConfig, state_dir: &str, day: &str) {
    let targeted: Vec<String> = config.targets.iter().map(|t| t.keyword.to_lowercase()).collect();
    let vetoed = vetoes(state_dir);
    for (keyword, e) in ledger.iter_mut() {
        if e.shipped.is_none() && targeted.contains(keyword) { e.shipped = Some(day.to_string()); }
        if let Some(reason) = vetoed.get(keyword).filter(|_| e.shipped.is_none()) { e.verdict = "否决".into(); e.objection = reason.clone(); }
    }
}

/// The findings a `SEO找词` tab receives, one per picked keyword.
pub fn findings(picks: &[Judged], dir: &str, gate: &Gate, own: &[(String, String)], local: &[Word]) -> Vec<Finding> {
    let reserved = own.iter().map(|(k, p)| format!("；新页名额给「{k}」({p})，未建成的种子页优先")).collect::<String>();
    let quota = Finding { key: "keyword:quota".into(), severity: "P2".into(),
        title: format!("今天改 {} 个词，最多新建 {} 个落地页", picks.len(), gate.cap), action: format!("其余词并入最接近页面{reserved}；{}；{}", gate.note, locale_words::RULE) };
    picks.iter().map(|j| {
        let c = &j.candidate;
        let born = c.age_days.map_or(String::new(), |d| format!("、Trends {d} 天前出现"));
        let merged = if j.synonyms.is_empty() { String::new() } else { format!("；同一需求的说法并入同一 section 的 keywords 与 FAQ：{}", j.synonyms.join("、")) };
        Finding { key: format!("keyword:{}", c.keyword), severity: "P1".into(),
            title: format!("「{}」US 月搜 {}、KD {}、CPC ${:.2}、趋势 季 {:+}% / 年 {:+}%{born}、胜率 {:.2}（来源 {}）", c.keyword, super::watch_signals::demand(c),
                c.kd.map_or("—".into(), |k| k.to_string()), c.cpc, c.quarterly, c.yearly, j.win_rate.unwrap_or_default(), c.source),
            action: format!("{}{}；规则唱反调：{}{merged}；前 10（首页 {} 个）：{}；SERP 原始数据 {dir}/serp_{}.json", own.iter().find(|(k, _)| *k == c.keyword).map_or(format!("最接近页面 {}", c.page),
                |(_, p)| format!("新建落地页 {p}（G8：月搜 ≥ 1000 且现有页面和 targets 未覆盖这个需求，最接近的旧页 {}）", c.page)), locale_words::kept_note(local, &c.page), j.objection, j.homepages,
                j.top.iter().map(|(d, r)| format!("{d}({r})")).collect::<Vec<_>>().join(", "), super::monid::slug(&c.keyword)) }
    }).chain(std::iter::once(quota)).collect()
}

/// True when today's round is due: the configured hour, or `force`, and not yet run today.
pub fn due(config: &WatchConfig, state: &WatchState, now: DateTime<FixedOffset>, force: bool) -> bool {
    let Some(cfg) = &config.discovery else { return false };
    let today = now.format("%Y-%m-%d").to_string();
    state.last_discover_day.as_deref() != Some(&today) && (force || now.hour() == cfg.hour_sgt)
}

/// This hour's keyword step: follow handed keywords, run the round when due and hand its picks to
/// one `SEO找词` tab. Returns the dispatch note; the ledger is saved unless `dry_run`.
#[allow(clippy::too_many_arguments)]
pub fn hourly(config: &WatchConfig, state_dir: &str, state: &mut WatchState, ledger: &mut Ledger, gsc: &[QueryRow], gate: &Gate, now: DateTime<FixedOffset>,
    force: bool, dry_run: bool, page_url: &str, run_dir: &str, errors: &mut Vec<String>) -> String {
    let day = now.format("%Y-%m-%d").to_string();
    follow(ledger, config, state_dir, &day);
    let mut note = String::new();
    if due(config, state, now, force) {
        note = match round(config, state_dir, gsc, gate, now, ledger) {
            Err(e) => { errors.push(format!("discover: {e}")); "找词失败".into() }
            Ok((r, picks)) => {
                if !dry_run { state.last_discover_day = Some(day.clone()); }
                let total: usize = r.pool.values().sum();
                if picks.is_empty() { format!("找词：{total} 个候选，今天没有可做的词") }
                else if dry_run { format!("（演练）将派「找词」标签处理 {} 个词{}；{}", picks.len(), r.own_pages.iter().map(|(k, p)| format!("，「{k}」新建 {p}")).collect::<String>(), gate.note) }
                else { match super::watch_publish::dispatch("找词", page_url, run_dir, &findings(&picks, &format!("{state_dir}/discover/{day}"), gate, &r.own_pages, &locale_words::read(locale_words::LEDGER))) {
                    Ok(session) => {
                        for p in &picks { if let Some(e) = ledger.get_mut(&p.candidate.keyword) { e.dispatched = Some(day.clone()); } }
                        state.last_session = Some(session.clone());
                        format!("已派「找词」标签处理 {} 个词：{}（{}）", picks.len(), r.picks.join("、"), super::watch_publish::tab_label(&session))
                    }
                    Err(e) => { errors.push(format!("dispatch: {e}")); "派「找词」失败".into() }
                } }
            }
        };
    }
    if !dry_run && let Ok(text) = stringify(ledger) { let _ = write_text(&ledger_path(state_dir), &text); }
    note
}

#[cfg(test)]
#[path = "watch_keywords_tests.rs"]
mod tests;
