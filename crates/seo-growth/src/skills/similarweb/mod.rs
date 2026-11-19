//! `similarweb` CLI and `mars-skill-tools similarweb <subcommand>`: fast, browserless Similarweb Pro
//! queries (`status`, `sync-cookies`, `overview`, `keywords`, `anomalies`) over HTTP using the
//! `.similarweb.com` cookies in `~/.local/state/similarweb/cookies.json`.

pub mod client;
pub mod overview;

use serde_json::{Value, json};

pub const SCRIPTS: &[&str] = &["status", "sync-cookies", "overview", "keywords", "anomalies"];
const DEFAULT_DOMAINS: &[&str] = &["attio.com", "folk.app", "getdex.com", "cloze.com", "rizz.app", "crystalknows.com"];

fn arg(argv: &[String], flag: &str) -> Option<String> {
    argv.iter().position(|a| a == flag).and_then(|i| argv.get(i + 1)).cloned()
}

fn has(argv: &[String], flag: &str) -> bool { argv.iter().any(|a| a == flag) }

fn positional(argv: &[String]) -> Option<String> {
    let mut skip = false;
    for a in argv {
        if skip { skip = false; continue; }
        if matches!(a.as_str(), "--node" | "--from" | "--to" | "--country" | "--source" | "--sort" | "--page" | "--limit" | "--min-change" | "--min-volume" | "--max-per-domain" | "--out" | "--window") {
            skip = true;
            continue;
        }
        if !a.starts_with('-') { return Some(a.clone()); }
    }
    None
}

fn usage() -> i32 {
    eprintln!(
        "usage: similarweb <subcommand> [args...]\n\
         subcommands:\n  \
           status [--no-sync]\n  \
           sync-cookies [--node 3..16]\n  \
           overview <domain> [--from YYYY-MM] [--to YYYY-MM] [--country 999]\n  \
           keywords <domain> [--window 3m|28d] [--source organic|paid|total] [--non-brand|--branded|--all-brands] [--sort Share|Clicks|Change|KwVolume] [--page 1] [--limit 100]\n  \
           anomalies [domain1,domain2,...] [--min-change 50] [--min-volume 100] [--max-per-domain 8] [--out <file>]"
    );
    2
}

pub fn simplify_record(r: &Value) -> Value {
    json!({
        "keyword": r["Keyword"],
        "clicks": r["Clicks"],
        "share": r["Share"],
        "change": r["Change"],
        "changeState": r["ChangeState"],
        "changePresentation": r["ClicksChangePresentation"],
        "volume": if r["KwVolume"].is_null() { &r["KwWindowVolume"] } else { &r["KwVolume"] },
        "volumeAverage": r["KwVolumeAverage"],
        "cpc": r["Cpc"],
        "kd": r["Difficulty"],
        "intent": r["PrimaryIntent"],
        "zeroClickShare": r["ZeroClicksShare"],
        "topUrl": r["TopUrl"],
        "serpFeatures": r["SerpFeatures"],
    })
}

/// Fetches the non-brand organic keywords of `domains` over the last 28 days, whose `changeState` and
/// `change` compare with the 28 days before, as the snapshot `watch_trends_similarweb` reads.
pub fn collect_snapshot(domains: &[String], page_size: usize, auto_sync: bool) -> Result<Value, String> {
    let mut window = client::window_28d();
    let mut map = serde_json::Map::new();
    for domain in domains {
        let raw = client::in_allowed_window(&mut window, |from, to| client::fetch_keywords(domain, "organic", false, true, "Share", 1, page_size, 999, from, to, true, auto_sync))?;
        let records: Vec<Value> = raw["Data"]["Records"].as_array().map(|a| a.iter().map(simplify_record).collect()).unwrap_or_default();
        map.insert(domain.clone(), json!({
            "totalCount": raw["TotalCount"],
            "overallClicks": raw["Data"]["OverallClicks"],
            "recordCount": records.len(),
            "records": records,
        }));
    }
    Ok(json!({
        "collectedAt": chrono::Utc::now().to_rfc3339(),
        "mode": "http-cookie-cli",
        "portal": "https://pro.similarweb.com",
        "origin": "https://pro.similarweb.com",
        "window": "28d",
        "from": window.0.replace("%7C", "-"),
        "to": window.1.replace("%7C", "-"),
        "country": 999,
        "domains": map,
    }))
}

