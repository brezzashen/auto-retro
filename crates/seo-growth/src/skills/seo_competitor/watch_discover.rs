//! The scout of the daily keyword room (founder 2026-11-04, report `seo-keyword-room-plan-20261104`):
//! DataForSEO related keywords of every landing-page seed, Google Trends rising queries, competitors'
//! new sitemap pages and their ranked keywords, and our own Search Console queries. Every Monid answer
//! is cached under `<state>/discover/`, so a rerun on the same day (or week, for ranked keywords) costs nothing.

use super::files::{at, items, num_or, read_json, stringify, text, write_text};
use super::monid::{MonidCall, first, run_all, slug};
use super::watch_analyze::QueryRow;
use super::watch_config::{DiscoveryConfig, Seed};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const CONCURRENCY: usize = 6;
/// Competitor page phrases looked up per round.
const MAX_PAGE_PHRASES: usize = 40;

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub keyword: String,
    /// `related:<seed>`, `trends:<seed>`, `anomaly:<page>`, `competitor-rank:<domain>`, `competitor-page:<domain>`,
    /// `site-loop:<domain>`, `similarweb-new:<domain>`, `unbrand:<word>` or `gsc`.
    pub source: String,
    /// Page of the seed that found it; empty until the judge maps a competitor or GSC keyword.
    pub page: String,
    /// DataForSEO `search_volume`: the mean of the last 12 months.
    pub volume: f64,
    /// Searches of the latest published month and the mean of the up to 11 months before it; 0 without history.
    pub latest: f64,
    pub base: f64,
    pub kd: Option<f64>,
    pub cpc: f64,
    pub quarterly: f64,
    pub yearly: f64,
    pub intent: String,
    /// Monthly searches scaled from the Trends reference word when the keyword tools show none (G2).
    pub trends_volume: f64,
    /// Days since the word first carried Trends searches; `None` before the probe or for an old word (G1).
    pub age_days: Option<i64>,
    /// Month (1-12) of a yearly Trends peak; 0 when not seasonal (G4).
    pub seasonal: u32,
    /// `YYYY-MM` of the first Google Ads month with searches; a Trends birth after it is a sampling artefact.
    pub first_month: String,
    /// Last-day interest over the peak of a 7-day curve, for a word born within 30 days; 0 when not read.
    pub fade: f64,
    /// Consecutive 4-week blocks of the 5-year Trends curve, back from the latest, each above the one before (G14).
    pub rising_months: u32,
    /// Google Ads competition index (0-1): the share of advertisers bidding on the word; 0 when unknown (G13).
    pub competition: f64,
    /// First-page results that show a price or a pricing sitelink, and app-store listings among them (G13).
    pub price_pages: usize,
    pub app_listings: usize,
}

/// One competitor of the registry.
pub struct Rival { pub domain: String, pub brand: Vec<String> }

pub fn rivals(registry: &str) -> Result<Vec<Rival>, String> {
    let json = read_json(registry)?;
    Ok(items(at(&json, &["groups"])).iter().flat_map(|g| items(at(g, &["competitors"])).iter()).map(|c| Rival {
        domain: text(at(c, &["domain"])).to_string(),
        brand: items(at(c, &["brand"])).iter().map(|b| text(b).to_lowercase()).collect(),
    }).collect())
}

fn dfs(endpoint: &str, body: Value, out: String) -> MonidCall {
    MonidCall { provider: "dataforseo".into(), endpoint: endpoint.into(), body, out }
}

/// Metrics of one DataForSEO Labs item, with or without its `keyword_data` wrapper.
pub fn candidate(item: &Value, source: &str, page: &str) -> Candidate {
    let data = if item.get("keyword_data").is_some() { at(item, &["keyword_data"]) } else { item };
    let trend = |span: &str| num_or(at(data, &["keyword_info", "search_volume_trend", span]), 0.0);
    let (latest, base) = super::watch_signals::months(data);
    Candidate {
        keyword: text(at(data, &["keyword"])).to_lowercase(), source: source.into(), page: page.into(),
        volume: num_or(at(data, &["keyword_info", "search_volume"]), 0.0), latest, base,
        kd: at(data, &["keyword_properties", "keyword_difficulty"]).as_f64(),
        cpc: num_or(at(data, &["keyword_info", "cpc"]), 0.0),
        competition: num_or(at(data, &["keyword_info", "competition"]), 0.0),
        quarterly: trend("quarterly"), yearly: trend("yearly"),
        intent: text(at(data, &["search_intent_info", "main_intent"])).to_string(),
        first_month: super::watch_signals::first_month(at(data, &["keyword_info"])),
        ..Default::default()
    }
}

