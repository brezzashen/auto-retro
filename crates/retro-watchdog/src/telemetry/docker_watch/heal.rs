//! Probes, cause evidence, Docker Desktop recovery and the beta Redis AOF repair.
use super::Probe;
use serde_json::{json, Value};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const ACTION_TIMEOUT: Duration = Duration::from_secs(300);
/// Engine plus Kubernetes readiness after a start (measured 2026-10-24: about 2.5 minutes).
const READY_WAIT: Duration = Duration::from_secs(420);
/// A backend younger than this is still starting; restarting it would only restart the wait.
pub(super) const FRESH_START_SECS: u64 = 240;
pub(super) const MIN_FREE_BYTES: u64 = 10 * 1024 * 1024 * 1024;
const PATH: &str = "/home/user/.docker/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
const KUBE_CONTEXT: &str = "docker-desktop";
const NAMESPACE: &str = "mars-crm";
const REDIS_NODES: &str = "app.kubernetes.io/name=mars-crm-redis,app.kubernetes.io/component=node";
pub(super) const AOF_SIGNATURE: &str = "Bad file format reading the append only file";
pub(super) const LOG_DIR: &str = "Library/Containers/com.docker.docker/Data/log/host";
const SETTINGS: &str = "Library/Group Containers/group.com.docker/settings-store.json";
const DEFAULT_DATA: &str = "Library/Containers/com.docker.docker/Data/vms/0/data";

pub(super) struct Out { pub code: Option<i32>, pub text: String }

/// Runs a command with a deadline; stdout and stderr are drained on threads so a full pipe can't
/// stall the child.
pub(super) fn run(program: &Path, args: &[&str], timeout: Duration) -> Out {
    let child = Command::new(program).args(args).env("PATH", PATH).stdin(Stdio::null())
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => return Out { code: None, text: format!("spawn {}: {error}", program.display()) },
    };
    let drain = |pipe: Option<Box<dyn Read + Send>>| std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(pipe) = pipe { let _ = pipe.take(1 << 20).read_to_string(&mut text); }
        text
    });
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let started = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(100)),
            _ => { let _ = child.kill(); let _ = child.wait(); break None; }
        }
    };
    let text = format!("{}{}", out.join().unwrap_or_default(), err.join().unwrap_or_default());
    Out { code, text }
}

fn docker(home: &Path) -> PathBuf { home.join(".docker/bin/docker") }
fn kubectl(home: &Path) -> PathBuf { home.join(".docker/bin/kubectl") }
fn kube(home: &Path, args: &[&str], timeout: Duration) -> Out {
    let mut full = vec!["--context", KUBE_CONTEXT, "-n", NAMESPACE];
    full.extend_from_slice(args);
    run(&kubectl(home), &full, timeout)
}

fn probe(program: &Path, args: &[&str]) -> Probe {
    let started = Instant::now();
    let out = run(program, args, PROBE_TIMEOUT);
    let ms = started.elapsed().as_millis() as u64;
    let ok = out.code == Some(0);
    Probe { ok, ms, error: (!ok).then(|| out.text.trim().chars().take(300).collect()) }
}

pub(super) fn probe_engine(home: &Path) -> Probe {
    probe(&docker(home), &["--context", "desktop-linux", "version", "--format", "{{.Server.Version}}"])
}

pub(super) fn probe_kube(home: &Path) -> Probe {
    probe(&kubectl(home), &["--context", KUBE_CONTEXT, "get", "--raw", "/readyz"])
}

