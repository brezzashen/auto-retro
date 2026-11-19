//! Deterministic waste metrics of stored Mars CRM chat turns (rows of `fetch.rs`).
//!
//! A step is one main-model call (round-optimization: "one round is one main-model call"). Steps
//! that start before the first text delay it; every one but the last of them wrote no text, so it
//! is classified by the tools it ran:
//!
//! - `error_retry`: the step before had a failed tool call and this step calls that tool again.
//! - `repeat_call`: every call of the step repeats an earlier call of the turn (same tool, same arguments).
//! - `split_record`: only record writes (`create`, `write-resource`), directly after another record-only step.
//! - `record_first`: only record writes, before any text.
//! - `split_read`: only reads, directly after another read-only step (a split batch or a redundant read;
//!   `analyze-rounds.ts` tells which from the stored arguments and results).
//! - `read`: only reads (usually a real dependency).
//! - `tool_discovery`: only `search_tools` / `load_tool`.
//! - `no_tool`: no recorded tool.
//! - `other`: any mix.
//!
//! A step's wait is the time from its model call to the next step's model call; its cost is the input
//! it resent and the credits it settled. Classes rank by wait, then credits.
use super::fetch::number;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};

const RECORD: [&str; 2] = ["create", "write-resource"];
const READ: [&str; 5] = ["read-resource", "list-resources", "grep-resources", "search-mentioned-people", "knowledge_search"];
const DISCOVERY: [&str; 2] = ["search_tools", "load_tool"];
/// Classes that are waste candidates; `read`, `record_first` and `other` need a judgment first.
pub(super) const WASTE: [&str; 5] = ["error_retry", "repeat_call", "split_record", "split_read", "tool_discovery"];
const WORST: usize = 20;

