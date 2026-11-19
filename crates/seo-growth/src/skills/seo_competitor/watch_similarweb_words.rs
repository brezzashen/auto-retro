//! Similarweb competitor keywords in the daily keyword room (report `gefei-keyword-method-20261110`):
//! a keyword whose KD shows "-" and whose clicks are new or surged in the last 28 days is a new word
//! (G9, uwnnxmenn1: "KD 为"-"、且28天内流量激增"), and KD and CPC are read as fuzzy values across
//! sources, extremes dropped before averaging (G10, hx51i9h2nt). The snapshot is the one the hourly
//! Trends pull last wrote (`<state>/trends/<hour>/similarweb.json`). Competitor brand misspellings are most of
//! those rows (35 of 131 on 2026-11-10 came from attio.com, "atttio", "attea crm"), so they are skipped first.

use super::files::{at, items, num_or, read_json, text};
use super::watch_discover::Candidate;
use super::watch_trends_similarweb::has_brand_typo;
use serde_json::Value;
use std::collections::BTreeMap;

/// Click change (1.0 = +100%) from which a keyword with no KD counts as surging.
const SURGE_CHANGE: f64 = 1.0;
const MAX_WORDS: usize = 60;

/// The newest Similarweb snapshot under `<state>/trends`.
pub fn latest(state_dir: &str) -> Option<Value> {
    let root = format!("{state_dir}/trends");
    let mut hours: Vec<String> = std::fs::read_dir(&root).ok()?.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
    hours.sort();
    hours.iter().rev().find_map(|h| read_json(&format!("{root}/{h}/similarweb.json")).ok())
}

fn records(snapshot: &Value) -> Vec<(String, &Value)> {
    at(snapshot, &["domains"]).as_object().map(|m| m.iter()
        .flat_map(|(d, v)| items(at(v, &["records"])).iter().map(move |r| (d.clone(), r))).collect()).unwrap_or_default()
}

/// G9: `(keyword, "similarweb-new:<domain>")` for every competitor keyword without a KD whose clicks are
/// new or at least doubled.
pub fn new_words(snapshot: &Value) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (domain, r) in records(snapshot) {
        let k = text(&r["keyword"]).to_lowercase();
        let surged = text(&r["changeState"]) == "New" || r["change"].as_f64().is_some_and(|c| c >= SURGE_CHANGE);
        if !k.is_empty() && !has_brand_typo(&k, &domain) && r["kd"].is_null() && surged && num_or(&r["clicks"], 0.0) > 0.0 && !out.iter().any(|(w, _)| *w == k) {
            out.push((k, format!("similarweb-new:{domain}")));
        }
    }
    out.truncate(MAX_WORDS);
    out
}

/// The fuzzy value of one metric across sources: the mean once the lowest and highest are dropped when
/// there are three or more, else the plain mean.
pub fn fuzzy(values: &[f64]) -> Option<f64> {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    v.sort_by(f64::total_cmp);
    let kept = if v.len() >= 3 { &v[1..v.len() - 1] } else { &v[..] };
    (!kept.is_empty()).then(|| (kept.iter().sum::<f64>() / kept.len() as f64 * 100.0).round() / 100.0)
}

/// G10: KD and CPC of every candidate Similarweb also measured, as the fuzzy value of both sources.
pub fn blend(candidates: &mut [Candidate], snapshot: &Value) {
    let mut seen: BTreeMap<String, (Vec<f64>, Vec<f64>)> = BTreeMap::new();
    for (_, r) in records(snapshot) {
        let e = seen.entry(text(&r["keyword"]).to_lowercase()).or_default();
        if let Some(kd) = r["kd"].as_f64() { e.0.push(kd); }
        if let Some(cpc) = r["cpc"].as_f64().filter(|c| *c > 0.0) { e.1.push(cpc); }
    }
    for c in candidates.iter_mut() {
        let Some((kds, cpcs)) = seen.get(&c.keyword) else { continue };
        if !kds.is_empty() { c.kd = fuzzy(&c.kd.into_iter().chain(kds.iter().copied()).collect::<Vec<_>>()); }
        if !cpcs.is_empty() { c.cpc = fuzzy(&std::iter::once(c.cpc).filter(|x| *x > 0.0).chain(cpcs.iter().copied()).collect::<Vec<_>>()).unwrap_or(c.cpc); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot() -> Value {
        // Record shape of `<state>/trends/2026-11-09T18/similarweb.json`.
        json!({"domains": {"rizz.app": {"records": [
            {"keyword": "AI Dating Assistant App", "clicks": 130, "change": null, "changeState": "New", "kd": null, "cpc": 0},
            {"keyword": "rizz app", "clicks": 900, "change": 0.2, "changeState": "Positive", "kd": null, "cpc": 1.2},
            {"keyword": "rizz pickup lines", "clicks": 300, "change": null, "changeState": "New", "kd": null, "cpc": 0},
            {"keyword": "text reply ai", "clicks": 300, "change": 2.5, "changeState": "Positive", "kd": null, "cpc": 0},
            {"keyword": "pickup lines", "clicks": 800, "change": 3.0, "changeState": "Positive", "kd": 41, "cpc": 0.4}]},
            "folk.app": {"records": [{"keyword": "personal crm", "clicks": 50, "change": 0.1, "changeState": "Positive", "kd": 30, "cpc": 9.0}]}}})
    }

    #[test]
    fn a_keyword_without_kd_whose_clicks_are_new_or_doubled_is_a_new_word() {
        let words: Vec<String> = new_words(&snapshot()).into_iter().map(|(k, s)| format!("{k} {s}")).collect();
        assert_eq!(words, ["ai dating assistant app similarweb-new:rizz.app", "text reply ai similarweb-new:rizz.app"]);
    }

    #[test]
    fn kd_and_cpc_are_fuzzy_across_sources() {
        assert_eq!(fuzzy(&[10.0, 30.0, 32.0, 90.0]), Some(31.0), "extremes dropped");
        assert_eq!((fuzzy(&[11.0, 30.0]), fuzzy(&[])), (Some(20.5), None));
        let mut cs = vec![Candidate { keyword: "personal crm".into(), kd: Some(11.0), cpc: 15.2, ..Default::default() },
            Candidate { keyword: "other".into(), kd: Some(5.0), ..Default::default() }];
        blend(&mut cs, &snapshot());
        assert_eq!((cs[0].kd, cs[0].cpc, cs[1].kd), (Some(20.5), 12.1, Some(5.0)));
    }
}
