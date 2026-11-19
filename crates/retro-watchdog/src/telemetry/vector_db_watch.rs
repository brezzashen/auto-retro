//! Vector database growth watch (founder request, 2026-11-18: a watchdog on the vector database that
//! opens a retro every 5 GB of growth, judges whether a database should be split and optimizes its
//! performance).
//!
//! 1. Every SAMPLE_SECS the loop only `stat`s the local-search databases (`index.db`, `index-dev.db`,
//!    `index-support.db`) and their vector sidecars (`<db>.<model>.<corpus>.q8.{rows,idx}`). WAL, shm,
//!    lock and temporary files are left out: a VACUUM's WAL or an APFS test clone must not fire.
//! 2. The baseline is the measured total at install and after every opened retro. It ratchets down
//!    when the total shrinks (a compact or a retention cut), so the threshold measures net growth.
//! 3. Growth of THRESHOLD_BYTES (5 GiB) over the baseline starts the evidence worker off the loop
//!    (`evidence.rs`: `local-search stats`, sqlite page and freelist counts, query latency per corpus,
//!    lock failures in the sync logs). Its snapshot opens one `vector_db_growth_reached` incident and
//!    one session on route `vector-db-retro`, then becomes the next baseline. No session opens while
//!    the previous one is unacknowledged (`blocked`), and at most MAX_PER_DAY per rolling 24 h.
//!
//! State: `vector-db-watch.json` (baseline snapshot, hourly size history, dispatches).
//! CLI: `cpu-watchdog vector-db-stats` prints the measure, the baseline and the distance to the threshold.
pub(super) mod evidence;
pub(super) mod validate;

use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

pub(super) const SAMPLE_SECS: f64 = 600.0;
/// 5 GiB (5 × 1024³ bytes).
pub(super) const THRESHOLD_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const MAX_PER_DAY: usize = 2;
const HISTORY_EVERY_MS: f64 = 3_600_000.0;
/// About 60 days of hourly samples.
const KEEP_HISTORY: usize = 1440;
const KEEP_DISPATCHES: usize = 50;
const TRIGGER: &str = "vector_db_growth_reached";

pub(super) fn index_dir(home: &Path) -> PathBuf {
    home.join(".local/share/local-search")
}

/// The database a counted file belongs to (`index.db`, `index-dev.db`, …), or None for any other file.
pub(super) fn counted(name: &str) -> Option<&str> {
    if !name.starts_with("index") {
        return None;
    }
    if name.ends_with(".db") {
        return Some(name);
    }
    if name.ends_with(".q8.rows") || name.ends_with(".q8.idx") {
        return name.find(".db.").map(|i| &name[..i + 3]);
    }
    None
}

/// Sizes of every counted file: `{total, files: {name: bytes}, dbs: {db: bytes}, wal: {name: bytes}}`.
pub(super) fn measure(dir: &Path) -> Value {
    let (mut total, mut files, mut dbs, mut wal) = (0u64, Map::new(), Map::new(), Map::new());
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        if name.starts_with("index") && name.ends_with(".db-wal") {
            wal.insert(name, json!(meta.len()));
            continue;
        }
        let Some(db) = counted(&name).map(str::to_string) else { continue };
        total += meta.len();
        let sum = dbs.get(&db).and_then(Value::as_u64).unwrap_or(0) + meta.len();
        dbs.insert(db, json!(sum));
        files.insert(name, json!(meta.len()));
    }
    json!({"total": total, "files": files, "dbs": dbs, "wal": wal})
}

#[derive(Default)]
pub(super) struct VectorDbWatch {
    loaded: bool,
    last_sample: f64,
    /// `{bytes, at_ms, files, dbs, lowered_at_ms, snapshot}`; `snapshot` is the evidence at that point.
    pub(super) baseline: Value,
    pub(super) history: Vec<Value>,
    pub(super) dispatches: Vec<Value>,
    pub(super) blocked: Option<Value>,
    last_measure: Value,
    /// Receives `(mode, snapshot)`; mode `baseline` fills the baseline snapshot, `crossing` opens a retro.
    worker: Option<Receiver<(String, Value)>>,
}

