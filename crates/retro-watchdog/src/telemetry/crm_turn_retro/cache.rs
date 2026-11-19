//! Prompt-cache review of the window's main-model steps (founder, 2026-11-12: every 100-turn retro
//! reviews the cache hit rate and fixes what keeps it under TARGET).
//!
//! Each step is chained to the request its cache could reuse: the previous step of its turn, else the
//! last step of the thread's previous turn (in the window or the lookback rows, `fetch::window`). The
//! ideal cached amount and the cause ladder are those of cache-optimization `analyze-cache.ts`, with
//! thresholds from the newest experiment's `analysisDefaults`:
//! ideal = min(input, max(previous input - blockSlackTokens, staticFloorTokens)); a thread's first step
//! may reuse only the shared static floor. `loss` = ideal - cached (what the cache failed to serve) and
//! `fresh` = input - max(ideal, cached) (new content no cache can serve), so loss + fresh = uncached input.
//! Loss is priced at (input rate - cached rate): Flash-Lite, whose cached rate equals its input rate, loses $0.
use super::analyze::{quantile, round, share, Call, Turn};
use super::prices;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};

/// The main-model weighted hit rate the product aims at (cache-optimization report goal).
pub(super) const TARGET: f64 = 0.90;
const EXPERIMENTS: &str = "/path/to/workspace/.claude/skills/cache-optimization/experiments";
const OBSERVER: &str = "observational-memory";
const WORST: usize = 10;

pub(super) struct Defaults { floor: f64, tolerance: f64, slack: f64, idle_min: f64, source: String }

/// `analysisDefaults` of the newest experiment record that has them (file names sort by date).
pub(super) fn defaults() -> Defaults {
    let built_in = |why: String| Defaults { floor: 36_000.0, tolerance: 4_000.0, slack: 4_500.0, idle_min: 2.0, source: format!("built-in ({why})") };
    if cfg!(test) { return built_in("test".into()); }
    let Ok(dir) = std::fs::read_dir(EXPERIMENTS) else { return built_in(format!("{EXPERIMENTS} unreadable")) };
    let mut files: Vec<_> = dir.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    for path in files.iter().rev() {
        let Some(d) = super::super::read(path).map(|v| v["analysisDefaults"].clone()).filter(|d| d.is_object()) else { continue };
        let n = |k: &str, f: f64| d[k].as_f64().unwrap_or(f);
        return Defaults { floor: n("staticFloorTokens", 36_000.0), tolerance: n("floorToleranceTokens", 4_000.0), slack: n("blockSlackTokens", 4_500.0),
            idle_min: n("idleExpiryMinutes", 2.0), source: path.display().to_string() };
    }
    built_in("no analysisDefaults".into())
}

/// One classified main step.
/// `cached` is clamped to `input`: a provider count above the prompt is an anomaly (`cache.cached_over_input_calls`).
pub(super) struct Row<'a> { pub(super) turn: &'a Turn, pub(super) call: &'a Call, pub(super) scope: &'static str, pub(super) cause: &'static str, pub(super) cached: f64, pub(super) loss: f64, fresh: f64, pub(super) gap_min: Option<f64> }

fn om(turn: &Turn) -> bool { turn.calls.iter().any(|c| c.agent == OBSERVER) }

/// An activation stamped this long after a call's recorded start still preceded its request.
const ACTIVATION_SLACK_MS: f64 = 2_000.0;

/// Whether Observational Memory can have changed the observation block between the request `call` could reuse
/// and `call`: an activation stored in this turn or the previous one after the previous request started. A
/// buffer seal alone keeps the prefix (mastra 6c04ecb874), so an observer call without an activation is not
/// enough. Rows saved before activations were read (`activations: None`) keep the observer-call test.
fn om_changed(turn: &Turn, call: &Call, index: usize, previous: Option<(&Call, &Turn)>) -> bool {
    let Some(mine) = &turn.activations else { return om(turn) || (index == 0 && previous.is_some_and(|(_, t)| om(t))) };
    let from = previous.map_or(f64::NEG_INFINITY, |(p, _)| p.start);
    let earlier = previous.filter(|(_, t)| !std::ptr::eq(*t, turn)).and_then(|(_, t)| t.activations.as_deref()).unwrap_or(&[]);
    mine.iter().chain(earlier).any(|a| *a > from && *a <= call.start + ACTIVATION_SLACK_MS)
}

