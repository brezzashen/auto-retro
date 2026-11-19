//! Mach port census of ControlCenter and WindowServer. On 2026-10-21 ControlCenter (up 6 d 17 h)
//! held 68,705 ports, about 22,600 of them paired with WindowServer (84,658 ports), and every
//! client waiting on a CARenderServer fence froze for 1-4.5 s. `killall ControlCenter` dropped them
//! to 1,043 / 17,028 and the stalls stopped. Counting another task's ports needs root, so every
//! INTERVAL_SECS one thread runs `sudo -A /usr/bin/lsmp -p <pid>` through the Keychain askpass,
//! streams the output and keeps only the total and the busiest peers. It writes
//! `port-census.json`, appends `port-census/<utc-day-ms>.ndjson`, and opens one incident per
//! process instance over its limit, which `crate::dispatch` hands to an Orca tab.
use super::diagnostics::incident;
use crate::{monitor::{root, write_json}, procs::Proc};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

pub const INTERVAL_SECS: f64 = 300.0;
const LSMP_TIMEOUT: Duration = Duration::from_secs(30);
const DAY_MS: f64 = 86_400_000.0;
const TOP_PEERS: usize = 5;

pub struct Target {
    pub key: &'static str,
    pub name: &'static str,
    /// Normal on this host after a restart: ControlCenter ~1,000-1,300, WindowServer ~17,000.
    /// ControlCenter dropped from 10,000 to 4,000 on 2026-10-21: it regrew ~4,000 ports an hour
    /// and stalls returned at ~4,500.
    pub limit: u64,
    /// Warning below the limit for a process that cannot be restarted (WindowServer).
    pub early: Option<Early>,
}

pub const TARGETS: [Target; 2] = [
    Target { key: "controlcenter", name: "ControlCenter", limit: 4_000, early: None },
    Target { key: "windowserver", name: "WindowServer", limit: 50_000,
        early: Some(Early { notify_at: 35_000, dead_per_hour: 1_000.0 }) },
];

impl Target {
    /// `<KEY>_PORT_LIMIT` (for example `CONTROLCENTER_PORT_LIMIT`) overrides the limit for an
    /// end-to-end trial of the dispatch path.
    fn effective_limit(&self) -> u64 {
        std::env::var(format!("{}_PORT_LIMIT", self.key.to_uppercase())).ok()
            .and_then(|value| value.parse().ok()).unwrap_or(self.limit)
    }
}

pub(super) struct Count {
    pub total: u64,
    pub peers: Vec<(String, u64)>,
    /// Rights whose receiver is gone; WindowServer's leak on 2026-11-05 was mostly these.
    pub dead_names: u64,
}

/// The start time recorded for this pid by the previous census when it is the same instance.
/// Start is now − etime in whole seconds and drifts by a second or two between snapshots; the
/// raw value made a new incident key for WindowServer pid 660 on 2026-11-05 (1793668353 at
/// 18:22, 1793668354 at 19:00) and opened a second session for the same leak.
fn stable_start(previous: &Value, pid: i32, start: i64) -> i64 {
    match (previous["pid"].as_i64(), previous["process_start"].as_i64()) {
        (Some(p), Some(recorded)) if p == i64::from(pid) && crate::procs::is_same_process(recorded, Some(start)) => recorded,
        _ => start,
    }
}

/// Counts every target present in the snapshot, then records and evaluates the result.
pub fn collect(home: &Path, now: f64, processes: &[Proc]) -> Result<()> {
    let mut counts = Vec::new();
    for target in &TARGETS {
        let process = processes.iter().filter(|p| p.name == target.name).min_by_key(|p| p.start);
        let count = process.map(|p| lsmp(p.pid).map_err(|error| format!("{error:#}")));
        counts.push((target, process.map(|p| (p.pid, p.start)), count));
    }
    evaluate(home, now, counts)
}