impl VectorDbWatch {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded {
            self.loaded = true;
            if let Some(state) = super::read(&root(home).join("vector-db-watch.json")) {
                self.baseline = state["baseline"].clone();
                self.history = state["history"].as_array().cloned().unwrap_or_default();
                self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
            }
        }
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok((mode, snapshot)) => {
                    self.worker = None;
                    self.absorb(home, &mode, snapshot, now)?;
                    self.write(home, now)?;
                }
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS {
            return Ok(());
        }
        self.last_sample = now;
        let measured = measure(&index_dir(home));
        if let Some(mode) = self.step(&measured, now * 1000.0, self.open_incident(home)) {
            self.spawn(home, mode)?;
        }
        self.last_measure = measured;
        self.write(home, now)
    }

    /// Applies one measurement: sets or lowers the baseline, records history and returns the worker
    /// mode to start, if any. `open` is the unacknowledged retro incident, if one exists.
    pub(super) fn step(&mut self, measured: &Value, now_ms: f64, open: Option<String>) -> Option<&'static str> {
        let total = measured["total"].as_u64().unwrap_or(0);
        if self.history.last().and_then(|h| h["at_ms"].as_f64()).is_none_or(|t| now_ms - t >= HISTORY_EVERY_MS) {
            self.history.push(json!({"at_ms": now_ms, "total": total, "dbs": measured["dbs"]}));
            let excess = self.history.len().saturating_sub(KEEP_HISTORY);
            self.history.drain(..excess);
        }
        if self.baseline["bytes"].as_u64().is_none() {
            self.baseline = json!({"bytes": total, "at_ms": now_ms, "files": measured["files"], "dbs": measured["dbs"], "snapshot": null});
            return self.worker.is_none().then_some("baseline");
        }
        let base = self.baseline["bytes"].as_u64().unwrap_or(0);
        // A restart lost the install snapshot before it was written: take it again.
        if self.baseline["snapshot"].is_null() && self.baseline["incident"].is_null() && self.worker.is_none()
            && total < base + THRESHOLD_BYTES {
            return Some("baseline");
        }
        if total < base {
            self.baseline["bytes"] = json!(total);
            self.baseline["files"] = measured["files"].clone();
            self.baseline["dbs"] = measured["dbs"].clone();
            self.baseline["lowered_at_ms"] = json!(now_ms);
            self.blocked = None;
            return None;
        }
        if total - base < THRESHOLD_BYTES {
            self.blocked = None;
            return None;
        }
        if let Some(id) = open {
            self.blocked = Some(json!({"by": id, "growth": total - base, "at_ms": now_ms}));
            return None;
        }
        let recent = self.dispatches.iter()
            .filter(|d| d["started"] == true && d["at_ms"].as_f64().is_some_and(|t| now_ms - t < 86_400_000.0)).count();
        if recent >= MAX_PER_DAY {
            self.blocked = Some(json!({"by": "daily_cap", "growth": total - base, "at_ms": now_ms}));
            return None;
        }
        self.blocked = None;
        self.worker.is_none().then_some("crossing")
    }

    fn spawn(&mut self, home: &Path, mode: &'static str) -> Result<()> {
        if cfg!(test) {
            return Ok(());
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let home_path = home.to_path_buf();
        std::thread::Builder::new().name("watchdog-vector-db".into()).spawn(move || {
            let _ = sender.send((mode.to_string(), evidence::snapshot(&home_path)));
        })?;
        self.worker = Some(receiver);
        Ok(())
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.iter().rev().find(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    pub(super) fn absorb(&mut self, home: &Path, mode: &str, snapshot: Value, now: f64) -> Result<()> {
        if mode == "baseline" {
            self.baseline["snapshot"] = snapshot;
            return Ok(());
        }
        let measured = snapshot["measure"].clone();
        let total = measured["total"].as_u64().unwrap_or(0);
        let base = self.baseline["bytes"].as_u64().unwrap_or(0);
        if total < base + THRESHOLD_BYTES {
            crate::log(&format!("vector db watch: growth fell below the threshold before the retro opened ({total} vs {base})"));
            return Ok(());
        }
        let growth = growth_by_db(&self.baseline["dbs"], &measured["dbs"]);
        let id = super::diagnostics::incident(home, &format!("{TRIGGER}:{}", (now * 1000.0) as u64), json!({
            "trigger": TRIGGER, "threshold_bytes": THRESHOLD_BYTES, "growth_bytes": total - base,
            "baseline": self.baseline, "now": snapshot, "growth_by_db": growth,
            "history": self.history.iter().rev().take(24 * 14).rev().cloned().collect::<Vec<_>>(),
        }))?;
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, "看门狗：向量库增长复盘", &prompt(home, &id, total - base))
        };
        self.dispatches.push(match &result {
            Ok(agent) => json!({"incident": id, "at_ms": now * 1000.0, "started": true, "agent": agent, "from": base, "to": total}),
            Err(error) => json!({"incident": id, "at_ms": now * 1000.0, "started": false, "error": format!("{error:#}")}),
        });
        let excess = self.dispatches.len().saturating_sub(KEEP_DISPATCHES);
        self.dispatches.drain(..excess);
        if result.is_ok() {
            self.baseline = json!({"bytes": total, "at_ms": now * 1000.0, "files": measured["files"], "dbs": measured["dbs"],
                "snapshot": snapshot, "incident": id});
        }
        let opened = if result.is_ok() { "已开向量库复盘会话" } else { "会话未能打开，见 vector-db-watch.json" };
        super::conversation_retro::notify("看门狗：向量库增长", &format!("净增 {:.1} GiB。{opened}。", gib(total - base)));
        Ok(())
    }

    fn write(&self, home: &Path, now: f64) -> Result<()> {
        write_json(&root(home).join("vector-db-watch.json"), &self.state(home, now))
    }

    fn state(&self, home: &Path, now: f64) -> Value {
        let total = self.last_measure["total"].as_u64().unwrap_or(0);
        let base = self.baseline["bytes"].as_u64().unwrap_or(total);
        json!({"at_ms": now * 1000.0, "error": null, "running": self.worker.is_some(),
            "open_incident": self.open_incident(home), "blocked": self.blocked,
            "measure": self.last_measure, "baseline": self.baseline,
            "growth_bytes": total.saturating_sub(base), "remaining_bytes": (base + THRESHOLD_BYTES).saturating_sub(total),
            "history": self.history, "dispatches": self.dispatches,
            "schedule": {"sample_s": SAMPLE_SECS, "threshold_bytes": THRESHOLD_BYTES, "max_per_day": MAX_PER_DAY, "dir": index_dir(home)}})
    }
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Growth per database between two `dbs` maps, largest first.
pub(super) fn growth_by_db(before: &Value, after: &Value) -> Vec<Value> {
    let mut rows: Vec<Value> = after.as_object().into_iter().flatten().map(|(db, bytes)| {
        let now = bytes.as_u64().unwrap_or(0);
        let was = before[db].as_u64().unwrap_or(0);
        json!({"db": db, "before": was, "after": now, "delta": now as i64 - was as i64})
    }).collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r["delta"].as_i64().unwrap_or(0)));
    rows
}

