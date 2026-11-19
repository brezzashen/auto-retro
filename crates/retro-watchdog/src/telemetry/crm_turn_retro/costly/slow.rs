//! Slow first text (founder 2026-11-18, `补充缺口`, after production user d53a9941 waited minutes for answers):
//! a turn whose first text came SLOW_FIRST_TEXT_MS or more after it started (or that ended that late without
//! text) is reviewed with the costly turns. The daily turn-scoring judges latency per account median, so one
//! account's slow turns never alert there.
//!
//! `facts` splits the wait before the first text into provider first-chunk waits of main calls (deadline cuts
//! included), generation after their first chunk, and the host time left over (tools, preparation). `cause` is
//! the pre-verdict the session confirms or overturns:
//! - `provider_wait`: a main call was cut at a first-chunk deadline or waited at least PROVIDER_SLOW_MS;
//! - `serial_rounds`: otherwise SERIAL_ROUNDS or more main steps ran before the text;
//! - else the largest part: `provider_wait`, `slow_generation` or `tool_wait`.
use super::super::analyze::{round, Turn};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(in crate::telemetry) const SLOW_FIRST_TEXT_MS: f64 = 180_000.0;
/// A main call cancelled without a first chunk after this long was cut at the 120 s or 300 s deadline.
const DEADLINE_CUT_MS: f64 = 115_000.0;
const PROVIDER_SLOW_MS: f64 = 60_000.0;
const SERIAL_ROUNDS: usize = 4;
/// Causes a session may name for an account's slow turns (`validate.rs`); `mixed` when no part dominates.
pub(super) const CAUSES: [&str; 5] = ["provider_wait", "serial_rounds", "slow_generation", "tool_wait", "mixed"];

/// The wait the person saw before any text: to the first text, else to the end of the turn.
pub(super) fn wait_ms(t: &Turn) -> Option<f64> { t.first_text_ms.or(t.total_ms) }

/// The split of a slow turn's wait, or null for a turn under SLOW_FIRST_TEXT_MS.
pub(super) fn facts(t: &Turn) -> Value {
    let Some(wait) = wait_ms(t).filter(|w| *w >= SLOW_FIRST_TEXT_MS) else { return Value::Null };
    let text_at = t.started + wait;
    let main: Vec<_> = t.calls.iter().filter(|c| c.role == "main" && c.start < text_at).collect();
    let (mut provider, mut generation, mut longest, mut cuts) = (0.0_f64, 0.0_f64, 0.0_f64, 0);
    for c in &main {
        let end = c.end.unwrap_or(text_at).min(text_at);
        match c.first_chunk {
            Some(first) => {
                let first = first.min(end);
                provider += first - c.start;
                generation += end - first;
                longest = longest.max(first - c.start);
            }
            None => {
                provider += end - c.start;
                if end - c.start >= DEADLINE_CUT_MS { cuts += 1; }
                longest = longest.max(end - c.start);
            }
        }
    }
    let host = (wait - provider - generation).max(0.0);
    let rounds = main.iter().filter(|c| c.is_step()).count();
    let cause = if cuts > 0 || longest >= PROVIDER_SLOW_MS { "provider_wait" }
        else if rounds >= SERIAL_ROUNDS { "serial_rounds" }
        else if provider >= generation && provider >= host { "provider_wait" }
        else if generation >= host { "slow_generation" }
        else { "tool_wait" };
    json!({"wait_ms": round(wait, 0), "answered": t.first_text_ms.is_some(), "user": t.user, "main_steps": rounds,
        "main_calls": main.len(), "deadline_cuts": cuts, "longest_first_chunk_ms": round(longest, 0),
        "provider_wait_ms": round(provider, 0), "generation_ms": round(generation, 0), "host_ms": round(host, 0), "cause": cause})
}

/// Slow turns per account, the most turns first: what the session must judge (`requirements…slow_accounts`).
pub(super) fn accounts(profiles: &[Value]) -> Vec<Value> {
    let mut by: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for p in profiles.iter().filter(|p| p["slow"].is_object()) {
        by.entry(p["slow"]["user"].as_str().unwrap_or("unknown").to_string()).or_default().push(p);
    }
    let mut out: Vec<Value> = by.into_iter().map(|(user, turns)| {
        let mut causes: BTreeMap<&str, usize> = BTreeMap::new();
        for p in &turns { *causes.entry(p["slow"]["cause"].as_str().unwrap_or("unknown")).or_default() += 1; }
        let max = turns.iter().filter_map(|p| p["slow"]["wait_ms"].as_f64()).fold(0.0, f64::max);
        json!({"account": user, "turns": turns.len(), "max_wait_ms": max, "pre_causes": causes,
            "examples": turns.iter().take(5).map(|p| p["key"].clone()).collect::<Vec<_>>()})
    }).collect();
    out.sort_by(|a, b| b["turns"].as_u64().cmp(&a["turns"].as_u64()));
    out
}
