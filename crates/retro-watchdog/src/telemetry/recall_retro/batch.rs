//! Rows of `queries.jsonl` that are not an agent's own recall, so they neither open a retro nor
//! enter its score. `local-search-eval audit` applies the same rules (`audit.rs`).
//!
//! 1. Batch: more than BATCH_MAX vector recalls from one parent pid within one minute. An agent's
//!    tool call starts a fresh shell per command, so its parent changes per call; a script loop
//!    keeps one parent. Incident 814771373664478643647: 319 of 331 rows were one golden-set scorer.
//!    Rows without `ppid` (older binaries) are never a batch.
//! 2. Tool: the row carries `caller` (`LOCAL_SEARCH_CALLER`), set by cpu-watchdog `retro-recall`
//!    and `log-recall` for their own receipts. Incident 83793984228306173904: 232 of 487 rows.
//! 3. Repeat: the same corpus and query text again within REPEAT_MS. Incident
//!    83793984228306173904: a cold/warm latency benchmark sent four query texts 36 times at
//!    2-39 s per cold call, too slow for the batch rule. The first call of each text still counts.
//! 4. Synthetic: the row carries `synthetic` (`SEARCH_WATCH_SYNTHETIC`), the label a retro or eval
//!    puts on its own probes. Recall-retro 8701297881706895176: retro 83879279163991646107 timed
//!    `agent resources route people facts` under that label, and the audit judged it as a miss.
use serde_json::Value;
use std::collections::HashMap;

pub(super) const BATCH_MAX: usize = 20;
const BUCKET_MS: f64 = 60_000.0;
const REPEAT_MS: f64 = 600_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Skip { Agent, Batch, Tool, Repeat, Synthetic }

fn key(row: &Value) -> Option<(u64, u64)> {
    Some((row["ppid"].as_u64()?, (row["ts_ms"].as_f64().unwrap_or(0.0) / BUCKET_MS) as u64))
}

/// The query as asked, trimmed and lower-cased. A trailing run number is kept: "agent 0" and
/// "agent 1" are different questions.
pub(super) fn normalized(query: &str) -> String {
    query.trim().to_lowercase()
}

/// One verdict per row, in row order.
pub(super) fn flags(rows: &[Value]) -> Vec<Skip> {
    let mut n: HashMap<(u64, u64), usize> = HashMap::new();
    for k in rows.iter().filter_map(key) {
        *n.entry(k).or_default() += 1;
    }
    let mut last: HashMap<(String, String), f64> = HashMap::new();
    rows.iter().map(|r| {
        if r["caller"].as_str().is_some_and(|c| !c.trim().is_empty()) {
            return Skip::Tool;
        }
        if r["synthetic"].as_str().is_some_and(|s| !s.trim().is_empty()) {
            return Skip::Synthetic;
        }
        if key(r).is_some_and(|k| n[&k] > BATCH_MAX) {
            return Skip::Batch;
        }
        let ts = r["ts_ms"].as_f64().unwrap_or(0.0);
        let id = (r["corpus"].as_str().unwrap_or("all").to_string(), normalized(r["query"].as_str().unwrap_or("")));
        let seen = last.get(&id).copied();
        if seen.is_none_or(|t| ts - t > REPEAT_MS) {
            last.insert(id, ts);
            Skip::Agent
        } else {
            Skip::Repeat
        }
    }).collect()
}
