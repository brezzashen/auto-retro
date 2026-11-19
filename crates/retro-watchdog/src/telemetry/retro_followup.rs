//! Recursive retro follow-up (founder, 2026-10-25): every conversation or runtime retro finding
//! that ships a change also registers a probe in `references/retro-probes.json`, a pattern over the
//! tool calls (or results) of agent transcripts. Each later retro first measures every probe in an
//! equal window before and after its change, so a change whose repetition did not shrink becomes
//! the next question instead of being re-derived or forgotten (eight retros before 2026-10-25 left
//! their effects `NOT-MEASURED` and none was re-measured).
//!
//! `cpu-watchdog retro-followup [<incident>]` prints the measurement and, for an incident, writes
//! `analysis/<id>/followups.json`. `monitor-ack` on an incident with `requirements.followups`
//! requires that receipt, a report.json `followups[]` verdict per probe, and a `probe` or
//! `no_probe_reason` on every finding.
use super::retro_followup_printed as printed;
use super::transcript_calls;
use crate::monitor::{root, write_json};
use anyhow::{ensure, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::{collections::{BTreeMap, BTreeSet}, path::Path};

/// Neither window is longer than this, so old behaviour does not dilute the comparison.
const MAX_WINDOW_MS: f64 = 72.0 * 3_600_000.0;
/// Fewer sessions than this in either window is too few to judge.
const MIN_SESSIONS: usize = 20;
/// Calls per 100 sessions under which a residue is noise: R18 and R41 measured 1 -> 3 calls in 477 -> 406
/// sessions (0.2 -> 0.7) and were judged `regressed` 39 and 12 retros in a row on single re-checks
/// (conversation retro 83835245359681385207), so they could never graduate.
const NOISE_PER_100: f64 = 1.0;
const VERDICTS: [&str; 4] = ["effective", "ineffective", "regressed", "insufficient_data"];
/// A tool call naming one of these belongs to a watchdog retro (or work on the retro machinery):
/// such a session quotes probe patterns and transcript lines, so it would count its own reading
/// as the repetition (incident 817393107815249082576: R4, R25, R28 and R36 "regressed" only through
/// retro sessions). Those sessions leave both windows, numerator and denominator.
const RETRO_MARKERS: [&str; 4] = ["cpu-watchdog/analysis/", "retro-probes.json", "cpu-watchdog retro-", "cpu-watchdog monitor-ack"];

fn is_retro_call(result: bool, text: &str) -> bool {
    !result && RETRO_MARKERS.iter().any(|m| text.contains(m))
}

pub(crate) fn registry(home: &Path) -> Result<Value> {
    let path = home.join(".codex/skills/watchdog/references/retro-probes.json");
    serde_json::from_slice(&std::fs::read(&path).with_context(|| format!("read {}", path.display()))?).context("retro-probes.json is not JSON")
}

/// `retro: true` keeps watchdog retro sessions in both windows: a probe that measures the retro
/// tooling itself (R67 log-verify contract errors, R52 hand chmod of analysis/<id>) only occurs
/// there, so the RETRO_MARKERS exclusion left it 0/0 `insufficient_data` in every retro
/// (conversation retro 817562151098843524452: 12 duplicate_analysis refusals in window, R67 counted 0).
struct Probe { id: String, needles: Vec<String>, pattern: Regex, after: Option<Regex>, within: usize, within_ms: Option<f64>, repeat: bool, same_match: bool, requires: Option<Regex>, result: bool, min_calls: usize, shipped: f64, retro: bool, printed: bool }

fn probes(registry: &Value) -> Result<Vec<Probe>> {
    let regex = |v: &Value| v.as_str().map(Regex::new).transpose();
    registry["probes"].as_array().into_iter().flatten().map(|p| Ok(Probe {
        id: p["id"].as_str().context("probe without id")?.to_string(),
        needles: p["needles"].as_array().filter(|n| !n.is_empty()).context("probe without needles")?.iter().filter_map(|n| n.as_str().map(String::from)).collect(),
        pattern: regex(&p["pattern"])?.context("probe without pattern")?,
        after: regex(&p["after"])?, within: p["within"].as_u64().unwrap_or(2) as usize, within_ms: p["within_ms"].as_f64(), repeat: p["repeat"] == true, same_match: p["same_match"] == true, requires: regex(&p["requires"])?,
        result: p["on"] == "result", min_calls: p["min_calls"].as_u64().unwrap_or(1) as usize,
        shipped: p["shipped"].as_str().and_then(crate::cause_tree::attribute::parse_time).context("probe without shipped time")?, retro: p["retro"] == true, printed: p["printed"] == true,
    })).collect()
}

/// Per session: tool calls `(at, is_result, text, path:line)` in time order.
type Sessions = BTreeMap<String, Vec<(f64, bool, String, String)>>;
/// Locators kept per window, so a retro reads the original lines behind a count without a search.
const EXAMPLES: usize = 5;

/// One probe over one window: sessions in the denominator, sessions hit, matching calls. With
/// `after`, only sessions that ran an `after` call count, and only matches in the `within` calls
/// that follow one. With `repeat`, a match counts only when the previous match came within the last
/// `within` calls: a re-run of the same lookup, not a single checkpoint (R18: an `after` equal to the
/// pattern counts nothing, because the triggering call is never itself counted). With `within_ms`, the
/// previous match must also be that recent: a /loop status report every 5 minutes runs two calls a
/// turn, so R18 counted founder-requested 5-minute snapshots as re-runs (conversation retro
/// 88170115502296253347, 15de5a08:255, 289, 330). With `same_match`, the repeat must also match the same
/// text (capture group 1, else the whole match): R18/R41 count a re-run on the same session id, not
/// two lookups of different sessions bundled with other work (conversation retro 810032196367849314096).
fn measure(probe: &Probe, sessions: &Sessions, retro: &BTreeSet<String>, from: f64, to: f64) -> Value {
    let (mut total, mut hit, mut calls) = (0usize, 0usize, 0usize);
    let mut examples: Vec<&str> = Vec::new();
    for (id, rows) in sessions {
        if !probe.retro && retro.contains(id) { continue; }
        let rows: Vec<&(f64, bool, String, String)> = rows.iter().filter(|r| r.0 >= from && r.0 < to).collect();
        if rows.is_empty() || probe.requires.as_ref().is_some_and(|r| !rows.iter().any(|c| !c.1 && r.is_match(&c.2))) { continue; }
        let (mut counted, mut found, mut since, mut gap, mut last_ms) = (probe.after.is_none(), 0usize, usize::MAX, usize::MAX, f64::NEG_INFINITY);
        let mut last_key: Option<String> = None;
        let mut shown: Vec<printed::Span> = Vec::new();
        for (at_ms, result, text, at) in rows.iter().map(|r| (r.0, r.1, &r.2, &r.3)) {
            if let Some(after) = &probe.after {
                if !result && after.is_match(text) { (counted, since) = (true, 0); shown.clear(); continue; }
                if !result { since = since.saturating_add(1); }
                if since > probe.within { continue; }
            }
            if probe.printed && result { shown.extend(printed::printed(text)); continue; }
            if !result { gap = gap.saturating_add(1); }
            if result != probe.result { continue; }
            if let Some(caps) = probe.pattern.captures(text) {
                let key = caps.get(1).or_else(|| caps.get(0)).map(|m| m.as_str().to_string());
                let same = !probe.same_match || last_key == key;
                let repeated = same && gap <= probe.within && probe.within_ms.is_none_or(|ms| at_ms - last_ms <= ms);
                (gap, last_ms, last_key) = (0, at_ms, key);
                if probe.repeat && !repeated { continue; }
                if probe.printed && !printed::read(text).is_some_and(|r| printed::redundant(&r, &shown)) { continue; }
                found += 1;
                if examples.len() < EXAMPLES { examples.push(at); }
            }
        }
        if !counted { continue; }
        total += 1;
        calls += found;
        hit += usize::from(found >= probe.min_calls);
    }
    json!({"sessions": total, "sessions_hit": hit, "calls": calls, "examples": examples,
        "share": if total > 0 { json!((hit as f64 / total as f64 * 1000.0).round() / 1000.0) } else { Value::Null },
        "calls_per_100_sessions": if total > 0 { json!((calls as f64 / total as f64 * 1000.0).round() / 10.0) } else { Value::Null }})
}
/// The verdict the numbers suggest; the retro confirms or overrides it with its reading.
fn suggest(before: &Value, after: &Value) -> &'static str {
    let n = |v: &Value| v["sessions"].as_u64().unwrap_or(0) as usize;
    if n(before) < MIN_SESSIONS || n(after) < MIN_SESSIONS { return "insufficient_data"; }
    let rate = |v: &Value| v["calls_per_100_sessions"].as_f64().unwrap_or(0.0);
    match (rate(before), rate(after)) {
        (b, a) if b == 0.0 && a == 0.0 => "insufficient_data",
        (b, a) if b < NOISE_PER_100 && a < NOISE_PER_100 => "effective",
        (b, a) if a <= b * 0.7 => "effective",
        (b, a) if a >= b * 1.3 => "regressed",
        _ => "ineffective",
    }
}

