//! `.claude/skills/seo-competitor/scripts/analyze.ts` `targetKeywords`: for each target keyword, its
//! volume, difficulty, CPC, trend, the first-page SERP with each domain's rank, and the win rate now
//! and after the link plan.

use super::analyze::{items_or_empty, required_items};
use super::config::RunConfig;
use super::files::{at, num_or, read_json, text};
use super::metrics::{rank_of, win_rate};
use super::monid::{first, slug};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TopRow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Value>,
    pub domain: String,
    pub rank: Value,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct KeywordSummary {
    pub keyword: String,
    pub volume: f64,
    pub kd: Option<f64>,
    pub cpc: Value,
    pub yoy: Value,
    pub top: Vec<TopRow>,
    pub win_now: f64,
    pub win_planned: f64,
}

/// `targetKeywords(config, targetRank)`.
pub fn target_keywords(raw: &str, config: &RunConfig, target_rank: f64) -> Result<Vec<KeywordSummary>, String> {
    let overview: HashMap<String, Value> = items_or_empty(&format!("{raw}/kw_overview.json"))?
        .into_iter()
        .map(|i| (text(at(&i, &["keyword"])).to_string(), i))
        .collect();
    let serp_ranks: HashMap<String, Value> = required_items(&format!("{raw}/serp/ranks.json"))?
        .into_iter()
        .map(|i| (text(at(&i, &["target"])).to_string(), at(&i, &["rank"]).clone()))
        .collect();
    let mut out = Vec::with_capacity(config.target_keywords.len());
    for keyword in &config.target_keywords {
        let serp = first(&read_json(&format!("{raw}/serp/{}.json", slug(keyword)))?).clone();
        let top = top_rows(&serp, &serp_ranks);
        let ranks: Vec<Option<f64>> = top.iter().map(|t| rank_of(&t.rank)).collect();
        let o = overview.get(keyword).unwrap_or(&Value::Null);
        let kd = at(o, &["keyword_properties", "keyword_difficulty"]).as_f64();
        out.push(KeywordSummary {
            keyword: keyword.clone(),
            volume: num_or(at(o, &["keyword_info", "search_volume"]), 0.0),
            kd,
            cpc: at(o, &["keyword_info", "cpc"]).clone(),
            yoy: at(o, &["keyword_info", "search_volume_trend", "yearly"]).clone(),
            win_now: win_rate(&ranks, kd, target_rank),
            win_planned: win_rate(&ranks, kd, config.planned_rank),
            top,
        });
    }
    Ok(out)
}

/// The first ten organic results of a SERP, each with its domain's backlink rank.
fn top_rows(serp: &Value, serp_ranks: &HashMap<String, Value>) -> Vec<TopRow> {
    super::files::items(at(serp, &["items"]))
        .iter()
        .filter(|i| text(at(i, &["type"])) == "organic")
        .take(10)
        .map(|i| {
            let domain = super::files::serp_domain(i);
            let rank = serp_ranks.get(&domain).cloned().unwrap_or(Value::Null);
            TopRow { position: i.get("rank_group").cloned(), domain, rank }
        })
        .collect()
}
