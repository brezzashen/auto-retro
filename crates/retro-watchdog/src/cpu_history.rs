//! Ten-minute archives preserve source CPU intervals and sanitized process observations.
use crate::{
    monitor::{root, write_json},
    procs::Proc,
    reporting::INTERVAL_MS,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

#[derive(Default, Deserialize, Serialize)]
struct Window {
    samples: Vec<Value>,
    frames: Vec<Value>,
}

pub fn record(
    home: &Path,
    now: f64,
    pid: i32,
    samples: Value,
    frame: Value,
    processes: &[Proc],
) -> Result<()> {
    let mut updates: BTreeMap<u64, Vec<Value>> = BTreeMap::new();
    for sample in samples.as_array().into_iter().flatten() {
        let Some(start) = sample["start"].as_f64() else {
            continue;
        };
        let Some(end) = sample["end"].as_f64() else {
            continue;
        };
        let Some(percent) = sample["percent"].as_f64() else {
            continue;
        };
        if start < 0.0
            || end <= start
            || end - start > 15000.0
            || end > now
            || now - end > 60000.0
            || !(0.0..=100.0).contains(&percent)
        {
            continue;
        }
        updates
            .entry(start as u64 / INTERVAL_MS * INTERVAL_MS)
            .or_default()
            .push(json!({"collector_pid":pid,"start":start,"end":end,"percent":percent}));
    }
    let bucket = now as u64 / INTERVAL_MS * INTERVAL_MS;
    updates.entry(bucket).or_default();
    for (start, rows) in updates {
        let path = root(home).join("cpu-windows").join(format!("{start}.json"));
        let mut window: Window = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Window::default(),
            Err(error) => return Err(error.into()),
        };
        for row in rows {
            if !window.samples.iter().any(|old| {
                old["collector_pid"] == row["collector_pid"]
                    && old["start"] == row["start"]
                    && old["end"] == row["end"]
            }) {
                window.samples.push(row);
            }
        }
        if start == bucket && !window.frames.iter().any(|row| row["at_ms"] == now) {
            let mut rss: Vec<_> = processes.iter().collect();
            rss.sort_by_key(|process| std::cmp::Reverse(process.rss_kib));
            window.frames.push(json!({"at_ms":now,"cpu":frame,"top_rss":rss.into_iter().take(12)
                .map(|p|json!({"pid":p.pid,"ppid":p.ppid,"start":p.start,"name":p.name,"rss_kib":p.rss_kib})).collect::<Vec<_>>()}));
        }
        anyhow::ensure!(
            window.samples.len() <= 1024 && window.frames.len() <= 1024,
            "CPU archive exceeded bounded window capacity"
        );
        window.samples.sort_by(|a, b| {
            a["start"]
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&b["start"].as_f64().unwrap_or(0.0))
        });
        write_json(&path, &window)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archives_deduplicate_and_preserve_cross_boundary_source_intervals() {
        let home =
            std::env::temp_dir().join(format!("watchdog-cpu-archive-{}", std::process::id()));
        let samples = json!([{"start":595000.0,"end":600000.0,"percent":95.0},
            {"start":600000.0,"end":605000.0,"percent":20.0},
            {"start":605000.0,"end":610000.0,"percent":-1.0}]);
        record(&home, 605000.0, 42, samples.clone(), Value::Null, &[]).unwrap();
        record(&home, 605000.0, 42, samples, Value::Null, &[]).unwrap();
        for bucket in [0, 600000] {
            let window: Window = serde_json::from_slice(
                &std::fs::read(
                    root(&home)
                        .join("cpu-windows")
                        .join(format!("{bucket}.json")),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(window.samples.len(), 1);
            assert_eq!(window.frames.len(), if bucket == 600000 { 1 } else { 0 });
        }
        std::fs::remove_dir_all(home).unwrap();
    }
}
