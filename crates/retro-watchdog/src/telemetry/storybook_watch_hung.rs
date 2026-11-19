//! State of the hung CRM Storybook server, captured before a restart stops it.
//!
//! Perf session 83222954824100139656 (2026-11-05 10:28 SGT): four /index.json probes timed out at
//! 56-93% host CPU, the restart stopped pid 3441, and nothing recorded whether that process spun a
//! core, was paged out (its RSS had fallen from ~800 MB to under the top-12 cutoff while the GPU
//! render held ~50 GB of unified memory) or was blocked. The cause stayed NOT-PROVEN. The record now
//! carries the process state, swap and memory level at the moment of the stop, plus a `sample`
//! call-graph file (thread stacks and physical footprint) beside the kept server log.
use serde_json::{json, Value};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
};

/// The manager's PID file for the CRM catalog, relative to the repository.
const PID_FILE: &str = ".claude/locks/storybook-catalogs/crm.pid";
/// `sample` duration; measured 7.7 s wall in total on 2026-11-05 (symbolication included).
const SAMPLE_SECS: &str = "2";

pub(super) fn capture(repo: &Path, directory: &Path, stamp: u64) -> Value {
    let Some(pid) = std::fs::read_to_string(repo.join(PID_FILE)).ok().and_then(|s| s.trim().parse::<u32>().ok()) else {
        return json!({"error": "no readable crm.pid"});
    };
    let pid_text = pid.to_string();
    let run = |program: &str, args: &[&str]| Command::new(program).args(args).stdin(Stdio::null()).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let ps = run("/bin/ps", &["-o", "stat=,%cpu=,rss=,etime=", "-p", &pid_text]);
    let sample = directory.join(format!("{stamp}-crm.sample.txt"));
    let sampled = Command::new("/usr/bin/sample").args([pid_text.as_str(), SAMPLE_SECS, "-file"]).arg(&sample)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status()
        .is_ok_and(|status| status.success())
        && std::fs::set_permissions(&sample, std::fs::Permissions::from_mode(0o600)).is_ok();
    json!({"pid": pid, "ps": parse_ps(&ps),
        "swap": run("/usr/sbin/sysctl", &["-n", "vm.swapusage"]),
        "memorystatus_level": run("/usr/sbin/sysctl", &["-n", "kern.memorystatus_level"]).parse::<u64>().ok(),
        "sample": sampled.then(|| sample.display().to_string())})
}

/// `ps -o stat=,%cpu=,rss=,etime=` output as fields; anything else is kept raw.
pub(super) fn parse_ps(line: &str) -> Value {
    match line.split_whitespace().collect::<Vec<_>>().as_slice() {
        [stat, cpu, rss, elapsed] => json!({"stat": stat, "cpu_percent_one_core": cpu.parse::<f64>().ok(),
            "rss_kib": rss.parse::<u64>().ok(), "elapsed": elapsed}),
        _ => json!({"raw": line}),
    }
}
