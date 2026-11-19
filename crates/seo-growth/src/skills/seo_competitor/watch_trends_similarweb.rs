//! Similarweb competitor keyword anomalies collected via the direct HTTP cookie CLI
//! (`skills::similarweb::collect_snapshot`, backed by `~/.local/state/similarweb/cookies.json` from
//! `https://pro.similarweb.com` / `https://pro.similarweb.com`; founder 2026-11-07: query Similarweb via CLI +
//! cookie instead of visual browser automation).

use super::files::{read_json, stringify, write_text};
use super::watch_trends::{BRAND, Lens, Pull, Rise};
use super::watch_trends_surge::{NEW, RISING};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// Apparent competitor brand misspellings that slip past Similarweb's non-brand filter.
const BRAND_TYPOS: &[&str] = &[
    "attio", "atttio", "atai", "aitto", "attea", "attrio", "atrio", "atio", "atterio", "atteo",
    "atti ", "getdex", "dec crm", "golk", "cloze", "clozze", "rizz", "wrizz", "rizxz", "crystal",
];

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SimilarwebConfig {
    pub domains: Vec<String>,
    /// Minimum positive percentage change for a non-`NEW` row to count as rising.
    pub min_change: f64,
    /// Minimum Similarweb search volume over the snapshot window for a row to enter the anomaly list.
    pub min_volume: f64,
    pub max_per_domain: usize,
}

pub fn has_brand_typo(query: &str, domain: &str) -> bool {
    let stem = domain.split('.').next().unwrap_or_default().to_lowercase();
    query.split_whitespace().any(|w| w == stem || w == "dex" || w == "folk")
        || BRAND_TYPOS.iter().any(|t| query.contains(t))
}

fn parse_api_record(rec: &Value, domain: &str, cfg: &SimilarwebConfig, lens: &Lens) -> Option<Rise> {
    let query = rec["keyword"].as_str().or_else(|| rec["Keyword"].as_str())?.trim().to_lowercase();
    if query.is_empty() || has_brand_typo(&query, domain) { return None; }
    let intent = rec["intent"].as_str().or_else(|| rec["PrimaryIntent"].as_str()).unwrap_or_default();
    if intent.eq_ignore_ascii_case("navigational") || intent.eq_ignore_ascii_case("nav") { return None; }
    let (kind, page) = lens.kind(&query);
    if kind == BRAND { return None; }
    let vol = rec["volume"].as_f64().or_else(|| rec["KwVolume"].as_f64())?;
    if vol < cfg.min_volume { return None; }
    let state = rec["changeState"].as_str().or_else(|| rec["ChangeState"].as_str()).unwrap_or_default();
    let (value, flag) = if state.eq_ignore_ascii_case("new") {
        (5000.0, NEW.to_string())
    } else if state.eq_ignore_ascii_case("positive") {
        let pct = (rec["change"].as_f64().or_else(|| rec["Change"].as_f64())? * 1000.0).round() / 10.0;
        if pct < cfg.min_change { return None; }
        (pct, if pct >= 100.0 { RISING.to_string() } else { String::new() })
    } else {
        return None;
    };
    Some(Rise {
        query, value, source: format!("similarweb:{domain}"), kind: kind.into(), page,
        volume: Some(vol), flag, ..Default::default()
    })
}

/// Extracts rising (`NEW` or `Positive` >= `min_change` %) non-brand, non-navigational competitor
/// keywords from a Similarweb snapshot JSON in click order.
pub fn extract(snapshot: &Value, cfg: &SimilarwebConfig, lens: &Lens) -> Vec<Rise> {
    let Some(domains) = snapshot["domains"].as_object() else { return Vec::new() };
    let mut merged: BTreeMap<String, Rise> = BTreeMap::new();
    for (domain, entry) in domains {
        let Some(records) = entry["records"].as_array() else { continue };
        let kept: Vec<Rise> = records.iter().filter_map(|r| parse_api_record(r, domain, cfg, lens)).collect();
        for r in kept.into_iter().take(cfg.max_per_domain) {
            let e = merged.entry(r.query.clone()).or_insert_with(|| Rise { source: String::new(), ..r.clone() });
            if !e.source.split(", ").any(|s| s == r.source) {
                e.source = if e.source.is_empty() { r.source.clone() } else { format!("{}, {}", e.source, r.source) };
            }
            if r.value > e.value { e.value = r.value; }
            if r.volume > e.volume { e.volume = r.volume; }
            if e.flag.is_empty() && !r.flag.is_empty() { e.flag = r.flag.clone(); }
        }
    }
    let mut out: Vec<Rise> = merged.into_values().collect();
    out.sort_by(|a, b| b.value.total_cmp(&a.value));
    out
}

