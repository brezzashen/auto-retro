//! Recall retro watch (founder, 2026-11-01): after every THRESHOLD real vector recalls, open one
//! session that measures what local-search actually returned: whether the hits were relevant
//! (recall@1/3/5 per corpus, judged) and whether each hit is usable as printed: code names its
//! function with `path:start-end`, a report hit is a working reports.example.com link with the case
//! verdict, a docs hit places the section in its file and heading trail.
//!
//! `local-search query|search` appends one line per call to `~/.local/share/local-search/
//! queries.jsonl`. Every SAMPLE_SECS this watch counts the new `query` lines that used vectors.
//! Reaching THRESHOLD, it drops script batches (`batch.rs`); when agent recalls still reach it, it
//! copies the window to `recall-retro/<id>.jsonl`, scores its telemetry
//! (`score.rs`: zero-hit share, latency, corpus mix) and opens one `recall_calls_reached` incident
//! plus one session on route `recall-retro`. None opens while the last is unacknowledged, and at
//! most MAX_DISPATCH_PER_DAY a day; the window keeps counting meanwhile. A first start without
//! state seeds the window with the last THRESHOLD vector recalls, so the first review needs no wait.
//!
//! A replaced or shorter log resumes after the last counted line (`cursor.rs`), never from 0.
//!
//! State: `recall-retro.json` (offsets, last counted `ts_ms`, log inode, window count, dispatches).
mod batch;
mod cursor;
mod score;
pub(super) mod validate;

use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub(super) const SAMPLE_SECS: f64 = 60.0;
pub(super) const THRESHOLD: u64 = 50;
const MAX_DISPATCH_PER_DAY: usize = 3;
const RETRY_SECS: f64 = 600.0;
const DAY_MS: f64 = 86_400_000.0;

#[derive(Default)]
pub(super) struct RecallRetro {
    loaded: bool,
    last_sample: f64,
    offset: u64,
    window_start: u64,
    window_count: u64,
    total: u64,
    /// `ts_ms` of the last counted line and of the window's first line, for `cursor::resume`.
    last_ms: f64,
    window_from_ms: f64,
    inode: u64,
    dispatches: Vec<Value>,
}

pub(super) fn log_path(home: &Path) -> PathBuf {
    home.join(".local/share/local-search/queries.jsonl")
}

/// A telemetry line that is one vector recall: a `query` call whose search used vectors.
fn is_recall(line: &str) -> bool {
    serde_json::from_str::<Value>(line).is_ok_and(|v| v["sub"] == "query" && v["vector_used"] == true)
}

