//! GPU-lost self-heal (founder, 2026-11-04: "遇到特殊情况自动重启自愈即可"). After an nvlddmkm TDR
//! storm the RTX 5090 can drop off the bus while Windows keeps running: `nvidia-smi` answers
//! "No devices were found" and offload marks win down (2026-11-03, report win-crash-dumps-20261104).
//! Only a reboot brings it back. Each win_guard round reads `nvidia-smi`; LOST_ROUNDS lost rounds in
//! a row on a host that answers SSH start one planned reboot (`docker_watch::self_heal_reboot`:
//! pause, `shutdown /r`, Wake-on-LAN for a shutdown that never powered back on, 24 h cap shared with every
//! remote reboot). A Docker watch pause (deliberate maintenance, a driver install) or a running beta
//! release defers it; a release defers at most RELEASE_DEFER_S.
use serde_json::Value;
use std::path::Path;

pub(super) const LOST_ROUNDS: u32 = 3;
pub(super) const RELEASE_DEFER_S: f64 = 1800.0;

/// The probe fragment: nvidia-smi's exit code and first output line.
pub(super) const PROBE: &str = r#"$gx=-1; $go=''; $smi="$env:SystemRoot\System32\nvidia-smi.exe"
if (Test-Path $smi) { $go=(& $smi --query-gpu=name --format=csv,noheader 2>&1 | Out-String).Trim(); $gx=$LASTEXITCODE }
$gpu=[ordered]@{exit=$gx; out=$go.Substring(0,[Math]::Min(200,$go.Length))}"#;

/// Why the sample says the GPU is lost, if it does. A sample without a `gpu` block (probe failed,
/// host unreachable, no nvidia-smi) is not evidence of a lost GPU.
pub(super) fn lost(sample: &Value) -> Option<String> {
    let gpu = sample.get("gpu")?;
    let exit = gpu["exit"].as_i64()?;
    let out = gpu["out"].as_str().unwrap_or("");
    if exit == -1 { return None; }
    let marker = ["No devices were found", "GPU is lost", "Unable to determine the device handle", "NVIDIA-SMI has failed", "Unknown Error"]
        .iter().any(|m| out.contains(m));
    (exit != 0 || marker).then(|| format!("nvidia-smi exit {exit}: {}", out.lines().next().unwrap_or("")))
}

/// Whether a beta release is running on the release board (`mars-deliver/queue.json`).
pub(super) fn beta_release_running(home: &Path) -> bool {
    let queue = std::fs::read_to_string(home.join(".local/state/mars-deliver/queue.json")).ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(Value::Null);
    ["beta", "beta-web"].iter().any(|env| match &queue["environments"][*env] {
        Value::Array(list) => !list.is_empty(),
        Value::Object(map) => !map.is_empty(),
        _ => false,
    })
}

/// What to do this round given the lost streak, an active pause, a running release and how long
/// the release has deferred the heal.
#[derive(Debug, PartialEq)]
pub(super) enum Step { Wait, Defer(&'static str), Heal }

pub(super) fn step(streak: u32, paused: bool, release: bool, deferred_s: f64) -> Step {
    if streak < LOST_ROUNDS { return Step::Wait; }
    if paused { return Step::Defer("docker watch paused"); }
    if release && deferred_s < RELEASE_DEFER_S { return Step::Defer("beta release running"); }
    Step::Heal
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_failing_nvidia_smi_is_a_lost_gpu() {
        assert_eq!(lost(&json!({"gpu": {"exit": 0, "out": "NVIDIA GeForce RTX 5090"}})), None);
        assert!(lost(&json!({"gpu": {"exit": 6, "out": "No devices were found"}})).is_some());
        assert!(lost(&json!({"gpu": {"exit": 0, "out": "Unable to determine the device handle for GPU0000:01:00.0: GPU is lost."}})).is_some());
        assert_eq!(lost(&json!({"gpu": {"exit": -1, "out": ""}})), None);
        assert_eq!(lost(&json!({"commit_free_mb": 1})), None);
        assert_eq!(lost(&Value::Null), None);
    }

    #[test]
    fn three_lost_rounds_heal_unless_paused_or_a_release_runs() {
        assert_eq!(step(2, false, false, 0.0), Step::Wait);
        assert_eq!(step(3, false, false, 0.0), Step::Heal);
        assert_eq!(step(3, true, false, 0.0), Step::Defer("docker watch paused"));
        assert_eq!(step(5, false, true, 60.0), Step::Defer("beta release running"));
        assert_eq!(step(5, false, true, RELEASE_DEFER_S), Step::Heal);
    }
}
