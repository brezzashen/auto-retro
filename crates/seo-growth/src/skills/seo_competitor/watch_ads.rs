//! Google Ads numbers of every watchdog run (block `googleAds` of `analysis.json`) for the hourly retro's bid and
//! results review (founder 2026-11-18). The Ads script `.claude/skills/seo-competitor/scripts/ads-hourly.gs.js`,
//! scheduled hourly in the account, writes its report tabs to one spreadsheet, read through `scripts/ads-sheet.js`;
//! until that script is authorized (`sheetId` unset) `scripts/ads-ui.js` reads the Ads UI tables instead. Both run
//! on a 9223 browser lease, which carries the signed-in Google session.

use super::files::read_json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::process::Command;

const RUNNER: &str = ".claude/skills/agent-browser/scripts/lease-page.js";
const SHEET_STEPS: &str = ".claude/skills/seo-competitor/scripts/ads-sheet.js";
const UI_STEPS: &str = ".claude/skills/seo-competitor/scripts/ads-ui.js";

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AdsConfig {
    /// Customer id shown in the retro, `594-270-3856`.
    pub account: String,
    /// The spreadsheet the Ads script writes; unset until the script is authorized and has run.
    #[serde(default)]
    pub sheet_id: Option<String>,
    /// The keyword candidate pool (`cpc_pool`); absent turns it off.
    #[serde(default)]
    pub pool: Option<super::cpc_pool::PoolConfig>,
}

/// The step file that reads the numbers: the script's spreadsheet when known, else the Ads UI tables.
pub fn reader(config: &AdsConfig) -> &'static str {
    if sheet(config).is_some() { SHEET_STEPS } else { UI_STEPS }
}

fn sheet(config: &AdsConfig) -> Option<&str> {
    config.sheet_id.as_deref().filter(|s| !s.is_empty())
}

/// The UI reader reads search terms before keywords when the pool last saw them over 6 hours ago (or never).
pub fn terms_due(state_dir: &str, now: chrono::DateTime<chrono::FixedOffset>) -> bool {
    let last = super::cpc_pool::read(state_dir).last_terms_read;
    last.as_deref().and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok()).is_none_or(|at| now - at > chrono::Duration::hours(6))
}

/// Reads the numbers once; `None` when the config has no `googleAds` block.
pub fn collect(config: Option<&AdsConfig>, run_dir: &str, terms_first: bool, dry_run: bool, errors: &mut Vec<String>) -> Option<Value> {
    let config = config?;
    if dry_run { return Some(json!({"account": config.account, "skipped": "dry run"})); }
    let out = format!("{run_dir}/ads");
    let read = std::fs::create_dir_all(&out).map_err(|e| e.to_string()).and_then(|_| run_reader(config, &out, terms_first));
    match read {
        Ok(mut value) => {
            value["account"] = json!(config.account);
            Some(value)
        }
        Err(error) => {
            errors.push(format!("ads: {error}"));
            Some(json!({"account": config.account, "sheetId": sheet(config), "error": error}))
        }
    }
}

fn run_reader(config: &AdsConfig, out: &str, terms_first: bool) -> Result<Value, String> {
    let steps = reader(config);
    let status = Command::new("bun")
        .args([RUNNER, "--label", "seo-ads", "--out", out, "--profile", "9223", "--timeout", "60000", "--budget", "420000", steps])
        .env("ADS_SHEET_ID", sheet(config).unwrap_or_default())
        .env("ADS_TERMS_FIRST", if terms_first { "1" } else { "0" })
        .output()
        .map_err(|e| format!("bun: {e}"))?;
    let saved = read_json(&format!("{out}/ads.json"));
    if !status.status.success() {
        let tail = String::from_utf8_lossy(&status.stderr).lines().rev().take(3).collect::<Vec<_>>().join(" | ");
        return partial(saved, format!("{steps} exited {}: {tail}", status.status));
    }
    saved
}

/// The UI reader saves after every table, so a run cut short still returns the tables it finished, marked `partial`.
fn partial(saved: Result<Value, String>, error: String) -> Result<Value, String> {
    match saved {
        Ok(mut value) if !value["tabs"].as_object().is_none_or(|t| t.is_empty()) => { value["partial"] = json!(error); Ok(value) }
        _ => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ui_reader_stands_in_until_the_script_sheet_is_known() {
        let mut config = AdsConfig { account: "594-270-3856".into(), sheet_id: None, pool: None };
        assert_eq!(reader(&config), UI_STEPS);
        config.sheet_id = Some(String::new());
        assert_eq!(reader(&config), UI_STEPS);
        config.sheet_id = Some("abc".into());
        assert_eq!(reader(&config), SHEET_STEPS);
        assert!(collect(None, "/nonexistent", false, false, &mut Vec::new()).is_none());
    }

    #[test]
    fn a_reader_cut_short_keeps_the_tables_it_saved() {
        let saved = json!({"source": "ui", "tabs": {"keywords_today": [{"keyword_text": "[camcard]"}]}, "failed": {"pending": "search terms"}});
        let kept = partial(Ok(saved), "budget spent".into()).unwrap();
        assert_eq!((kept["partial"].as_str(), kept["tabs"]["keywords_today"][0]["keyword_text"].as_str()), (Some("budget spent"), Some("[camcard]")));
        assert_eq!(partial(Ok(json!({"tabs": {}})), "x".into()), Err("x".to_string()));
        assert_eq!(partial(Err("no file".into()), "x".into()), Err("x".to_string()));
    }

    #[test]
    fn a_dry_run_reads_nothing() {
        let config = AdsConfig { account: "594-270-3856".into(), sheet_id: None, pool: None };
        let mut errors = Vec::new();
        assert_eq!(collect(Some(&config), "/nonexistent", false, true, &mut errors).unwrap()["skipped"], "dry run");
        assert!(errors.is_empty());
    }
}