pub(super) fn evaluate(home: &Path, now: f64, counts: Vec<(&Target, Option<(i32, i64)>, Option<Result<Count, String>>)>) -> Result<()> {
    let mut state = serde_json::Map::new();
    let mut errors = Vec::new();
    let last = super::read(&root(home).join("port-census.json")).unwrap_or(Value::Null);
    state.insert("at_ms".into(), json!(now * 1000.0));
    for (target, identity, count) in counts {
        let entry = match (identity, count) {
            (Some((pid, start)), Some(Ok(count))) => {
                let peers: Vec<Value> = count.peers.iter().map(|(name, n)| json!({"peer": name, "ports": n})).collect();
                let previous = &last[target.key];
                let start = stable_start(previous, pid, start);
                let samples = early::window(previous, pid, start, now * 1000.0, count.dead_names);
                let at = early::Instance { target, pid, start, count: &count, peers: &peers, previous };
                let (warning, error) = early::check(home, now, &at, &samples)?;
                errors.extend(error);
                // The limit is part of the key, so a lowered-limit trial never suppresses the real incident.
                let limit = target.effective_limit();
                if count.total >= limit {
                    let id = incident(home, &format!("{}_port_leak:{pid}:{start}:{limit}", target.key), json!({
                        "trigger": format!("{}_port_leak", target.key), "process": target.name, "pid": pid,
                        "process_start": start, "uptime_s": now as i64 - start, "ports": count.total,
                        "top_peers": peers, "threshold": {"ports": limit, "interval_s": INTERVAL_SECS},
                        // Per-app microphone sessions and their measured ControlCenter port deltas today.
                        "mic_sessions_today": super::read(&root(home).join("mic-sessions.json")).map(|m| m["today"].clone()),
                    }))?;
                    // Stop the bleeding first: only ControlCenter is restarted automatically.
                    if target.key == "controlcenter" && !cfg!(test) {
                        let restart = super::cc_restart::restart(home, now, pid, start, count.total, &id)?;
                        if restart["outcome"].as_str().is_some_and(|o| o.starts_with("failed")) {
                            errors.push(format!("restart {id}: {}", restart["outcome"]));
                        }
                    }
                    if let Err(error) = crate::dispatch::port_leak(home, now, &id, target.name) {
                        errors.push(format!("dispatch {id}: {error:#}"));
                    }
                }
                json!({"pid": pid, "process_start": start, "uptime_s": now as i64 - start,
                    "ports": count.total, "dead_names": count.dead_names, "top_peers": peers, "limit": limit,
                    "dead_window": if target.early.is_some() { json!(samples) } else { Value::Null },
                    "early": warning})
            }
            (Some((pid, _)), Some(Err(error))) => {
                errors.push(format!("{}: {error}", target.name));
                json!({"pid": pid, "error": error, "limit": target.limit})
            }
            _ => json!({"pid": null, "error": "process not running", "limit": target.limit}),
        };
        state.insert(target.key.into(), entry);
    }
    state.insert("probe_errors".into(), json!(errors));
    state.insert("error".into(), Value::Null);
    let state = Value::Object(state);
    write_json(&root(home).join("port-census.json"), &state)?;
    append(home, now, &state)
}

fn append(home: &Path, now: f64, state: &Value) -> Result<()> {
    let directory = root(home).join("port-census");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let day = ((now * 1000.0) / DAY_MS).floor() * DAY_MS;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(directory.join(format!("{}.ndjson", day as u64)))?;
    file.write_all(format!("{state}\n").as_bytes())?;
    Ok(())
}

/// ControlCenter's current port count for the microphone session ledger; None when unavailable.
/// ControlCenter's pid and Mach port count; the pid tells a respawned instance apart.
pub(super) fn controlcenter_ports() -> Option<(i32, u64)> {
    let out = Command::new("/usr/bin/pgrep").args(["-x", "ControlCenter"]).stdin(Stdio::null()).output().ok()?;
    let pid: i32 = String::from_utf8_lossy(&out.stdout).lines().next()?.trim().parse().ok()?;
    lsmp(pid).ok().map(|count| (pid, count.total))
}

/// Streams `lsmp` (WindowServer's dump reached 158,000 lines) and keeps the first `total` line
/// plus per-peer right counts; a timer kills the run after LSMP_TIMEOUT.
fn lsmp(pid: i32) -> Result<Count> {
    let home = std::env::var("HOME").context("HOME")?;
    let mut child = Command::new("/usr/bin/sudo")
        .args(["-A", "/usr/bin/lsmp", "-p", &pid.to_string()])
        .env("SUDO_ASKPASS", format!("{home}/.local/bin/sudo-askpass"))
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().context("start sudo lsmp")?;
    let sudo = child.id() as i32;
    let (done, timed_out) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if timed_out.recv_timeout(LSMP_TIMEOUT).is_err() {
            // SAFETY: signals only the sudo child this function started.
            unsafe { libc::kill(sudo, libc::SIGKILL) };
        }
    });
    let parsed = parse(BufReader::new(child.stdout.take().context("lsmp stdout")?));
    let _ = done.send(());
    let status = child.wait()?;
    let count = parsed?;
    anyhow::ensure!(status.success() || count.is_some(), "sudo lsmp exited {status}");
    count.context("lsmp printed no total")
}

pub(super) fn parse(mut reader: impl BufRead) -> Result<Option<Count>> {
    let mut peers: HashMap<String, u64> = HashMap::new();
    let mut dead_names = 0;
    let mut total = None;
    let mut raw = Vec::new();
    loop {
        raw.clear();
        if reader.read_until(b'\n', &mut raw)? == 0 { break; }
        // lsmp cuts peer names at a byte limit, which can split a multibyte character
        // ("SimMetalHost (客服消息弹…"); decode lossily so one such peer cannot fail the count.
        let line = String::from_utf8_lossy(&raw);
        let line = line.trim_end_matches(['\n', '\r']);
        if total.is_none() && line.starts_with("total") {
            total = line.split('=').nth(1).and_then(|n| n.trim().parse().ok());
        }
        // Peer columns end in "(<pid>) <name>".
        if line.starts_with("Process") { continue; }
        if line.split_whitespace().nth(2) == Some("dead-name") { dead_names += 1; }
        if let Some(open) = line.rfind(") ").and_then(|end| line[..end].rfind('(').map(|start| (start, end))) {
            let (start, end) = open;
            if line[start + 1..end].chars().all(|c| c.is_ascii_digit()) && end > start + 1 {
                *peers.entry(line[start..].trim().to_string()).or_default() += 1;
            }
        }
    }
    let mut peers: Vec<(String, u64)> = peers.into_iter().collect();
    peers.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    peers.truncate(TOP_PEERS);
    Ok(total.map(|total| Count { total, peers, dead_names }))
}

#[path = "port_census_early.rs"]
mod early;
pub use early::Early;

#[cfg(test)]
#[path = "port_census_tests.rs"]
mod tests;