/// What the host shows about Docker right now: processes, VM disk copies, data volume, logs.
fn evidence(home: &Path) -> Value {
    let ps = run(Path::new("/bin/ps"), &["-axo", "pid=,etime=,command="], PROBE_TIMEOUT).text;
    let docker_lines: Vec<&str> = ps.lines().filter(|l| ["com.docker", "Docker Desktop", "Docker.raw", "DockerDesktop"].iter().any(|k| l.contains(k))).collect();
    let copying: Vec<&str> = docker_lines.iter().copied().filter(|l| is_disk_copy(l)).collect();
    let blocked: Vec<&str> = docker_lines.iter().copied().filter(|l| is_blocked_exclusion(l)).collect();
    let backend_age = docker_lines.iter().find(|l| l.trim_end().ends_with("com.docker.backend"))
        .and_then(|l| l.split_whitespace().nth(1)).and_then(etime_secs);
    let logs = home.join(LOG_DIR);
    let settings = super::super::read(&home.join(SETTINGS));
    let folder = settings.as_ref().and_then(|s| s["DataFolder"].as_str().map(String::from))
        .or_else(|| std::fs::read_to_string(logs.join("com.docker.virtualization.log")).ok().and_then(|l| disk_folder(&l, home)))
        .unwrap_or_else(|| home.join(DEFAULT_DATA).display().to_string());
    let exists = Path::new(&folder).exists();
    let free = exists.then(|| run(Path::new("/bin/df"), &["-k", &folder], PROBE_TIMEOUT).text).and_then(|t| df_free_bytes(&t));
    let status = run(&docker(home), &["desktop", "status", "--format", "json"], Duration::from_secs(15));
    let short: Vec<String> = docker_lines.iter().filter(|l| !l.contains("Docker Desktop Helper"))
        .map(|l| l.chars().take(300).collect()).collect();
    json!({"processes": short, "copying": copying, "blocked": blocked, "backend_age_s": backend_age,
        "data_folder": folder, "data_exists": exists, "data_free_bytes": free,
        "desktop_status": {"exit": status.code, "output": status.text.chars().take(600).collect::<String>()},
        "backend_errors": error_lines(&tail(&logs.join("com.docker.backend.log"), 400), 30),
        "virtualization_tail": tail(&logs.join("com.docker.virtualization.log"), 15),
        "crash_reports": crash_reports(home), "signals": signals(),
        "stopped_by": if cfg!(test) { Value::Null } else { super::restarts::current(home, crate::unix_now() * 1000.0) }})
}

/// Unified-log lines of the last 15 minutes where a Docker process exited on a signal and the
/// sender is named (`exited due to SIGTERM | sent by pkill[82489]`). The log keeps them only for
/// days, so they are read when the outage is detected.
fn signals() -> Vec<String> {
    if cfg!(test) { return Vec::new(); }
    let out = run(Path::new("/usr/bin/log"), &["show", "--last", "15m", "--style", "compact", "--predicate",
        "eventMessage CONTAINS \"sent by\" AND eventMessage CONTAINS[c] \"docker\""], Duration::from_secs(60));
    out.text.lines().filter(|l| l.contains("sent by")).take(20).map(|l| l.chars().take(300).collect()).collect()
}

/// The cause the evidence supports, in the order that decides whether a restart is safe.
pub(super) fn classify(evidence: &Value, engine_ok: bool) -> &'static str {
    if evidence["copying"].as_array().is_some_and(|c| !c.is_empty()) { return "vm_disk_copy"; }
    if evidence["data_exists"] == false { return "data_volume_missing"; }
    if evidence["data_free_bytes"].as_u64().is_some_and(|f| f < MIN_FREE_BYTES) { return "data_volume_full"; }
    if evidence["blocked"].as_array().is_some_and(|b| !b.is_empty()) { return "volume_access_blocked"; }
    if evidence["backend_age_s"].as_u64().is_some_and(|a| a < FRESH_START_SECS) { return "starting"; }
    if engine_ok { return "kubernetes_down"; }
    if evidence["backend_age_s"].is_null() { "backend_dead" } else { "engine_hung" }
}

