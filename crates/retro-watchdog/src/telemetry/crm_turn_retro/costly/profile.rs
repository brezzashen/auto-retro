//! Per-turn facts of a costly Mars CRM turn (more than `costly::THRESHOLD_CREDITS` settled) and the script's
//! pre-verdict, which the session confirms or overturns from the turn's own calls.
//!
//! The cache chain needs the turn's neighbours: `profiles` classifies every main step of the whole window
//! (`cache::classify`, the thread's previous turn included through the lookback rows) and only then keeps the
//! costly turns, so a thread's first turn in the window is never mislabelled `thread_start`.
//!
//! Signals (abnormal candidates), in priority order:
//! - `cache_invalidated:<cause>`: the main steps lost at least CACHE_LOSS_SHARE of their input to a cache cause
//!   (`cache.rs` ladder; the cause with the most lost tokens is named).
//! - `waste:<class>`: a silent step of a waste class (`analyze::WASTE`).
//! - `tool_failures`: a tool call failed (protective refusals included; the session tells them apart).
//! - `transport_retry`: a main-model call was retried.
//! - `residual_credits`: settled beyond the token price by more than RESIDUAL_MIN and RESIDUAL_SHARE.
//! - `incomplete`: the turn ended without `completed` yet settled credits.
//!
//! A turn with no signal is `heavy:<driver>`: what made it costly without a defect (`many_steps`, `sub_agents`,
//! `reasoning`, `long_context`, `base`). `signature` groups turns for the session: the first signal, or the driver.
use super::super::analyze::{round, share, Turn, WASTE};
use super::super::cache::{self, Row};
use super::super::prices::{self, Prices};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};

/// A turn whose main steps lost this share of their input to cache causes counts as cache-invalidated.
pub(super) const CACHE_LOSS_SHARE: f64 = 0.10;
const RESIDUAL_MIN: f64 = 5.0;
const RESIDUAL_SHARE: f64 = 0.25;
const MANY_STEPS: usize = 6;
const SUB_AGENT_SHARE: f64 = 0.40;
const REASONING_SHARE: f64 = 0.50;
const LONG_CONTEXT: f64 = 100_000.0;
const STEP_ROWS: usize = 12;

/// One profile per turn id in `keep` found in `rows` (window rows with lookback), oldest first.
pub(in crate::telemetry) fn profiles(rows: &[Value], keep: &HashSet<String>) -> Vec<Value> {
    let turns = super::super::analyze::turns(rows);
    let prior = super::super::analyze::prior_turns(rows);
    let classified = cache::classify(&turns, &prior, &cache::defaults());
    let prices = prices::current().as_ref().ok();
    turns.iter().filter(|t| keep.contains(&t.turn))
        .map(|t| profile(t, &classified.iter().filter(|r| r.turn.turn == t.turn).collect::<Vec<_>>(), prices)).collect()
}

