//! Win guard (founder, 2026-11-03): the studio Windows PC runs beta's Docker/WSL VM and offload's
//! GPU renders. Render commit memory lives outside the VM's `.wslconfig` cap: on 2026-11-03 nine
//! Blender renders, seven orphaned after their Mac-side `offload render` died (Windows OpenSSH
//! keeps a lost session's processes), left 7.7 of 129.8 GB commit free and wedged the VM
//! (incident 810663098879893038580). offload now admits no GPU job below its commit floor and stops
//! a failed job's remote process; this guard reclaims what still slips through.
//!
//! Every SAMPLE_SECS a worker asks the remote beta host (the tunnel's SSH alias) for its commit
//! memory and every process whose command line carries an `offload-job=` tag, then:
//! 1. Orphans: a tagged process older than ORPHAN_AGE_S whose job no live offload slot on this Mac
//!    holds (`~/.local/state/offload/<host>-<pool>/slot-*/owner.json`), seen so in two rounds in a
//!    row, is stopped (`Stop-Process -Force`).
//! 2. Commit floor: when free commit after the reap stays under CRITICAL_COMMIT_MB, the newest held
//!    render is stopped too (its offload exits non-zero and falls back), at most MAX_STOPS_HOUR an
//!    hour, so the VM keeps its memory.
//!
//! A host that does not answer is left to docker_watch. Records go to `win-guard/<day-ms>.ndjson`,
//! state to `win-guard.json`.
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
    sync::mpsc::{self, Receiver, TryRecvError},
};

pub(super) const SAMPLE_SECS: f64 = 60.0;
const ORPHAN_AGE_S: i64 = 120;
pub(super) const CRITICAL_COMMIT_MB: i64 = 12 * 1024;
const MAX_STOPS_HOUR: usize = 2;
const SSH_SECS: u64 = 45;
const PROBE: &str = r#"$ErrorActionPreference='SilentlyContinue'
$o=Get-CimInstance Win32_OperatingSystem
$now=Get-Date
$jobs=@(Get-CimInstance Win32_Process -Filter "CommandLine like '%offload-job=%'" | ForEach-Object { if ($_.CommandLine -match 'offload-job=([^\s"'']+)') { [ordered]@{pid=[int64]$_.ProcessId; tag=$Matches[1]; name=$_.Name; age_s=[int64]($now - $_.CreationDate).TotalSeconds; commit_mb=[int64]($_.PageFileUsage/1KB)} } })
ConvertTo-Json -Compress -Depth 4 -InputObject ([ordered]@{commit_free_mb=[int64]($o.FreeVirtualMemory/1KB); commit_total_mb=[int64]($o.TotalVirtualMemorySize/1KB); jobs=$jobs; gpu=$gpu})"#;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Job { pub pid: i64, pub tag: String, pub age_s: i64, pub commit_mb: i64 }

/// What one round decided: processes to stop as orphans, to stop for memory, and this round's
/// orphan pids (reaped next round if still there).
#[derive(Debug, Default, PartialEq)]
pub(super) struct Plan { pub reap: Vec<i64>, pub relieve: Vec<i64>, pub suspects: Vec<i64> }

struct Round { host: String, sample: Value, plan: Plan, stop_output: Option<String>, error: Option<String> }

/// GPU-lost self-heal state (`win_guard/gpu.rs`).
#[derive(Default)]
struct GpuHeal { streak: u32, defer_since: f64, block_until: f64, last: Value, worker: Option<Receiver<Value>> }

#[derive(Default)]
pub(super) struct WinGuard { last_sample: f64, suspects: Vec<i64>, relief_ms: Vec<f64>, last: Value, worker: Option<Receiver<Round>>, gpu: GpuHeal }

impl WinGuard {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(round) => { self.worker = None; self.absorb(home, now, round); }
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        self.gpu.poll(now);
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS { return Ok(()); }
        self.last_sample = now;
        let Some(host) = super::docker_watch::remote_host(home) else {
            return write_json(&root(home).join("win-guard.json"), &json!({"at_ms": now * 1000.0, "error": null, "host": null}));
        };
        if self.worker.is_none() && self.gpu.worker.is_none() {
            self.relief_ms.retain(|ms| now * 1000.0 - ms < 3_600_000.0);
            let (suspects, relief_left, home) = (self.suspects.clone(), MAX_STOPS_HOUR.saturating_sub(self.relief_ms.len()), home.to_path_buf());
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::Builder::new().name("watchdog-win-guard".into())
                .spawn(move || { let _ = sender.send(round(&home, &host, &suspects, relief_left)); })?;
            self.worker = Some(receiver);
        }
        write_json(&root(home).join("win-guard.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "last": self.last, "suspects": self.suspects, "relief_ms": self.relief_ms, "gpu": self.gpu.state(),
            "threshold": {"sample_s": SAMPLE_SECS, "orphan_age_s": ORPHAN_AGE_S, "critical_commit_mb": CRITICAL_COMMIT_MB, "max_stops_hour": MAX_STOPS_HOUR}}))
    }

    fn absorb(&mut self, home: &Path, now: f64, round: Round) {
        self.gpu.absorb(home, now, &round.host, &round.sample);
        self.suspects = round.plan.suspects.clone();
        if !round.plan.relieve.is_empty() { self.relief_ms.push(now * 1000.0); }
        let acted = !round.plan.reap.is_empty() || !round.plan.relieve.is_empty();
        let record = json!({"at_ms": now * 1000.0, "kind": "win_guard", "sample": round.sample, "reap": round.plan.reap,
            "relieve": round.plan.relieve, "stop_output": round.stop_output, "sample_error": round.error});
        if acted {
            crate::log(&format!("win guard: {record}"));
            if let Err(error) = append(home, now, &record) { crate::log(&format!("win guard log: {error:#}")); }
            crate::dispatch::notify("看门狗：win 资源回收", &format!("停掉残留渲染 {} 个、为内存让路 {} 个，可用提交内存 {} MB",
                round.plan.reap.len(), round.plan.relieve.len(), round.sample["commit_free_mb"]));
        }
        self.last = record;
    }
}

