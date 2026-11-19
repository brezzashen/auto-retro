//! Wedged remote host reboot (founder, 2026-11-03: a hung Windows PC must not take beta down until
//! a person reboots it). On 2026-11-03 commit-memory exhaustion wedged win's WSL VM: the host still
//! answered SSH, but `docker desktop stop`, `wsl --shutdown --force`, WslService stop and `hcsdiag
//! kill` all hung, vmcompute sat in StartPending, and the logon-task start changed nothing
//! (incident 810663098879893038580). Only a reboot healed it, and the soft one hung in shutdown.
//!
//! `remote::recover` calls `escalate` when the engine is still down after its own start task and
//! ENGINE_WAIT while the host answers SSH:
//! 1. At most MAX_REBOOTS_24H per 24 h (ledger `remote-reboots.json`); over the cap it is held.
//! 2. Evidence first (commit memory, vmcompute, tagged offload jobs), then `shutdown /r /t 10`.
//! 3. Wait for engine and Kubernetes up to HOST_WAIT. A shutdown that has not brought the host
//!    back within REBOOT_GRACE gets a Wake-on-LAN packet (`power::step_with`,
//!    the OS-alive check skipped), within its own 24 h cap; a host hung mid-shutdown is held.
use super::remote::{host_reachable, powershell, probe_engine, ssh};
use crate::monitor::{root, write_json};
use serde_json::{json, Value};
use std::{path::Path, time::{Duration, Instant}};

pub(super) const REBOOT_GRACE: Duration = Duration::from_secs(300);
const HOST_WAIT: Duration = Duration::from_secs(1800);
/// A planned reboot holds recovery for HOST_WAIT plus the evidence and wake steps.
const PLANNED_PAUSE: Duration = Duration::from_secs(2100);
const MAX_REBOOTS_24H: usize = 2;
const LEDGER: &str = "remote-reboots.json";
const EVIDENCE: &str = r#"$ProgressPreference='SilentlyContinue'
$o=Get-CimInstance Win32_OperatingSystem
$j=@(Get-CimInstance Win32_Process -Filter "CommandLine like '%offload-job=%'").Count
ConvertTo-Json -Compress -InputObject ([ordered]@{commit_free_mb=[int64]($o.FreeVirtualMemory/1KB); commit_total_mb=[int64]($o.TotalVirtualMemorySize/1KB); vmcompute="$((Get-Service vmcompute).Status)"; offload_jobs=$j})"#;
const BOOT_AGE: &str = "[int64]((Get-Date).ToUniversalTime() - (Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToUniversalTime()).TotalSeconds";
const REBOOT: &str = "shutdown /r /t 10 /c \"Mars watchdog: beta Docker engine wedged, automatic reboot\"";

/// Whether another reboot fits the 24 h cap, given the ledger's recent reboot times.
pub(super) fn allowed(recent_ms: &[f64], now_ms: f64) -> bool {
    recent_ms.iter().filter(|ms| now_ms - **ms < 86_400_000.0).count() < MAX_REBOOTS_24H
}