impl RecallRetro {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded {
            self.load(home)?;
        }
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS {
            return Ok(());
        }
        self.last_sample = now;
        self.count(home)?;
        let open = self.escalate(home, now * 1000.0)?;
        self.write(home, now, open)
    }

    fn load(&mut self, home: &Path) -> Result<()> {
        self.loaded = true;
        if let Some(state) = super::read(&root(home).join("recall-retro.json")) {
            self.offset = state["offset"].as_u64().unwrap_or(0);
            self.window_start = state["window_start"].as_u64().unwrap_or(0);
            self.window_count = state["window_count"].as_u64().unwrap_or(0);
            self.total = state["total"].as_u64().unwrap_or(0);
            self.last_ms = state["last_ms"].as_f64().unwrap_or(0.0);
            self.window_from_ms = state["window_from_ms"].as_f64().unwrap_or(0.0);
            self.inode = state["inode"].as_u64().unwrap_or(0);
            self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
            return Ok(());
        }
        self.seed(home)
    }

    /// Without state: the window starts at the THRESHOLD-th last vector recall of the log.
    fn seed(&mut self, home: &Path) -> Result<()> {
        let Ok(bytes) = std::fs::read(log_path(home)) else { return Ok(()) };
        let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |e| e + 1);
        let mut starts: Vec<u64> = Vec::new();
        let mut at = 0usize;
        for line in bytes[..end].split(|b| *b == b'\n') {
            if is_recall(&String::from_utf8_lossy(line)) {
                starts.push(at as u64);
            }
            at += line.len() + 1;
        }
        let take = starts.len().min(THRESHOLD as usize);
        self.offset = end as u64;
        self.window_start = starts.get(starts.len() - take).copied().unwrap_or(end as u64);
        self.window_count = take as u64;
        self.last_ms = bytes[..end].split(|b| *b == b'\n').map(cursor::ts_of).fold(0.0, f64::max);
        self.window_from_ms = cursor::ts_of(bytes[self.window_start as usize..end].split(|b| *b == b'\n').next().unwrap_or_default());
        Ok(())
    }

    /// Counts the vector recalls among the complete lines appended since the last sample.
    fn count(&mut self, home: &Path) -> Result<()> {
        let Ok(mut file) = std::fs::File::open(log_path(home)) else { return Ok(()) };
        let meta = file.metadata()?;
        let (len, inode) = (meta.len(), std::os::unix::fs::MetadataExt::ino(&meta));
        if len < self.offset || (self.inode != 0 && inode != self.inode) {
            // Replaced (copy, restore) or truncated: resume by timestamp, never from offset 0.
            let bytes = std::fs::read(log_path(home))?;
            (self.offset, self.window_start) = cursor::resume(&bytes, self.last_ms, self.window_from_ms);
        }
        self.inode = inode;
        if self.window_from_ms <= 0.0 && self.window_start < self.offset {
            // State written before cursor.rs: date the window by its first line, not the next fresh one.
            let mut head = Vec::new();
            file.seek(SeekFrom::Start(self.window_start))?;
            (&mut file).take(self.offset - self.window_start).read_to_end(&mut head)?;
            self.window_from_ms = head.split(|b| *b == b'\n').map(cursor::ts_of).find(|t| *t > 0.0).unwrap_or(0.0);
        }
        let len = file.metadata()?.len().max(self.offset);
        file.seek(SeekFrom::Start(self.offset))?;
        let mut fresh = Vec::new();
        file.take(len - self.offset).read_to_end(&mut fresh)?;
        let Some(end) = fresh.iter().rposition(|b| *b == b'\n') else { return Ok(()) };
        let lines: Vec<&[u8]> = fresh[..end].split(|b| *b == b'\n').collect();
        let recalls = lines.iter().filter(|l| is_recall(&String::from_utf8_lossy(l))).count() as u64;
        let first_ms = lines.iter().map(|l| cursor::ts_of(l)).find(|t| *t > 0.0).unwrap_or(0.0);
        if self.window_from_ms <= 0.0 && self.window_start == self.offset {
            self.window_from_ms = first_ms;
        }
        self.last_ms = lines.iter().map(|l| cursor::ts_of(l)).fold(self.last_ms, f64::max);
        self.offset += end as u64 + 1;
        self.window_count += recalls;
        self.total += recalls;
        Ok(())
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.last().filter(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    fn escalate(&mut self, home: &Path, at: f64) -> Result<Option<String>> {
        let open = self.open_incident(home);
        let last = self.dispatches.last();
        let failed_recently = last.is_some_and(|d| d["started"] != true && at - d["at_ms"].as_f64().unwrap_or(0.0) < RETRY_SECS * 1000.0);
        let today = self.dispatches.iter().filter(|d| d["started"] == true && d["at_ms"].as_f64().is_some_and(|t| at - t < DAY_MS)).count();
        if self.window_count < THRESHOLD || open.is_some() || failed_recently || today >= MAX_DISPATCH_PER_DAY {
            return Ok(open);
        }
        let lines = self.window_lines(home)?;
        let scored = score::score(&lines);
        // The raw count reached THRESHOLD; script batches (`batch.rs`) alone do not open a retro.
        if scored["recalls"].as_u64().unwrap_or(0) < THRESHOLD {
            return Ok(open);
        }
        let previous: Vec<Value> = self.dispatches.iter().rev().filter(|d| d["started"] == true).take(5)
            .map(|d| json!({"incident": d["incident"], "recalls": d["recalls"],
                "report": root(home).join("analysis").join(d["incident"].as_str().unwrap_or("")).join("report.json")})).collect();
        let id = super::diagnostics::incident(home, &format!("recall_calls_reached:{}", self.window_start), json!({
            "trigger": "recall_calls_reached", "recalls": scored["recalls"], "raw_recalls": self.window_count, "threshold": THRESHOLD,
            // from_ms/to_ms let `retro-recall` mark each recalled transcript line in or out of the
            // window; without them every hit read back `in_window: null` (813940471853705769812).
            "window": {"from_offset": self.window_start, "to_offset": self.offset, "lines": lines.len(),
                "from_ms": ts_ms(lines.first()), "to_ms": ts_ms(lines.last())},
            "score": scored, "previous": previous,
        }))?;
        let batch = root(home).join("recall-retro").join(format!("{id}.jsonl"));
        std::os::unix::fs::DirBuilderExt::mode(std::fs::DirBuilder::new().recursive(true), 0o700).create(batch.parent().unwrap_or(Path::new(".")))?;
        write_lines(&batch, &lines)?;
        let recalls = scored["recalls"].as_u64().unwrap_or(0);
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, &format!("看门狗：向量召回评测 {recalls} 次"), &prompt(home, &id, recalls, &scored))
        };
        self.dispatches.push(match &result {
            Ok(agent) => json!({"incident": id, "recalls": recalls, "at_ms": at, "started": true, "agent": agent}),
            Err(error) => json!({"incident": id, "recalls": recalls, "at_ms": at, "started": false, "error": format!("{error:#}")}),
        });
        if result.is_err() {
            return Ok(None);
        }
        self.window_start = self.offset;
        self.window_from_ms = 0.0;
        self.window_count = 0;
        super::conversation_retro::notify("看门狗：向量召回评测", &format!("{recalls} 次向量召回，已开会话评测召回率与召回内容"));
        Ok(Some(id))
    }

    fn window_lines(&self, home: &Path) -> Result<Vec<String>> {
        let mut file = std::fs::File::open(log_path(home))?;
        file.seek(SeekFrom::Start(self.window_start))?;
        let mut bytes = Vec::new();
        file.take(self.offset - self.window_start).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).lines().filter(|l| serde_json::from_str::<Value>(l).is_ok()).map(String::from).collect())
    }

    fn write(&self, home: &Path, now: f64, open: Option<String>) -> Result<()> {
        write_json(&root(home).join("recall-retro.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "log": log_path(home), "offset": self.offset, "window_start": self.window_start, "window_count": self.window_count,
            "last_ms": self.last_ms, "window_from_ms": self.window_from_ms, "inode": self.inode,
            "total": self.total, "open_incident": open, "dispatches": self.dispatches,
            "threshold": {"recalls": THRESHOLD, "sample_s": SAMPLE_SECS, "max_dispatch_per_day": MAX_DISPATCH_PER_DAY}}))
    }
}

