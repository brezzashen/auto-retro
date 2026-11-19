//! Follow-up of shipped CRM turn fixes. Every fix a CRM turn retro ships is a probe in
//! `~/.codex/skills/watchdog/references/crm-turn-probes.json`:
//! `{id, finding, change, commit, shipped (ISO time it reached production), metric, direction, min_turns?}`.
//! `metric` is a dotted path into the window summary (`classes.error_retry.wait_share`,
//! `tools.create.errors_per_100_turns.true`, `steps.mean`); `direction` is `lower` or `higher`.
//!
//! `cpu-watchdog crm-turn-stats --followup <incident>` measures each probe over the turns of equal
//! spans (at most WINDOW_MS) before and after `shipped`, suggests a verdict and writes
//! `analysis/<id>/crm-turn-followups.json`; the retro judges each one in report.json `followups[]`.
use super::{analyze, fetch};
use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const WINDOW_MS: f64 = 72.0 * 3_600_000.0;
const MIN_TURNS: f64 = 50.0;
/// A relative change below this is noise, not an effect.
const EFFECT: f64 = 0.10;
pub(super) const VERDICTS: [&str; 4] = ["effective", "ineffective", "regressed", "insufficient_data"];

pub(super) fn registry(home: &Path) -> PathBuf { home.join(".codex/skills/watchdog/references/crm-turn-probes.json") }

pub(super) fn load(home: &Path) -> Vec<Value> {
    super::super::read(&registry(home)).and_then(|v| v["probes"].as_array().cloned()).unwrap_or_default()
}

/// A missing class, tool or cache cause row means none was seen: its counts and shares are zero.
fn value(summary: &Value, path: &str) -> Option<f64> {
    analyze::metric(summary, path).or_else(|| ["classes.", "tools.", "cache.causes."].iter().any(|p| path.starts_with(p)).then_some(0.0))
}

/// The suggested verdict for `before` and `after` summaries of one probe.
pub(super) fn judge(probe: &Value, before: &Value, after: &Value) -> Value {
    let path = probe["metric"].as_str().unwrap_or("");
    let min = probe["min_turns"].as_f64().unwrap_or(MIN_TURNS);
    let (b, a) = (value(before, path), value(after, path));
    let turns = |s: &Value| s["answered"].as_f64().unwrap_or(0.0);
    let verdict = match (b, a) {
        _ if turns(before) < min || turns(after) < min => "insufficient_data",
        (Some(b), Some(a)) => {
            let gain = if probe["direction"] == "higher" { a - b } else { b - a };
            let scale = b.abs().max(a.abs());
            if scale == 0.0 || gain.abs() / scale < EFFECT { "ineffective" } else if gain > 0.0 { "effective" } else { "regressed" }
        }
        _ => "insufficient_data",
    };
    json!({"probe": probe["id"], "metric": path, "direction": probe["direction"], "shipped": probe["shipped"], "suggested": verdict,
        "before": {"turns": turns(before), "value": b}, "after": {"turns": turns(after), "value": a}})
}

/// Measures every probe at `now_ms` and returns the receipt.
pub(super) fn measure(home: &Path, now_ms: f64) -> Result<Value> {
    let mut rows = Vec::new();
    for probe in load(home) {
        let shipped = super::super::release_retro::stats::ms(probe["shipped"].as_str())
            .with_context(|| format!("probe {} needs an ISO shipped time", probe["id"]))?;
        let span = (now_ms - shipped).clamp(0.0, WINDOW_MS);
        let summary = |from: f64, to: f64| -> Result<Value> { Ok(analyze::summarize_rows(&fetch::window(from, to)?)) };
        let (before, after) = if span > 0.0 { (summary(shipped - span, shipped)?, summary(shipped, shipped + span)?) } else { (Value::Null, Value::Null) };
        rows.push(judge(&probe, &before, &after));
    }
    Ok(json!({"at_ms": now_ms, "registry": registry(home), "probes": rows}))
}

pub(super) fn receipt(home: &Path, id: &str) -> PathBuf { root(home).join("analysis").join(id).join("crm-turn-followups.json") }

/// `crm-turn-stats --followup <incident>`: writes and prints the receipt.
pub(super) fn cli(home: &Path, id: &str) -> Result<()> {
    anyhow::ensure!(!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()), "invalid incident id");
    let mut out = measure(home, crate::unix_now() * 1000.0)?;
    out["incident"] = json!(id);
    let path = receipt(home, id);
    std::fs::create_dir_all(path.parent().context("receipt path")?)?;
    write_json(&path, &out)?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