/// `cpu-watchdog vector-db-stats`: the live measure, the baseline and the distance to the threshold.
pub(super) fn cli(home: &Path) -> Result<()> {
    let state = super::read(&root(home).join("vector-db-watch.json")).unwrap_or(Value::Null);
    let measured = measure(&index_dir(home));
    let total = measured["total"].as_u64().unwrap_or(0);
    let base = state["baseline"]["bytes"].as_u64();
    println!("{}", serde_json::to_string_pretty(&json!({
        "measure": measured, "total_gib": round2(gib(total)), "threshold_gib": gib(THRESHOLD_BYTES),
        "baseline_bytes": base, "baseline_at_ms": state["baseline"]["at_ms"],
        "growth_gib": base.map(|b| round2(gib(total.saturating_sub(b)))),
        "remaining_gib": base.map(|b| round2(gib((b + THRESHOLD_BYTES).saturating_sub(total)))),
        "baseline_snapshot": !state["baseline"]["snapshot"].is_null(),
        "open_incident": state["open_incident"], "blocked": state["blocked"],
        "dispatches": state["dispatches"], "state_at_ms": state["at_ms"],
    }))?);
    Ok(())
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
pub(super) fn prompt(home: &Path, id: &str, growth: u64) -> String {
    format!("cpu-watchdog incident {id}: the local-search vector databases grew {g:.1} GiB net since the last vector-db retro (threshold 5 GiB). {header} \
        Incident: {s}/incidents/{id}.json (baseline and now snapshots: file sizes per database and vector sidecar, local-search stats per corpus, sqlite page and freelist counts, query latency p50/p95 per corpus, lock failures in the sync logs, hourly size history). \
        Attribute the growth to corpora and sources, decide whether a database should be split (a proposal for the founder, never executed by this session) and apply measured performance optimizations that keep recall unchanged (vector-recall golden sets before and after). \
        Write {s}/analysis/{id}/report.md in Chinese and report.json with report_url, growth[] {{scope, bytes, cause}}, split {{verdict keep|propose, reason, plan when propose}}, performance {{verdict ok|regressed, evidence}} and optimizations[] {{action, verdict applied|proposed|rejected, reason, and before, after and recall when applied}}, publish the encrypted report page, then run cpu-watchdog monitor-ack {id}.",
        g = gib(growth), header = crate::dispatch::route_header("vector-db-retro"), s = root(home).display())
}

#[cfg(test)]
#[path = "vector_db_watch/tests.rs"]
mod tests;