/// The `ts_ms` of one telemetry line, `null` when the line is missing or carries none.
fn ts_ms(line: Option<&String>) -> Value {
    line.and_then(|l| serde_json::from_str::<Value>(l).ok()).map(|v| v["ts_ms"].clone()).unwrap_or(Value::Null)
}

fn write_lines(path: &Path, lines: &[String]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let mut file = std::fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(path)?;
    std::io::Write::write_all(&mut file, text.as_bytes())?;
    Ok(())
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
pub(super) fn prompt(home: &Path, id: &str, recalls: u64, scored: &Value) -> String {
    format!("cpu-watchdog incident {id}: agents made {recalls} local-search vector recalls (threshold {THRESHOLD}); zero-hit share {}%, query p50 {} ms. {header} \
        Incident: {s}/incidents/{id}.json; window: {s}/recall-retro/{id}.jsonl; state root {s}. {recall} \
        Run the content audit, judge relevance of every audited query's Top5, compute recall@1/3/5 and content pass rates per corpus, fix the top defects with measured before/after, \
        publish the encrypted report page and give its link, write {s}/analysis/{id}/report.md in Chinese and report.json, then run cpu-watchdog monitor-ack {id}.",
        scored["zero_hit_pct"], scored["latency_ms"]["p50"], header = crate::dispatch::route_header("recall-retro"), recall = crate::retro_recall::instruction(id), s = root(home).display())
}

#[cfg(test)]
#[path = "recall_retro/tests.rs"]
mod tests;
