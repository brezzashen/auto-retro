//! `.claude/skills/seo-competitor/scripts/analyze.ts`: turns `<run>/raw` into `<run>/analysis.json`:
//! per-competitor keywords, keyword shares and growth, plus the target keywords with their SERP and
//! the target's win rate. `analyze <run-dir>`.

use super::analyze_gsc::gsc_summary;
use super::analyze_keywords::target_keywords;
use super::analyze_site::{Site, site_of};
use super::config::{load_config, raw_dir};
use super::files::{at, items, num_or, read_json, stringify, text, to_pretty_one, write_text};
use super::monid::first;
use serde_json::Value;
use std::collections::HashMap;

/// The raw inputs analyze reads for every site: rank, referring domains and history by domain.
pub struct Ctx {
    pub raw: String,
    pub ranks: HashMap<String, Value>,
    pub refdom: HashMap<String, Value>,
    pub history: HashMap<String, Vec<Value>>,
}

/// `first(read(path)).items`, which the TypeScript maps without a default (a missing list throws).
pub fn required_items(path: &str) -> Result<Vec<Value>, String> {
    let json = read_json(path)?;
    match at(first(&json), &["items"]).as_array() {
        Some(list) => Ok(list.clone()),
        None => Err(format!("{path}: items is not a list")),
    }
}

/// `(first(read(path)).items ?? [])`.
pub fn items_or_empty(path: &str) -> Result<Vec<Value>, String> {
    let json = read_json(path)?;
    Ok(items(at(first(&json), &["items"])).to_vec())
}

impl Ctx {
    pub fn load(raw: &str) -> Result<Self, String> {
        let mut ranks = HashMap::new();
        for i in required_items(&format!("{raw}/bl_ranks.json"))? {
            ranks.insert(text(at(&i, &["target"])).to_string(), at(&i, &["rank"]).clone());
        }
        let mut refdom = HashMap::new();
        for i in required_items(&format!("{raw}/bl_refdom.json"))? {
            refdom.insert(text(at(&i, &["target"])).to_string(), at(&i, &["referring_domains"]).clone());
        }
        let mut history = HashMap::new();
        for i in required_items(&format!("{raw}/hist.json"))? {
            history.insert(text(at(&i, &["target"])).to_string(), items(at(&i, &["metrics", "organic"])).to_vec());
        }
        Ok(Self { raw: raw.to_string(), ranks, refdom, history })
    }
}

/// `{...a, ...b}`: keys of `a` in order, values of `b` replacing or appended.
fn merge(base: &Value, extra: &Value) -> Value {
    let mut out = base.as_object().cloned().unwrap_or_default();
    if let Some(extra) = extra.as_object() {
        for (key, value) in extra {
            out.insert(key.clone(), value.clone());
        }
    }
    Value::Object(out)
}

fn run(run_dir: &str) -> Result<String, String> {
    let (config_raw, config) = load_config(run_dir)?;
    let ctx = Ctx::load(&raw_dir(run_dir))?;
    let raw_competitors = items(&config_raw["competitors"]);
    let mut competitors = Vec::with_capacity(config.competitors.len());
    for (competitor, raw_competitor) in config.competitors.iter().zip(raw_competitors) {
        let site: Site = site_of(&ctx, &competitor.domain, &competitor.brand)?;
        competitors.push(merge(raw_competitor, &serde_json::to_value(&site).map_err(|e| e.to_string())?));
    }
    let target_site = site_of(&ctx, &config.target.domain, &[])?;
    let target_site_value = serde_json::to_value(&target_site).map_err(|e| e.to_string())?;
    let gsc = gsc_summary(&ctx.raw)?;
    let target = merge(&target_site_value, &serde_json::json!({"gsc": gsc}));
    let target_rank = num_or(&target["rank"], 0.0);
    let keywords = target_keywords(&ctx.raw, &config, target_rank)?;
    let analysis = serde_json::json!({
        "config": config_raw,
        "target": target,
        "competitors": competitors,
        "keywords": serde_json::to_value(&keywords).map_err(|e| e.to_string())?,
    });
    let path = format!("{run_dir}/analysis.json");
    write_text(&path, &to_pretty_one(&analysis)?)?;
    stringify(&serde_json::json!({
        "analysis": path,
        "competitors": competitors.len(),
        "keywords": keywords.len(),
        "targetRank": analysis["target"]["rank"],
    }))
}

/// `bun analyze.ts <run-dir>`.
pub fn main(argv: &[String]) -> i32 {
    let Some(run_dir) = argv.first().filter(|d| !d.is_empty()) else {
        eprintln!("usage: bun analyze.ts <run-dir>");
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
