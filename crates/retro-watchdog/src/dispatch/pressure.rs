//! Admission uses the existing CPU collector and the kernel pressure level, never RSS sums.
use serde_json::Value;
use std::path::Path;
use anyhow::{ensure, Result};

pub(super) const CPU_LIMIT: f64 = 85.0;
const WINDOW_MS: f64 = 30_000.0;
const FRESH_MS: f64 = 15_000.0;

pub(super) fn check(home: &Path, now: f64) -> Result<()> {
    let history = crate::telemetry::read_state(&home.join(".local/state/mars-cpu-build-gate/history.json"))
        .ok_or_else(|| anyhow::anyhow!("CPU history unavailable"))?;
    let memory = crate::telemetry::command::capture(home, "/usr/sbin/sysctl",
        &["-n", "kern.memorystatus_vm_pressure_level"], 3)?;
    evaluate(&history, now, memory.trim().parse()?)
}

pub(super) fn evaluate(history: &Value, now: f64, memory: u32) -> Result<()> {
    ensure!(memory == 1, "memory pressure is not normal ({memory})");
    let updated = history["updated"].as_f64().unwrap_or(0.0);
    ensure!(now >= updated && now - updated <= FRESH_MS, "CPU collector stale or future-dated");
    let samples = history["samples"].as_array().ok_or_else(|| anyhow::anyhow!("CPU samples missing"))?;
    let end = samples.last().and_then(|s| s["end"].as_f64()).unwrap_or(0.0);
    ensure!(now >= end && now - end <= FRESH_MS, "CPU samples stale or future-dated");
    let cutoff = end - WINDOW_MS;
    let mut previous = None;
    for s in samples.iter().filter(|s| s["end"].as_f64().is_some_and(|t| t > cutoff)) {
        let start = s["start"].as_f64().unwrap_or(f64::NAN);
        let stop = s["end"].as_f64().unwrap_or(f64::NAN);
        let percent = s["percent"].as_f64().unwrap_or(f64::NAN);
        ensure!(start.is_finite() && stop.is_finite() && percent.is_finite()
            && stop > start && stop - start <= FRESH_MS && (0.0..CPU_LIMIT).contains(&percent),
            "CPU is busy or sample invalid (requires <{CPU_LIMIT}%)");
        if let Some(last) = previous {
            ensure!(start >= last && start - last <= 1000.0, "CPU sampling gap");
        } else {
            ensure!(start <= cutoff, "CPU window incomplete");
        }
        previous = Some(stop);
    }
    ensure!(previous == Some(end), "CPU window incomplete");
    Ok(())
}
