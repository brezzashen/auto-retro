//! `.claude/skills/seo-competitor/scripts/collect.ts`: collects every API input of a competitor report
//! into `<run>/raw`, reusing the files already there. `collect <run-dir>`.

use super::config::{RunConfig, load_config, raw_dir};
use super::files::{at, items, stringify, text, write_text};
use super::gsc::collect_gsc;
use super::monid::{MonidCall, first, run_all, slug};
use crate::skills::similarweb::{client, overview};
use serde_json::{Value, json};

const CONCURRENCY: usize = 8;

fn dfs(endpoint: &str, body: Value, out: String) -> MonidCall {
    MonidCall { provider: "dataforseo".to_string(), endpoint: endpoint.to_string(), body, out }
}

/// Target first, then every competitor, without repeats.
fn domains(c: &RunConfig) -> Vec<String> {
    std::iter::once(c.target.domain.clone()).chain(c.competitors.iter().map(|x| x.domain.clone())).collect()
}

/// Ranked keywords, history, backlinks, for every site.
fn site_calls(c: &RunConfig, raw: &str) -> Vec<MonidCall> {
    let sites = domains(c);
    let mut calls: Vec<MonidCall> = sites
        .iter()
        .map(|d| {
            let body = json!({
                "target": d, "location_code": c.market.location_code, "language_code": c.market.language_code,
                "limit": 200, "item_types": ["organic"], "order_by": ["ranked_serp_element.serp_item.etv,desc"],
            });
            dfs("/labs/ranked-keywords", body, format!("{raw}/rk_{d}.json"))
        })
        .collect();
    let history_body = json!({
        "targets": sites, "location_code": c.market.location_code, "language_code": c.market.language_code,
        "date_from": c.history_from, "date_to": c.history_to, "item_types": ["organic"],
    });
    calls.push(dfs("/labs/historical-bulk-traffic-estimation", history_body, format!("{raw}/hist.json")));
    calls.push(dfs("/backlinks/bulk-referring-domains", json!({"targets": sites}), format!("{raw}/bl_refdom.json")));
    calls.push(dfs("/backlinks/bulk-ranks", json!({"targets": sites, "rank_scale": "one_hundred"}), format!("{raw}/bl_ranks.json")));
    calls
}

/// Similarweb overview of every site from the dash subscription, over the three months ending at
/// `historyTo`'s month (or the nearest window the subscription allows). A file already there is kept.
fn collect_similarweb(c: &RunConfig, raw: &str) -> Result<(), String> {
    let mut window = client::default_window(None, Some(&c.history_to));
    std::fs::create_dir_all(format!("{raw}/sw")).map_err(|e| format!("{raw}/sw: {e}"))?;
    for d in domains(c) {
        let out = format!("{raw}/sw/sw_{d}.json");
        if std::path::Path::new(&out).exists() {
            continue;
        }
        let site = client::in_allowed_window(&mut window, |from, to| overview::fetch_overview(&d, 999, from, to, true))?;
        write_text(&out, &stringify(&overview::site_snapshot(&site))?)?;
    }
    Ok(())
}

/// Keyword overview, and the SERP of every target keyword.
fn keyword_calls(c: &RunConfig, raw: &str) -> Vec<MonidCall> {
    let overview_body = json!({"keywords": c.target_keywords, "location_code": c.market.location_code, "language_code": c.market.language_code});
    let mut calls = vec![dfs("/labs/keyword-overview", overview_body, format!("{raw}/kw_overview.json"))];
    for k in &c.target_keywords {
        let body = json!({"keyword": k, "location_code": c.market.location_code, "language_code": c.market.language_code, "depth": 10});
        calls.push(dfs("/serp/google-organic", body, format!("{raw}/serp/{}.json", slug(k))));
    }
    calls
}

/// The target plus every domain in the first ten organic results of its keyword SERPs, read from
/// the cached SERP files.
fn serp_domains(c: &RunConfig, raw: &str) -> Result<Vec<String>, String> {
    let mut set: Vec<String> = vec![c.target.domain.clone()];
    for k in &c.target_keywords {
        let path = format!("{raw}/serp/{}.json", slug(k));
        let serp = first(&super::files::read_json(&path)?).clone();
        let organic = items(at(&serp, &["items"])).iter().filter(|i| text(at(i, &["type"])) == "organic").take(10);
        for item in organic {
            let domain = super::files::serp_domain(item);
            if !set.contains(&domain) {
                set.push(domain);
            }
        }
    }
    Ok(set)
}

fn run(run_dir: &str) -> Result<String, String> {
    let (_raw_config, config) = load_config(run_dir)?;
    let raw = raw_dir(run_dir);
    let mut calls = site_calls(&config, &raw);
    calls.extend(keyword_calls(&config, &raw));
    run_all(calls, CONCURRENCY)?;
    collect_similarweb(&config, &raw)?;
    let rank_targets = serp_domains(&config, &raw)?;
    run_all(vec![dfs("/backlinks/bulk-ranks", json!({"targets": rank_targets, "rank_scale": "one_hundred"}), format!("{raw}/serp/ranks.json"))], CONCURRENCY)?;
    collect_gsc(&config, &raw)?;
    let summary = json!({"raw": raw, "competitors": config.competitors.len(), "keywords": config.target_keywords.len()});
    stringify(&summary)
}

/// `bun collect.ts <run-dir>`.
pub fn main(argv: &[String]) -> i32 {
    let Some(run_dir) = argv.first().filter(|d| !d.is_empty()) else {
        eprintln!("usage: bun collect.ts <run-dir>");
        return 2;
    };
    match run(run_dir) {
        Ok(line) => {
            println!("{line}");
            0
        }
        Err(message) => super::files::fail(&message),
    }
}