/// True when a word of `text` shares a stem with one of `stems`: a common prefix covering all but at
/// most one letter of the shorter word ("notes" meets "note", "messaging" meets "message", "messamore" misses it).
/// A stem with a space is a phrase and matches only those words in that order ("high value man").
pub fn shares_stem(text: &str, stems: &[String]) -> bool {
    stems.iter().any(|s| phrase_words(text, s) > 0) || text.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3)
        .any(|w| stems.iter().filter(|s| !s.contains(' ')).any(|s| word_shares_stem(w, s)))
}

fn word_shares_stem(word: &str, stem: &str) -> bool {
    let n = word.chars().zip(stem.chars()).take_while(|(x, y)| x == y).count();
    let short = word.len().min(stem.len());
    stem.len() >= 3 && n >= short.min(4) && n + 1 >= short
}

/// The word count of `phrase` when it is a multi-word stem that `text` contains as whole words, else 0.
pub fn phrase_words(text: &str, phrase: &str) -> usize {
    if !phrase.contains(' ') { return 0; }
    let norm = |t: &str| format!(" {} ", t.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" "));
    if norm(text).contains(&norm(phrase)) { phrase.split_whitespace().count() } else { 0 }
}

/// Rising Google Trends queries of one seed that stay on its topic; Trends mixes in unrelated
/// viral queries ("how to bake a cake" under "personal crm", verified 2026-11-04).
pub fn rising_on_topic(trends: &Value, seed: &Seed) -> Vec<String> {
    items(at(first(trends), &["items"])).iter().flat_map(|i| items(at(i, &["data", "rising"])).iter())
        .map(|q| text(at(q, &["query"])).to_lowercase()).filter(|q| shares_stem(q, &seed.fit)).collect()
}

/// A search phrase from a page URL's last path segment, e.g. `/blog/best-personal-crm/` → "best personal crm".
pub fn page_phrase(url: &str) -> Option<String> {
    let path = url.split_once("://").map_or(url, |(_, r)| r).split(['?', '#']).next()?;
    let last = path.trim_end_matches('/').rsplit('/').next()?.trim_end_matches(".html");
    let words: Vec<&str> = last.split(['-', '_']).filter(|w| !w.is_empty() && !w.chars().all(|c| c.is_ascii_digit())).collect();
    (2..=8).contains(&words.len()).then(|| words.join(" ").to_lowercase())
}

/// What one round of the scout found.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Scout {
    pub candidates: Vec<Candidate>,
    /// Competitor pages that appeared since the last round, `domain → urls`.
    pub new_pages: BTreeMap<String, Vec<String>>,
    pub errors: Vec<String>,
}

/// Our Search Console queries with at least one impression, as `(keyword, "gsc")` lookups.
pub fn gsc_lookups(gsc: &[QueryRow]) -> Vec<(String, String)> {
    gsc.iter().filter(|q| q.impressions >= 1.0).map(|q| (q.query.to_lowercase(), "gsc".to_string())).collect()
}

/// Runs every source for `day` and returns the candidate pool, deduplicated by keyword. `extra` are
/// `(keyword, source)` lookups without metrics: Trends leads judged `机会` (`anomaly:<page>`) and `gsc`.
#[allow(clippy::too_many_arguments)]
pub fn scout(cfg: &DiscoveryConfig, rivals: &[Rival], extra: &[(String, String)], loop_sites: &[String], dir: &str, week_dir: &str, prior_week: Option<&str>, sitemaps: &str) -> Scout {
    let mut out = Scout::default();
    let market = |mut body: Value| { body["location_code"] = json!(cfg.location_code); body["language_code"] = json!(cfg.language_code); body };
    let mut calls: Vec<MonidCall> = cfg.seeds.iter().map(|s| dfs("/labs/related-keywords",
        market(json!({"keyword": s.keyword, "depth": 2, "limit": 300})), format!("{dir}/related_{}.json", slug(&s.keyword)))).collect();
    calls.extend(cfg.seeds.iter().filter(|s| s.trends).map(|s| dfs("/keywords/google-trends-explore",
        json!({"keywords": [s.keyword], "location_code": cfg.location_code, "time_range": "past_90_days", "item_types": ["google_trends_queries_list"]}),
        format!("{dir}/trends_{}.json", slug(&s.keyword)))));
    calls.extend(rivals.iter().map(|r| dfs("/labs/ranked-keywords", market(json!({"target": r.domain, "limit": 200,
        "item_types": ["organic"], "order_by": ["ranked_serp_element.serp_item.etv,desc"]})), format!("{week_dir}/rank_{}.json", r.domain))));
    calls.extend(rivals.iter().map(|r| MonidCall { provider: "search1api".into(), endpoint: "/sitemap".into(),
        body: json!({"url": format!("https://{}", r.domain)}), out: format!("{dir}/sitemap_{}.json", r.domain) }));
    calls.extend(super::watch_site_loop::calls(cfg, loop_sites, dir));
    if let Err(e) = run_all(calls, CONCURRENCY) { out.errors.push(format!("scout: {e}")); }

    let mut pool: Vec<Candidate> = Vec::new();
    let mut lookup: Vec<(String, String)> = Vec::new();
    for s in &cfg.seeds {
        if let Ok(json) = read_json(&format!("{dir}/related_{}.json", slug(&s.keyword))) {
            pool.extend(items(at(first(&json), &["items"])).iter().map(|i| candidate(i, &format!("related:{}", s.keyword), &s.page)));
        }
        if let Ok(json) = read_json(&format!("{dir}/trends_{}.json", slug(&s.keyword))) {
            lookup.extend(rising_on_topic(&json, s).into_iter().map(|q| (q, format!("trends:{}", s.keyword))));
        }
    }
    for r in rivals { pool.extend(new_ranked(&r.domain, week_dir, prior_week)); }
    for d in loop_sites { pool.extend(super::watch_site_loop::fresh_words(d, dir)); }
    out.new_pages = new_pages(rivals, dir, sitemaps);
    for (domain, urls) in &out.new_pages {
        lookup.extend(urls.iter().filter_map(|u| page_phrase(u)).take(MAX_PAGE_PHRASES).map(|p| (p, format!("competitor-page:{domain}"))));
    }
    lookup.extend(extra.iter().cloned());
    pool.extend(overview(cfg, dir, &lookup, &mut out.errors));
    let mut seen = BTreeSet::new();
    out.candidates = pool.into_iter().filter(|c| !c.keyword.is_empty() && seen.insert(c.keyword.clone())).collect();
    out
}

