use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::Path;

pub const INTERVAL_MS: u64 = 600_000;

fn read(path: &Path) -> Result<Option<Value>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn due(home: &Path, now_ms: u64) -> Result<Value> {
    let directory = root(home).join("periodic");
    let Some(config) = read(&directory.join("config.json"))? else {
        return Ok(json!({"due":false,"enabled":false}));
    };
    if config["enabled"] != true {
        return Ok(json!({"due":false,"enabled":false}));
    }
    let baseline = config["baseline_end_ms"]
        .as_u64()
        .context("invalid periodic reporting baseline")?;
    let state = read(&directory.join("state.json"))?;
    let start = state
        .as_ref()
        .map(|state| {
            state["last_reported_end_ms"]
                .as_u64()
                .context("invalid periodic reporting cursor")
        })
        .transpose()?
        .unwrap_or(baseline);
    anyhow::ensure!(
        start % INTERVAL_MS == 0,
        "periodic cursor must be on a ten-minute boundary"
    );
    let end = now_ms / INTERVAL_MS * INTERVAL_MS;
    Ok(
        json!({"enabled":true,"due":end>start,"window_start_ms":start,"window_end_ms":end,
        "nominal_interval_ms":INTERVAL_MS,"next_report_at_ms":start.saturating_add(INTERVAL_MS),
        "enabled_at_ms":config["enabled_at_ms"],"delayed":end.saturating_sub(start)>INTERVAL_MS,
        "report_directory":directory.join(end.to_string())}),
    )
}

pub fn check(home: &Path) -> Result<()> {
    let state = due(home, (crate::unix_now() * 1000.0) as u64)
        .unwrap_or_else(|error| json!({"due":false,"error":format!("{error:#}")}));
    println!("{}", serde_json::to_string_pretty(&state)?);
    if state["due"] != true && state["error"].is_null() {
        std::process::exit(1);
    }
    Ok(())
}

fn acknowledge(home: &Path, end: u64, now: u64) -> Result<()> {
    let pending = due(home, now)?;
    anyhow::ensure!(
        pending["due"] == true
            && end % INTERVAL_MS == 0
            && end
                > pending["window_start_ms"]
                    .as_u64()
                    .context("missing periodic start")?
            && end
                <= pending["window_end_ms"]
                    .as_u64()
                    .context("missing periodic end")?,
        "report window is not currently due"
    );
    let directory = root(home).join("periodic").join(end.to_string());
    let report = read(&directory.join("report.json"))?
        .context("write report.json before acknowledgement")?;
    anyhow::ensure!(
        report["window_start_ms"] == pending["window_start_ms"] && report["window_end_ms"] == end,
        "report window does not match pending window"
    );
    anyhow::ensure!(
        std::fs::metadata(directory.join("report.md"))?.len() > 0,
        "report.md is empty"
    );
    crate::log_recall::verify(&directory)?;
    write_json(
        &root(home).join("periodic/state.json"),
        &json!({"last_reported_end_ms":end,
        "acknowledged_at_ms":now,"report_directory":directory}),
    )
}

pub fn ack(home: &Path, id: Option<String>) -> Result<()> {
    let id = id.context("usage: cpu-watchdog report-ack <window-end-ms>")?;
    anyhow::ensure!(
        !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()),
        "invalid report window id"
    );
    acknowledge(home, id.parse()?, (crate::unix_now() * 1000.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn periodic_windows_survive_restart_and_require_matching_durable_reports() {
        let home = std::env::temp_dir().join(format!("watchdog-periodic-{}", std::process::id()));
        assert_eq!(due(&home, 600000).unwrap()["due"], false);
        write_json(
            &root(&home).join("periodic/config.json"),
            &json!({"enabled":true,"baseline_end_ms":600000,"enabled_at_ms":650000}),
        )
        .unwrap();
        assert_eq!(due(&home, 1199999).unwrap()["due"], false);
        assert_eq!(due(&home, 1200000).unwrap()["due"], true);
        assert!(acknowledge(&home, 1200000, 1200000).is_err());
        let directory = root(&home).join("periodic/1200000");
        write_json(
            &directory.join("report.json"),
            &json!({"window_start_ms":0,"window_end_ms":1200000}),
        )
        .unwrap();
        std::fs::write(directory.join("report.md"), "report").unwrap();
        assert!(acknowledge(&home, 1200000, 1200000).is_err());
        write_json(
            &directory.join("report.json"),
            &json!({"window_start_ms":600000,"window_end_ms":1200000}),
        )
        .unwrap();
        assert!(acknowledge(&home, 1200000, 1800000).is_err());
        crate::log_recall::tests::capture_ok(&directory);
        let report = json!({"window_start_ms":600000,"window_end_ms":1200000,
            "log_recall_evidence":["/logs/test.log:3"],"duplicate_analysis":"No repeated failure in the recalled logs."});
        write_json(&directory.join("report.json"), &report).unwrap();
        std::fs::write(directory.join("report.md"), "report after recall").unwrap();
        acknowledge(&home, 1200000, 1800000).unwrap();
        assert_eq!(due(&home, 1200000).unwrap()["due"], false);
        assert_eq!(due(&home, 2400000).unwrap()["delayed"], true);
        assert!(ack(&home, Some("../state".into())).is_err());
        std::fs::remove_dir_all(home).unwrap();
    }
}