fn round(home: &Path, host: &str, suspects: &[i64], relief_left: usize) -> Round {
    let probe = ssh(home, host, &super::docker_watch::powershell(&format!("{}\n{PROBE}", gpu::PROBE)));
    let sample = match probe.as_deref().map(|text| serde_json::from_str::<Value>(text.trim())) {
        Ok(Ok(value)) => value,
        Ok(Err(e)) => return Round { host: host.into(), sample: Value::Null, plan: Plan::default(), stop_output: None, error: Some(format!("unreadable probe: {e}")) },
        Err(e) => return Round { host: host.into(), sample: Value::Null, plan: Plan::default(), stop_output: None, error: Some(format!("{e:#}")) },
    };
    let held = held_jobs(&home.join(".local/state/offload"), host);
    let plan = decide(&jobs(&sample["jobs"]), &held, suspects, sample["commit_free_mb"].as_i64().unwrap_or(i64::MAX), relief_left);
    let pids: Vec<String> = plan.reap.iter().chain(&plan.relieve).map(i64::to_string).collect();
    let stop_output = (!pids.is_empty()).then(|| {
        let script = format!("Stop-Process -Id {} -Force -ErrorAction SilentlyContinue; 'stopped'", pids.join(","));
        ssh(home, host, &super::docker_watch::powershell(&script)).unwrap_or_else(|e| format!("{e:#}"))
    });
    Round { host: host.into(), sample, plan, stop_output, error: None }
}

fn ssh(home: &Path, host: &str, command: &str) -> Result<String> {
    super::command::capture(home, "/usr/bin/ssh", &["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", host, command], SSH_SECS)
}

/// The probe's `jobs`: an array, a bare object when PowerShell unrolled a single one, or null.
pub(super) fn jobs(value: &Value) -> Vec<Job> {
    let list = match value { Value::Array(list) => list.clone(), Value::Object(_) => vec![value.clone()], _ => Vec::new() };
    list.iter().filter_map(|j| Some(Job { pid: j["pid"].as_i64()?, tag: j["tag"].as_str()?.to_string(),
        age_s: j["age_s"].as_i64().unwrap_or(0), commit_mb: j["commit_mb"].as_i64().unwrap_or(0) })).collect()
}

/// Job ids held by a live offload slot on this Mac for `host` (any pool).
pub(super) fn held_jobs(state: &Path, host: &str) -> Vec<String> {
    let alive = |pid: i64| pid > 0 && unsafe { libc::kill(pid as libc::pid_t, 0) == 0 || *libc::__error() == libc::EPERM };
    ["gpu", "cpu"].iter().flat_map(|pool| std::fs::read_dir(state.join(format!("{host}-{pool}"))).into_iter().flatten().flatten())
        .filter_map(|slot| serde_json::from_str::<Value>(&std::fs::read_to_string(slot.path().join("owner.json")).ok()?).ok())
        .filter(|owner| owner["pid"].as_i64().is_some_and(alive))
        .filter_map(|owner| owner["job"].as_str().map(str::to_string)).collect()
}

pub(super) fn decide(jobs: &[Job], held: &[String], suspects: &[i64], commit_free_mb: i64, relief_left: usize) -> Plan {
    let orphans: Vec<&Job> = jobs.iter().filter(|j| !held.contains(&j.tag) && j.age_s >= ORPHAN_AGE_S).collect();
    // Short of commit, an orphan is stopped at first sight instead of on its second round.
    let critical = commit_free_mb < CRITICAL_COMMIT_MB;
    let reap: Vec<i64> = orphans.iter().filter(|j| critical || suspects.contains(&j.pid)).map(|j| j.pid).collect();
    let freed: i64 = jobs.iter().filter(|j| reap.contains(&j.pid)).map(|j| j.commit_mb).sum();
    let mut plan = Plan { suspects: orphans.iter().map(|j| j.pid).filter(|p| !reap.contains(p)).collect(), reap, relieve: Vec::new() };
    if commit_free_mb.saturating_add(freed) < CRITICAL_COMMIT_MB && relief_left > 0 {
        if let Some(newest) = jobs.iter().filter(|j| !plan.reap.contains(&j.pid) && !plan.suspects.contains(&j.pid)).min_by_key(|j| j.age_s) {
            plan.relieve.push(newest.pid);
        }
    }
    plan
}

fn append(home: &Path, now: f64, record: &Value) -> Result<()> {
    let directory = root(home).join("win-guard");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let day = (now / 86_400.0).floor() * 86_400_000.0;
    std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(directory.join(format!("{}.ndjson", day as u64)))?.write_all(format!("{record}\n").as_bytes())?;
    Ok(())
}

#[path = "win_guard/gpu.rs"]
mod gpu;
#[path = "win_guard/heal.rs"]
mod heal;

#[cfg(test)]
#[path = "win_guard/tests.rs"]
mod tests;
