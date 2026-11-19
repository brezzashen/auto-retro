//! One durable admission queue for all watchdog model sessions. Recovery never waits on it.
//! At most MAX_ACTIVE analyses hold a slot; an urgent job starts before every ordinary queued job
//! and may also take the URGENT_RESERVE slot (founder, 2026-11-01).
//! A launching/uncertain receipt is never replayed blindly: after PROBE_AFTER_MS an uncertain job whose
//! prompt opened no session is requeued, and one whose session is live counts as launched.
//! A launched session that ended without its ack is taken over once, then abandoned to free the slot.
use crate::monitor::{root, write_json};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{fs::{File, OpenOptions}, os::{fd::AsRawFd, unix::fs::OpenOptionsExt}, path::Path};

#[path = "pressure.rs"]
mod pressure;
#[path = "liveness.rs"]
pub(crate) mod liveness;
pub(crate) use liveness::opened_for;
const MAX_JOBS: usize = 512;
/// Analysis sessions that may run at once.
pub(super) const MAX_ACTIVE: usize = 2;
/// Extra slot only an urgent job may take when the ordinary slots are full.
pub(super) const URGENT_RESERVE: usize = 1;
const STALE_ACTIVE_MS: f64 = 7_200_000.0;

#[derive(Debug)]
pub(super) struct Preflight(pub String);
impl std::fmt::Display for Preflight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.0) }
}
impl std::error::Error for Preflight {}

struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) { unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN); } }
}
fn lock(home: &Path) -> Result<Lock> {
    std::fs::create_dir_all(root(home))?;
    let file = OpenOptions::new().create(true).truncate(false).read(true).write(true).mode(0o600)
        .open(root(home).join("analysis-queue.lock"))?;
    ensure!(unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0, "analysis queue lock failed");
    Ok(Lock(file))
}
fn read(home: &Path) -> Result<Value> {
    let path = root(home).join("analysis-queue.json");
    if !path.exists() { return Ok(json!({"version":1,"jobs":[]})); }
    let bytes = std::fs::read(path)?;
    ensure!(bytes.len() <= 8 * 1024 * 1024, "analysis queue exceeds 8 MiB");
    let value: Value = serde_json::from_slice(&bytes)?;
    ensure!(value["version"] == 1 && value["jobs"].is_array(), "invalid analysis queue");
    Ok(value)
}
fn save(home: &Path, now: f64, value: &mut Value) -> Result<()> {
    value["at_ms"] = json!(now);
    value["max_active"] = json!(MAX_ACTIVE);
    value["urgent_reserve"] = json!(URGENT_RESERVE);
    value["cpu_limit_percent"] = json!(pressure::CPU_LIMIT);
    write_json(&root(home).join("analysis-queue.json"), value)
}

pub(super) fn enqueue(home: &Path, id: &str, title: &str, text: &str) -> Result<Value> {
    push(home, id, title, text, false, None)
}

/// Like `enqueue`, but ahead of every ordinary queued job, and it may take the reserved urgent slot.
pub(super) fn enqueue_urgent(home: &Path, id: &str, title: &str, text: &str) -> Result<Value> {
    push(home, id, title, text, true, None)
}

/// Like `enqueue`, for a session the model family `lead` must lead. The job keeps `lead`, so a
/// takeover of the same job starts on that model again.
pub(super) fn enqueue_led(home: &Path, id: &str, title: &str, text: &str, lead: &str) -> Result<Value> {
    push(home, id, title, text, false, Some(lead))
}

fn push(home: &Path, id: &str, title: &str, text: &str, urgent: bool, lead: Option<&str>) -> Result<Value> {
    let _guard = lock(home)?;
    let mut state = read(home)?;
    let jobs = state["jobs"].as_array_mut().unwrap();
    if !jobs.iter().any(|j| j["incident"] == id) {
        jobs.retain(|j| j["state"] != "complete");
        ensure!(jobs.len() < MAX_JOBS, "analysis queue full; incident retained for inspection");
        let job = json!({"incident":id,"title":title,"prompt":text,"state":"queued","urgent":urgent,"lead":lead,
            "queued_at_ms":crate::unix_now()*1000.0});
        // Urgent jobs keep their own order, ahead of the first ordinary queued job.
        let at = if urgent { jobs.iter().position(|j| j["state"] == "queued" && j["urgent"] != true).unwrap_or(jobs.len()) } else { jobs.len() };
        jobs.insert(at, job);
        save(home, crate::unix_now()*1000.0, &mut state)?;
    }
    Ok(json!({"mode":"queued","incident":id,"verification":"durably-queued","urgent":urgent}))
}

