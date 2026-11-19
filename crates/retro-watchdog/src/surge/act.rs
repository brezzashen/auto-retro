//! Runs one surge response: evidence first (incident written before any signal), then each rule
//! decision with a re-checked pid/start identity, the ControlCenter check, the whole-host CPU
//! after the actions, and the report. A real episode then wakes one `surge` session (`wake.rs`,
//! user request 2026-10-24), which reviews the actions and the cause and acknowledges the incident.
//! An episode joined to an open session or deferred by its daily cap is acknowledged here only when
//! every action and the report succeeded; otherwise `monitor-check` keeps it pending.
use super::{control_center, history::History, launchd, rules::{self, Action, Decision}};
use crate::{monitor::{root, write_json}, policy::Policy, procs::{self, Proc}};
use anyhow::Result;
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path, process::{Command, Stdio}, time::Duration};

const GRACE_SECS: u64 = 30;

pub struct Episode {
    pub id: String,
    pub trigger: &'static str,
    /// False for `surge-now`: decisions are reported, not executed.
    pub execute: bool,
    pub since_ms: f64,
    pub end_ms: f64,
    pub frames: Value,
}

pub fn respond(home: &Path, episode: &Episode, processes: &[Proc], history: &History) -> Result<()> {
    let now = crate::unix_now();
    let jobs = launchd::list(home);
    let job_list = jobs.as_deref().unwrap_or_default();
    let failing: Vec<Value> = job_list.iter().filter(|job| job.failing()).map(|job| json!({
        "label": job.label, "pid": job.pid, "last_exit": job.status, "own": rules::own_job(&job.label),
        "stderr_tail": job.stderr.as_deref().and_then(launchd::tail)})).collect();
    let ledger_path = root(home).join("surge-kills.json");
    let mut killed: HashMap<String, f64> = crate::telemetry::read_state(&ledger_path)
        .and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default();
    let policy = Policy::from_env();
    let decisions = rules::decide(&rules::Inputs { now, processes, history, policy: &policy, jobs: job_list,
        killed: &killed, own_pid: std::process::id() as i32 });
    let incident = root(home).join("incidents").join(format!("{}.json", episode.id));
    write_json(&incident, &json!({"id": episode.id, "trigger": episode.trigger,
        "hot_since_ms": episode.since_ms, "detected_at_ms": episode.end_ms, "process_frames": episode.frames,
        "failing_launchd_jobs": failing, "launchd_error": jobs.as_ref().err().map(|e| format!("{e:#}")),
        "recent_crash_reports": launchd::recent_crashes(home), "decisions": decisions,
        "telemetry": crate::telemetry::evidence(home)}))?;
    let mut outcomes = Vec::new();
    for decision in &decisions {
        if !episode.execute {
            outcomes.push(json!({"decision": decision, "outcome": "not executed (surge-now)"}));
            continue;
        }
        let outcome = execute(decision).unwrap_or_else(|error| format!("failed: {error:#}"));
        if let Action::Killall { name } = &decision.action {
            killed.insert(name.clone(), now);
            write_json(&ledger_path, &killed)?;
        }
        crate::log(&format!("cpu surge {}: {} {} -> {outcome}", episode.id, decision.rule, decision.target));
        outcomes.push(json!({"decision": decision, "outcome": outcome}));
    }
    let control_center = control_center::check(home, crate::unix_now(), processes, history, &episode.id);
    let report = json!({"id": episode.id, "status": "responded", "hot_since_ms": episode.since_ms,
        "detected_at_ms": episode.end_ms, "actions": outcomes, "failing_launchd_jobs": failing,
        "recent_crash_reports": launchd::recent_crashes(home), "control_center": control_center,
        "host_cpu_after": host_cpu_after(home), "finished_at_ms": crate::unix_now() * 1000.0});
    super::report::write(home, &episode.id, &report)?;
    let clean = outcomes.iter().all(|o| !o["outcome"].as_str().unwrap_or("").starts_with("failed"))
        && jobs.is_ok() && control_center["census_error"].is_null()
        && !control_center["restart"]["outcome"].as_str().unwrap_or("").starts_with("failed");
    // The surge session also serves as the recovery review of these actions.
    let detail = format!("whole-host CPU stayed above 95% for {:.0} s; {} rule action(s) taken",
        (episode.end_ms - episode.since_ms) / 1000.0, outcomes.len());
    let wake = episode.execute.then(|| crate::wake::request(home, crate::unix_now() * 1000.0, crate::wake::Condition::CpuSurge, &episode.id, &detail));
    // Receipts below close deterministic actions, not a model root-cause diagnosis.
    let receipt = json!({"deterministic_recovery": true, "diagnosis": "not_performed"});
    match wake {
        Some(Ok(crate::wake::Outcome::Queued)) => {}
        Some(Ok(outcome)) if clean => crate::wake::ack(home, &episode.id, outcome, receipt)?,
        Some(Err(error)) => {
            crate::log(&format!("cpu surge {}: session request failed: {error:#}", episode.id));
            if clean { crate::wake::ack(home, &episode.id, crate::wake::Outcome::Deferred, json!({"deterministic_recovery": true, "wake_error": format!("{error:#}")}))?; }
        }
        None if clean => write_json(&incident.with_extension("ack"), &json!({"acknowledged_at_ms":crate::unix_now()*1000.0,
            "kind":"deterministic_recovery","diagnosis":"not_performed"}))?,
        _ => {}
    }
    Ok(())
}

