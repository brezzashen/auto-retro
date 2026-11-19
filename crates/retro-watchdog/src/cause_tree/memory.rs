//! The largest memory holder behind a `memory_exhausted` episode (cause-tree probe `mem_holder`).
//! CPU class shares cannot explain memory: on 2026-11-15 (perf retro 1794671950116) fseventsd held
//! 38 GiB of footprint at 5% of host CPU, and nine episodes were compared with rustc's CPU share.
//! The memory guard now records the largest footprints in the incident (`relief.holders`); a node
//! with this probe explains the episode when the largest holder is one of its `match` names and
//! holds at least `min_gib`. It outranks a CPU class: who burns CPU does not say who holds memory.
//! A process-tree group (`relief.holders.groups`, e.g. `simulators`) larger than the largest single
//! process is the holder instead: on 2026-11-17 (perf retro 1794856006950) six simulators held about
//! 58 GiB in processes of under 1 GiB each while the largest row was WindowServer at 4.1 GiB.
use serde_json::{json, Value};

pub(crate) const PROBE: &str = "mem_holder";
const TRIGGER: &str = "memory_exhausted";
const DEFAULT_MIN_GIB: f64 = 16.0;

/// The probe result for a `memory_exhausted` event; None for another trigger or an incident
/// written before the guard recorded holders. `explains` needs an answered node.
pub(crate) fn probe(tree: &Value, event: &Value) -> Option<Value> {
    if event["trigger"] != TRIGGER { return None; }
    let captured = &event["relief"]["holders"];
    let first = |key: &str| captured[key].as_array().and_then(|rows| rows.first())
        .and_then(|row| Some((row["name"].as_str()?, row["footprint_gib"].as_f64()?)));
    let (name, footprint) = match (first("holders"), first("groups")) {
        (Some(row), Some(group)) => if group.1 > row.1 { group } else { row },
        (row, group) => row.or(group)?,
    };
    let node = super::nodes(tree).iter().find(|n| n["probe"] == PROBE
        && n["match"].as_array().into_iter().flatten().filter_map(Value::as_str).any(|m| m == name));
    let need = node.and_then(|n| n["min_gib"].as_f64()).unwrap_or(DEFAULT_MIN_GIB);
    Some(json!({"node": node.map(|n| n["id"].clone()), "holder": name, "footprint_gib": footprint, "min_gib": need,
        "explains": node.is_some_and(|n| footprint >= need && super::answered(n))}))
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