pub(crate) fn poll(home: &Path, now: f64) -> Result<()> {
    close_known(home, now);
    let admission = pressure::check(home, now).map_err(|e| format!("{e:#}"));
    reap(home, now, |id, prompt| liveness::probe(home, id, prompt))?;
    poll_with(home, now, admission, |job| super::start_now(home,
        job["incident"].as_str().context("missing incident")?,
        job["title"].as_str().context("missing title")?,
        job["prompt"].as_str().context("missing prompt")?, job["lead"].as_str()))
}

/// Queued port-leak jobs that a cause-tree node added while they waited close by script; the
/// ack it writes marks the job complete in `poll_with`. `now` is in milliseconds.
fn close_known(home: &Path, now: f64) {
    let ids: Vec<String> = match lock(home).and_then(|_guard| read(home)) {
        Ok(state) => state["jobs"].as_array().into_iter().flatten().filter(|j| j["state"] == "queued")
            .filter_map(|j| j["incident"].as_str().map(str::to_string)).collect(),
        Err(_) => return,
    };
    for id in ids {
        match crate::cause_tree::queued::close_queued(home, now / 1000.0, &id) {
            Ok(Some(closed)) if !cfg!(test) => super::notify("看门狗：端口泄漏",
                &format!("排队中的事件由已知原因 {} 解释，脚本结案，不再开会话", closed["node"].as_str().unwrap_or("?"))),
            Ok(_) => {}
            Err(error) => crate::log(&format!("cause tree queued {id}: {error:#}")),
        }
    }
}

/// Requeue the slot holder once when its session ended without an ack, or when an uncertain
/// launch never opened a session; abandon it the next time.
fn reap(home: &Path, now: f64, probe: impl FnOnce(&str, &str) -> Result<Option<Value>>) -> Result<()> {
    let holds_slot = |j: &Value| matches!(j["state"].as_str(), Some("launched" | "uncertain"));
    let (id, prompt, uncertain) = {
        let _guard = lock(home)?;
        let mut state = read(home)?;
        let acked = |j: &Value| j["incident"].as_str().is_some_and(|id| root(home).join("incidents").join(format!("{id}.ack")).exists());
        let Some(job) = state["jobs"].as_array_mut().unwrap().iter_mut().find(|j| holds_slot(j) && !acked(j)
            && now - j["started_at_ms"].as_f64().unwrap_or(now) > liveness::PROBE_AFTER_MS
            && now - j["liveness_checked_ms"].as_f64().unwrap_or(0.0) >= liveness::PROBE_EVERY_MS) else { return Ok(()) };
        job["liveness_checked_ms"] = json!(now);
        let triple = (job["incident"].as_str().unwrap_or_default().to_string(), job["prompt"].as_str().unwrap_or_default().to_string(),
            job["state"] == "uncertain");
        save(home, now, &mut state)?;
        triple
    };
    let session = match probe(&id, &prompt) {
        Ok(Some(session)) if liveness::dead(&session) => session,
        Ok(Some(_)) if uncertain => return settle(home, now, &id, "launched"),
        Ok(None) if uncertain => json!({"never_opened": true}),
        Ok(_) => return Ok(()),
        Err(error) => { crate::log(&format!("analysis queue liveness {id}: {error:#}")); return Ok(()) }
    };
    let _guard = lock(home)?;
    let mut state = read(home)?;
    let Some(job) = state["jobs"].as_array_mut().unwrap().iter_mut()
        .find(|j| j["incident"] == id.as_str() && holds_slot(j)) else { return Ok(()) };
    let takeovers = job["takeovers"].as_u64().unwrap_or(0);
    let mut ended = job["ended_sessions"].as_array().cloned().unwrap_or_default();
    ended.push(json!({"at_ms": now, "session": session}));
    job["ended_sessions"] = json!(ended);
    if takeovers >= liveness::MAX_TAKEOVERS {
        job["state"] = json!("abandoned");
        job["abandoned_at_ms"] = json!(now);
        if !cfg!(test) { super::notify("看门狗：分析会话未完成", &format!("事件 {id} 的会话 {} 次结束都未确认，已释放队列，见 cpu-watchdog retro-overdue", takeovers + 1)); }
    } else {
        let original = job["origin_prompt"].as_str().or(job["prompt"].as_str()).unwrap_or_default().to_string();
        job["prompt"] = json!(if session["never_opened"] == true { original.clone() }
            else { liveness::takeover_prompt(&id, &original, &session, takeovers + 1) });
        job["origin_prompt"] = json!(original);
        job["takeovers"] = json!(takeovers + 1);
        job["state"] = json!("queued");
        job["started_at_ms"] = Value::Null;
        job["agent"] = Value::Null;
        job["error"] = Value::Null;
    }
    save(home, now, &mut state)
}