fn execute(decision: &Decision) -> Result<String> {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    match &decision.action {
        Action::Terminate { pid, start } => terminate(*pid, *start),
        Action::SudoKillall { name } => run(Command::new("/usr/bin/sudo").args(["-A", "/usr/bin/killall", name])
            .env("SUDO_ASKPASS", format!("{}/.local/bin/sudo-askpass", std::env::var("HOME")?))),
        Action::Killall { name } => run(Command::new("/usr/bin/killall").arg(name)),
        Action::Kickstart { label } => run(Command::new("/bin/launchctl").args(["kickstart", "-k", &format!("gui/{uid}/{label}")])),
        Action::StopJob { label } => run(Command::new("/bin/launchctl").args(["kill", "SIGTERM", &format!("gui/{uid}/{label}")])),
    }
}

fn run(command: &mut Command) -> Result<String> {
    let output = command.stdin(Stdio::null()).output()?;
    anyhow::ensure!(output.status.success(), "{} {}", output.status, String::from_utf8_lossy(&output.stderr).trim());
    Ok("done".into())
}

/// SIGTERM to the verified process, SIGKILL after GRACE_SECS if the same process survives.
fn terminate(pid: i32, start: i64) -> Result<String> {
    anyhow::ensure!(procs::still_same_process(pid, start, crate::unix_now), "pid exited or was reused");
    // SAFETY: signals only the process whose pid and start time were just re-verified.
    anyhow::ensure!(unsafe { libc::kill(pid, libc::SIGTERM) } == 0, "SIGTERM: {}", std::io::Error::last_os_error());
    for _ in 0..GRACE_SECS {
        std::thread::sleep(Duration::from_secs(1));
        if !procs::still_same_process(pid, start, crate::unix_now) {
            return Ok("exited after SIGTERM".into());
        }
    }
    // SAFETY: same verified identity, re-checked on the last loop iteration.
    anyhow::ensure!(unsafe { libc::kill(pid, libc::SIGKILL) } == 0, "SIGKILL: {}", std::io::Error::last_os_error());
    Ok("SIGKILL after the 30 s grace".into())
}

/// The newest whole-host sample from the build gate's collector, taken after the actions.
fn host_cpu_after(home: &Path) -> Value {
    std::thread::sleep(Duration::from_secs(10));
    crate::telemetry::read_state(&home.join(".local/state/mars-cpu-build-gate/history.json"))
        .and_then(|history| history["samples"].as_array().and_then(|s| s.last().cloned()))
        .unwrap_or(Value::Null)
}