/// Merges `sw_rises` into `pull.rising` so a query found by both Trends and Similarweb carries both sources.
pub fn merge_rises(rising: &mut Vec<Rise>, sw_rises: Vec<Rise>) {
    for r in sw_rises {
        if let Some(existing) = rising.iter_mut().find(|x| x.query == r.query) {
            if !existing.source.split(", ").any(|s| s == r.source) {
                existing.source = format!("{}, {}", existing.source, r.source);
            }
            if existing.volume.is_none() { existing.volume = r.volume; }
            if existing.flag.is_empty() { existing.flag = r.flag; }
        } else {
            rising.push(r);
        }
    }
}

/// Collects Similarweb competitor keywords over HTTP using cached `.similarweb.com` cookies (auto-syncing on
/// expiry) and merges the risers into `pull`.
pub fn collect(pull: &mut Pull, cfg: &SimilarwebConfig, lens: &Lens, dir: &str, state_dir: &str) {
    let run_file = format!("{dir}/similarweb.json");
    let state_file = format!("{state_dir}/trends/similarweb.json");
    if !std::path::Path::new(&run_file).exists() && !cfg.domains.is_empty() {
        match super::super::similarweb::collect_snapshot(&cfg.domains, 100, true) {
            Ok(snap) => {
                if let Ok(text) = stringify(&snap) {
                    let _ = write_text(&run_file, &text);
                    let _ = write_text(&state_file, &text);
                }
            }
            Err(e) => pull.errors.push(format!("similarweb: {e}")),
        }
    }
    let json = read_json(&run_file).or_else(|_| read_json(&state_file));
    if let Ok(snapshot) = json {
        merge_rises(&mut pull.rising, extract(&snapshot, cfg, lens));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_new_and_rising_non_brand_keywords() {
        let snap = json!({
            "domains": {
                "attio.com": {
                    "records": [
                        {"keyword": "atttio", "clicks": 840, "change": 6.56, "changeState": "Positive", "volume": 1100.0, "intent": "Informational"},
                        {"keyword": "ai crm", "clicks": 500, "change": 1.4167, "changeState": "Positive", "volume": 12690.0, "intent": "Informational"},
                        {"keyword": "odoo", "clicks": 320, "change": 0.0, "changeState": "New", "volume": 922600.0, "intent": "Navigational"},
                        {"keyword": "crm of the future", "clicks": 110, "change": 0.0, "changeState": "New", "volume": 220.0, "intent": "Informational"}
                    ]
                },
                "rizz.app": {
                    "records": [
                        {"keyword": "ai dating assistant app", "clicks": 130, "change": null, "changeState": "New", "volume": 280.0, "intent": null}
                    ]
                }
            }
        });
        let cfg = SimilarwebConfig { domains: vec!["attio.com".into(), "rizz.app".into()], min_change: 50.0, min_volume: 100.0, max_per_domain: 8 };
        let lens = Lens { seeds: Vec::new(), targets: Vec::new(), brands: vec!["hubspot".into()] };
        let got = extract(&snap, &cfg, &lens);
        let queries: Vec<&str> = got.iter().map(|r| r.query.as_str()).collect();
        assert_eq!(queries, vec!["ai dating assistant app", "crm of the future", "ai crm"]);
        assert_eq!(got[0].source, "similarweb:rizz.app");
        assert_eq!(got[0].flag, NEW);
        assert_eq!(got[2].flag, RISING);
        assert_eq!(got[2].volume, Some(12690.0));
    }
}
