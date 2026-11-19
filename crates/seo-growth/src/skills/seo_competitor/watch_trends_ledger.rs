//! The Trends watch's memory: `<state>/trends/anomalies.json` (every riser's first sighting, peak and
//! sources), the hourly retro's judgments in `<state>/trend-labels.jsonl`, the keyword-room leads they
//! give, and the hourly step that pulls when `everyHours` passed.

use super::files::{read_json, stringify, text, write_text};
use super::watch_config::{WatchConfig, WatchState};
use super::watch_trends::{Lens, OPPORTUNITY, Pull, pull};
use super::watch_trends_surge::enrich;
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Sighting {
    pub kind: String,
    pub sources: Vec<String>,
    pub first_seen: String,
    pub last_seen: String,
    pub peak: f64,
    pub pulls: usize,
}

pub type Ledger = BTreeMap<String, Sighting>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Label {
    pub label: String,
    pub page: String,
    pub note: String,
}

pub fn record(ledger: &mut Ledger, pull: &Pull) {
    for r in &pull.rising {
        let e = ledger.entry(r.query.clone()).or_insert_with(|| Sighting { first_seen: pull.at.clone(), ..Default::default() });
        e.kind = r.kind.clone();
        e.last_seen = pull.at.clone();
        e.peak = e.peak.max(r.value);
        e.pulls += 1;
        for s in r.source.split(", ") { if !e.sources.iter().any(|x| x == s) { e.sources.push(s.to_string()); } }
    }
}

pub fn ledger(state_dir: &str) -> Ledger {
    read_json(&format!("{state_dir}/trends/anomalies.json")).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// The latest judgment per query from `trend-labels.jsonl` (`{"query","label","page","note","at"}` per line).
pub fn labels(state_dir: &str) -> BTreeMap<String, Label> {
    std::fs::read_to_string(format!("{state_dir}/trend-labels.jsonl")).unwrap_or_default().lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| (text(&v["query"]).trim().to_lowercase(), Label { label: text(&v["label"]).into(), page: text(&v["page"]).into(), note: text(&v["note"]).into() }))
        .filter(|(q, _)| !q.is_empty()).collect()
}

/// Keyword-room leads: only words judged `机会` with a page (`anomaly:<page>`); a topic pull's focus
/// riser waits for the same judgment ("relationship" risers are mostly celebrity gossip).
pub fn leads(state_dir: &str) -> Vec<(String, String)> {
    let handed = super::watch_trends_followup::followed(state_dir);
    labels(state_dir).into_iter().filter(|(q, l)| l.label == OPPORTUNITY && l.page.starts_with('/') && !handed.contains_key(q))
        .map(|(q, l)| (q, format!("anomaly:{}", l.page))).collect()
}

/// The keyword-room day directories of the last 7 days (filler shows under different seeds on different days).
fn week_of_seed_pulls(state_dir: &str, today: chrono::NaiveDate) -> Vec<String> {
    let oldest = (today - chrono::Duration::days(6)).to_string();
    let root = format!("{state_dir}/discover");
    let mut days: Vec<String> = std::fs::read_dir(&root).ok().into_iter().flatten().filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.len() == 10 && n.as_str() >= oldest.as_str() && n.as_str() <= today.to_string().as_str()).collect();
    days.sort();
    days.into_iter().map(|d| format!("{root}/{d}")).collect()
}

/// This hour's Trends step: a new pull when `everyHours` passed, else the latest one on disk.
pub fn hourly(config: &WatchConfig, state_dir: &str, state: &mut WatchState, now: DateTime<FixedOffset>, dry_run: bool, errors: &mut Vec<String>) -> Option<Pull> {
    let cfg = config.trends.as_ref()?;
    let latest = format!("{state_dir}/trends/latest.json");
    let lens = Lens::new(config);
    let age = state.last_trends_at.as_deref().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map_or(f64::INFINITY, |t| (now - t).num_minutes() as f64 / 60.0);
    if age < cfg.every_hours {
        let mut pull: Pull = read_json(&latest).ok().and_then(|v| serde_json::from_value(v).ok())?;
        if let Some(sw) = &cfg.similarweb && !pull.rising.iter().any(|r| r.source.contains("similarweb:"))
            && let Ok(snap) = read_json(&format!("{state_dir}/trends/similarweb.json")) {
                let tdir = format!("{state_dir}/trends/{}", now.format("%Y-%m-%dT%H"));
                let _ = std::fs::create_dir_all(&tdir);
                super::watch_trends_similarweb::merge_rises(&mut pull.rising, super::watch_trends_similarweb::extract(&snap, sw, &lens));
                if let Some(surge) = &cfg.surge {
                    enrich(&mut pull, surge, cfg, &tdir, state_dir, now.date_naive(), &labels(state_dir));
                }
                if !dry_run && let Ok(t) = stringify(&pull) { let _ = write_text(&latest, &t); }
            }
        return Some(pull);
    }
    let dir = format!("{state_dir}/trends/{}", now.format("%Y-%m-%dT%H"));
    if let Err(e) = std::fs::create_dir_all(&dir) { errors.push(format!("trends: {e}")); return None; }
    let seed_dir = state.last_discover_day.as_ref().map(|d| format!("{state_dir}/discover/{d}"));
    let mut pull = pull(cfg, &lens, &dir, seed_dir.as_deref(), &week_of_seed_pulls(state_dir, now.date_naive()), &now.to_rfc3339());
    if let Some(sw) = &cfg.similarweb {
        super::watch_trends_similarweb::collect(&mut pull, sw, &lens, &dir, state_dir);
    }
    if let Some(surge) = &cfg.surge {
        enrich(&mut pull, surge, cfg, &dir, state_dir, now.date_naive(), &labels(state_dir));
    }
    errors.extend(pull.errors.iter().cloned());
    if !dry_run {
        let mut known = ledger(state_dir);
        record(&mut known, &pull);
        for (path, text) in [(latest, stringify(&pull)), (format!("{state_dir}/trends/anomalies.json"), stringify(&known))] {
            if let Err(e) = text.and_then(|t| write_text(&path, &t)) { errors.push(format!("trends: {e}")); }
        }
        state.last_trends_at = Some(now.to_rfc3339());
    }
    Some(pull)
}
