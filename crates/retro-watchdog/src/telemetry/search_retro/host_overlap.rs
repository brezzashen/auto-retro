//! Host perf incidents open while a slow engine row ran (`host_incidents` beside `sync_steps`).
//! Search-retro 812181474823629636591: 25 of 28 slow rows were `cpu_starved`, 23 of them between
//! 00:58 and 01:42 SGT on 2026-11-15, inside a memory-exhaustion episode (26 `memory_exhausted`
//! incidents in the window, swap 51.7 of 53 GiB, fseventsd at 38 GiB); the match against
//! `incidents/` and `cpu-windows/` was made by hand. Read only.
use serde_json::{json, Value};
use std::path::Path;

/// An incident covers a row from its `hot_since_ms` until this long after `detected_at_ms`: the
/// watch re-detects a lasting episode every few minutes (memory_exhausted, 2026-11-15: 3 to 38 min).
const TAIL_MS: f64 = 15.0 * 60_000.0;

/// Incidents named beside one row, nearest first.
const PER_ROW: usize = 3;

/// One host incident: `hot_since_ms`, `detected_at_ms`, id, trigger, reading.
type Incident = (f64, f64, String, String, Value);

/// Host incidents detected inside `[from_ms - 1 h, to_ms + TAIL_MS]`. Host incidents are named by
/// their detection time in ms; retro incidents (20+ digit ids) never parse into that range.
pub(super) fn load(home: &Path, from_ms: f64, to_ms: f64) -> Vec<Incident> {
    let Ok(dir) = std::fs::read_dir(home.join(".local/state/cpu-watchdog/incidents")) else { return Vec::new() };
    let mut out: Vec<Incident> = dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let ms = name.strip_suffix(".json")?.parse::<f64>().ok()?;
            if ms < from_ms - 3.6e6 || ms > to_ms + TAIL_MS {
                return None;
            }
            let v: Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            let detected = v["detected_at_ms"].as_f64().unwrap_or(ms);
            let hot = v["hot_since_ms"].as_f64().unwrap_or(detected);
            Some((hot, detected, v["id"].as_str().unwrap_or(&name).to_string(), v["trigger"].as_str()?.to_string(), v["reading"].clone()))
        })
        .collect();
    out.sort_by(|a, b| a.1.total_cmp(&b.1));
    out
}

/// The incidents covering `[at_ms - ms, at_ms]`, as `{id, trigger, detected_at_ms, reading}`.
pub(super) fn overlapping(incidents: &[Incident], at_ms: f64, ms: f64) -> Value {
    let mut found: Vec<&Incident> = incidents.iter().filter(|(hot, detected, ..)| *hot <= at_ms && detected + TAIL_MS >= at_ms - ms).collect();
    found.sort_by(|a, b| (a.1 - at_ms).abs().total_cmp(&(b.1 - at_ms).abs()));
    json!(found
        .into_iter()
        .take(PER_ROW)
        .map(|(_, detected, id, trigger, reading)| json!({"id": id, "trigger": trigger, "detected_at_ms": detected.round(), "reading": reading}))
        .collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn a_slow_row_inside_a_memory_exhaustion_episode_names_it() {
        let home = std::env::temp_dir().join(format!("host-overlap-{}", std::process::id()));
        let dir = home.join(".local/state/cpu-watchdog/incidents");
        std::fs::create_dir_all(&dir).unwrap();
        // 1794677000379: hot 01:22:03, detected 01:23:20 SGT; the 01:27:44 15.3 s query lies inside it.
        write(&dir, "1794677000379.json", r#"{"id":"1794677000379","trigger":"memory_exhausted","hot_since_ms":1794676923824.8,"detected_at_ms":1794677000379.1,"reading":{"swap_used_gib":51.72}}"#);
        // A retro incident and an incident long before the row are not named.
        write(&dir, "812181474823629636591.json", r#"{"trigger":"search_retro"}"#);
        write(&dir, "1794660000000.json", r#"{"trigger":"memory_exhausted","detected_at_ms":1794660000000}"#);
        let all = load(&home, 1794668737992.0, 1794683183458.0);
        let found = overlapping(&all, 1794677264326.0, 15258.0);
        assert_eq!(found.as_array().unwrap().len(), 1);
        assert_eq!(found[0]["trigger"], "memory_exhausted");
        assert_eq!(found[0]["reading"]["swap_used_gib"], 51.72);
        // A row 20 min after the detection is outside the tail; one before the hot start too.
        assert!(overlapping(&all, 1794677000379.0 + 20.0 * 60_000.0, 500.0).as_array().unwrap().is_empty());
        assert!(overlapping(&all, 1794676900000.0, 500.0).as_array().unwrap().is_empty());
        std::fs::remove_dir_all(&home).unwrap();
    }
}
