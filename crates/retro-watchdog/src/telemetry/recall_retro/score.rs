//! Telemetry summary of one recall window: how many vector recalls per corpus, the share that
//! returned nothing, query latency (embed + search + rerank ms) and stale-cache answers. Relevance
//! and content quality are measured by the session (`local-search-eval audit` plus judging).
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn pct(n: usize, d: usize) -> Value {
    if d == 0 { Value::Null } else { json!((n as f64 * 1000.0 / d as f64).round() / 10.0) }
}

fn quantile(sorted: &[f64], p: f64) -> Value {
    if sorted.is_empty() { return Value::Null; }
    json!((sorted[((sorted.len() - 1) as f64 * p).round() as usize] * 10.0).round() / 10.0)
}

/// Rows left out of the agent count, by rule (`batch.rs`).
#[derive(Default, Debug, PartialEq, Eq)]
pub(super) struct Excluded { pub batch: usize, pub tool: usize, pub repeat: usize, pub synthetic: usize }

/// The window's agent recalls: vector `query` rows that are not a script batch, a tool's own
/// receipt, a repeat or a labelled synthetic probe (`batch.rs`), and how many rows each rule left out.
pub(super) fn agent_recalls(lines: &[String]) -> (Vec<Value>, Excluded) {
    use super::batch::Skip;
    let all: Vec<Value> = lines.iter().filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["sub"] == "query" && v["vector_used"] == true).collect();
    let flags = super::batch::flags(&all);
    let n = |s: Skip| flags.iter().filter(|f| **f == s).count();
    let excluded = Excluded { batch: n(Skip::Batch), tool: n(Skip::Tool), repeat: n(Skip::Repeat), synthetic: n(Skip::Synthetic) };
    (all.into_iter().zip(flags).filter(|(_, f)| *f == Skip::Agent).map(|(r, _)| r).collect(), excluded)
}

pub(super) fn score(lines: &[String]) -> Value {
    let (rows, ex) = agent_recalls(lines);
    let mut corpora: BTreeMap<String, usize> = BTreeMap::new();
    let (mut zero, mut stale, mut ms) = (0, 0, Vec::new());
    for r in &rows {
        *corpora.entry(r["corpus"].as_str().unwrap_or("all").to_string()).or_default() += 1;
        zero += usize::from(r["hits"].as_u64() == Some(0));
        stale += usize::from(r["stale_cache"] == true);
        ms.push(["embed_ms", "search_ms", "rerank_ms"].iter().filter_map(|k| r[*k].as_f64()).sum::<f64>());
    }
    ms.sort_by(f64::total_cmp);
    json!({"recalls": rows.len(), "batch_excluded": ex.batch, "tool_excluded": ex.tool, "repeat_excluded": ex.repeat, "synthetic_excluded": ex.synthetic,
        "corpora": corpora, "zero_hit_pct": pct(zero, rows.len()), "stale_cache_pct": pct(stale, rows.len()),
        "latency_ms": {"p50": quantile(&ms, 0.5), "p90": quantile(&ms, 0.9)}})
}
