//! Universal Control attaches, read from DeviceHub's unified log. Incident 811358242637012976142
//! (ControlCenter 42901, 2026-10-29/30 SGT): with no microphone session, capture stream or
//! sharing session, the menu bar still rebuilt each time a peer device's pointer entered this Mac.
//! DeviceHub then lists the virtual `V-UC Automouse` among connected devices, ControlCenter logs a
//! Biome set donation within about 1 s and its ports jump about +97 and keep about +65. Over the
//! 22 h instance 35 attaches held +1,519 of +3,264; the same windows shifted by -5, +5 or +15 min
//! held +589, +517 and +446. DeviceHub lines outlive ControlCenter's module lines (N2.8).
use anyhow::{Context, Result};
use serde_json::Value;
use std::process::{Command, Stdio};

const VIRTUAL_MOUSE: &str = "V-UC Automouse";

/// Attach times (ms) of the Universal Control virtual mouse since `start_s`.
pub(crate) fn attaches(start_s: f64) -> Result<Vec<f64>> {
    let minutes = ((crate::unix_now() - start_s) / 60.0).ceil().max(1.0) as u64 + 1;
    let predicate = "process == \"DeviceHub\" AND eventMessage BEGINSWITH \"Connected devices changed\"";
    let out = Command::new("/usr/bin/log")
        .args(["show", "--style", "ndjson", "--last", &format!("{minutes}m"), "--predicate", predicate])
        .stdin(Stdio::null()).stderr(Stdio::null()).output().context("run log show")?;
    anyhow::ensure!(out.status.success(), "log show exited {}", out.status);
    Ok(times(&String::from_utf8_lossy(&out.stdout), start_s * 1000.0))
}

/// Times at which the virtual mouse appears in DeviceHub's device list after being absent. Each
/// line lists every connected device, so an attach is presence after absence; the first line
/// counts only when it is the one that added a device.
pub(crate) fn times(ndjson: &str, from_ms: f64) -> Vec<f64> {
    let mut present: Option<bool> = None;
    let mut out = Vec::new();
    for e in ndjson.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
        let (Some(text), Some(at)) = (e["eventMessage"].as_str(), e["timestamp"].as_str().and_then(super::attribute::parse_time)) else { continue };
        if !text.starts_with("Connected devices changed") { continue }
        let now = text.contains(VIRTUAL_MOUSE);
        let attached = now && match present { Some(before) => !before, None => text.starts_with("Connected devices changed (1 added") };
        if attached && at >= from_ms { out.push(at) }
        present = Some(now);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_presence_after_absence() {
        let line = |at: &str, text: &str| format!("{{\"timestamp\":\"2026-10-30 {at}+0800\",\"eventMessage\":{}}}\n", serde_json::to_string(text).unwrap());
        let log = [
            line("14:35:07.000", "Connected devices changed (1 added, 0 removed) -> [8] {( <GCMouse 'V-UC Automouse'>, <GCMouse 'ROG AZOTH'> )}"),
            line("14:35:09.000", "Connected devices changed (1 added, 0 removed) -> [9] {( <GCMouse 'V-UC Automouse'>, <GCKeyboard 'Generic Keyboard'> )}"),
            line("14:35:12.000", "Connected devices changed (0 added, 1 removed) -> [7] {( <GCMouse 'ROG AZOTH'> )}"),
            line("14:48:31.000", "Connected devices changed (1 added, 0 removed) -> [8] {( <GCMouse 'V-UC Automouse'> )}"),
        ].concat();
        let t = |at: &str| super::super::attribute::parse_time(&format!("2026-10-30 {at}+0800")).unwrap();
        assert_eq!(times(&log, 0.0), vec![t("14:35:07.000"), t("14:48:31.000")]);
        assert_eq!(times(&log, t("14:40:00.000")), vec![t("14:48:31.000")]);
    }
}
