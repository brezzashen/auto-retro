//! One memory reading per minute in `memory-guard/series/<day>.ndjson`, so a later session can
//! check what an admission gate read. Three perf sessions on 2026-11-17 lacked it: 1794856006950
//! rebuilt a swap step from two render-queue `held` rows, 1794864522060 could not say why a relief
//! spared four devices, and 1794885309576 could not say whether swap was past 24 GiB when sim-lease
//! admitted two extra simulator boots 17 s apart at 11:07 SGT. Each row carries the guard's own
//! reading (swap, free pages), the cumulative `vm_stat` paging counters, and the two sysctls that
//! render-queue `gate.rs` and sim-lease `memory.rs` gate on (`kern.memorystatus_vm_pressure_level`,
//! `kern.memorystatus_level`). A failed sysctl leaves those two fields null; the row is still kept.
use super::memory_guard::{Paging, Reading};
use crate::monitor::root;
use anyhow::Result;
use serde_json::{json, Value};
use std::{io::Write, path::Path};

pub const EVERY_SECS: f64 = 60.0;
const KEEP_DAYS: u64 = 30;

/// `sysctl -n kern.memorystatus_vm_pressure_level kern.memorystatus_level`: one value per line.
pub fn parse_gate(text: &str) -> Option<(u32, u32)> {
    let mut values = text.lines().map(str::trim).filter(|line| !line.is_empty());
    Some((values.next()?.parse().ok()?, values.next()?.parse().ok()?))
}

pub fn row(now: f64, reading: &Reading, paging: Option<Paging>, gate: Option<(u32, u32)>) -> Value {
    let gib = |bytes: f64| (bytes / 1_073_741_824.0 * 100.0).round() / 100.0;
    json!({"at_ms": (now * 1000.0).round(), "swap_used_gib": gib(reading.swap_used), "swap_total_gib": gib(reading.swap_total),
        "free_gib": gib(reading.free), "pressure_level": gate.map(|g| g.0), "free_percent": gate.map(|g| g.1),
        "swapouts_gib": paging.map(|p| gib(p.swapouts)), "swapins_gib": paging.map(|p| gib(p.swapins)),
        "compressions_gib": paging.map(|p| gib(p.compressions)), "decompressions_gib": paging.map(|p| gib(p.decompressions))})
}

#[derive(Default)]
pub struct Series { last: f64 }

impl Series {
    /// True once per `EVERY_SECS`; the caller then appends one row.
    pub fn due(&mut self, now: f64) -> bool {
        if now - self.last < EVERY_SECS { return false; }
        self.last = now;
        true
    }

    pub fn record(&mut self, home: &Path, now: f64, reading: &Reading, paging: Option<Paging>) -> Result<()> {
        if !self.due(now) { return Ok(()); }
        let gate = crate::telemetry::command::capture(home, "/usr/sbin/sysctl", &["-n", "kern.memorystatus_vm_pressure_level", "kern.memorystatus_level"], 3)
            .ok().and_then(|text| parse_gate(&text));
        append(&root(home).join("memory-guard").join("series"), now, &row(now, reading, paging, gate))
    }
}

/// Appends to the day's file; a new day's first row drops files older than `KEEP_DAYS`.
pub fn append(directory: &Path, now: f64, row: &Value) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let day = (now / 86_400.0).floor() as u64;
    let path = directory.join(format!("{day}.ndjson"));
    if !path.exists() {
        for entry in std::fs::read_dir(directory)?.flatten() {
            let old = entry.file_name().to_string_lossy().strip_suffix(".ndjson").and_then(|d| d.parse::<u64>().ok()).is_some_and(|d| d + KEEP_DAYS < day);
            if old { let _ = std::fs::remove_file(entry.path()); }
        }
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{row}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: f64 = 1_073_741_824.0;

    #[test]
    fn a_row_carries_the_guard_reading_and_both_gate_sysctls() {
        // 2026-11-17 11:34:22 SGT: swap 22.75 GiB used of 24, 0.06 GiB free, pressure normal, 36 % free.
        let reading = Reading { swap_total: 24.0 * GIB, swap_used: 22.75 * GIB, free: 0.06 * GIB };
        let paging = Paging { swapins: 2.0 * GIB, swapouts: 3.0 * GIB, compressions: 400.0 * GIB, decompressions: 380.0 * GIB };
        let row = row(1_794_886_462.0, &reading, Some(paging), parse_gate("1\n36\n"));
        assert_eq!((row["swap_used_gib"].as_f64(), row["free_gib"].as_f64()), (Some(22.75), Some(0.06)));
        assert_eq!((row["pressure_level"].as_u64(), row["free_percent"].as_u64()), (Some(1), Some(36)));
        assert_eq!(row["swapouts_gib"].as_f64(), Some(3.0));
        let bare = super::row(1.0, &reading, None, parse_gate("garbage"));
        assert!(bare["pressure_level"].is_null() && bare["compressions_gib"].is_null() && bare["swap_total_gib"] == 24.0);
    }

    #[test]
    fn one_row_per_minute_and_old_days_are_dropped() {
        let mut series = Series::default();
        assert!(series.due(1_000.0));
        assert!(!series.due(1_059.0));
        assert!(series.due(1_060.0));
        let dir = std::env::temp_dir().join(format!("cpu-watchdog-memory-series-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let day = 86_400.0;
        append(&dir, 5.0 * day, &json!({"n": 1})).unwrap();
        append(&dir, 5.0 * day + 60.0, &json!({"n": 2})).unwrap();
        append(&dir, 35.0 * day, &json!({"n": 3})).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("5.ndjson")).unwrap().lines().count(), 2);
        append(&dir, 36.0 * day, &json!({"n": 4})).unwrap();
        assert!(!dir.join("5.ndjson").exists(), "day 5 is more than 30 days before day 36");
        assert!(dir.join("35.ndjson").exists() && dir.join("36.ndjson").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
