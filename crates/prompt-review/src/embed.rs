//! Unit vectors from local-search's batch Qwen3-Embedding-4B llama-server (no network, no model call
//! outside this Mac). Documents get no instruction prefix, so both sides of a pair are embedded alike.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

pub const MODEL: &str = "qwen3-embedding-4b";
pub const URL: &str = "http://127.0.0.1:18970/v1/embeddings";
const BATCH: usize = 16;
const MAX_INPUT: usize = 2_000;

fn request(url: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
    let resp: Value = ureq::post(url).timeout(Duration::from_secs(300))
        .send_json(json!({"model": MODEL, "input": inputs})).context("embedding server")?.into_json()?;
    let mut rows: Vec<(usize, Vec<f32>)> = resp["data"].as_array().into_iter().flatten()
        .map(|d| (d["index"].as_u64().unwrap_or(0) as usize,
            d["embedding"].as_array().into_iter().flatten().filter_map(Value::as_f64).map(|x| x as f32).collect()))
        .collect();
    if rows.len() != inputs.len() {
        bail!("embedding server returned {} vectors for {} inputs", rows.len(), inputs.len());
    }
    rows.sort_by_key(|(i, _)| *i);
    Ok(rows.into_iter().map(|(_, v)| normalize(v)).collect())
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

/// One normalized vector per text, in order. A failed batch fails the run: a partial pass would
/// report fewer duplicates than exist.
pub fn vectors(texts: &[String]) -> Result<Vec<Vec<f32>>> {
    use crate::embed_cache as cache;
    let url = std::env::var("PROMPT_REVIEW_EMBED_URL").unwrap_or_else(|_| URL.to_string());
    let path = crate::tree::state_dir().join(format!("vectors-{MODEL}.bin"));
    let mut known = cache::load(&path);
    let inputs: Vec<String> = texts.iter().map(|t| t.chars().take(MAX_INPUT).collect()).collect();
    let mut missing: Vec<String> = inputs.iter().filter(|t| !known.contains_key(&cache::key(t))).cloned().collect();
    missing.sort();
    missing.dedup();
    for batch in missing.chunks(BATCH) {
        let vecs = request(&url, batch).or_else(|_| request(&url, batch))?;
        let rows: Vec<(cache::Key, Vec<f32>)> = batch.iter().map(|t| cache::key(t)).zip(vecs).collect();
        cache::append(&path, &rows)?;
        known.extend(rows);
    }
    Ok(inputs.iter().map(|t| known[&cache::key(t)].clone()).collect())
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