pub(super) fn recover(home: &Path, engine_ok: bool) -> Value {
    let started_at_ms = crate::unix_now() * 1000.0;
    let evidence = evidence(home);
    let cause = classify(&evidence, engine_ok);
    let base = |outcome: String, steps: Vec<Value>| json!({"at_ms": crate::unix_now() * 1000.0,
        "started_at_ms": started_at_ms, "kind": "docker_recovery", "cause": cause,
        "outcome": outcome, "steps": steps, "evidence": evidence});
    match cause {
        "vm_disk_copy" | "starting" => return base(format!("skipped: {cause}"), vec![]),
        "data_volume_missing" | "data_volume_full" | "volume_access_blocked" => return base(format!("held: {cause}"), vec![]),
        _ => {}
    }
    let action = if cause == "backend_dead" { "start" } else { "restart" };
    let out = run(&docker(home), &["desktop", action], ACTION_TIMEOUT);
    let mut steps = vec![json!({"action": action, "exit": out.code, "output": out.text.chars().take(600).collect::<String>()})];
    let waited = Instant::now();
    let ready = loop {
        if probe_engine(home).ok && probe_kube(home).ok { break true; }
        if waited.elapsed() >= READY_WAIT { break false; }
        std::thread::sleep(Duration::from_secs(5));
    };
    steps.push(json!({"action": "wait_ready", "ready": ready, "seconds": waited.elapsed().as_secs()}));
    base(if ready { "recovered".into() } else { "failed".into() }, steps)
}

/// Repairs each beta Redis replica that crash-loops on a truncated AOF: back up the AOF
/// directory, truncate the bad tail with `redis-check-aof --fix`, and delete the pod so it
/// restarts and resyncs from the primary. `skip` holds pods handled earlier in the window.
pub(super) fn repair_redis(home: &Path, skip: &[String]) -> Vec<Value> {
    let pods = kube(home, &["get", "pods", "-l", REDIS_NODES, "-o", "json"], PROBE_TIMEOUT);
    let Ok(pods) = serde_json::from_str::<Value>(&pods.text) else { return Vec::new() };
    let (crashing, ready) = redis_pods(&pods);
    let crashing: Vec<String> = crashing.into_iter().filter(|p| !skip.contains(p)).collect();
    if crashing.is_empty() { return Vec::new(); }
    let primary = ready.first().map(|pod| kube(home, &["exec", pod, "-c", "sentinel", "--", "redis-cli", "-p", "26379",
        "sentinel", "get-master-addr-by-name", "mars"], PROBE_TIMEOUT).text).unwrap_or_default();
    let primary = primary.lines().next().unwrap_or_default().trim().to_string();
    let mut records = Vec::new();
    for pod in crashing {
        let log = kube(home, &["logs", &pod, "-c", "redis", "--previous", "--tail=20"], PROBE_TIMEOUT).text;
        if !log.contains(AOF_SIGNATURE) { continue; }
        let record = |outcome: &str, steps: Vec<Value>| json!({"at_ms": crate::unix_now() * 1000.0,
            "kind": "redis_aof_repair", "pod": pod, "primary": primary, "outcome": outcome,
            "log_tail": log.lines().rev().take(3).collect::<Vec<_>>(), "steps": steps});
        if primary.is_empty() || primary.starts_with(&format!("{pod}.")) {
            records.push(record("held: primary or unknown primary", vec![]));
            continue;
        }
        let script = format!("cp -a /data/appendonlydir /data/appendonlydir.bak-{} && yes | redis-check-aof --fix /data/appendonlydir/appendonly.aof.manifest",
            crate::unix_now() as u64);
        let fix = kube(home, &["exec", &pod, "-c", "sentinel", "--", "sh", "-c", &script], Duration::from_secs(120));
        let fixed = fix.code == Some(0) && fix.text.contains("All AOF files and manifest are valid");
        let mut steps = vec![json!({"action": "check_aof_fix", "exit": fix.code, "output": fix.text.chars().rev().take(600).collect::<String>().chars().rev().collect::<String>()})];
        if fixed {
            let delete = kube(home, &["delete", "pod", &pod, "--wait=false"], PROBE_TIMEOUT);
            steps.push(json!({"action": "delete_pod", "exit": delete.code, "output": delete.text.trim()}));
        }
        records.push(record(if fixed { "repaired" } else { "failed" }, steps));
    }
    records
}

