//! `.claude/skills/seo-competitor/scripts/config.ts`: the run configuration and its raw directory.

use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Competitor {
    pub domain: String,
    pub name: String,
    /// Lowercase substrings that mark a keyword as this competitor's brand term.
    pub brand: Vec<String>,
    /// `saas` or `consumer`; drives the paid-conversion range in mrrEstimate.
    pub kind: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub domain: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Market {
    pub location_code: i64,
    pub language_code: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Gsc {
    pub property: String,
    pub service_account: String,
    pub caller_account: String,
    pub start_date: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    pub target: Target,
    pub market: Market,
    /// Similarweb snapshot month and DataForSEO history window, YYYY-MM-DD.
    pub history_from: String,
    pub history_to: String,
    pub competitors: Vec<Competitor>,
    pub target_keywords: Vec<String>,
    /// Domain rank the target is expected to reach after the link plan.
    pub planned_rank: f64,
    pub gsc: Option<Gsc>,
}

/// `<run>/raw`, as `RAW(runDir)` builds it (plain concatenation, no normalisation).
pub fn raw_dir(run_dir: &str) -> String {
    format!("{run_dir}/raw")
}

/// `loadConfig(runDir)`: the parsed JSON (kept whole for analysis.json) and its typed view.
pub fn load_config(run_dir: &str) -> Result<(Value, RunConfig), String> {
    let raw: Value = super::files::read_json(&format!("{run_dir}/config.json"))?;
    let config: RunConfig = serde_json::from_value(raw.clone()).map_err(|e| format!("config.json: {e}"))?;
    if config.competitors.is_empty() {
        return Err("config.json: competitors is empty".to_string());
    }
    if config.target_keywords.is_empty() {
        return Err("config.json: targetKeywords is empty".to_string());
    }
    Ok((raw, config))
}
