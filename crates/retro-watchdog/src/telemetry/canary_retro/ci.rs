//! The fourth Canary retro check (founder, 2026-11-02): what in the release process belongs in CI
//! or in a check ahead of the release, so a release spends its minutes on the rolling update and
//! never finds what a commit-time check could have found. Script signals, each with `s`, the
//! release seconds it cost, so the retro ranks by time and rejects a CI step that costs every
//! release more than it saves (no over-design that slows releases down):
//!
//! - `late_failure`: the run failed on a `code`, `input` or `tooling` class (release_retro
//!   `FAILURES` groups) after more than LATE_FAIL_S. A check before the release (deliver refusing
//!   before it builds, the touched suites at push time, a CI lane) would have stopped it without a
//!   wasted release. 40 Canary runs to 2026-11-01 held eight such failures (seven `test`, one
//!   `typecheck`; 247-1364 s each). Environment, policy and operator classes are not CI work.
//! - `preflight_on_path`: a live run whose preflight outlasted Cloud Build by more than
//!   PREFLIGHT_EXCESS_MAX_S. Those seconds leave the release path if the same suites run before the
//!   release and the release reuses that commit's green result (preflight led 15 of those 40 runs).
use serde_json::{json, Value};

/// A failure inside the first minute (migration gate, missing flag) already costs almost nothing.
pub(crate) const LATE_FAIL_S: f64 = 60.0;
/// Preflight beyond the Cloud Build path by more than this is release time a pre-release check could take.
pub(crate) const PREFLIGHT_EXCESS_MAX_S: f64 = 60.0;
const CI_GROUPS: [&str; 3] = ["code", "input", "tooling"];

/// The CI signals of one run. `class` is the board failure class, `build_path` the Cloud Build queue plus run seconds.
/// `prepare_s` caps the preflight excess: a run that found its prepare done waited only that long.
pub(super) fn signals(outcome: &str, class: &Value, seconds: Option<f64>, live: bool, preflight_s: Option<f64>, build_path: f64, prepare_s: Option<f64>) -> Vec<Value> {
    let mut out = Vec::new();
    if let (Some(class), Some(s)) = (class.as_str(), seconds) {
        let group = super::super::release_retro::stats::group_of(class);
        if outcome == "failed" && CI_GROUPS.contains(&group) && s > LATE_FAIL_S {
            out.push(json!({"kind": "late_failure", "class": class, "group": group, "s": s}));
        }
    }
    if let Some(excess) = preflight_s.filter(|_| live).map(|p| (p - build_path).min(prepare_s.unwrap_or(f64::INFINITY))).filter(|e| *e > PREFLIGHT_EXCESS_MAX_S) {
        out.push(json!({"kind": "preflight_on_path", "preflight_s": preflight_s, "build_path_s": build_path, "s": excess.round()}));
    }
    out
}

/// Window totals: seconds per kind and per failure class, largest first, so the retro fixes the costliest first.
pub(super) fn cost(rows: &[Value]) -> Value {
    let mut by: std::collections::BTreeMap<String, f64> = Default::default();
    for item in rows.iter().flat_map(|r| r["ci"].as_array().into_iter().flatten()) {
        let key = match item["class"].as_str() { Some(c) => format!("{}:{c}", item["kind"].as_str().unwrap_or("?")), None => item["kind"].as_str().unwrap_or("?").into() };
        *by.entry(key).or_default() += item["s"].as_f64().unwrap_or(0.0);
    }
    let mut ranked: Vec<(String, f64)> = by.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    json!({"total_s": ranked.iter().map(|r| r.1).sum::<f64>(), "ranked": ranked.iter().map(|(k, s)| json!({"key": k, "s": s})).collect::<Vec<_>>(),
        "late_fail_s": LATE_FAIL_S, "preflight_excess_max_s": PREFLIGHT_EXCESS_MAX_S})
}

/// crm-ci's verdicts on mastra heads checked inside the window (`~/.local/state/crm-ci/runs.jsonl`,
/// founder 2026-11-02): how often CI went red before a release did, and its false positives.
/// A row counts by when its verdict was known (`at_ms` is the run start, plus `seconds`): crm-ci's red
/// run on 1757869ced started 4.7 s before canary run 20261109T093309Z-canary-92284 opened the window and
/// finished inside it, and a start-time filter reported `red: 0` (retro 81747139553455209209).
pub(crate) fn runner(home: &std::path::Path, from_ms: f64, to_ms: f64) -> Value {
    let rows: Vec<Value> = std::fs::read_to_string(home.join(".local/state/crm-ci/runs.jsonl")).unwrap_or_default().lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|r| r["at_ms"].as_f64().map(|t| t + r["seconds"].as_f64().unwrap_or(0.0) * 1000.0).is_some_and(|t| t >= from_ms && t <= to_ms)).collect();
    let count = |v: &str| rows.iter().filter(|r| r["verdict"] == v).count();
    let (flaky, red) = (count("flaky"), count("red"));
    json!({"heads": rows.len(), "green": count("green"), "flaky": flaky, "red": red, "error": count("error"),
        "false_positive_rate": if flaky + red == 0 { Value::Null } else { json!((flaky as f64 * 1000.0 / (flaky + red) as f64).round() / 10.0) },
        "red_heads": rows.iter().filter(|r| r["verdict"] == "red").map(|r| json!({"sha": r["sha"], "failures": r["failures"]})).collect::<Vec<_>>()})
}
