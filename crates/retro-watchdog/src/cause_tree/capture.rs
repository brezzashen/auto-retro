//! Screen-capture stream starts, read from replayd's unified log. Incident 815083625503085831257
//! (ControlCenter 583, 2026-10-26/27 SGT): a ScreenCaptureKit capture ("Content sharing streams
//! changed to count 1") made ControlCenter add the AudioVideoModule element (+103 at 13:22:54),
//! but ControlCenter's own lines had rotated out of the unified log after about 9 h while
//! replayd's `-[SLContentStream start:]` lines covered the instance's 24.5 h; 32 of its 48 jumps
//! over 40 ports followed such a start within 12 s.
use anyhow::{Context, Result};
use serde_json::Value;
use std::process::{Command, Stdio};

/// Start times (ms) of replayd capture streams since `start_s`.
pub(crate) fn starts(start_s: f64) -> Result<Vec<f64>> {
    let minutes = ((crate::unix_now() - start_s) / 60.0).ceil().max(1.0) as u64 + 1;
    let predicate = "process == \"replayd\" AND eventMessage BEGINSWITH \"-[SLContentStream start:]\"";
    let out = Command::new("/usr/bin/log")
        .args(["show", "--style", "ndjson", "--last", &format!("{minutes}m"), "--predicate", predicate])
        .stdin(Stdio::null()).stderr(Stdio::null()).output().context("run log show")?;
    anyhow::ensure!(out.status.success(), "log show exited {}", out.status);
    Ok(times(&String::from_utf8_lossy(&out.stdout), start_s * 1000.0))
}

/// Stream start times from `log show --style ndjson` lines, dropping those before `from_ms`.
pub(crate) fn times(ndjson: &str, from_ms: f64) -> Vec<f64> {
    ndjson.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["eventMessage"].as_str().is_some_and(|t| t.starts_with("-[SLContentStream start:]")))
        .filter_map(|e| e["timestamp"].as_str().and_then(super::attribute::parse_time))
        .filter(|at| *at >= from_ms)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_stream_starts_after_launch() {
        let line = |at: &str, text: &str| format!("{{\"timestamp\":\"2026-10-26 {at}+0800\",\"eventMessage\":\"{text}\"}}\n");
        let log = [
            line("16:01:00.000", "-[SLContentStream start:]: self = 0x1"),
            line("19:29:41.142", "-[SLContentStream start:]: self = 0x7770c45b60"),
            line("19:29:46.279", "-[SLContentStream stop:]: self = 0x7770c45b60"),
        ].concat();
        let t = |at: &str| super::super::attribute::parse_time(&format!("2026-10-26 {at}+0800")).unwrap();
        assert_eq!(times(&log, t("16:02:08.000")), vec![t("19:29:41.142")]);
    }
}