/// (replicas whose redis container is in CrashLoopBackOff, pods whose sentinel is ready).
pub(super) fn redis_pods(pods: &Value) -> (Vec<String>, Vec<String>) {
    let (mut crashing, mut ready) = (Vec::new(), Vec::new());
    for pod in pods["items"].as_array().into_iter().flatten() {
        let Some(name) = pod["metadata"]["name"].as_str() else { continue };
        for status in pod["status"]["containerStatuses"].as_array().into_iter().flatten() {
            if status["name"] == "redis" && status["state"]["waiting"]["reason"] == "CrashLoopBackOff" { crashing.push(name.to_string()); }
            if status["name"] == "sentinel" && status["ready"] == true { ready.push(name.to_string()); }
        }
    }
    ready.retain(|p| !crashing.contains(p));
    (crashing, ready)
}

/// A cp/rsync/ditto/mv whose arguments name the Docker VM disk or its data folder.
pub(super) fn is_disk_copy(line: &str) -> bool {
    let command = line.split_whitespace().nth(2).unwrap_or_default();
    let tool = command.rsplit('/').next().unwrap_or_default();
    ["cp", "rsync", "ditto", "mv"].contains(&tool) && (line.contains("Docker.raw") || line.contains("DockerDesktop"))
}

/// The backend's `tmutil addexclusion` on the data volume, still running after FRESH_START_SECS.
/// Twice on 2026-10-24 the VM start waited for it while it waited on an unanswered removable-volume
/// access prompt, and `docker desktop restart` could not stop Docker meanwhile.
pub(super) fn is_blocked_exclusion(line: &str) -> bool {
    let mut words = line.split_whitespace();
    let age = words.nth(1).and_then(etime_secs);
    let tool = words.next().unwrap_or_default().rsplit('/').next().unwrap_or_default();
    tool == "tmutil" && line.contains(" addexclusion ") && age.is_some_and(|a| a >= FRESH_START_SECS)
}

/// `ps` etime (`[[dd-]hh:]mm:ss`) in seconds.
pub(super) fn etime_secs(etime: &str) -> Option<u64> {
    let (days, rest) = etime.split_once('-').map_or((0, etime), |(d, r)| (d.parse().unwrap_or(0), r));
    let parts: Vec<u64> = rest.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let secs = parts.iter().fold(0, |acc, p| acc * 60 + p);
    Some(days * 86_400 + secs)
}

pub(super) fn df_free_bytes(text: &str) -> Option<u64> {
    text.lines().nth(1)?.split_whitespace().nth(3)?.parse::<u64>().ok().map(|kib| kib * 1024)
}

/// Data folder from `com.docker.virtualization.log` when `Group Containers` is TCC-blocked to the LaunchAgent.
pub(super) fn disk_folder(log: &str, home: &Path) -> Option<String> {
    let raw = log.lines().rev().find_map(|l| Some(l.split_once("Disk 0 (")?.1.split_once("/Docker.raw)")?.0))?;
    Some(raw.strip_prefix("<HOME>").map_or_else(|| raw.to_string(), |r| format!("{}{r}", home.display())))
}

/// Error lines of the backend log, without the event-stream EOFs every client disconnect writes.
pub(super) fn error_lines(lines: &[String], keep: usize) -> Vec<String> {
    let errors: Vec<String> = lines.iter().filter(|l| {
        let lower = l.to_ascii_lowercase();
        (lower.contains("error") || lower.contains("fatal") || lower.contains("panic")) && !lower.contains("event streamer: error: eof")
    }).cloned().collect();
    errors[errors.len().saturating_sub(keep)..].to_vec()
}

fn tail(path: &Path, lines: usize) -> Vec<String> {
    let text = std::fs::read(path).map(|b| String::from_utf8_lossy(&b[b.len().saturating_sub(lines * 400)..]).into_owned()).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].iter().map(|l| l.chars().take(400).collect()).collect()
}

/// Docker crash reports written in the last two hours.
fn crash_reports(home: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(home.join("Library/Logs/DiagnosticReports")) else { return Vec::new() };
    entries.flatten().filter(|e| e.file_name().to_string_lossy().to_ascii_lowercase().contains("docker"))
        .filter(|e| e.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age.as_secs() < 7200))
        .map(|e| e.file_name().to_string_lossy().into_owned()).collect()
}

#[cfg(test)]
pub(super) fn evidence_for_test(home: &Path) -> Value { evidence(home) }