/// Every main step of the window's turns with its scope and cause.
pub(super) fn classify<'a>(turns: &'a [Turn], prior: &'a [Turn], d: &Defaults) -> Vec<Row<'a>> {
    let mut threads: BTreeMap<&str, Vec<&Turn>> = BTreeMap::new();
    for t in prior.iter().chain(turns) { threads.entry(t.thread.as_str()).or_default().push(t); }
    let mut rows = Vec::new();
    for list in threads.values_mut() {
        list.sort_by(|a, b| a.at_ms.total_cmp(&b.at_ms));
        let mut previous: Option<(&Call, &Turn)> = None;
        for turn in list.iter() {
            let mut deficit_before = 0.0;
            for (index, call) in turn.calls.iter().filter(|c| c.is_step()).enumerate() {
                if !turn.prior {
                    let row = cause(turn, call, index, previous, deficit_before, d);
                    deficit_before = row.1;
                    rows.push(row.0);
                }
                previous = Some((call, turn));
            }
        }
    }
    rows
}

fn cause<'a>(turn: &'a Turn, call: &'a Call, index: usize, previous: Option<(&Call, &Turn)>, deficit_before: f64, d: &Defaults) -> (Row<'a>, f64) {
    let (input, cached) = (call.input, call.cached.min(call.input));
    let scope = if index > 0 { "within_turn" } else if previous.is_some() { "cross_turn" } else { "thread_start" };
    let ideal = input.min(previous.map_or(d.floor, |(p, _)| (p.input - d.slack).max(d.floor)));
    let deficit = ideal - cached;
    let shrank = previous.is_some_and(|(p, _)| input < p.input - d.slack);
    let om_nearby = om_changed(turn, call, index, previous);
    let gap_min = previous.and_then(|(p, _)| p.end).map(|end| (call.start - end) / 60_000.0);
    let steady_tail = index > 0 && deficit_before > d.slack && (deficit - deficit_before).abs() <= deficit.max(deficit_before) * 0.5;
    let name = if cached + d.slack >= ideal { "ok" } else {
        match previous {
            None => if cached == 0.0 { "cold_static_prefix" } else { "static_prefix_partial" },
            Some((p, t)) if p.model != call.model || (index == 0 && t.level != turn.level) => "config_switch",
            _ if gap_min.is_some_and(|g| g > d.idle_min) => "idle_expiry",
            Some((p, _)) if shrank && om_nearby && input < p.input * 0.9 => "om_rewrite",
            _ if shrank => "prefix_shrank",
            _ if cached == 0.0 => "provider_miss",
            _ if (cached - d.floor).abs() <= d.tolerance => if om_nearby { "om_observation_block_changed" } else { "history_prefix_diverged" },
            _ if steady_tail => "trailing_block_uncached",
            _ => "partial_divergence",
        }
    };
    (Row { turn, call, scope, cause: name, cached, loss: deficit.max(0.0), fresh: input - ideal.max(cached), gap_min }, deficit.max(0.0))
}

#[derive(Default)]
struct Sum { calls: usize, input: f64, cached: f64, loss: f64, fresh: f64, loss_usd: f64, fresh_usd: f64, turns: HashSet<String>, gaps: Vec<f64> }
impl Sum {
    fn add(&mut self, r: &Row, usd: Option<(f64, f64)>) {
        self.calls += 1; self.input += r.call.input; self.cached += r.cached; self.loss += r.loss; self.fresh += r.fresh;
        if let Some((l, f)) = usd { self.loss_usd += l; self.fresh_usd += f; }
        self.turns.insert(r.turn.turn.clone());
        if let Some(g) = r.gap_min { self.gaps.push(round(g, 1)); }
    }
    fn json(&self) -> Value {
        json!({"calls": self.calls, "turns": self.turns.len(), "input": self.input, "cached": self.cached, "hit": share(self.cached, self.input),
            "ceiling": share(self.cached + self.loss, self.input), "loss_tokens": self.loss, "loss_usd": round(self.loss_usd, 4),
            "fresh_tokens": self.fresh, "fresh_usd": round(self.fresh_usd, 4),
            "gap_min": {"p50": quantile(&self.gaps, 0.5), "p90": quantile(&self.gaps, 0.9)}})
    }
}

