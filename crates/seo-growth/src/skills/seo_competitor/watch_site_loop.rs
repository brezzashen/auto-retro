//! Site-to-word loop of the keyword room (Gefei, G5: "网站<->词<->更多词<->更多网站", 6beccum67n,
//! qvnqmuvo8p, tspz9aydgq): the SEO sites in the top 3 of today's `做` words are read tomorrow for the
//! keywords they newly rank for (DataForSEO `rank_changes.is_new`, the Labs analogue of Similarweb's
//! "全新点击", pcghg93zo2), which enter the pool as `site-loop:<domain>`. A dropped brand word also leaves its
//! feature words for tomorrow ("把elevenlabs去掉，只看 voice isolator", tbs7ysyug6) as `unbrand:<word>`.

use super::files::{at, is_forum, items, num_or, read_json, stringify, write_text};
use super::monid::{MonidCall, first};
use super::watch_config::DiscoveryConfig;
use super::watch_discover::{Candidate, candidate};
use super::watch_judge::{DO, GIANT_RANK, Judged, is_brand};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Rank a newly ranking keyword of a loop site must reach to count.
const LOOP_TOP: f64 = 20.0;

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
pub struct Loop {
    /// Singapore day of the round that picked the sites.
    pub day: String,
    pub sites: Vec<String>,
    /// `(keyword, source)` lookups for the next round.
    #[serde(default)]
    pub leads: Vec<(String, String)>,
}

pub fn path(root: &str) -> String { format!("{root}/site-loop.json") }

/// What an earlier round left for today; today's own pick waits for tomorrow.
pub fn read(root: &str, today: &str) -> Loop {
    let found: Loop = read_json(&path(root)).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    if found.day.as_str() < today { found } else { Loop::default() }
}

pub fn write(root: &str, next: &Loop) -> Result<(), String> { write_text(&path(root), &stringify(next)?) }

/// The feature words of every dropped brand word that fits a Mars page, when at least two words remain.
pub fn unbranded(judged: &[Judged], brands: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for j in judged.iter().filter(|j| j.reason == "品牌词" && !j.candidate.page.is_empty()) {
        let k = &j.candidate.keyword;
        let rest = brands.iter().filter(|b| is_brand(k, std::slice::from_ref(*b))).fold(format!(" {k} "), |s, b| s.replace(&format!(" {b} "), " "));
        let rest = rest.split_whitespace().collect::<Vec<_>>().join(" ");
        if rest.split(' ').count() >= 2 && rest != *k && !rest.contains("mars") && !out.iter().any(|(w, _)| *w == rest) { out.push((rest, format!("unbrand:{k}"))); }
    }
    out
}

/// One ranked-keywords pull per loop site, cached in the day folder.
pub fn calls(cfg: &DiscoveryConfig, sites: &[String], dir: &str) -> Vec<MonidCall> {
    sites.iter().map(|d| MonidCall { provider: "dataforseo".into(), endpoint: "/labs/ranked-keywords".into(), out: file(dir, d),
        body: json!({"target": d, "limit": 100, "item_types": ["organic"], "location_code": cfg.location_code, "language_code": cfg.language_code,
            "order_by": ["ranked_serp_element.serp_item.etv,desc"]}) }).collect()
}

fn file(dir: &str, domain: &str) -> String { format!("{dir}/loop_{domain}.json") }

/// Keywords a loop site newly ranks for in the top `LOOP_TOP`.
pub fn fresh_words(domain: &str, dir: &str) -> Vec<Candidate> {
    let Ok(json) = read_json(&file(dir, domain)) else { return Vec::new() };
    items(at(first(&json), &["items"])).iter().filter(|i| {
        let serp = at(i, &["ranked_serp_element", "serp_item"]);
        num_or(at(serp, &["rank_group"]), 100.0) <= LOOP_TOP && at(serp, &["rank_changes", "is_new"]).as_bool() == Some(true)
    }).map(|i| candidate(i, &format!("site-loop:{domain}"), "")).collect()
}

/// The SEO sites (no forum post, no giant, not ours) in the top 3 of every word still `做` after the
/// SERP check, first seen first, at most `n`.
pub fn next_sites(judged: &[Judged], ours: &str, n: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for j in judged.iter().filter(|j| j.verdict == DO && j.win_rate.is_some()) {
        for (d, r) in j.top.iter().take(3) {
            let seo = !is_forum(d) && *r < GIANT_RANK && d != ours && d != "google.com";
            if seo && !out.contains(d) && out.len() < n { out.push(d.clone()); }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_three_seo_sites_of_do_words_feed_tomorrow_and_only_new_rankings_return() {
        let j = |verdict: &str, top: &[(&str, f64)]| Judged { verdict: verdict.into(), win_rate: Some(0.6),
            top: top.iter().map(|(d, r)| (d.to_string(), *r)).collect(), ..Default::default() };
        let judged = [j(DO, &[("reddit.com", 0.0), ("forbes.com", 95.0), ("textdecoder.app", 22.0), ("late.io", 10.0)]), j(DO, &[("newsite.app", 0.0)]),
            j(DO, &[("textdecoder.app", 22.0), ("example.com", 18.0), ("flirtcopilot.com", 30.0)]), j("观察", &[("watched.io", 5.0)])];
        assert_eq!(next_sites(&judged, "example.com", 5), ["textdecoder.app", "newsite.app", "flirtcopilot.com"], "a new site without a backlink rank still counts");
        let dir = std::env::temp_dir().join("seo-site-loop-test");
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.to_str().unwrap();
        let item = |k: &str, rank: f64, new: bool| json!({"keyword_data": {"keyword": k, "keyword_info": {"search_volume": 300}},
            "ranked_serp_element": {"serp_item": {"rank_group": rank, "rank_changes": {"is_new": new}}}});
        let rows = json!([{"items": [item("decode his text", 4.0, true), item("old word", 3.0, false), item("deep new word", 45.0, true)]}]);
        write_text(&file(dir, "textdecoder.app"), &rows.to_string()).unwrap();
        let words: Vec<String> = fresh_words("textdecoder.app", dir).into_iter().map(|c| format!("{} {}", c.keyword, c.source)).collect();
        assert_eq!(words, ["decode his text site-loop:textdecoder.app"]);
        write(dir, &Loop { day: "2026-11-10".into(), sites: vec!["textdecoder.app".into()], leads: vec![] }).unwrap();
        assert!(read(dir, "2026-11-10").sites.is_empty(), "today's pick waits for tomorrow");
        assert_eq!(read(dir, "2026-11-11").sites, ["textdecoder.app"]);
    }

    #[test]
    fn a_dropped_brand_word_leaves_its_feature_words_for_tomorrow() {
        let drop = |k: &str| Judged { verdict: "放弃".into(), reason: "品牌词".into(), candidate: Candidate { keyword: k.into(), page: "/ai-meeting-notes".into(), ..Default::default() }, ..Default::default() };
        let brands = vec!["otter".to_string(), "read ai".to_string()];
        let off_topic = Judged { candidate: Candidate { page: String::new(), ..drop("more stories on otter").candidate }, ..drop("x") };
        let judged = [drop("otter ai meeting notes"), drop("read ai meeting summary"), drop("otter login"), drop("otter.ai"), off_topic];
        assert_eq!(unbranded(&judged, &brands), [("ai meeting notes".to_string(), "unbrand:otter ai meeting notes".to_string()),
            ("meeting summary".into(), "unbrand:read ai meeting summary".into())]);
    }
}