/// The after window: the latest `MAX_WINDOW_MS` before `now_ms`, never before the change shipped. Measuring
/// shipped..now let the first days after an old change decide its verdict forever: R4, R9, R11, R18, R21,
/// R24 and R25 kept citing 2026-10-27 sessions as "after" evidence in every retro through 2026-11-02
/// (conversation retro 83785989888175541344), so their escalations could not move.
pub(crate) fn after_window(shipped: f64, now_ms: f64) -> (f64, f64) {
    (shipped.max(now_ms - MAX_WINDOW_MS), now_ms)
}

pub(crate) fn followups(home: &Path, now_ms: f64) -> Result<Vec<Value>> {
    let registry = registry(home)?;
    let probes = probes(&registry)?;
    let lifecycle = crate::retro_consolidate::lifecycle(home);
    let span = |p: &Probe| (now_ms - p.shipped).clamp(0.0, MAX_WINDOW_MS);
    let from = probes.iter().map(|p| p.shipped - span(p)).fold(now_ms, f64::min);
    let needles: Vec<&str> = probes.iter().flat_map(|p| p.needles.iter().map(String::as_str)).collect();
    let mut sessions: Sessions = BTreeMap::new();
    let mut copies = transcript_calls::Copies::default();
    let mut retro: BTreeSet<String> = BTreeSet::new();
    for (_, path) in transcript_calls::files(home, from) {
        // Every row counts for denominators and `within`; only rows holding a needle keep their text.
        transcript_calls::scan(&path, from, now_ms, &[], &mut |call| {
            if !copies.first(&call) { return; }
            if is_retro_call(call.result, &call.text) { retro.insert(call.session.to_string()); }
            let keep = needles.iter().any(|n| call.text.contains(n)) || (call.result && call.text.contains("==> "));
            let entry = sessions.entry(call.session.to_string()).or_default();
            let at = format!("{}:{}", call.path.display(), call.line);
            entry.push((call.at_ms, call.result, if keep { call.text } else { String::new() }, at));
        });
    }
    for rows in sessions.values_mut() { rows.sort_by(|a, b| a.0.total_cmp(&b.0)); }
    Ok(probes.iter().zip(registry["probes"].as_array().into_iter().flatten()).map(|(p, raw)| {
        let before = measure(p, &sessions, &retro, p.shipped - span(p), p.shipped);
        let (after_from, after_to) = after_window(p.shipped, now_ms);
        let after = measure(p, &sessions, &retro, after_from, after_to);
        json!({"probe": p.id, "finding": raw["finding"], "change": raw["change"], "shipped": raw["shipped"], "after_from_ms": after_from,
            "window_hours": (span(p) / 3_600_000.0 * 10.0).round() / 10.0, "before": before, "after": after, "suggested": suggest(&before, &after),
            "lifecycle": crate::retro_consolidate::status(&lifecycle, &p.id),
            "required": required(&crate::retro_consolidate::status(&lifecycle, &p.id), suggest(&before, &after))})
    }).collect())
}

