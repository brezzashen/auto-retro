//! Repeated-action census for conversation and runtime retros (founder, 2026-10-25: whether a retro
//! fires by count or by time, it must dig deeper into the repeated actions). History recall finds
//! what a retro asks about; this census finds what the agents actually did again and again, so a
//! loop such as decrypting reports by hand for two days cannot pass unnoticed. Every tool call of
//! the window becomes action signatures (`signature.rs`); each signature is ranked by extra calls
//! (calls past a session's first), stuck sessions (STUCK_RUN calls of it within STUCK_SPAN calls of
//! one session) and failed calls.
//!
//! `cpu-watchdog retro-repeats <incident> | --hours <n> [--top <n>] [--show <sig>] [--text]` prints the census (`--show`
//! adds every original call of one signature with its result; `--text` prints one line per row or call); for an incident it
//! writes `analysis/<id>/repeats.json`. Its `required` signatures (every recurring one the founder
//! pointed at in `references/retro-focus.json`, then the top REQUIRED not judged necessary in
//! `references/retro-necessary.json`) each need a report.json `repeats[]` deep dive before
//! `monitor-ack` accepts an incident created with `requirements.repeats`.
mod labels;
mod signature;
mod text;

use super::transcript_calls;
use crate::monitor::{root, write_json};
use anyhow::{ensure, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::{collections::{BTreeMap, VecDeque}, path::Path, sync::LazyLock};

pub(crate) use signature::signatures;

const STUCK_RUN: usize = 3;
const STUCK_SPAN: usize = 8;
/// Deep dives a retro owes; more would dilute the dig into the worst ones.
const REQUIRED: usize = 6;
const TOP: usize = 40;
const VERDICTS: [&str; 5] = ["scripted", "skill", "existing_entry", "necessary", "deferred"];
static FAILED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(exit code [1-9]|<error>|error:)|exited with code [1-9]|exit_code\W+[1-9]").unwrap());

#[derive(Default)]
struct Tally { calls: usize, failed: usize, duplicates: usize, retries: usize, sessions: BTreeMap<String, Vec<(usize, String)>>, sample: String }

/// Ranked signatures of every main-transcript tool call in `from_ms..=to_ms`; with `show`, also every
/// call carrying that signature and its result (`shown`), the original lines a deep dive starts from.
pub(crate) fn census(home: &Path, from_ms: f64, to_ms: f64, show: Option<&str>) -> Value {
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    let mut shown: Vec<Value> = Vec::new();
    let (mut sessions, mut calls) = (0usize, 0usize);
    let mut copies = transcript_calls::Copies::default();
    for (_, path) in transcript_calls::files(home, from_ms) {
        let (mut index, mut pending) = (0usize, VecDeque::<(Vec<String>, Option<usize>, Option<String>)>::new());
        let mut hindsight = labels::Session::default();
        transcript_calls::scan(&path, from_ms, to_ms, &[], &mut |call| {
            if call.result {
                let Some((sigs, drill, key)) = pending.pop_front() else { return };
                if let Some(i) = drill { shown[i]["result"] = json!(call.text.chars().take(300).collect::<String>()); }
                let failed = FAILED.is_match(call.text.trim_start());
                if let Some(key) = key { hindsight.result(&key, failed); }
                if !failed { return; }
                for sig in &sigs {
                    if let Some(tally) = tallies.get_mut(sig) { tally.failed += 1; }
                }
                return;
            }
            if !copies.first(&call) { pending.push_back((Vec::new(), None, None)); return; }
            index += 1;
            let sigs = signatures(&call.text);
            let (label, key) = hindsight.call(&call.text);
            for sig in &sigs {
                let tally = tallies.entry(sig.clone()).or_default();
                tally.calls += 1;
                tally.duplicates += usize::from(label == labels::Label::Duplicate);
                tally.retries += usize::from(label == labels::Label::Retry);
                if tally.sample.is_empty() { tally.sample = call.text.chars().take(200).collect(); }
                tally.sessions.entry(call.session.to_string()).or_default().push((index, format!("{}:{}", call.path.display(), call.line)));
            }
            let drill = show.filter(|s| sigs.iter().any(|sig| sig == s)).map(|_| {
                shown.push(json!({"at": format!("{}:{}", call.path.display(), call.line), "session": call.session,
                    "label": label.name(), "call": call.text.chars().take(600).collect::<String>()}));
                shown.len() - 1
            });
            pending.push_back((sigs, drill, key));
        });
        calls += index;
        sessions += usize::from(index > 0);
    }
    let mut rows: Vec<Value> = tallies.into_iter().filter_map(|(sig, tally)| row(sig, tally)).collect();
    rows.sort_by(|a, b| b["score"].as_u64().cmp(&a["score"].as_u64()));
    json!({"window": {"from_ms": from_ms, "to_ms": to_ms}, "sessions": sessions, "calls": calls,
        "method": format!("signatures per call (signature.rs); stuck = {STUCK_RUN} calls of one signature within {STUCK_SPAN} calls of a session; failed = the paired result reads as an error (heuristic)"),
        "top": rows, "shown": show.map(|s| json!({"signature": s, "calls": shown}))})
}

fn row(sig: String, tally: Tally) -> Option<Value> {
    let mut stuck = Vec::new();
    let mut extra = 0;
    for runs in tally.sessions.values() {
        extra += runs.len() - 1;
        if let Some(i) = (0..runs.len().saturating_sub(STUCK_RUN - 1))// checked_sub: a resumed transcript can restart call positions; release builds wrapped there
        // (never stuck) and debug builds panicked.
        .find(|&i| runs[i + STUCK_RUN - 1].0.checked_sub(runs[i].0).is_some_and(|d| d < STUCK_SPAN)) {
            stuck.push(runs[i].1.clone());
        }
    }
    if tally.sessions.len() < 3 && stuck.is_empty() { return None; }
    let mut examples: Vec<String> = stuck.iter().take(4).cloned().collect();
    examples.extend(tally.sessions.values().filter_map(|r| r.first().map(|f| f.1.clone())).filter(|l| !stuck.contains(l)).take(4 - examples.len()));
    Some(json!({"signature": sig, "sessions": tally.sessions.len(), "calls": tally.calls, "extra_calls": extra,
        "stuck_sessions": stuck.len(), "failed_calls": tally.failed, "score": extra + 5 * stuck.len() + tally.failed,
        "duplicate_calls": tally.duplicates, "retry_calls": tally.retries,
        "examples": examples, "sample": tally.sample}))
}

/// `signature`s of a references registry file (`retro-necessary.json`, `retro-focus.json`).
fn registry(home: &Path, file: &str) -> Vec<Value> {
    let path = home.join(".codex/skills/watchdog/references").join(file);
    let registry: Value = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    registry["signatures"].as_array().cloned().unwrap_or_default()
}

fn necessary(home: &Path) -> Vec<String> {
    registry(home, "retro-necessary.json").iter().filter_map(|s| s["signature"].as_str().map(String::from)).collect()
}

fn judged(known: &[String], sig: &str) -> bool {
    known.iter().any(|k| k == sig || k.strip_suffix('*').is_some_and(|p| sig.starts_with(p)))
}

/// Splits the ranking into the signatures a retro must dig into and those already judged necessary.
/// Focus signatures (repeats the founder pointed at, `retro-focus.json`) are required whenever they
/// recur in the window, however they rank; then come the top REQUIRED open ones. `top` rows are kept.
pub(crate) fn receipt(home: &Path, id: Option<&str>, mut census: Value, top: usize) -> Value {
    let known = necessary(home);
    let rows = census["top"].as_array().cloned().unwrap_or_default();
    let find = |sig: &str| rows.iter().find(|r| r["signature"] == sig);
    let focus: Vec<Value> = registry(home, "retro-focus.json").into_iter().map(|f| {
        let seen = f["signature"].as_str().and_then(find);
        json!({"signature": f["signature"], "concern": f["concern"], "recurring": seen.is_some(),
            "sessions": seen.map_or(json!(0), |r| r["sessions"].clone()), "calls": seen.map_or(json!(0), |r| r["calls"].clone()),
            "stuck_sessions": seen.map_or(json!(0), |r| r["stuck_sessions"].clone())})
    }).collect();
    let mut required: Vec<Value> = focus.iter().filter(|f| f["recurring"] == true).map(|f| f["signature"].clone()).collect();
    let (old, open): (Vec<&Value>, Vec<&Value>) = rows.iter().partition(|r| judged(&known, r["signature"].as_str().unwrap_or_default()));
    let ranked: Vec<Value> = open.iter().map(|r| r["signature"].clone()).filter(|s| !required.contains(s)).take(REQUIRED).collect();
    required.extend(ranked);
    let kept: Vec<Value> = rows.iter().enumerate().filter(|(i, r)| *i < top || required.contains(&r["signature"])).map(|(_, r)| r.clone()).collect();
    census["incident"] = json!(id);
    census["focus"] = json!(focus);
    census["required"] = json!(required);
    census["known_necessary"] = json!(old.iter().take(top).map(|r| json!({"signature": r["signature"], "sessions": r["sessions"], "calls": r["calls"]})).collect::<Vec<_>>());
    census["top"] = json!(kept);
    census
}

/// `cpu-watchdog retro-repeats <incident> | --hours <n> [--top <n>] [--show <signature>] [--text]`.
pub(crate) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let now = crate::unix_now() * 1000.0;
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str);
    let (top, show) = (flag("--top").and_then(|n| n.parse().ok()).unwrap_or(TOP), flag("--show"));
    let value = match args.first().map(String::as_str) {
        Some("--hours") => {
            let hours: f64 = flag("--hours").and_then(|h| h.parse().ok()).context("--hours needs a number")?;
            receipt(home, None, census(home, now - hours * 3_600_000.0, now, show), top)
        }
        Some(id) if !id.starts_with('-') => {
            let event: Value = serde_json::from_slice(&std::fs::read(root(home).join("incidents").join(format!("{id}.json"))).context("incident does not exist")?)?;
            let from = event["window"]["from_ms"].as_f64().context("incident has no window")?;
            let value = receipt(home, Some(id), census(home, from, event["window"]["to_ms"].as_f64().unwrap_or(now), show), top);
            // A drill-down only prints; the receipt keeps the ranking the gate checks.
            if show.is_none() { write_json(&root(home).join("analysis").join(id).join("repeats.json"), &value)?; }
            value
        }
        _ => anyhow::bail!("usage: cpu-watchdog retro-repeats <incident-id> | --hours <n> [--top <n>] [--show <signature>] [--text]"),
    };
    if args.iter().any(|a| a == "--text") { println!("{}", text::render(&value)); } else { println!("{}", serde_json::to_string_pretty(&value)?); }
    Ok(())
}

