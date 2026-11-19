//! Host load beside a slow engine row (`host_load` next to `host_incidents`): the memory guard's
//! per-minute reading nearest the row (`memory-guard/series/<day>.ndjson`, `memory_series.rs`) and the
//! 5 s CPU samples covering it (`cpu-windows/<10-min start ms>.json`). Search-retro
//! 85504050829602525700: two of four `cpu_starved` rows had no host incident, yet one ran at memory
//! pressure level 2 with 0.06 GiB free and the other at 85-98 % host CPU; three retros in a row made
//! that match by hand. Read only; null parts when the files are missing.
use serde_json::{json, Value};
use std::path::Path;

/// A memory reading counts for a row when taken this close to the row's span (one per minute).
const MEMORY_NEAR_MS: f64 = 90_000.0;

/// CPU samples (5 s each) are taken this far around the row's span.
const CPU_PAD_MS: f64 = 5_000.0;

const CPU_WINDOW_MS: f64 = 600_000.0;

fn lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Nearest memory reading to `[at_ms - ms, at_ms]` within `MEMORY_NEAR_MS`.
fn memory(root: &Path, at_ms: f64, ms: f64) -> Value {
    let start = at_ms - ms;
    let dir = root.join("memory-guard/series");
    let days = [(start - MEMORY_NEAR_MS) / 86_400_000.0, (at_ms + MEMORY_NEAR_MS) / 86_400_000.0].map(|d| d.floor() as u64);
    let mut rows: Vec<Value> = Vec::new();
    for day in days[0]..=days[1] {
        rows.extend(lines(&dir.join(format!("{day}.ndjson"))));
    }
    let gap = |r: &Value| r["at_ms"].as_f64().map(|t| if t < start { start - t } else if t > at_ms { t - at_ms } else { 0.0 });
    rows.into_iter()
        .filter_map(|r| gap(&r).filter(|g| *g <= MEMORY_NEAR_MS).map(|g| (g, r)))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(Value::Null, |(_, r)| json!({"at_ms": r["at_ms"], "pressure_level": r["pressure_level"], "free_gib": r["free_gib"], "swap_used_gib": r["swap_used_gib"]}))
}

/// Max and mean host CPU percent of the samples overlapping the row's span, padded by `CPU_PAD_MS`.
fn cpu(root: &Path, at_ms: f64, ms: f64) -> Value {
    let (from, to) = (at_ms - ms - CPU_PAD_MS, at_ms + CPU_PAD_MS);
    let first = (from / CPU_WINDOW_MS).floor() as u64;
    let last = (to / CPU_WINDOW_MS).floor() as u64;
    let mut percents: Vec<f64> = Vec::new();
    for w in first..=last {
        let path = root.join("cpu-windows").join(format!("{}.json", w * CPU_WINDOW_MS as u64));
        let Some(v) = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
        for s in v["samples"].as_array().into_iter().flatten() {
            let (Some(a), Some(b), Some(p)) = (s["start"].as_f64(), s["end"].as_f64(), s["percent"].as_f64()) else { continue };
            if b >= from && a <= to { percents.push(p); }
        }
    }
    if percents.is_empty() { return Value::Null; }
    let max = percents.iter().cloned().fold(0.0, f64::max);
    let mean = percents.iter().sum::<f64>() / percents.len() as f64;
    json!({"samples": percents.len(), "max_percent": max.round(), "mean_percent": mean.round()})
}

/// `{memory, cpu}` for a row ending at `at_ms` that took `ms`; `root` is the cpu-watchdog state root.
pub(super) fn reading(root: &Path, at_ms: f64, ms: f64) -> Value {
    json!({"memory": memory(root, at_ms, ms), "cpu": cpu(root, at_ms, ms)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_row_names_the_memory_reading_and_cpu_samples_around_it() {
        let root = std::env::temp_dir().join(format!("host-load-{}", std::process::id()));
        std::fs::create_dir_all(root.join("memory-guard/series")).unwrap();
        std::fs::create_dir_all(root.join("cpu-windows")).unwrap();
        // Shape of 2026-11-17 12:14 SGT: a 2060 ms query, readings 64 s before and 85 s after it.
        std::fs::write(root.join("memory-guard/series/20774.ndjson"), concat!(
            r#"{"at_ms":1794888815000,"pressure_level":2,"free_gib":0.14,"swap_used_gib":23.49}"#, "\n",
            r#"{"at_ms":1794888879155,"pressure_level":2,"free_gib":0.06,"swap_used_gib":23.3}"#, "\n",
            r#"{"at_ms":1794888964000,"pressure_level":2,"free_gib":0.11,"swap_used_gib":23.2}"#, "\n")).unwrap();
        std::fs::write(root.join("cpu-windows/1794888600000.json"), r#"{"samples":[
            {"start":1794888888865,"end":1794888893900,"percent":58.0},
            {"start":1794888894023,"end":1794888899100,"percent":57.0},
            {"start":1794888999000,"end":1794889004000,"percent":99.0}]}"#).unwrap();
        let v = reading(&root, 1794888898367.0, 2060.0);
        assert_eq!(v["memory"]["free_gib"], 0.06);
        assert_eq!(v["memory"]["pressure_level"], 2);
        assert_eq!(v["cpu"]["samples"], 2);
        assert_eq!(v["cpu"]["max_percent"], 58.0);
        // Far from every reading and sample: both parts null.
        let far = reading(&root, 1794880000000.0, 500.0);
        assert!(far["memory"].is_null() && far["cpu"].is_null());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
