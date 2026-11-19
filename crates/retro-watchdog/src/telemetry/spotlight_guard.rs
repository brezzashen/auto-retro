//! Spotlight guard (user request, 2026-10-29): keep Spotlight indexing off on the build volume.
//!
//! On 2026-10-29 the Spotlight menu showed "Indexing..." for days: `/Volumes/MarsBuild` (2.2M files,
//! cargo targets, DerivedData, Docker.raw) had indexing on since it was created on 2026-10-24, and
//! constant build writes kept the index from ever catching up. Folder markers do not work on
//! macOS 27 and the Data volume's privacy list does not cover another volume, so the volume itself
//! must be `mdutil -i off`.
//!
//! Every CHECK_SECS `mdutil -s` reads each VOLUMES entry that is mounted. One that reports
//! "Indexing enabled" is turned off with `sudo -A mdutil -i off` and the user is notified. State
//! goes to `spotlight-guard.json`.
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::json;
use std::{
    os::unix::fs::MetadataExt,
    path::Path,
    process::{Command, Stdio},
};

const CHECK_SECS: f64 = 600.0;
const VOLUMES: [&str; 1] = ["/Volumes/MarsBuild"];
const PROBE_SECS: u64 = 20;

#[derive(Default)]
pub(super) struct SpotlightGuard {
    last_check: f64,
}

impl SpotlightGuard {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if self.last_check > 0.0 && now >= self.last_check && now - self.last_check < CHECK_SECS {
            return Ok(());
        }
        self.last_check = now;
        let mut volumes = Vec::new();
        for volume in VOLUMES.iter().filter(|v| mounted(Path::new(v))) {
            let status = super::command::capture(home, "/usr/bin/mdutil", &["-s", volume], PROBE_SECS)?;
            let mut record = json!({"volume": volume, "indexing": indexing(&status)});
            if record["indexing"] == "enabled" {
                record["action"] = match disable(volume) {
                    Ok(()) => json!("disabled"),
                    Err(error) => json!(format!("disable failed: {error:#}")),
                };
                crate::dispatch::notify("看门狗：Spotlight",
                    &format!("{volume} 的 Spotlight 索引被重新打开，已关闭（{}）", record["action"].as_str().unwrap_or("?")));
            }
            volumes.push(record);
        }
        write_json(&root(home).join("spotlight-guard.json"), &json!({"at_ms": now * 1000.0, "volumes": volumes}))
    }
}

/// A path is a mount point when its device differs from its parent's; an empty `/Volumes/X`
/// directory left by an unmounted disk is not.
fn mounted(path: &Path) -> bool {
    let (Ok(own), Some(Ok(parent))) = (std::fs::metadata(path), path.parent().map(std::fs::metadata)) else {
        return false;
    };
    own.dev() != parent.dev()
}

/// `mdutil -s` prints "Indexing enabled." or "Indexing disabled." on the line after the volume.
pub(super) fn indexing(status: &str) -> &'static str {
    if status.contains("Indexing enabled") { "enabled" }
    else if status.contains("Indexing disabled") { "disabled" }
    else { "unknown" }
}

fn disable(volume: &str) -> Result<()> {
    let output = Command::new("/usr/bin/sudo").args(["-A", "/usr/bin/mdutil", "-i", "off", volume])
        .env("SUDO_ASKPASS", format!("{}/.local/bin/sudo-askpass", std::env::var("HOME")?))
        .stdin(Stdio::null()).output()?;
    anyhow::ensure!(output.status.success(), "sudo mdutil {} {}", output.status, String::from_utf8_lossy(&output.stderr).trim());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_mdutil_status() {
        assert_eq!(indexing("/System/Volumes/Data/Volumes/MarsBuild:\n\tIndexing enabled. \n"), "enabled");
        assert_eq!(indexing("/System/Volumes/Data/Volumes/MarsBuild:\n\tIndexing disabled.\n"), "disabled");
        assert_eq!(indexing("/Volumes/Recovery: \tError: unknown indexing state.\n"), "unknown");
    }

    #[test]
    fn an_unmounted_directory_is_not_a_volume() {
        assert!(!mounted(Path::new("/nonexistent-volume")));
        assert!(!mounted(&std::env::temp_dir()));
    }
}
