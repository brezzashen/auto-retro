//! `mars-skill-tools seo-competitor cpcPool --run <run_dir> [--state <dir>] [--config <file>] [--dry-run]`: updates the
//! keyword pool from the Google Ads tables of one finished watchdog run (its `analysis.json` `googleAds`, or
//! `<run_dir>/ads/ads.json`) without the rest of the hourly watch, previews new proposal files in Ads (never applies them) and prints the `cpcPool` summary. The hourly
//! watch runs the same update itself; this is for a manual refresh or a rerun after a fix.

use super::files::{read_json, stringify};
use chrono::Utc;
use serde_json::Value;

const USAGE: &str = "Usage: mars-skill-tools seo-competitor cpcPool --run <run_dir> [--state <dir>] [--config <file>] [--dry-run]";
const DEFAULT_CONFIG: &str = ".claude/skills/seo-competitor/references/watch.config.json";

fn arg(argv: &[String], name: &str) -> Option<String> {
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).cloned()
}

/// The run's Ads tables: the newer `ads/ads.json` when present (a rerun of the reader), else the analysis block.
fn ads(run_dir: &str) -> Option<Value> {
    read_json(&format!("{run_dir}/ads/ads.json")).ok()
        .or_else(|| read_json(&format!("{run_dir}/analysis.json")).ok().map(|a| a["googleAds"].clone()).filter(|v| !v.is_null()))
}

pub fn main(argv: &[String]) -> i32 {
    if argv.iter().any(|a| a == "-h" || a == "--help") { println!("{USAGE}"); return 0; }
    let Some(run_dir) = arg(argv, "--run") else { eprintln!("missing --run\n{USAGE}"); return 2 };
    let home = std::env::var("HOME").unwrap_or_default();
    let state_dir = arg(argv, "--state").unwrap_or_else(|| format!("{home}/.local/state/seo-watch"));
    let config = match super::watch_config::read_config(&arg(argv, "--config").unwrap_or_else(|| DEFAULT_CONFIG.into())) {
        Ok(c) => c,
        Err(error) => { eprintln!("cpcPool: {error}"); return 1; }
    };
    let Some(pool) = config.google_ads.as_ref().and_then(|a| a.pool.as_ref()) else { eprintln!("cpcPool: googleAds.pool is not configured"); return 1 };
    let sgt = chrono::FixedOffset::east_opt(8 * 3600).expect("valid offset");
    let mut errors = Vec::new();
    let (now, dry_run) = (Utc::now().with_timezone(&sgt), argv.iter().any(|a| a == "--dry-run"));
    let mut summary = super::cpc_pool_view::run(Some(pool), ads(&run_dir).as_ref(), &state_dir, now, dry_run, &mut errors);
    if let Some(s) = summary.as_mut() { super::cpc_pool_preview::run(s, &state_dir, &run_dir, &now.to_rfc3339(), super::cpc_pool_preview::Reader::Manual, dry_run, &mut errors); }
    println!("{}", stringify(&serde_json::json!({"cpcPool": summary, "errors": errors})).unwrap_or_default());
    if errors.is_empty() { 0 } else { 1 }
}