fn run(script: &str, argv: &[String]) -> Result<Value, String> {
    let auto_sync = !has(argv, "--no-sync");
    let country = arg(argv, "--country").and_then(|s| s.parse().ok()).unwrap_or(999);
    let (from, to) = client::default_window(arg(argv, "--from").as_deref(), arg(argv, "--to").as_deref());
    match script {
        "status" => client::fetch_status(auto_sync),
        "sync-cookies" => client::sync_cookies(arg(argv, "--node").and_then(|s| s.parse().ok())),
        "overview" => {
            let domain = positional(argv).ok_or_else(|| "overview requires <domain>".to_string())?;
            client::in_allowed_window(&mut (from, to), |from, to| overview::fetch_overview(&domain, country, from, to, auto_sync))
        }
        "keywords" => {
            let domain = positional(argv).ok_or_else(|| "keywords requires <domain>".to_string())?;
            let source = arg(argv, "--source").unwrap_or_else(|| "organic".into()).to_lowercase();
            let (branded, non_branded) = if has(argv, "--all-brands") { (true, true) } else if has(argv, "--branded") { (true, false) } else { (false, true) };
            let sort = arg(argv, "--sort").unwrap_or_else(|| "Share".into());
            let page = arg(argv, "--page").and_then(|s| s.parse().ok()).unwrap_or(1);
            let limit = arg(argv, "--limit").and_then(|s| s.parse().ok()).unwrap_or(100);
            let rolling = arg(argv, "--window").as_deref() == Some("28d");
            let mut window = if rolling { client::window_28d() } else { (from, to) };
            let raw = client::in_allowed_window(&mut window, |from, to| client::fetch_keywords(&domain, &source, branded, non_branded, &sort, page, limit, country, from, to, rolling, auto_sync))?;
            let records: Vec<Value> = raw["Data"]["Records"].as_array().map(|a| a.iter().map(simplify_record).collect()).unwrap_or_default();
            Ok(json!({
                "domain": domain,
                "source": source,
                "nonBrandedOnly": non_branded && !branded,
                "from": window.0.replace("%7C", "-"),
                "to": window.1.replace("%7C", "-"),
                "totalCount": raw["TotalCount"],
                "overallClicks": raw["Data"]["OverallClicks"],
                "recordCount": records.len(),
                "records": records,
            }))
        }
        "anomalies" => {
            let domains: Vec<String> = positional(argv)
                .map(|s| s.split(',').map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect())
                .unwrap_or_else(|| DEFAULT_DOMAINS.iter().map(|s| s.to_string()).collect());
            let min_change = arg(argv, "--min-change").and_then(|s| s.parse().ok()).unwrap_or(50.0);
            let min_volume = arg(argv, "--min-volume").and_then(|s| s.parse().ok()).unwrap_or(100.0);
            let max_per_domain = arg(argv, "--max-per-domain").and_then(|s| s.parse().ok()).unwrap_or(8);
            let snap = collect_snapshot(&domains, 100, auto_sync)?;
            if let Some(out) = arg(argv, "--out") {
                if let Some(parent) = std::path::Path::new(&out).parent() { let _ = std::fs::create_dir_all(parent); }
                std::fs::write(&out, serde_json::to_string_pretty(&snap).unwrap_or_default()).map_err(|e| e.to_string())?;
            }
            let cfg = super::seo_competitor::watch_trends_similarweb::SimilarwebConfig {
                domains, min_change, min_volume, max_per_domain,
            };
            let lens = super::seo_competitor::watch_trends::Lens {
                seeds: Vec::new(), targets: Vec::new(), brands: Vec::new(),
            };
            let rises = super::seo_competitor::watch_trends_similarweb::extract(&snap, &cfg, &lens);
            Ok(json!({
                "collectedAt": snap["collectedAt"],
                "from": snap["from"],
                "to": snap["to"],
                "anomalyCount": rises.len(),
                "anomalies": rises,
            }))
        }
        _ => Err("unknown subcommand".into()),
    }
}

pub fn main(script: &str, argv: &[String]) -> Option<i32> {
    if script == "help" || script == "--help" || script == "-h" { return Some(usage()); }
    if !SCRIPTS.contains(&script) { return None; }
    match run(script, argv) {
        Ok(val) => {
            println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
            Some(0)
        }
        Err(e) => {
            eprintln!("similarweb {script}: {e}");
            Some(1)
        }
    }
}
