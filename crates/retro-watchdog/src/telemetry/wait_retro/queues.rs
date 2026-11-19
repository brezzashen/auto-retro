//! Queue-side evidence for one wait retro window: what the host queues themselves recorded while
//! the agents waited, and which queue entries are stuck right now.
//!
//! Sources: release queue waits (`mars-deliver/queue-waits.jsonl`), build gate admissions
//! (`mars-cpu-build-gate/slot-events.jsonl`), job-bus lifecycle events (`job-bus/events.jsonl`),
//! release judges (`release-watch.json`), and the live simulator lease and browser task queues.
//! A job-bus job whose last event is not final for SILENT_MS, a lease past its expiry and a
//! ticket whose process is gone are the "stuck or never released" candidates.
use crate::monitor::root;
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

const SILENT_MS: f64 = 30.0 * 60_000.0;
/// Gate admissions shorter than this are normal.
const GATE_MIN_MS: f64 = 60_000.0;
const TOP: usize = 8;
const FINAL: [&str; 3] = ["finished", "failed", "cancelled"];

fn alive(pid: i64) -> bool {
    pid > 0 && (unsafe { libc::kill(pid as i32, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

fn jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn within(at: Option<f64>, from: f64, to: f64) -> bool { at.is_some_and(|t| t >= from && t <= to) }

/// Count, total, p90 and max of `seconds`, with the TOP longest rows.
fn summary(mut rows: Vec<(f64, Value)>) -> Value {
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    let total: f64 = rows.iter().map(|r| r.0).sum();
    let p90 = rows.get(rows.len() / 10).map_or(0.0, |r| r.0);
    json!({"n": rows.len(), "total_s": total.round(), "p90_s": p90.round(), "max_s": rows.first().map_or(0.0, |r| r.0.round()),
        "top": rows.into_iter().take(TOP).map(|r| r.1).collect::<Vec<_>>()})
}

fn release_queue(state: &Path, from: f64, to: f64) -> Value {
    summary(jsonl(&state.join("mars-deliver/queue-waits.jsonl")).into_iter().filter(|w| within(w["ended_ms"].as_f64(), from, to))
        .map(|w| (w["waited_s"].as_f64().unwrap_or(0.0), json!({"id": w["id"], "queue": w["queue"], "target": w["target"], "waited_s": w["waited_s"], "last_line": w["last_line"]})))
        .collect())
}

/// The gate's own wait parts of one slot-events row, in seconds, named as the row names them.
/// Wait retro 814389437903940359459: a 481 s admission showed `slot_s 0, cpu_s 0` because all of it
/// was `exclusiveWaitMs` (a render-queue exclusive memory window); the parts below say which one held.
const GATE_PARTS: [(&str, &str); 7] = [("slot_s", "slotWaitMs"), ("cpu_s", "cpuWaitMs"), ("exclusive_s", "exclusiveWaitMs"),
    ("cargo_lock_s", "cargoLockWaitMs"), ("heavy_s", "heavyWaitMs"), ("spacing_s", "spacingMs"), ("headroom_s", "headroomWaitMs")];

fn gate_parts(e: &Value) -> serde_json::Map<String, Value> {
    // slot_s and cpu_s always appear (0 when absent), the others only when they held the build.
    GATE_PARTS.iter().enumerate().map(|(i, (name, field))| (i, (*name).to_string(), (e[*field].as_f64().unwrap_or(0.0) / 1000.0).round()))
        .filter(|(i, _, s)| *i < 2 || *s > 0.0).map(|(_, name, s)| (name, json!(s))).collect()
}

fn build_gate(state: &Path, from: f64, to: f64) -> Value {
    let events: Vec<Value> = jsonl(&state.join("mars-cpu-build-gate/slot-events.jsonl")).into_iter().filter(|e| within(e["at"].as_f64(), from, to)).collect();
    let all: f64 = events.iter().filter_map(|e| e["waitedMs"].as_f64()).sum();
    let mut out = summary(events.iter().filter(|e| e["waitedMs"].as_f64().unwrap_or(0.0) >= GATE_MIN_MS)
        .map(|e| {
            let mut row = json!({"at": e["at"], "command": e["command"], "waited_s": (e["waitedMs"].as_f64().unwrap_or(0.0) / 1000.0).round(), "holders": e["holders"]});
            row.as_object_mut().unwrap().extend(gate_parts(e));
            (e["waitedMs"].as_f64().unwrap_or(0.0) / 1000.0, row)
        }).collect());
    let parts: serde_json::Map<String, Value> = GATE_PARTS.iter()
        .map(|(name, field)| ((*name).to_string(), json!((events.iter().filter_map(|e| e[*field].as_f64()).sum::<f64>() / 1000.0).round()))).collect();
    out["admissions"] = json!(events.len());
    out["all_waited_s"] = json!((all / 1000.0).round());
    out["parts_s"] = Value::Object(parts);
    out
}

/// Per job: queued → admitted wait inside the window, and jobs silent for SILENT_MS right now.
fn job_bus(state: &Path, from: f64, to: f64, now: f64) -> Value {
    let mut jobs: BTreeMap<(String, String), (Option<f64>, Option<f64>, f64, String, usize)> = BTreeMap::new();
    // A job that reached a terminal event is over even when a `verdict` or a late detached
    // `deployed` follows it (job-bus `latest`); 2026-10-29 that listed ~60 ended runs as silent.
    let mut ended: std::collections::BTreeSet<(String, String)> = std::collections::BTreeSet::new();
    for e in jsonl(&state.join("job-bus/events.jsonl")) {
        let (Some(ms), Some(topic), Some(job), Some(event)) = (e["ms"].as_f64(), e["topic"].as_str(), e["job"].as_str(), e["event"].as_str()) else { continue };
        if FINAL.contains(&event) { ended.insert((topic.to_string(), job.to_string())); }
        let entry = jobs.entry((topic.to_string(), job.to_string())).or_insert((None, None, 0.0, String::new(), 0));
        match event {
            // The relay repeats `admitted` for later lines (merge offers, rollout lines): the queue
            // ends at the first one after `queued` (wait retro 818033576248735926453: 62 of 113 jobs
            // repeated it, and taking the last one turned 1421 s of deploy into a 1394 s queue wait).
            "queued" => (entry.0, entry.1) = (Some(ms), None),
            "admitted" if entry.0.is_some() && entry.1.is_none() => entry.1 = Some(ms),
            "progress" => entry.4 += 1,
            _ => {}
        }
        (entry.2, entry.3) = (ms, event.to_string());
    }
    let waits = jobs.iter().filter_map(|((topic, job), (queued, admitted, ..))| {
        let (q, a) = ((*queued)?, (*admitted)?);
        within(Some(a), from, to).then(|| ((a - q) / 1000.0, json!({"topic": topic, "job": job, "waited_s": ((a - q) / 1000.0).round()})))
    }).collect();
    let silent: Vec<Value> = jobs.iter().filter(|(key, (_, _, last, event, _))| !ended.contains(*key) && !FINAL.contains(&event.as_str()) && now - last >= SILENT_MS)
        .map(|((topic, job), (_, _, last, event, progress))| json!({"topic": topic, "job": job, "last_event": event, "silent_min": ((now - last) / 60_000.0).round(), "progress_events": progress}))
        .collect();
    json!({"admission_waits": summary(waits), "silent": silent})
}

/// Live simulator leases past expiry and tickets or browser tasks whose process is gone.
fn live(state: &Path, now: f64) -> Value {
    let files = |dir: &Path| -> Vec<(String, Value)> {
        std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let value = std::fs::read(e.path()).ok().and_then(|b| serde_json::from_slice(&b).ok())?;
            Some((name, value))
        }).collect()
    };
    let expired: Vec<Value> = files(&state.join("sim-lease/leases")).into_iter()
        .filter(|(_, l)| l["expires"].as_f64().is_some_and(|t| t * 1000.0 < now))
        .map(|(_, l)| json!({"udid": l["udid"], "name": l["name"], "owner": l["owner"], "purpose": l["purpose"], "expired_min": ((now - l["expires"].as_f64().unwrap_or(0.0) * 1000.0) / 60_000.0).round()}))
        .collect();
    let orphan_tickets: Vec<Value> = files(&state.join("sim-lease/queue")).into_iter()
        .filter(|(_, t)| !alive(t["pid"].as_i64().unwrap_or(0)))
        .map(|(name, t)| json!({"ticket": name, "pid": t["pid"], "owner": t["owner"], "purpose": t["purpose"]})).collect();
    let tasks = super::super::read(&state.join("browser-task-queue/queue.json")).and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let orphan_tasks: Vec<Value> = tasks.into_iter().filter(|t| !alive(t["pid"].as_i64().unwrap_or(0))).collect();
    json!({"sim_leases_expired": expired, "sim_tickets_orphaned": orphan_tickets, "browser_tasks_orphaned": orphan_tasks})
}

pub(in crate::telemetry) fn evidence(home: &Path, from: f64, to: f64, now: f64) -> Value {
    let state = home.join(".local/state");
    let judges: Vec<Value> = super::super::read(&root(home).join("release-watch.json"))
        .and_then(|s| s["dispatches"].as_array().cloned()).unwrap_or_default().into_iter()
        .filter(|d| within(d["at_s"].as_f64().map(|s| s * 1000.0), from, to)).collect();
    json!({"window": {"from_ms": from, "to_ms": to}, "release_queue": release_queue(&state, from, to), "build_gate": build_gate(&state, from, to),
        "job_bus": job_bus(&state, from, to, now), "release_judges": judges, "live": live(&state, now)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gate_row_names_the_part_that_held_it() {
        // The 2026-11-17 17:34 SGT row of wait retro 814389437903940359459 (render-queue exclusive window).
        let dir = std::env::temp_dir().join(format!("wait-retro-gate-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("mars-cpu-build-gate")).unwrap();
        let row = json!({"at": 1794908073633u64, "command": "xcodebuild -scheme MarsCRM", "cpuWaitMs": 239, "exclusiveWaitMs": 480506,
            "holders": 1, "slotWaitMs": 17, "spacingMs": 0, "waitedMs": 480774});
        let light = json!({"at": 1794908073700u64, "command": "bun test", "cpuWaitMs": 2000, "light": true, "waitedMs": 2000});
        std::fs::write(dir.join("mars-cpu-build-gate/slot-events.jsonl"), format!("{row}\n{light}\n")).unwrap();
        let out = build_gate(&dir, 0.0, 1.0e13);
        let top = &out["top"][0];
        assert_eq!((top["waited_s"].as_f64(), top["exclusive_s"].as_f64(), top["slot_s"].as_f64(), top["cpu_s"].as_f64()), (Some(481.0), Some(481.0), Some(0.0), Some(0.0)), "{out}");
        assert!(top.get("spacing_s").is_none() && top.get("cargo_lock_s").is_none(), "{out}");
        assert_eq!((out["parts_s"]["exclusive_s"].as_f64(), out["parts_s"]["cpu_s"].as_f64(), out["admissions"].as_u64()), (Some(481.0), Some(2.0), Some(2)), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_job_waits_until_its_first_admission_not_its_last_relayed_one() {
        let dir = std::env::temp_dir().join(format!("wait-retro-jobbus-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("job-bus")).unwrap();
        let line = |ms: f64, job: &str, event: &str| json!({"ms": ms, "topic": "release", "job": job, "event": event}).to_string();
        let events = [
            line(1_000.0, "a", "queued"), line(481_000.0, "a", "admitted"), line(1_380_000.0, "a", "admitted"),
            line(1_395_000.0, "a", "admitted"), line(1_422_000.0, "a", "finished"),
            line(2_000.0, "b", "admitted"), line(3_000.0, "b", "queued"), line(63_000.0, "b", "admitted"),
        ];
        std::fs::write(dir.join("job-bus/events.jsonl"), events.join("\n") + "\n").unwrap();
        let out = job_bus(&dir, 0.0, 2_000_000.0, 2_000_000.0);
        let waits = &out["admission_waits"];
        assert_eq!(waits["n"], 2, "{out}");
        assert_eq!(waits["max_s"], 480.0, "{out}");
        assert_eq!(waits["total_s"], 540.0, "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
