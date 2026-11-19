//! Agent GUI activity behind a hot WindowServer (cause-tree probe `ws_hot_agent_gui`, node N3.4.1).
//! A `windowserver_hot` window never reaches a perf_class share: one process at 80% of one core is
//! a few percent of the CPU the frames see (2026-11-13 11:39: N8.9 at 0.21 on a 62% busy host), so
//! every such episode opened a model session. Measured twice (2026-10-27 night, 2026-11-13 11:39)
//! the load was a computer-use service taking screenshots: each one starts a ScreenCaptureKit
//! stream that WindowServer composites. The probe holds when the node's `match` names appear in at
//! least `min_frames` cpu-windows frames of the window and the render probe, which must have run,
//! counted no stall there, so the desktop stayed usable.
use crate::monitor::root;
use serde_json::{json, Value};
use std::path::Path;

pub(crate) const PROBE: &str = "ws_hot_agent_gui";
const TRIGGER: &str = "windowserver_hot";
const DAY_MS: f64 = 86_400_000.0;
const DEFAULT_MIN_FRAMES: u64 = 2;

/// Render-probe ticks inside `from..to` and the stalls (fences of 250 ms or more) they counted.
fn render(home: &Path, from: f64, to: f64) -> (u64, u64) {
    let (mut ticks, mut stalls) = (0, 0);
    let mut day = (from / DAY_MS).floor() * DAY_MS;
    while day <= to {
        let text = std::fs::read_to_string(root(home).join(format!("render-probe/{}.ndjson", day as u64))).unwrap_or_default();
        for tick in text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|t| t["at_ms"].as_f64().is_some_and(|at| at >= from && at <= to)) {
            ticks += 1;
            stalls += tick["stall_count"].as_u64().unwrap_or(0);
        }
        day += DAY_MS;
    }
    (ticks, stalls)
}

/// The probe result for a `windowserver_hot` event over `frames` (the cpu-windows frames of
/// `from..to`); None for another trigger or a tree without the node. `explains` needs an answered
/// node: missing render-probe ticks are missing evidence, never a healthy desktop.
pub(crate) fn probe(home: &Path, tree: &Value, event: &Value, frames: &[Value], from: f64, to: f64) -> Option<Value> {
    if event["trigger"] != TRIGGER { return None; }
    let node = super::nodes(tree).iter().find(|n| n["probe"] == PROBE)?;
    let names: Vec<&str> = node["match"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let hits: Vec<f64> = frames.iter().map(|frame| frame["cpu"]["by_name"].as_array().into_iter().flatten()
        .filter(|p| p["name"].as_str().is_some_and(|n| names.contains(&n)))
        .filter_map(|p| p["cpu_percent_one_core"].as_f64()).sum::<f64>()).filter(|cpu| *cpu > 0.0).collect();
    let (ticks, stalls) = render(home, from, to);
    let need = node["min_frames"].as_u64().unwrap_or(DEFAULT_MIN_FRAMES);
    let seen = hits.len() as u64;
    let cpu = if hits.is_empty() { 0.0 } else { (hits.iter().sum::<f64>() / hits.len() as f64).round() };
    Some(json!({"node": node["id"], "agent_frames": seen, "frames": frames.len(), "min_frames": need, "agent_cpu_one_core": cpu,
        "render_ticks": ticks, "render_stalls": stalls,
        "explains": seen >= need && ticks > 0 && stalls == 0 && super::answered(node)}))
}

#[cfg(test)]
#[path = "gui_tests.rs"]
mod tests;