/// One provider attempt of a turn, any role; `attempt` > 1 is a transport retry.
pub(super) struct Call {
    pub(super) start: f64, pub(super) end: Option<f64>, pub(super) input: f64, pub(super) cached: f64, pub(super) output: f64, pub(super) reasoning: f64,
    pub(super) credits: Option<f64>, pub(super) model: String, pub(super) role: String, pub(super) agent: String, attempt: f64,
    /// Epoch ms of the first provider byte; `None` for a call that sent none or a row saved before the column existed.
    pub(super) first_chunk: Option<f64>,
}
impl Call {
    /// A main-model step: the main agent's first attempt.
    pub(super) fn is_step(&self) -> bool { self.role == "main" && self.attempt <= 1.0 }
    fn parse(c: &Value) -> Option<Call> {
        Some(Call { start: number(&c[0])?, end: number(&c[1]), input: number(&c[2]).unwrap_or(0.0), cached: number(&c[3]).unwrap_or(0.0),
            output: number(&c[4]).unwrap_or(0.0), reasoning: number(&c[10]).unwrap_or(0.0), credits: number(&c[8]),
            model: text(&c[9]), role: text(&c[5]), agent: text(&c[11]), attempt: number(&c[6]).unwrap_or(1.0), first_chunk: number(&c[12]) })
    }
}
pub(super) struct Tool { start: f64, pub(super) name: String, pub(super) failed: bool, pub(super) error: Option<String>, args: Option<String> }
pub(super) struct Step { pub(super) class: &'static str, pub(super) tools: Vec<String>, pub(super) wait_ms: f64, input: f64, cached: f64, credits: f64 }

pub(super) struct Turn {
    pub(super) thread: String, pub(super) turn: String, pub(super) user: String, pub(super) at_ms: f64, pub(super) started: f64, pub(super) level: String,
    /// Every provider attempt, oldest first; `steps` counts the main steps among them.
    pub(super) calls: Vec<Call>,
    /// A thread's turn before the window, read only as the request its next call could reuse.
    pub(super) prior: bool,
    /// Epoch-ms times Observational Memory activated observations in the turn (`fetch::activations`);
    /// `None` for a row saved before that column existed.
    pub(super) activations: Option<Vec<f64>>,
    pub(super) first_text_ms: Option<f64>, pub(super) total_ms: Option<f64>, pub(super) completed: bool, pub(super) credits: f64,
    /// Every main-model step of the turn.
    pub(super) steps: usize,
    /// Steps before the first text that wrote none (all but the last pre-text step).
    pub(super) silent: Vec<Step>,
    pub(super) tools: Vec<Tool>, pub(super) retries: usize, sub_calls: usize, sub_input: f64, input: f64, cached: f64, pub(super) repeats: usize,
}

fn text(v: &Value) -> String { v.as_str().unwrap_or("").to_string() }

/// One stored row as a turn; reminder turns and rows without a start time are not chat turns.
pub(super) fn turn(row: &Value) -> Option<Turn> {
    if row["trig"] == "reminder" { return None; }
    let started = number(&row["s"])?;
    let first_text = number(&row["txt"]);
    let mut all: Vec<Call> = row["calls"].as_array().into_iter().flatten().filter_map(Call::parse).collect();
    all.sort_by(|a, b| a.start.total_cmp(&b.start));
    let retries = all.iter().filter(|c| c.role == "main" && !c.is_step()).count();
    let (sub_calls, sub_input) = all.iter().filter(|c| c.role != "main").fold((0, 0.0), |a, c| (a.0 + 1, a.1 + c.input));
    let calls: Vec<&Call> = all.iter().filter(|c| c.is_step()).collect();
    let stored: Vec<&Value> = row["tools"].as_array().into_iter().flatten().filter(|t| t[3] == "main").collect();
    // The message parts carry each call's error code and arguments; they pair with telemetry by order.
    let parts: Vec<&Value> = row["inv"].as_array().into_iter().flatten().collect();
    let paired = parts.len() == stored.len() && parts.iter().zip(&stored).all(|(p, t)| p[0] == t[2]);
    let mut tools: Vec<Tool> = stored.iter().enumerate().filter_map(|(i, t)| Some(Tool {
        start: number(&t[0])?, name: text(&t[2]), failed: t[4] == "failed" || (paired && !parts[i][1].is_null()),
        error: paired.then(|| parts[i][1].as_str().map(str::to_string)).flatten(),
        args: paired.then(|| parts[i][2].as_str().map(str::to_string)).flatten(),
    })).collect();
    tools.sort_by(|a, b| a.start.total_cmp(&b.start));
    let pre = first_text.map_or(calls.len(), |t| calls.iter().filter(|c| c.start < t).count());
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let (mut silent, mut repeats) = (Vec::new(), 0);
    let mut previous: Option<(bool, Vec<String>, &'static str)> = None;
    for i in 0..pre.saturating_sub(1) {
        let (from, to) = (calls[i].start, calls[i + 1].start);
        let mine: Vec<&Tool> = tools.iter().filter(|t| t.start >= from && t.start < to).collect();
        let names: Vec<String> = mine.iter().map(|t| t.name.clone()).collect();
        let repeated = mine.iter().filter(|t| t.args.as_ref().is_some_and(|a| seen.contains(&(t.name.clone(), a.clone())))).count();
        repeats += repeated;
        for t in &mine { if let Some(a) = &t.args { seen.insert((t.name.clone(), a.clone())); } }
        let only = |set: &[&str]| !names.is_empty() && names.iter().all(|n| set.contains(&n.as_str()));
        let class = match &previous {
            Some((true, failed, _)) if names.iter().any(|n| failed.contains(n)) => "error_retry",
            _ if !mine.is_empty() && repeated == mine.len() => "repeat_call",
            Some((_, _, "record_first" | "split_record")) if only(&RECORD) => "split_record",
            _ if only(&RECORD) => "record_first",
            Some((_, _, "read" | "split_read")) if only(&READ) => "split_read",
            _ if only(&READ) => "read",
            _ if only(&DISCOVERY) => "tool_discovery",
            _ if names.is_empty() => "no_tool",
            _ => "other",
        };
        let failed: Vec<String> = mine.iter().filter(|t| t.failed).map(|t| t.name.clone()).collect();
        previous = Some((!failed.is_empty(), failed, class));
        // Without this step the next call would not exist: its resent input and credits are the step's cost.
        silent.push(Step { class, tools: names, wait_ms: to - from, input: calls[i + 1].input, cached: calls[i + 1].cached, credits: calls[i + 1].credits.unwrap_or(0.0) });
    }
    Some(Turn {
        thread: text(&row["thread_id"]), turn: text(&row["turn"]), user: text(&row["u"]), started, at_ms: number(&row["at_ms"]).unwrap_or(0.0), level: text(&row["lvl"]),
        first_text_ms: first_text.map(|t| t - started).filter(|ms| *ms >= 0.0), total_ms: number(&row["ended"]).map(|e| e - started),
        completed: row["status"] == "completed", credits: number(&row["credits"]).unwrap_or(0.0), steps: calls.len(), silent,
        input: calls.iter().map(|c| c.input).sum(), cached: calls.iter().map(|c| c.cached).sum(), tools, retries, sub_calls, sub_input, repeats,
        prior: row["prior"] == true, calls: all,
        activations: row.get("oma").and_then(Value::as_array).map(|a| a.iter().filter_map(number).collect()),
    })
}

/// Chat turns of `rows`, one per turn id (the newest row wins), oldest first; lookback rows are left out.
pub(super) fn turns(rows: &[Value]) -> Vec<Turn> { collect(rows, false) }

/// The lookback rows (`prior: true`): each window thread's turn before the window.
pub(super) fn prior_turns(rows: &[Value]) -> Vec<Turn> { collect(rows, true) }

/// `summarize` of saved or fetched rows, lookback included.
pub(super) fn summarize_rows(rows: &[Value]) -> Value { summarize(&turns(rows), &prior_turns(rows)) }

fn collect(rows: &[Value], prior: bool) -> Vec<Turn> {
    let mut by_id: BTreeMap<String, Turn> = BTreeMap::new();
    for (i, row) in rows.iter().enumerate() {
        let Some(t) = turn(row).filter(|t| t.prior == prior) else { continue };
        let key = if t.turn.is_empty() { format!("row-{i}") } else { t.turn.clone() };
        by_id.insert(key, t);
    }
    let mut all: Vec<Turn> = by_id.into_values().collect();
    all.sort_by(|a, b| a.at_ms.total_cmp(&b.at_ms));
    all
}

pub(super) fn quantile(values: &[f64], p: f64) -> Value {
    if values.is_empty() { return Value::Null; }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    json!(sorted[(p * (sorted.len() - 1) as f64).floor() as usize])
}

pub(super) fn round(value: f64, digits: i32) -> f64 { let k = 10f64.powi(digits); (value * k).round() / k }
pub(super) fn share(part: f64, whole: f64) -> Value { if whole > 0.0 { json!(round(part / whole, 4)) } else { Value::Null } }

/// The window's figures: steps before first text, waste classes ranked by wait and credits, tools, cost, worst turns.
pub(super) fn summarize(turns: &[Turn], prior: &[Turn]) -> Value {
    let answered: Vec<&Turn> = turns.iter().filter(|t| t.first_text_ms.is_some()).collect();
    let first: Vec<f64> = answered.iter().filter_map(|t| t.first_text_ms).collect();
    let total_first: f64 = first.iter().sum();
    let credits: Vec<f64> = turns.iter().map(|t| t.credits).collect();
    let total_credits: f64 = credits.iter().sum();
    let mut histogram: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_steps: BTreeMap<usize, Vec<f64>> = BTreeMap::new();
    let mut classes: BTreeMap<&str, (usize, HashSet<&str>, f64, f64, f64, f64)> = BTreeMap::new();
    let mut sets: BTreeMap<String, (usize, f64)> = BTreeMap::new();
    let mut tools: BTreeMap<&str, (usize, usize, usize, BTreeMap<String, usize>)> = BTreeMap::new();
    for t in &answered {
        let n = t.silent.len() + 1;
        *histogram.entry(if n >= 5 { "5+".into() } else { n.to_string() }).or_default() += 1;
        by_steps.entry(n.min(4)).or_default().push(t.first_text_ms.unwrap_or(0.0));
        for s in &t.silent {
            let c = classes.entry(s.class).or_default();
            c.0 += 1; c.1.insert(&t.turn); c.2 += s.wait_ms; c.3 += s.input; c.4 += s.input - s.cached; c.5 += s.credits;
            let mut names: Vec<&str> = s.tools.iter().map(String::as_str).collect::<HashSet<_>>().into_iter().collect();
            names.sort();
            let set = sets.entry(if names.is_empty() { "(none)".into() } else { names.join("+") }).or_default();
            set.0 += 1; set.1 += s.wait_ms;
            for name in names { tools.entry(name).or_default().2 += 1; }
        }
    }
    for t in turns {
        for tool in &t.tools {
            let row = tools.entry(&tool.name).or_default();
            row.0 += 1;
            if tool.failed { row.1 += 1; *row.3.entry(tool.error.clone().unwrap_or_else(|| "failed".into())).or_default() += 1; }
        }
    }
    let silent_wait: f64 = classes.values().map(|c| c.2).sum();
    let mut ranked: Vec<Value> = classes.iter().map(|(class, c)| json!({"class": class, "waste_candidate": WASTE.contains(class),
        "steps": c.0, "turns": c.1.len(), "turn_share": share(c.1.len() as f64, answered.len() as f64), "wait_ms": c.2,
        "wait_share": share(c.2, total_first), "resent_input_tokens": c.3, "uncached_resent_tokens": c.4, "credits": c.5, "credit_share": share(c.5, total_credits)})).collect();
    ranked.sort_by(|a, b| b["wait_ms"].as_f64().unwrap_or(0.0).total_cmp(&a["wait_ms"].as_f64().unwrap_or(0.0))
        .then(b["credits"].as_f64().unwrap_or(0.0).total_cmp(&a["credits"].as_f64().unwrap_or(0.0))));
    let class_map: Map<String, Value> = ranked.iter().map(|c| (c["class"].as_str().unwrap_or("").to_string(), c.clone())).collect();
    let tool_map: Map<String, Value> = tools.iter().map(|(name, t)| (name.to_string(), json!({"calls": t.0, "failed": t.1,
        "failure_rate": share(t.1 as f64, t.0 as f64), "silent_steps": t.2,
        "silent_steps_per_100_turns": if answered.is_empty() { Value::Null } else { json!(round(100.0 * t.2 as f64 / answered.len() as f64, 1)) }, "errors": t.3,
        "errors_per_100_turns": t.3.iter().map(|(code, n)| (code.clone(), json!(round(100.0 * *n as f64 / turns.len().max(1) as f64, 2)))).collect::<Map<_, _>>()}))).collect();
    let mut top_sets: Vec<Value> = sets.iter().map(|(set, s)| json!({"tools": set, "steps": s.0, "wait_ms": s.1})).collect();
    top_sets.sort_by(|a, b| b["wait_ms"].as_f64().unwrap_or(0.0).total_cmp(&a["wait_ms"].as_f64().unwrap_or(0.0)));
    top_sets.truncate(15);
    let mut worst: Vec<&&Turn> = answered.iter().filter(|t| !t.silent.is_empty()).collect();
    let waste = |t: &Turn| t.silent.iter().filter(|s| WASTE.contains(&s.class)).map(|s| s.wait_ms).sum::<f64>();
    worst.sort_by(|a, b| waste(b).total_cmp(&waste(a)).then(b.first_text_ms.unwrap_or(0.0).total_cmp(&a.first_text_ms.unwrap_or(0.0))));
    let worst: Vec<Value> = worst.into_iter().take(WORST).map(|t| json!({"thread": t.thread, "turn": t.turn, "user": t.user, "at_ms": t.at_ms, "level": t.level,
        "first_text_ms": t.first_text_ms, "total_ms": t.total_ms, "credits": t.credits, "waste_wait_ms": waste(t),
        "silent_steps": t.silent.iter().map(|s| json!({"class": s.class, "tools": s.tools, "wait_ms": s.wait_ms})).collect::<Vec<_>>()})).collect();
    let (input, cached): (f64, f64) = turns.iter().fold((0.0, 0.0), |a, t| (a.0 + t.input, a.1 + t.cached));
    let users: HashSet<&str> = turns.iter().map(|t| t.user.as_str()).collect();
    let largest = users.iter().map(|u| turns.iter().filter(|t| t.user == *u).count()).max().unwrap_or(0);
    json!({
        "turns": turns.len(), "answered": answered.len(), "no_text": turns.len() - answered.len(),
        "not_completed": turns.iter().filter(|t| !t.completed).count(), "users": users.len(), "largest_user_share": share(largest as f64, turns.len() as f64),
        "window": {"from_ms": turns.first().map(|t| t.at_ms), "to_ms": turns.last().map(|t| t.at_ms)},
        "steps_before_first_text": histogram,
        "steps": {"mean": if answered.is_empty() { Value::Null } else { json!(round(answered.iter().map(|t| t.silent.len() as f64 + 1.0).sum::<f64>() / answered.len() as f64, 2)) },
            "p50": quantile(&answered.iter().map(|t| t.silent.len() as f64 + 1.0).collect::<Vec<_>>(), 0.5)},
        "first_text": {"p50_ms": quantile(&first, 0.5), "p90_ms": quantile(&first, 0.9), "total_ms": total_first,
            "by_steps": by_steps.iter().map(|(n, v)| (if *n >= 4 { "4+".to_string() } else { n.to_string() }, json!({"turns": v.len(), "p50_ms": quantile(v, 0.5), "p90_ms": quantile(v, 0.9)}))).collect::<Map<_, _>>(),
            "silent_step_share": share(silent_wait, total_first)},
        "classes": class_map, "ranked": ranked, "tools": tool_map, "silent_tool_sets": top_sets,
        "tokens": {"main_input": input, "main_cached": cached, "cache_hit": share(cached, input),
            "resent_by_silent_steps": classes.values().map(|c| c.3).sum::<f64>(), "sub_agent_calls": turns.iter().map(|t| t.sub_calls).sum::<usize>(),
            "sub_agent_input": turns.iter().map(|t| t.sub_input).sum::<f64>()},
        "credits": {"total": total_credits, "p50": quantile(&credits, 0.5), "p90": quantile(&credits, 0.9), "silent_steps": classes.values().map(|c| c.5).sum::<f64>()},
        "transport_retries": turns.iter().map(|t| t.retries).sum::<usize>(), "repeated_calls": turns.iter().map(|t| t.repeats).sum::<usize>(),
        "main_steps": turns.iter().map(|t| t.steps).sum::<usize>(),
        "worst": worst,
        "cache": super::cache::summarize(turns, prior), "cost": super::cost::summarize(turns),
    })
}

/// A dotted path into a summary (`classes.error_retry.wait_share`, `tools.create.failure_rate`).
pub(super) fn metric(summary: &Value, path: &str) -> Option<f64> {
    path.split('.').try_fold(summary, |v, key| v.get(key)).and_then(Value::as_f64)
}