fn recent(home: &Path) -> Vec<f64> {
    let ledger = std::fs::read_to_string(root(home).join(LEDGER)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    ledger.and_then(|l| l["reboots_ms"].as_array().cloned()).unwrap_or_default().iter().filter_map(Value::as_f64).collect()
}

/// Reboot the wedged host and wait for beta's engine and API. Returns the steps and readiness.
pub(super) fn escalate(home: &Path, host: &str) -> (Vec<Value>, bool) {
    reboot(home, host, REBOOT)
}

/// Issue `command` (a `shutdown /r` line) on the host within the 24 h cap, then wait for engine and
/// API, waking a host that has not come back past REBOOT_GRACE.
fn reboot(home: &Path, host: &str, command: &str) -> (Vec<Value>, bool) {
    let now_ms = crate::unix_now() * 1000.0;
    let mut ledger = recent(home);
    if !allowed(&ledger, now_ms) {
        return (vec![json!({"action": "reboot_host", "decision": "held: reboot_cap_24h", "recent_24h": ledger.len()})], false);
    }
    let evidence = ssh(host, &powershell(EVIDENCE), Duration::from_secs(45));
    let evidence = serde_json::from_str::<Value>(evidence.text.trim())
        .unwrap_or_else(|_| json!({"exit": evidence.code, "output": evidence.text.chars().take(300).collect::<String>()}));
    let issued = ssh(host, command, Duration::from_secs(30));
    ledger.retain(|ms| now_ms - ms < 86_400_000.0);
    ledger.push(now_ms);
    let _ = write_json(&root(home).join(LEDGER), &json!({"reboots_ms": ledger, "last": {"host": host, "at_ms": now_ms}}));
    crate::log(&format!("docker watch: rebooting host {host} ({command}): {evidence}"));
    let mut steps = vec![json!({"action": "reboot_host", "decision": "shutdown_r", "evidence": evidence,
        "exit": issued.code, "output": issued.text.chars().take(300).collect::<String>()})];
    let started = Instant::now();
    let mut power: Option<Value> = None;
    let ready = loop {
        std::thread::sleep(Duration::from_secs(5));
        // Ready only on a boot after the command: `shutdown /r /t 10` (plus a pending update's
        // pre-shutdown servicing) leaves the old engine answering for a while, and on 2026-11-04
        // 12:43Z that old engine counted as ready after 5 s and released the pause before shutdown.
        if probe_engine(home, host).ok && super::heal::probe_kube(home).ok && rebooted(host, started.elapsed()) { break true; }
        if power.is_none() && started.elapsed() >= REBOOT_GRACE && !rebooted(host, started.elapsed()) {
            power = super::power::step_with(home, host, started.elapsed(), REBOOT_GRACE, false);
        }
        if started.elapsed() >= HOST_WAIT { break false; }
    };
    steps.extend(power);
    steps.push(json!({"action": "wait_after_reboot", "ready": ready, "seconds": started.elapsed().as_secs()}));
    (steps, ready)
}

/// The `shutdown /r` line of a planned reboot; the comment keeps only characters cmd.exe passes on.
pub(super) fn planned_command(reason: &str) -> String {
    let clean: String = reason.chars().filter(|c| c.is_alphanumeric() || " -_.,:/()".contains(*c)).take(120).collect();
    format!("shutdown /r /t 10 /c \"Mars planned reboot: {}\"", clean.trim())
}

const USAGE: &str = "usage: cpu-watchdog remote-reboot [--dry-run] <reason ...>\n  Reboots the remote beta host (shutdown /r) under a Docker watch pause.\n  --dry-run  print the plan (host, command, 24 h cap) and reboot nothing\n  -h, --help print this help and reboot nothing";

#[derive(Debug, PartialEq)]
enum Planned { Run { dry: bool, reason: String }, Usage, Refused(String) }

/// Parse `remote-reboot` arguments. Help flags print usage; any other flag-shaped argument is
/// refused, never taken as the reason (2026-11-04: `remote-reboot --help` rebooted win and beta
/// went down, release-queue T1 incident 87591666755952193621).
fn parse_planned(args: &[String]) -> Planned {
    if args.iter().any(|a| matches!(a.as_str(), "-h" | "--help" | "help" | "-?" | "/?")) { return Planned::Usage; }
    if let Some(bad) = args.iter().find(|a| a.starts_with('-') && *a != "--dry-run") {
        return Planned::Refused(format!("unknown option {bad:?}; nothing was rebooted"));
    }
    let dry = args.iter().any(|a| a == "--dry-run");
    let reason = args.iter().filter(|a| *a != "--dry-run").cloned().collect::<Vec<_>>().join(" ");
    if reason.trim().is_empty() { return Planned::Refused("a reason is required; nothing was rebooted".into()); }
    Planned::Run { dry, reason }
}

/// `remote-reboot [--dry-run] <reason ...>` (founder, 2026-11-03): a planned reboot of the remote
/// beta host under a pause that outlasts HOST_WAIT, with the same Wake-on-LAN heal of a restart that
/// hangs after shutdown (N4.13: both soft reboots that day hung; a fixed 15 min pause expired first).
pub(super) fn planned_cli(home: &Path, args: Vec<String>) -> anyhow::Result<()> {
    let (dry, reason) = match parse_planned(&args) {
        Planned::Run { dry, reason } => (dry, reason),
        Planned::Usage => { println!("{USAGE}"); return Ok(()); }
        Planned::Refused(why) => anyhow::bail!("{why}\n{USAGE}"),
    };
    let host = super::remote::host(home).ok_or_else(|| anyhow::anyhow!("no remote beta host: the offload tunnel agent is not installed"))?;
    let now = crate::unix_now();
    let recent_24h = recent(home).iter().filter(|ms| now * 1000.0 - **ms < 86_400_000.0).count();
    let plan = json!({"host": host, "command": planned_command(&reason), "reboots_24h": recent_24h,
        "allowed": allowed(&recent(home), now * 1000.0), "pause_s": PLANNED_PAUSE.as_secs(), "reboot_grace_s": REBOOT_GRACE.as_secs()});
    if dry { println!("{}", serde_json::to_string_pretty(&plan)?); return Ok(()); }
    anyhow::ensure!(plan["allowed"] == true, "held: {recent_24h} remote reboots in 24 h (cap {MAX_REBOOTS_24H}); ask the founder");
    let record = planned(home, &host, &reason, "remote_reboot")?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    anyhow::ensure!(record["ready"] == true, "host {host} not ready after the reboot; see the record above");
    Ok(())
}

/// A planned reboot under its own pause: hold recovery, reboot (Wake-on-LAN if it never powers back on),
/// release, and append a `kind` record. Shared by the CLI and win_guard's GPU-lost self-heal
/// (founder, 2026-11-04: restart win automatically when it hangs or its GPU dies).
pub(super) fn planned(home: &Path, host: &str, reason: &str, kind: &str) -> anyhow::Result<Value> {
    let now = crate::unix_now();
    super::hold(home, now, PLANNED_PAUSE.as_secs_f64(), &format!("{kind}: {reason}"))?;
    let (steps, ready) = reboot(home, host, &planned_command(reason));
    let end = crate::unix_now();
    super::hold(home, end, 0.0, &format!("{kind} done: {reason}"))?;
    let boot = if host_reachable(host) { super::remote::host_boot(host) } else { Value::Null };
    let record = json!({"kind": kind, "at_ms": end * 1000.0, "started_at_ms": now * 1000.0, "reason": reason,
        "host": host, "ready": ready, "steps": steps, "host_boot": boot});
    super::append(home, end, &record)?;
    crate::log(&format!("docker watch: {record}"));
    Ok(record)
}

/// Whether another reboot fits the 24 h cap now.
pub(super) fn cap_allows(home: &Path) -> bool {
    allowed(&recent(home), crate::unix_now() * 1000.0)
}

/// The host came back from our reboot and is starting Docker (SSH answers and it booted after the
/// reboot was issued), so the card must not reset it mid-start. SSH answering from the old boot
/// means the shutdown hung.
fn rebooted(host: &str, since_issued: Duration) -> bool {
    if !host_reachable(host) { return false; }
    let age = ssh(host, &powershell(BOOT_AGE), Duration::from_secs(30));
    age.text.trim().parse::<u64>().is_ok_and(|s| booted_since(s, since_issued))
}

pub(super) fn booted_since(boot_age_s: u64, since_issued: Duration) -> bool {
    boot_age_s < since_issued.as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_reboots_a_day_at_most() {
        let now = 100_000_000.0;
        assert!(allowed(&[], now));
        assert!(allowed(&[now - 3_600_000.0], now));
        assert!(!allowed(&[now - 3_600_000.0, now - 60_000.0], now));
        assert!(allowed(&[now - 90_000_000.0, now - 60_000.0], now));
    }

    #[test]
    fn a_planned_reboot_comment_cannot_break_out_of_cmd() {
        assert_eq!(planned_command("64 GB page file"), "shutdown /r /t 10 /c \"Mars planned reboot: 64 GB page file\"");
        assert_eq!(planned_command("a\" & del x | y %PATH% ^z"), "shutdown /r /t 10 /c \"Mars planned reboot: a  del x  y PATH z\"");
    }

    #[test]
    fn help_and_unknown_flags_never_reboot() {
        let p = |a: &[&str]| parse_planned(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        for help in [&["--help"][..], &["-h"], &["--dry-run", "--help"], &["page", "file", "-h"], &["help"]] {
            assert_eq!(p(help), Planned::Usage, "{help:?}");
        }
        for bad in [&["--force"][..], &["-x", "reason"], &["reason", "--verbose"], &[], &["--dry-run"]] {
            assert!(matches!(p(bad), Planned::Refused(_)), "{bad:?}");
        }
        assert_eq!(p(&["--dry-run", "64", "GB"]), Planned::Run { dry: true, reason: "64 GB".into() });
        assert_eq!(p(&["wedged", "engine"]), Planned::Run { dry: false, reason: "wedged engine".into() });
    }

    #[test]
    fn a_boot_older_than_the_reboot_is_a_hung_shutdown() {
        let issued = Duration::from_secs(320);
        assert!(booted_since(200, issued));
        assert!(!booted_since(86_000, issued));
    }
}
