//! Private, bounded evidence captured before starting a repair agent.
use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{io::Read, os::unix::fs::OpenOptionsExt, path::Path};

pub(super) fn save(home: &Path, event: &Value) -> Result<()> {
    let id = event["id"].as_str().context("Missing incident id")?;
    let directory = root(home).join("analysis").join(id);
    if directory.join("evidence.json").exists() {
        return Ok(());
    }
    write_json(&directory.join("incident.json"), event)?;
    let source = Path::new(
        event["report"]["path"]
            .as_str()
            .context("Missing crash report path")?,
    );
    let expected = home.join("Library/Logs/DiagnosticReports");
    anyhow::ensure!(
        source.parent() == Some(expected.as_path()),
        "Crash report outside DiagnosticReports"
    );
    let metadata = std::fs::symlink_metadata(source)?;
    anyhow::ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() <= 4 * 1024 * 1024,
        "Crash report is not a bounded regular file"
    );
    let mut file = std::fs::File::open(source)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= 4 * 1024 * 1024,
        "Crash report grew beyond 4 MiB"
    );
    use std::io::Write;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(directory.join("report.ips"))?
        .write_all(&bytes)?;
    let mut snapshots = serde_json::Map::new();
    for name in [
        "tasks",
        "xcode",
        "xcode-errors",
        "desktop",
        "guest",
        "container-peaks",
    ] {
        let value: Value = std::fs::read(root(home).join(format!("{name}.json")))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| json!({"error":"Evidence unavailable"}));
        snapshots.insert(name.into(), value);
    }
    write_json(
        &directory.join("evidence.json"),
        &json!({"captured_at_ms":crate::unix_now()*1000.0,
        "original_report":source,"raw_copy":"report.ips","snapshots":snapshots,
        "semantics":"Capture-time snapshots, not proof of state at the crash. Original timestamps remain intact; no raw argv or environment collected."}),
    )
}