/// The prompt sentence of a retro that must dig into its repeated actions.
pub(super) fn instruction(id: &str) -> String {
    format!("Repeated actions are the core of this retro: run cpu-watchdog retro-repeats {id} and give every signature in analysis/{id}/repeats.json required[] \
        a report.json repeats[] deep dive (signature, why: the root cause from original transcript lines of why agents repeated it or got stuck on it, \
        e.g. no script, an existing script nobody finds, a broken script, an unclear skill; evidence path:line; verdict scripted|skill|existing_entry|necessary|deferred). \
        A meaningless repeat becomes a script or skill entry that gets the agent straight to the result (change, plus probe or no_probe_reason); \
        necessary goes into ~/.codex/skills/watchdog/references/retro-necessary.json with its reason; deferred needs next. \
        Drill into one signature with --show <signature> --text (one line per call: path:line [label] call => result). A focus[] entry leaves retro-focus.json only when its probe verdict is effective.")
}

/// monitor-ack gate for retros created with `requirements.repeats`.
pub(crate) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    if event["requirements"]["repeats"].is_null() { return Ok(()); }
    let analysis = root(home).join("analysis").join(id);
    let receipt: Value = serde_json::from_slice(&std::fs::read(analysis.join("repeats.json"))
        .context("run cpu-watchdog retro-repeats <id> first: analysis/<id>/repeats.json is missing")?)?;
    ensure!(receipt["incident"] == id, "repeats.json belongs to another incident");
    let report: Value = serde_json::from_slice(&std::fs::read(analysis.join("report.json")).context("report.json missing")?)?;
    let answered = report["repeats"].as_array().cloned().unwrap_or_default();
    let known = necessary(home);
    let probes: Vec<String> = super::retro_followup::registry(home).map(|r| r["probes"].as_array().into_iter().flatten()
        .filter_map(|p| p["id"].as_str().map(String::from)).collect()).unwrap_or_default();
    let text = |v: &Value| v.as_str().is_some_and(|s| !s.trim().is_empty());
    for sig in receipt["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let entry = answered.iter().find(|r| r["signature"] == sig).with_context(|| format!("report.json repeats[] has no deep dive for {sig}"))?;
        let verdict = entry["verdict"].as_str().unwrap_or_default();
        ensure!(VERDICTS.contains(&verdict), "repeats {sig}: verdict must be one of {VERDICTS:?}");
        ensure!(text(&entry["why"]), "repeats {sig}: why (the root cause of the repetition) is empty");
        ensure!(entry["evidence"].as_array().is_some_and(|e| !e.is_empty() && e.iter().all(text)), "repeats {sig}: evidence needs original path:line");
        match verdict {
            "necessary" => ensure!(judged(&known, sig), "repeats {sig}: add it to retro-necessary.json with its reason"),
            "deferred" => ensure!(text(&entry["next"]), "repeats {sig}: deferred needs next"),
            _ => {
                ensure!(text(&entry["change"]), "repeats {sig}: {verdict} needs change (the script or skill entry that replaces the repeat)");
                ensure!(entry["probe"].as_str().is_some_and(|p| probes.iter().any(|k| k == p)) || text(&entry["no_probe_reason"]),
                    "repeats {sig}: needs probe (an id in retro-probes.json) or no_probe_reason");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "retro_repeats/tests.rs"]
mod tests;