/// Keywords a competitor ranks for this week that it did not rank for in the prior week's pull;
/// with no prior week, everything it ranks for in the top 20.
fn new_ranked(domain: &str, week_dir: &str, prior_week: Option<&str>) -> Vec<Candidate> {
    let keywords = |dir: &str| read_json(&format!("{dir}/rank_{domain}.json")).map(|j| items(at(first(&j), &["items"])).to_vec()).unwrap_or_default();
    let before: BTreeSet<String> = prior_week.map(|d| keywords(d).iter().map(|i| candidate(i, "", "").keyword).collect()).unwrap_or_default();
    keywords(week_dir).iter()
        .filter(|i| num_or(at(i, &["ranked_serp_element", "serp_item", "rank_group"]), 100.0) <= 20.0)
        .map(|i| candidate(i, &format!("competitor-rank:{domain}"), ""))
        .filter(|c| !before.contains(&c.keyword)).collect()
}

/// URLs in each competitor's sitemap today that the stored list lacks; the first sighting of a domain only records it.
fn new_pages(rivals: &[Rival], dir: &str, sitemaps: &str) -> BTreeMap<String, Vec<String>> {
    let mut known: BTreeMap<String, Vec<String>> = read_json(sitemaps).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let mut fresh = BTreeMap::new();
    for r in rivals {
        let Ok(json) = read_json(&format!("{dir}/sitemap_{}.json", r.domain)) else { continue };
        let links: Vec<String> = items(at(first(&json), &["links"])).iter().map(|l| text(l).to_string()).filter(|l| !l.is_empty()).collect();
        if links.is_empty() { continue; }
        if let Some(old) = known.get(&r.domain) {
            let old: BTreeSet<&String> = old.iter().collect();
            let added: Vec<String> = links.iter().filter(|l| !old.contains(l)).cloned().collect();
            if !added.is_empty() { fresh.insert(r.domain.clone(), added); }
        }
        known.insert(r.domain.clone(), links);
    }
    if let Ok(text) = stringify(&known) { let _ = write_text(sitemaps, &text); }
    fresh
}

/// Metrics for keywords that arrived without them (Trends, competitor pages, Search Console); an
/// `anomaly:<page>` lead keeps the page its judgment named. A Trends word the Labs database does not
/// know yet stays in the pool with no metrics, so Google Ads and the Trends reference can size it (G2).
fn overview(cfg: &DiscoveryConfig, dir: &str, lookup: &[(String, String)], errors: &mut Vec<String>) -> Vec<Candidate> {
    let source: BTreeMap<&str, &str> = lookup.iter().rev().map(|(k, s)| (k.as_str(), s.as_str())).collect();
    if source.is_empty() { return Vec::new(); }
    let path = format!("{dir}/overview.json");
    // Labs refuses the whole batch when one keyword is longer than 80 characters (rounds of 2026-11-08 and -09).
    let keywords: Vec<&str> = source.keys().copied().filter(|k| k.chars().count() <= 80).take(700).collect();
    let call = dfs("/labs/keyword-overview", json!({"keywords": keywords, "location_code": cfg.location_code, "language_code": cfg.language_code}), path.clone());
    if let Err(e) = run_all(vec![call], 1) { errors.push(format!("overview: {e}")); return Vec::new(); }
    let known: Vec<Candidate> = read_json(&path).map(|j| items(at(first(&j), &["items"])).iter().map(|i| candidate(i, "", "")).collect()).unwrap_or_default();
    with_unknown(known, &source)
}

