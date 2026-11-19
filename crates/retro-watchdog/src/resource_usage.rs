//! Host GPU samples share the existing minute worker. Percentages describe device activity,
//! not task attribution, energy, or a partition of CPU plus GPU work.
use crate::monitor::{root, write_json};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, path::Path};

mod summary;
mod validation;
pub use summary::run;
pub use validation::validate;

pub fn collect(home: &Path) -> Result<()> {
    let at = crate::unix_now() * 1000.0;
    let result = crate::telemetry::command::capture(home, "/usr/sbin/ioreg", &["-r", "-c", "IOAccelerator", "-l", "-w", "0"], 5)
        .and_then(|text| parse(&text));
    let state = match result {
        Ok(gpu) => json!({"at_ms":at,"error":null,"gpu":gpu}),
        Err(error) => json!({"at_ms":at,"error":format!("{error:#}"),"gpu":null}),
    };
    write_json(&root(home).join("resource-usage.json"), &state)?;
    let path = root(home).join("resource-samples").join(format!("{}.jsonl", at as u64 / 3_600_000 * 3_600_000));
    std::fs::create_dir_all(path.parent().unwrap())?;
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(path.parent().unwrap(), std::fs::Permissions::from_mode(0o700))?; }
    let mut file = OpenOptions::new().create(true).append(true).mode(0o600).open(path)?;
    writeln!(file, "{state}")?;
    Ok(())
}

fn parse(text: &str) -> Result<Value> {
    let mut devices = Vec::new();
    for line in text.lines().filter(|line| line.contains("\"PerformanceStatistics\" =")) {
        let percent = |key: &str| -> Option<f64> {
            let marker = format!("\"{key}\"=");
            let tail = line.split_once(&marker)?.1;
            let value: f64 = tail.split([',', '}']).next()?.trim().parse().ok()?;
            ((0.0..=100.0).contains(&value)).then_some(value)
        };
        let device = percent("Device Utilization %");
        ensure!(device.is_some(), "GPU device utilization missing or invalid; not zero activity");
        devices.push(json!({"device_utilization_pct":device,
            "renderer_utilization_pct":percent("Renderer Utilization %"),
            "tiler_utilization_pct":percent("Tiler Utilization %"),"raw_statistics":line.trim()}));
    }
    ensure!(!devices.is_empty(), "IOAccelerator returned no GPU utilization counters");
    Ok(json!({"source":"ioreg IOAccelerator PerformanceStatistics", "devices":devices,
        "scope":"host-wide; no per-task attribution; driver sampling interval unspecified"}))
}

#[cfg(test)]
mod tests;