fn settle(home: &Path, now: f64, id: &str, to: &str) -> Result<()> {
    let _guard = lock(home)?;
    let mut state = read(home)?;
    if let Some(job) = state["jobs"].as_array_mut().unwrap().iter_mut().find(|j| j["incident"] == id && j["state"] == "uncertain") {
        job["state"] = json!(to);
    }
    save(home, now, &mut state)
}

fn poll_with(home: &Path, now: f64, admission: std::result::Result<(), String>,
    launch: impl FnOnce(&Value) -> Result<Value>) -> Result<()> {
    let job = {
        let _guard = lock(home)?;
        let mut state = read(home)?;
        state["error"] = Value::Null;
        let jobs = state["jobs"].as_array_mut().unwrap();
        for job in jobs.iter_mut() {
            if job["incident"].as_str().is_some_and(|id| root(home).join("incidents").join(format!("{id}.ack")).exists()) {
                job["state"] = json!("complete");
                job["prompt"] = Value::Null;
            }
        }
        let holding: Vec<&Value> = jobs.iter().filter(|j| matches!(j["state"].as_str(), Some("launching" | "launched" | "uncertain"))).collect();
        let inspect = holding.iter().any(|j| j["state"] == "uncertain" || now - j["started_at_ms"].as_f64().unwrap_or(now) > STALE_ACTIVE_MS);
        let active = holding.len();
        let queued = |urgent: bool| move |j: &Value| j["state"] == "queued" && (!urgent || j["urgent"] == true);
        // An urgent job jumps every ordinary one, even a takeover requeued ahead of it.
        let next = jobs.iter().position(queued(true)).or_else(|| jobs.iter().position(queued(false)));
        if inspect { state["error"] = json!("Active analysis needs inspection; never replay an ambiguous launch"); }
        let Some(next) = next else {
            state["waiting"] = Value::Null;
            save(home, now, &mut state)?;
            return Ok(());
        };
        let urgent = state["jobs"][next]["urgent"] == true;
        let limit = MAX_ACTIVE + if urgent { URGENT_RESERVE } else { 0 };
        if active >= limit {
            state["waiting"] = json!(format!("{active} analyses already active (limit {limit})"));
            save(home, now, &mut state)?;
            return Ok(());
        }
        if let Err(reason) = admission {
            state["waiting"] = json!(reason);
            save(home, now, &mut state)?;
            return Ok(());
        }
        let job = &mut state["jobs"][next];
        job["state"] = json!("launching");
        job["started_at_ms"] = json!(now);
        let job = job.clone();
        state["waiting"] = Value::Null;
        save(home, now, &mut state)?;
        job
    };
    // Never hold the file lock across account discovery or terminal creation.
    let result = launch(&job);
    let _guard = lock(home)?;
    let mut state = read(home)?;
    let record = state["jobs"].as_array_mut().unwrap().iter_mut()
        .find(|j| j["incident"] == job["incident"]).context("analysis job disappeared")?;
    match result {
        Ok(receipt) => { record["state"] = json!("launched"); record["agent"] = receipt; record["error"] = Value::Null; }
        Err(error) => {
            record["state"] = json!(if error.is::<Preflight>() { "queued" } else { "uncertain" });
            record["error"] = json!(format!("{error:#}"));
        }
    }
    save(home, now, &mut state)
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