/// Labs answers with their lookup source, plus every Trends or anomaly word Labs left out, without metrics.
pub fn with_unknown(known: Vec<Candidate>, source: &BTreeMap<&str, &str>) -> Vec<Candidate> {
    let have: BTreeSet<String> = known.iter().map(|c| c.keyword.clone()).collect();
    let unknown = source.iter().filter(|(k, s)| !have.contains(**k) && (s.starts_with("trends:") || s.starts_with("anomaly:") || s.starts_with("similarweb-new:")))
        .map(|(k, _)| Candidate { keyword: k.to_string(), ..Default::default() });
    known.into_iter().chain(unknown).map(|c| {
        let src = source.get(c.keyword.as_str()).copied().unwrap_or("gsc");
        Candidate { source: src.to_string(), page: src.strip_prefix("anomaly:").unwrap_or_default().to_string(), ..c }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed() -> Seed { Seed { keyword: "personal crm".into(), page: "/personal-crm".into(), fit: vec!["crm".into(), "contacts".into()], trends: true, evergreen: None, core_market: false } }

    #[test]
    fn off_topic_rising_queries_are_dropped() {
        let trends = json!([{"items": [{"data": {"rising": [{"query": "how to bake a cake"}, {"query": "Mesh CRM"}, {"query": "best time to visit maldives"}]}}]}]);
        assert_eq!(rising_on_topic(&trends, &seed()), ["mesh crm"]);
        let message = vec!["message".to_string(), "note".to_string()];
        assert!(shares_stem("instant messaging app", &message) && shares_stem("ai notes", &message));
        assert!(!shares_stem("john messamore", &message) && !shares_stem("texas", &["text".to_string()]));
        let man = vec!["high value man".to_string(), "good man".to_string()];
        assert!(shares_stem("signs of a high-value man", &man) && shares_stem("qualities of a good man", &man));
        assert!(!shares_stem("customer lifetime value", &man) && !shares_stem("high value items", &man) && !shares_stem("good manager", &man));
    }

    #[test]
    fn labs_items_read_with_or_without_the_keyword_data_wrapper() {
        let wrapped = json!({"keyword_data": {"keyword": "Personal CRM", "keyword_info": {"search_volume": 880, "cpc": 15.2, "search_volume_trend": {"quarterly": 5, "yearly": -3}},
            "keyword_properties": {"keyword_difficulty": 11}, "search_intent_info": {"main_intent": "commercial"}}});
        let c = candidate(&wrapped, "related:x", "/p");
        assert_eq!((c.keyword.as_str(), c.volume, c.kd, c.quarterly, c.intent.as_str()), ("personal crm", 880.0, Some(11.0), 5.0, "commercial"));
        assert_eq!(candidate(&json!({"keyword": "a b", "keyword_info": {"search_volume": 10}}), "", "").volume, 10.0);
        let months = json!([{"year": 2026, "month": 9, "search_volume": 1000}, {"year": 2026, "month": 8, "search_volume": 100}]);
        let riser = candidate(&json!({"keyword": "r", "keyword_info": {"search_volume": 170, "monthly_searches": months}}), "", "");
        assert_eq!((riser.volume, riser.latest, riser.base), (170.0, 1000.0, 100.0));
    }

    #[test]
    fn a_trends_word_labs_does_not_know_stays_without_metrics() {
        let source: BTreeMap<&str, &str> = [("ai crm", "related:personal crm"), ("vibe crm", "trends:personal crm"), ("crm export csv", "gsc"), ("red flag app", "anomaly:/relationship-red-flags")].into();
        let known = vec![Candidate { keyword: "ai crm".into(), volume: 900.0, ..Default::default() }];
        let all = with_unknown(known, &source);
        let words: Vec<(&str, &str, f64)> = all.iter().map(|c| (c.keyword.as_str(), c.page.as_str(), c.volume)).collect();
        assert_eq!(words, [("ai crm", "", 900.0), ("red flag app", "/relationship-red-flags", 0.0), ("vibe crm", "", 0.0)]);
    }

    #[test]
    fn page_urls_become_phrases_only_when_they_read_like_a_search() {
        assert_eq!(page_phrase("https://getdex.com/blog/best-personal-crm/").as_deref(), Some("best personal crm"));
        assert_eq!(page_phrase("https://getdex.com/pricing/"), None);
        assert_eq!(page_phrase("https://x.com/2024/10/"), None);
    }
}