/// A graduated probe (retro_consolidate) needs a verdict again once it measures ineffective or regressed.
fn required(lifecycle: &str, suggested: &str) -> bool {
    lifecycle != "graduated" || matches!(suggested, "ineffective" | "regressed")
}

/// The prompt sentence of a retro that must follow up on the earlier ones.
pub(super) fn instruction(id: &str) -> String {
    format!("Before new findings run cpu-watchdog retro-followup {id}: give every probe with required: true (a graduated probe needs one only when it measures ineffective or regressed) a report.json followups[] entry \
        (probe, verdict effective|ineffective|regressed|insufficient_data, analysis from original transcript lines) and, \
        for an ineffective or regressed change, next: find why the repetition persisted and fix that first. \
        Register each new finding's measurable pattern in ~/.codex/skills/watchdog/references/retro-probes.json and name it as the finding's probe, or give no_probe_reason.")
}

/// `cpu-watchdog retro-followup [<incident>]`.
pub(crate) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let now = crate::unix_now() * 1000.0;
    let rows = followups(home, now)?;
    let value = json!({"at_ms": now, "probes": rows});
    if let Some(id) = args.first() {
        ensure!(root(home).join("incidents").join(format!("{id}.json")).is_file(), "incident {id} does not exist");
        write_json(&root(home).join("analysis").join(id).join("followups.json"), &value)?;
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

/// monitor-ack gate for retros created with `requirements.followups`.
pub(crate) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    if event["requirements"]["followups"].is_null() { return Ok(()); }
    let analysis = root(home).join("analysis").join(id);
    let receipt: Value = serde_json::from_slice(&std::fs::read(analysis.join("followups.json"))
        .context("run cpu-watchdog retro-followup <id> first: analysis/<id>/followups.json is missing")?)?;
    let report: Value = serde_json::from_slice(&std::fs::read(analysis.join("report.json")).context("report.json missing")?)?;
    let answered = report["followups"].as_array().cloned().unwrap_or_default();
    for probe in receipt["probes"].as_array().into_iter().flatten() {
        if probe["required"] == false { continue; }
        let pid = probe["probe"].as_str().unwrap_or_default();
        let entry = answered.iter().find(|f| f["probe"] == pid).with_context(|| format!("report.json followups[] has no entry for probe {pid}"))?;
        let verdict = entry["verdict"].as_str().unwrap_or_default();
        ensure!(VERDICTS.contains(&verdict), "followups {pid}: verdict must be one of {VERDICTS:?}");
        ensure!(entry["analysis"].as_str().is_some_and(|a| !a.trim().is_empty()), "followups {pid}: analysis is empty");
        ensure!(!matches!(verdict, "ineffective" | "regressed") || entry["next"].as_str().is_some_and(|n| !n.trim().is_empty()),
            "followups {pid}: an {verdict} change needs next (the follow-up finding or why none)");
    }
    let known: Vec<String> = registry(home).map(|r| r["probes"].as_array().into_iter().flatten().filter_map(|p| p["id"].as_str().map(String::from)).collect()).unwrap_or_default();
    for (index, finding) in report["findings"].as_array().into_iter().flatten().enumerate() {
        let probe = finding["probe"].as_str();
        ensure!(probe.is_some_and(|p| known.iter().any(|k| k == p)) || finding["no_probe_reason"].as_str().is_some_and(|r| !r.trim().is_empty()),
            "findings[{index}] needs probe (an id added to retro-probes.json) or no_probe_reason");
    }
    Ok(())
}

#[cfg(test)]
#[path = "retro_followup_tests.rs"]
mod tests;
