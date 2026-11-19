//! Cost structure of the window's turns (founder, 2026-11-12: every 100-turn retro analyses where the
//! money goes and optimizes it). Two views that are never derived from each other:
//! - list cost in US$ at the official price of each call's day (`prices.rs`), split into uncached input,
//!   cached input, output and reasoning, by `role/agent/model` group;
//! - settled credits, exact from each call's ledger settlement (`officialCostUsd` is not recorded).
//!
//! `credits_from_tokens` is what `pricing.rs` charges for the tokens alone (x3 markup, rounded up, at
//! least one credit a call); `rounding` is that minus the unrounded amount; `residual` is settled minus
//! token credits on priced calls: settled beyond the token price, cause unconfirmed per call (paid tool
//! fees carried on a call are one known source; 2026-11-12 `thread-title` calls with no tools settled 4-8).
//! Coverage compares the calls' settled sum with the turns' settled total: a call outliving its turn
//! (an Observational Memory fork) can settle after the turn was stored.
use super::analyze::{quantile, round, share, Turn};
use super::prices::{self, Cost, Prices};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

const COMPONENTS: [&str; 4] = ["uncached_input", "cached_input", "output", "reasoning"];
/// A dated price band starting within this many days is projected on the window's usage.
const UPCOMING_DAYS: f64 = 45.0;
const LINES: usize = 10;

fn parts(c: &Cost) -> [f64; 4] { [c.uncached_input, c.cached_input, c.output, c.reasoning] }

#[derive(Default)]
struct Group { calls: usize, input: f64, cached: f64, output: f64, reasoning: f64, usd: [f64; 4], settled: f64, settled_calls: usize, token_credits: f64, residual: f64, unpriced: usize }

impl Group {
    fn json(&self, total_usd: f64, total_settled: f64) -> Value {
        let usd: f64 = self.usd.iter().sum();
        json!({"calls": self.calls, "input": self.input, "cached": self.cached, "output": self.output, "reasoning": self.reasoning,
            "usd": round(usd, 4), "usd_share": share(usd, total_usd),
            "components": COMPONENTS.iter().zip(self.usd).map(|(k, v)| (k.to_string(), json!(round(v, 4)))).collect::<Map<_, _>>(),
            "credits_settled": self.settled, "credit_share": share(self.settled, total_settled), "credits_from_tokens": self.token_credits,
            "credits_residual": self.residual, "unpriced_calls": self.unpriced})
    }
}

/// The window's cost block.
pub(super) fn summarize(turns: &[Turn]) -> Value {
    let prices = match prices::current() { Ok(p) => p, Err(e) => return json!({"error": e}) };
    let (mut groups, mut total) = (BTreeMap::<String, Group>::new(), Group::default());
    let (mut rounding, mut unpriced_models) = (0.0, BTreeMap::<String, usize>::new());
    let mut upcoming: BTreeMap<(String, String), (f64, f64)> = BTreeMap::new();
    let mut per_turn = Vec::new();
    for t in turns {
        let mut turn_usd = 0.0;
        for c in &t.calls {
            let g = groups.entry(format!("{}/{}/{}", c.role, c.agent, c.model)).or_default();
            let cost = prices.cost(&c.model, c.input, c.cached, c.output, c.reasoning, c.start, None);
            for x in [&mut *g, &mut total] {
                x.calls += 1; x.input += c.input; x.cached += c.cached; x.output += c.output; x.reasoning += c.reasoning;
                if let Some(s) = c.credits { x.settled += s; x.settled_calls += 1; }
            }
            let Some(cost) = cost else {
                g.unpriced += 1; total.unpriced += 1;
                *unpriced_models.entry(c.model.clone()).or_default() += 1;
                continue;
            };
            let credits = Prices::credits(&cost, c.input + c.output + c.reasoning);
            rounding += credits - cost.total() * 300.0;
            turn_usd += cost.total();
            for x in [&mut *g, &mut total] {
                for (slot, v) in x.usd.iter_mut().zip(parts(&cost)) { *slot += v; }
                x.token_credits += credits;
                if let Some(s) = c.credits { x.residual += s - credits; }
            }
            if let Some(from) = prices.upcoming(&c.model, c.start, UPCOMING_DAYS) {
                let then = prices.cost(&c.model, c.input, c.cached, c.output, c.reasoning, c.start, Some(&from)).map_or(0.0, |x| x.total());
                let row = upcoming.entry((c.model.clone(), from)).or_default();
                row.0 += cost.total(); row.1 += then;
            }
        }
        per_turn.push(turn_usd);
    }
    let usd: f64 = total.usd.iter().sum();
    let turn_settled: f64 = turns.iter().map(|t| t.credits).sum();
    let mut lines: Vec<Value> = groups.iter().flat_map(|(k, g)| COMPONENTS.iter().zip(g.usd).map(move |(c, v)| (format!("{k}:{c}"), v)))
        .filter(|(_, v)| *v > 0.0).map(|(line, v)| json!({"line": line, "usd": round(v, 4), "usd_share": share(v, usd)})).collect();
    lines.sort_by(|a, b| b["usd"].as_f64().unwrap_or(0.0).total_cmp(&a["usd"].as_f64().unwrap_or(0.0)));
    lines.truncate(LINES);
    let mut by_group: Vec<(String, Value)> = groups.iter().map(|(k, g)| (k.clone(), g.json(usd, total.settled))).collect();
    by_group.sort_by(|a, b| b.1["credits_settled"].as_f64().unwrap_or(0.0).total_cmp(&a.1["credits_settled"].as_f64().unwrap_or(0.0)));
    json!({
        "usd": round(usd, 4), "usd_per_turn": {"mean": share(usd, turns.len() as f64), "p50": quantile(&per_turn.iter().map(|v| round(*v, 4)).collect::<Vec<_>>(), 0.5), "p90": quantile(&per_turn.iter().map(|v| round(*v, 4)).collect::<Vec<_>>(), 0.9)},
        "components": COMPONENTS.iter().zip(total.usd).map(|(k, v)| (k.to_string(), json!({"usd": round(v, 4), "share": share(v, usd)}))).collect::<Map<_, _>>(),
        "credits": {"settled_calls": total.settled, "settled_turns": turn_settled, "coverage": share(total.settled, turn_settled),
            "calls_without_settlement": total.calls - total.settled_calls, "from_tokens": total.token_credits, "rounding": round(rounding, 2),
            "residual": total.residual, "effective_credits_per_list_usd": if usd > 0.0 { json!(round(total.settled / usd, 1)) } else { Value::Null }},
        "groups": by_group.into_iter().map(|(k, mut g)| { g["group"] = json!(k); g }).collect::<Vec<_>>(), "lines": lines,
        "upcoming_prices": upcoming.iter().map(|((m, from), (now, then))| json!({"model": m, "from": from, "usd_now": round(*now, 4), "usd_then": round(*then, 4)})).collect::<Vec<_>>(),
        "unpriced_models": unpriced_models, "pricing": prices.source,
    })
}
