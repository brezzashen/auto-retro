//! Starts a `mars-ops-crm` command for the watchdog's read-only Mars CRM probes (`cache-watch`,
//! `quota-economics`).
//!
//! On 2026-11-06 Mars_CRM split `mars-ops` (`rust/crates/mars-ops/src/cli/forward.rs`): plain `mars-ops`
//! links no pg client and refuses every database read with "a pg session runs in mars-ops-crm". It forwards
//! only its `CRM_COMMANDS` and the `k8s|canary` database commands, not `cache-watch` or `quota-economics`,
//! so `mars <script>` (package `mars-ops`) failed every run from 2026-11-06 16:04 SGT to 2026-11-16.
//!
//! The newest `tool-prebuild` binary (origin/mastra, `cpu-build-gate` `latest_tool::record_prebuilt`) runs
//! first, as `mars-locate` does, so no compile counts against a probe's timeout. Without one,
//! `cpu-build-gate cargo-run -p mars-ops-crm` builds it from the checkout.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub(super) const CRM: &str = "/path/to/workspace/Mars_CRM";
pub(super) const PACKAGE: &str = "mars-ops-crm";

/// The `cpu-build-gate` tool cache that holds `tools/<package>/prebuilt.json`.
fn tool_cache(home: &Path) -> PathBuf {
    std::env::var_os("CPU_BUILD_GATE_TOOL_CACHE").map_or_else(|| home.join("Library/Caches/cpu-build-gate"), PathBuf::from)
}

/// `(binary, sha)` of the newest prebuilt `mars-ops-crm` recorded under `cache`, if its binary exists.
fn prebuilt(cache: &Path) -> Option<(PathBuf, String)> {
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(cache.join(format!("tools/{PACKAGE}/prebuilt.json"))).ok()?).ok()?;
    let binary = PathBuf::from(record["binary"].as_str()?);
    binary.is_file().then(|| (binary, record["sha"].as_str().unwrap_or_default().to_string()))
}

/// `mars-ops-crm <args>` in the Mars_CRM checkout, with the watchdog's script PATH.
pub(super) fn command(home: &Path, args: &[&str]) -> Command {
    command_in(&tool_cache(home), Path::new(CRM), args)
}

pub(super) fn command_in(cache: &Path, crm: &Path, args: &[&str]) -> Command {
    let mut command = match prebuilt(cache) {
        Some((binary, sha)) => {
            let mut command = Command::new(binary);
            command.args(args).env("CPU_BUILD_GATE_TOOLING", sha);
            command
        }
        None => {
            // Same form as Mars_CRM `forward::crm_command`.
            let mut command = Command::new("cpu-build-gate");
            command.args(["cargo-run", "--shared-cache", "--root-env"]).arg(format!("MARS_CRM_ROOT={}", crm.display()))
                .args(["--", "--quiet", "--release", "--manifest-path"]).arg(crm.join("rust/Cargo.toml"))
                .args(["-p", PACKAGE, "--"]).args(args);
            command
        }
    };
    command.current_dir(crm).env("MARS_CRM_ROOT", crm).env("PATH", super::MARS_SCRIPT_PATH);
    command
}

#[cfg(test)]
#[path = "crm_ops_tests.rs"]
mod tests;
