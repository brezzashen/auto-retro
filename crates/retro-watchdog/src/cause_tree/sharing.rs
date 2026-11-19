//! Screen Sharing sessions into this Mac, read from screensharingd's unified log. Incident
//! 86540618438375945180 (2026-10-25 20:49 and 21:01 SGT): two sessions rebuilt the menu bar
//! (+550 and +500 ports) without an avconferenced microphone row, one because the viewer sent no
//! audio and one because its row was not written yet. Each session is its own screensharingd
//! process: it starts at `set virtual display count to N` (N > 0) and ends at that process's
//! CoreAnalytics `Exiting exit handler.` line, or is still open at `end_ms`.
use anyhow::{Context, Result};
use serde_json::Value;
use std::{collections::BTreeMap, process::{Command, Stdio}};

/// Session spans (ms) seen since `start_s`, each closed at `end_ms` when still open.
pub(crate) fn sessions(start_s: f64, end_ms: f64) -> Result<Vec<(f64, f64)>> {
    let minutes = ((crate::unix_now() - start_s) / 60.0).ceil().max(1.0) as u64 + 1;
    let predicate = "process == \"screensharingd\" AND (eventMessage BEGINSWITH \"set virtual display count\" OR eventMessage == \"Exiting exit handler.\")";
    let out = Command::new("/usr/bin/log")
        .args(["show", "--info", "--style", "ndjson", "--last", &format!("{minutes}m"), "--predicate", predicate])
        .stdin(Stdio::null()).stderr(Stdio::null()).output().context("run log show")?;
    anyhow::ensure!(out.status.success(), "log show exited {}", out.status);
    Ok(spans(&String::from_utf8_lossy(&out.stdout), start_s * 1000.0, end_ms))
}

/// Spans from `log show --style ndjson` lines; sessions that ended before `from_ms` are dropped.
pub(crate) fn spans(ndjson: &str, from_ms: f64, end_ms: f64) -> Vec<(f64, f64)> {
    let mut by_pid: BTreeMap<i64, (Option<f64>, Option<f64>)> = BTreeMap::new();
    for e in ndjson.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
        let (Some(pid), Some(at), Some(text)) = (e["processID"].as_i64(), e["timestamp"].as_str().and_then(super::attribute::parse_time), e["eventMessage"].as_str()) else { continue };
        let entry = by_pid.entry(pid).or_default();
        match text.strip_prefix("set virtual display count to ").and_then(|n| n.trim().parse::<u32>().ok()) {
            Some(n) if n > 0 => { entry.0.get_or_insert(at); }
            Some(_) => {}
            None if entry.0.is_some() => entry.1 = Some(at),
            None => {}
        }
    }
    by_pid.into_values()
        .filter_map(|(start, end)| Some((start?, end.unwrap_or(end_ms).min(end_ms))))
        .filter(|(start, end)| *end >= from_ms && start <= end)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_span_per_screensharingd_process() {
        let line = |pid: i64, at: &str, text: &str| format!("{{\"processID\":{pid},\"timestamp\":\"2026-10-25 {at}+0800\",\"eventMessage\":\"{text}\"}}\n");
        let log = [
            line(78243, "20:39:17.205", "set virtual display count to 1"),
            line(78243, "20:40:22.766", "Exiting exit handler."),
            line(44567, "20:34:35.000", "Exiting exit handler."),
            line(58858, "21:01:16.686", "set virtual display count to 1"),
        ].concat();
        let t = |at: &str| super::super::attribute::parse_time(&format!("2026-10-25 {at}+0800")).unwrap();
        let got = spans(&log, t("20:25:32.000"), t("21:06:42.000"));
        assert_eq!(got, vec![(t("21:01:16.686"), t("21:06:42.000")), (t("20:39:17.205"), t("20:40:22.766"))]);
    }
}
