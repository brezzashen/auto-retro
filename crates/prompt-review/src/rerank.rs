//! Second stage of the duplicate pass: local-search's `qwen3-reranker-0.6b` cross-encoder (port
//! 18972, on this Mac) judges whether a candidate pair states the same rule. Cosine alone does not
//! separate them: on the 2026-11-09 capture true copies of the delete rule scored 0.80-0.84, the
//! same band as rows of the resource table that only share vocabulary; the reranker gave the copies
//! ≥ 0.995 and those rows 0.06-0.98.
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;

pub const URL: &str = "http://127.0.0.1:18972/v1/rerank";
pub const MODEL: &str = "qwen3-reranker-0.6b";
const MAX_CHARS: usize = 900;
const TASK: &str = "Judge whether the Document states the same rule or instruction as the Query, in other words.";

fn call(url: &str, query: &str, docs: &[String]) -> Result<Vec<f32>> {
    let q = format!("Instruct: {TASK}\nQuery: {}", query.chars().take(MAX_CHARS).collect::<String>());
    let docs: Vec<String> = docs.iter().map(|d| d.chars().take(MAX_CHARS).collect()).collect();
    let v: Value = ureq::post(url).timeout(Duration::from_secs(120))
        .send_json(json!({"model": MODEL, "query": q, "documents": docs})).context("reranker server")?.into_json()?;
    let mut out = vec![f32::NAN; docs.len()];
    for item in v["results"].as_array().context("reranker reply without results")? {
        if let (Some(i), Some(s)) = (item["index"].as_u64(), item["relevance_score"].as_f64()) {
            if let Some(slot) = out.get_mut(i as usize) { *slot = s as f32; }
        }
    }
    anyhow::ensure!(out.iter().all(|s| !s.is_nan()), "reranker scored {} of {} documents", out.iter().filter(|s| !s.is_nan()).count(), docs.len());
    Ok(out)
}

/// Scores of `(anchor, other)` candidate pairs, in input order; one request per anchor.
pub fn judge(texts: &[String], candidates: &[(usize, usize)]) -> Result<Vec<f32>> {
    let url = std::env::var("PROMPT_REVIEW_RERANK_URL").unwrap_or_else(|_| URL.to_string());
    let mut by_anchor: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    for (k, (a, b)) in candidates.iter().enumerate() {
        by_anchor.entry(*a).or_default().push((k, *b));
    }
    let mut out = vec![0.0; candidates.len()];
    for (anchor, rows) in by_anchor {
        let docs: Vec<String> = rows.iter().map(|(_, b)| texts[*b].clone()).collect();
        let scores = call(&url, &texts[anchor], &docs).or_else(|_| call(&url, &texts[anchor], &docs))?;
        for ((k, _), s) in rows.iter().zip(scores) {
            out[*k] = s;
        }
    }
    Ok(out)
}