/// The window's cache block: hit rate against TARGET, ceiling, loss by cause, model and scope, worst threads.
pub(super) fn summarize(turns: &[Turn], prior: &[Turn]) -> Value {
    let d = defaults();
    let prices = prices::current().as_ref();
    let rows = classify(turns, prior, &d);
    let (mut total, mut unpriced) = (Sum::default(), 0);
    let (mut models, mut scopes, mut causes, mut threads) = (BTreeMap::<&str, Sum>::new(), BTreeMap::<&str, Sum>::new(), BTreeMap::<&str, Sum>::new(), BTreeMap::<&str, (Sum, BTreeMap<&str, f64>)>::new());
    let mut by_turn: BTreeMap<&str, (f64, f64)> = BTreeMap::new();
    let mut cause_models: BTreeMap<&str, BTreeMap<&str, Sum>> = BTreeMap::new();
    for r in &rows {
        let usd = prices.ok().and_then(|p| p.rates(&r.call.model, r.call.input, r.call.start))
            .map(|x| (r.loss * (x.input - x.cached) / 1e6, r.fresh * x.input / 1e6));
        if usd.is_none() { unpriced += 1; }
        for sum in [&mut total, models.entry(r.call.model.as_str()).or_default(), scopes.entry(r.scope).or_default(), causes.entry(r.cause).or_default()] { sum.add(r, usd); }
        cause_models.entry(r.cause).or_default().entry(r.call.model.as_str()).or_default().add(r, usd);
        let thread = threads.entry(r.turn.thread.as_str()).or_default();
        thread.0.add(r, usd);
        if r.loss > 0.0 { *thread.1.entry(r.cause).or_default() += r.loss; }
        let t = by_turn.entry(r.turn.turn.as_str()).or_default();
        t.0 += r.call.input; t.1 += r.cached;
    }
    let below = by_turn.values().filter(|(i, c)| *i > 0.0 && c / i < TARGET).count();
    let threads_below = threads.values().filter(|(s, _)| s.input > 0.0 && s.cached / s.input < TARGET).count();
    let map = |m: &BTreeMap<&str, Sum>| m.iter().map(|(k, s)| (k.to_string(), s.json())).collect::<Map<_, _>>();
    let mut ranked: Vec<Value> = causes.iter().filter(|(c, s)| **c != "ok" && s.loss > 0.0)
        .map(|(c, s)| { let mut v = s.json(); v["cause"] = json!(c); v["loss_share"] = share(s.loss, total.loss); v }).collect();
    let key = |v: &Value| (v["loss_usd"].as_f64().unwrap_or(0.0), v["loss_tokens"].as_f64().unwrap_or(0.0));
    ranked.sort_by(|a, b| key(b).0.total_cmp(&key(a).0).then(key(b).1.total_cmp(&key(a).1)));
    let mut worst: Vec<(&&str, &(Sum, BTreeMap<&str, f64>))> = threads.iter().filter(|(_, (s, _))| s.loss > 0.0).collect();
    worst.sort_by(|a, b| b.1.0.loss_usd.total_cmp(&a.1.0.loss_usd).then(b.1.0.loss.total_cmp(&a.1.0.loss)));
    let worst: Vec<Value> = worst.into_iter().take(WORST).map(|(id, (s, c))| { let mut v = s.json(); v["thread"] = json!(id); v["causes"] = json!(c); v }).collect();
    let mut aux: BTreeMap<String, (usize, f64, f64)> = BTreeMap::new();
    for c in turns.iter().flat_map(|t| &t.calls).filter(|c| c.role != "main") {
        let a = aux.entry(format!("{}/{}", c.agent, c.model)).or_default();
        a.0 += 1; a.1 += c.input; a.2 += c.cached;
    }
    json!({
        "target": TARGET, "hit": share(total.cached, total.input), "ceiling": share(total.cached + total.loss, total.input),
        "below_target": total.input > 0.0 && total.cached / total.input < TARGET,
        "main": total.json(), "fresh_share": share(total.fresh, total.input), "loss_share": share(total.loss, total.input),
        "by_model": map(&models), "by_scope": map(&scopes), "causes": causes.iter().map(|(c, s)| { let mut v = s.json(); v["by_model"] = json!(map(&cause_models[c])); (c.to_string(), v) }).collect::<Map<_, _>>(), "ranked": ranked,
        "turns_below_target": {"turns": below, "share": share(below as f64, by_turn.len() as f64)}, "threads_below_target": threads_below,
        "worst": worst, "unpriced_calls": unpriced, "cached_over_input_calls": rows.iter().filter(|r| r.call.cached > r.call.input).count(), "lookback_turns": prior.len(),
        "auxiliary": aux.iter().map(|(k, a)| (k.clone(), json!({"calls": a.0, "input": a.1, "cached": a.2, "hit": share(a.2, a.1)}))).collect::<Map<_, _>>(),
        "thresholds": {"source": d.source, "static_floor": d.floor, "floor_tolerance": d.tolerance, "block_slack": d.slack, "idle_expiry_min": d.idle_min},
        "pricing": prices.map(|p| json!(p.source)).unwrap_or_else(|e| json!({"error": e})),
    })
}