pub(super) fn profile(t: &Turn, steps: &[&Row], prices: Option<&Prices>) -> Value {
    let main_input: f64 = steps.iter().map(|r| r.call.input).sum();
    let main_cached: f64 = steps.iter().map(|r| r.cached).sum();
    let loss: f64 = steps.iter().map(|r| r.loss).sum();
    let mut causes: BTreeMap<&str, f64> = BTreeMap::new();
    for r in steps.iter().filter(|r| r.loss > 0.0 && r.cause != "ok") { *causes.entry(r.cause).or_default() += r.loss; }
    let top_cause = causes.iter().max_by(|a, b| a.1.total_cmp(b.1)).map(|(c, _)| *c);
    let invalidated = main_input > 0.0 && loss / main_input >= CACHE_LOSS_SHARE && top_cause.is_some();

    let (mut by_group, mut token_credits, mut residual, mut priced) = (BTreeMap::<String, f64>::new(), 0.0, 0.0, true);
    let (mut output, mut reasoning, mut input, mut cached) = (0.0, 0.0, 0.0, 0.0);
    for c in &t.calls {
        *by_group.entry(format!("{}/{}/{}", c.role, c.agent, c.model)).or_default() += c.credits.unwrap_or(0.0);
        output += c.output; reasoning += c.reasoning; input += c.input; cached += c.cached.min(c.input);
        match prices.and_then(|p| p.cost(&c.model, c.input, c.cached, c.output, c.reasoning, c.start, None)) {
            Some(cost) => {
                let credits = Prices::credits(&cost, c.input + c.output + c.reasoning);
                token_credits += credits;
                if let Some(s) = c.credits { residual += s - credits; }
            }
            None => priced = false,
        }
    }
    let settled: f64 = by_group.values().sum();
    let sub_credits: f64 = by_group.iter().filter(|(k, _)| !k.starts_with("main/")).map(|(_, v)| v).sum();
    let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
    for s in &t.silent { *classes.entry(s.class).or_default() += 1; }
    let failed: Vec<Value> = t.tools.iter().filter(|x| x.failed).map(|x| json!({"tool": x.name, "error": x.error})).collect();

    let mut signals: Vec<String> = Vec::new();
    if invalidated { signals.push(format!("cache_invalidated:{}", top_cause.unwrap_or("unknown"))); }
    for class in WASTE.iter().filter(|c| classes.contains_key(**c)) { signals.push(format!("waste:{class}")); }
    if !failed.is_empty() { signals.push("tool_failures".into()); }
    if t.retries > 0 { signals.push("transport_retry".into()); }
    if priced && residual > RESIDUAL_MIN && residual > t.credits * RESIDUAL_SHARE { signals.push("residual_credits".into()); }
    if !t.completed { signals.push("incomplete".into()); }
    let driver = if t.steps >= MANY_STEPS { "many_steps" }
        else if t.credits > 0.0 && sub_credits / t.credits.max(settled) >= SUB_AGENT_SHARE { "sub_agents" }
        else if output + reasoning > 0.0 && reasoning / (output + reasoning) >= REASONING_SHARE { "reasoning" }
        else if t.steps > 0 && main_input / t.steps as f64 >= LONG_CONTEXT { "long_context" }
        else { "base" };
    let signature = if signals.is_empty() { format!("heavy:{driver}") } else { format!("suspect:{}", signals[0]) };

    let mut groups: Vec<(String, f64)> = by_group.into_iter().filter(|(_, v)| *v > 0.0).collect();
    groups.sort_by(|a, b| b.1.total_cmp(&a.1));
    let models: Vec<&str> = { let mut m: Vec<&str> = steps.iter().map(|r| r.call.model.as_str()).collect(); m.dedup(); m };
    json!({
        "key": t.turn, "turn": t.turn, "thread": t.thread, "at_ms": t.at_ms, "level": t.level, "completed": t.completed,
        "credits": round(t.credits, 3), "steps": t.steps, "calls": t.calls.len(), "transport_retries": t.retries, "main_models": models,
        "first_text_ms": t.first_text_ms, "total_ms": t.total_ms,
        "tokens": {"input": input, "cached": cached, "output": output, "reasoning": reasoning, "hit": share(cached, input)},
        "credits_by_group": groups.iter().take(6).map(|(k, v)| (k.clone(), json!(round(*v, 3)))).collect::<Map<_, _>>(),
        "credits_sub_share": share(sub_credits, t.credits.max(settled)),
        "credits_from_tokens": if priced { json!(round(token_credits, 3)) } else { Value::Null },
        "credits_residual": if priced { json!(round(residual, 3)) } else { Value::Null },
        "cache": {"invalidated": invalidated, "main_input": main_input, "main_cached": main_cached, "main_hit": share(main_cached, main_input),
            "loss_tokens": loss, "loss_share": share(loss, main_input), "top_cause": top_cause, "causes": causes,
            "steps": steps.iter().take(STEP_ROWS).enumerate().map(|(i, r)| json!({"step": i, "scope": r.scope, "cause": r.cause, "model": r.call.model,
                "input": r.call.input, "cached": r.cached, "loss": r.loss, "gap_min": r.gap_min.map(|g| round(g, 1))})).collect::<Vec<_>>()},
        "waste": {"silent_steps": t.silent.len(), "classes": classes, "repeats": t.repeats, "tool_failures": failed},
        "signals": signals, "driver": driver, "signature": signature, "slow": super::slow::facts(t),
    })
}

/// Turns grouped by signature: count, credits and the costliest turn ids, costliest group first.
pub(super) fn groups(profiles: &[Value]) -> Vec<Value> {
    let mut by: BTreeMap<String, (usize, f64, Vec<(f64, String)>)> = BTreeMap::new();
    for p in profiles {
        let g = by.entry(p["signature"].as_str().unwrap_or("unknown").to_string()).or_default();
        let credits = p["credits"].as_f64().unwrap_or(0.0);
        g.0 += 1; g.1 += credits; g.2.push((credits, p["key"].as_str().unwrap_or("").to_string()));
    }
    let mut out: Vec<Value> = by.into_iter().map(|(sig, (n, credits, mut turns))| {
        turns.sort_by(|a, b| b.0.total_cmp(&a.0));
        json!({"signature": sig, "turns": n, "credits": round(credits, 1), "examples": turns.iter().take(5).map(|(_, k)| k.clone()).collect::<Vec<_>>()})
    }).collect();
    out.sort_by(|a, b| b["credits"].as_f64().unwrap_or(0.0).total_cmp(&a["credits"].as_f64().unwrap_or(0.0)));
    out
}
