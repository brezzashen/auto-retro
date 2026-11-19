//! Search retro watch (user request, 2026-10-23): after every THRESHOLD local-search calls, rate how
//! well search served the agents and open one session that scripts away the recurring misses.
//! Since 2026-11-02 (founder) a window is 100 local-search calls, not 1000 search events, and the
//! retro drives answer, speed and adoption rates toward quality::TARGET.
//!
//! `script-guard` (PreToolUse Bash, every Claude account and Codex) appends one line per rg/grep/
//! find search and per `local-search query|search` to `~/.local/state/search-watch/events.jsonl`.
//! Every SAMPLE_SECS this watch counts the complete lines the log gained and, of those, the
//! local_search ones (`window_count`). Reaching THRESHOLD local-search calls copies
//! the window to `search-retro/<id>.jsonl`, scores it (`score.rs`: local-search policy compliance,
//! grade A-F) and opens one `search_calls_reached` incident plus one session on route
//! `search-retro` through `dispatch::start_session`; the window then restarts. Since 2026-11-12 the
//! window is cut where it filled and measured SETTLE_MS after its last call, so every call has its
//! whole adoption window; calls logged meanwhile start the next window. None opens while the
//! last is unacknowledged and at most MAX_DISPATCH_PER_DAY a day; the window keeps counting meanwhile.
//! The incident also carries `quality` (`quality.rs`, transcripts read by `steps.rs`): the hit rate,
//! speed and adoption of the window's local-search calls; `cpu-watchdog search-quality` recomputes it
//! for any incident or time range, so a retro can measure before and after a fix.
//! Traffic a test harness labelled with SEARCH_WATCH_SYNTHETIC (`synthetic.rs`) neither fills a
//! window nor enters its score or rates; the incident carries `score_with_synthetic` and the
//! quality's `with_synthetic` beside them (founder decision on search-retro 810495355461229102702).
//!
//! State: `search-retro.json` (offsets, window count, dispatches).
mod adopt;
mod answered;
mod domain;
mod echoed;
mod excluded;
mod edit_check;
mod elsewhere;
mod engine;
mod followed;
mod glob;
mod inner;
mod loop_var;
mod looser;
mod bare;
mod chain;
mod member;
mod module_ref;
mod none;
mod none_block;
mod owner;
mod package;
mod pattern;
mod phrase;
mod quality;
mod ran;
mod returned;
mod redirected;
mod cut_hidden;
mod timing_prefix;
mod score;
mod scoped;
mod sibling;
mod skill_load;
mod split;
mod steps;
mod subcommand;
mod sync_overlap;
mod host_overlap;
mod host_load;
mod synthetic;
mod task_pace;
mod transcripts;
mod unhooked;
mod unindexed;
mod values;

use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

pub(super) const SAMPLE_SECS: f64 = 60.0;
/// Local-search calls per window (founder, 2026-11-02: every 100 searches).
pub(super) const THRESHOLD: u64 = 100;
/// About one window every hour of the busiest day; one session at a time still applies.
const MAX_DISPATCH_PER_DAY: usize = 24;
/// `window_count` counts local-search lines since this unit was introduced.
const UNIT: &str = "local_search";
/// A session that failed to start is retried after this long.
const RETRY_SECS: f64 = 600.0;
const DAY_MS: f64 = 86_400_000.0;
/// A full window is measured only once its last call's adoption window (`adopt::ADOPT_MS`) has
/// passed, plus a margin for transcripts written after the step (the Antigravity bridge). Search-retro
/// 814337939526298636027 was measured 3 s after its last event: 1e58f6ea…jsonl:22 read its returned
/// `landing/IntroLanding.tsx` at :47 two minutes later and counted unadopted (3 failures, 2 when
/// remeasured); 812685146552855153702 (8 to 5) and 816805219412517074086 (2 to 1) lost calls the same way.
const SETTLE_MS: f64 = adopt::ADOPT_MS + 120_000.0;

#[derive(Default)]
pub(super) struct SearchRetro {
    loaded: bool,
    last_sample: f64,
    /// Bytes of the log already counted, and where the current window starts.
    offset: u64,
    window_start: u64,
    window_count: u64,
    total: u64,
    dispatches: Vec<Value>,
    /// The full window's end (log offset, last event ms) while it waits SETTLE_MS to be measured.
    cut: Option<(u64, f64)>,
}

pub(super) fn log_path(home: &Path) -> PathBuf {
    home.join(".local/state/search-watch/events.jsonl")
}

impl SearchRetro {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded {
            self.load(home);
        }
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS {
            return Ok(());
        }
        self.last_sample = now;
        self.count(home)?;
        let open = self.escalate(home, now * 1000.0)?;
        self.write(home, now, open)
    }

    fn load(&mut self, home: &Path) {
        self.loaded = true;
        if let Some(state) = super::read(&root(home).join("search-retro.json")) {
            self.offset = state["offset"].as_u64().unwrap_or(0);
            self.window_start = state["window_start"].as_u64().unwrap_or(0);
            self.window_count = state["window_count"].as_u64().unwrap_or(0);
            self.total = state["total"].as_u64().unwrap_or(0);
            self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
            self.cut = state["cut"]["offset"].as_u64().zip(state["cut"]["to_ms"].as_f64());
            // Recount the open window with the current rule, so a change of what fills a window
            // (every search event before 2026-11-02, replays until search-retro
            // 813254723049142099088) applies to it at the next start.
            self.window_count = self.local_calls_between(home, self.window_start, self.offset);
        }
    }

    /// Counts the complete lines appended since the last sample. A log shorter than the offset lost
    /// rows (a cleanup rewrote it) or was rotated: the window restarts at the log's end. Restarting
    /// at 0 made one window of the whole log on 2026-11-11 (16,406 replay rows removed at 15:35 SGT):
    /// 148,453 events and every bridged Antigravity command since October went into one measurement.
    fn count(&mut self, home: &Path) -> Result<()> {
        let path = log_path(home);
        let Ok(mut file) = std::fs::File::open(&path) else { return Ok(()) };
        let len = file.metadata()?.len();
        if len < self.offset {
            let end = line_end(&mut file, len)?;
            crate::log(&format!("search retro: the log shrank from offset {} to {len} bytes; the window restarts at {end}", self.offset));
            (self.offset, self.window_start, self.window_count, self.cut) = (end, end, 0, None);
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut fresh = Vec::new();
        file.take(len - self.offset).read_to_end(&mut fresh)?;
        let Some(end) = fresh.iter().rposition(|b| *b == b'\n') else { return Ok(()) };
        let lines = fresh[..=end].iter().filter(|b| **b == b'\n').count() as u64;
        self.offset += end as u64 + 1;
        self.window_count += local_calls(&fresh[..=end]);
        self.total += lines;
        Ok(())
    }

    fn local_calls_between(&self, home: &Path, from: u64, to: u64) -> u64 {
        let Ok(mut file) = std::fs::File::open(log_path(home)) else { return 0 };
        let mut bytes = Vec::new();
        if file.seek(SeekFrom::Start(from)).is_err() || file.take(to.saturating_sub(from)).read_to_end(&mut bytes).is_err() { return 0; }
        local_calls(&bytes)
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.last().filter(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    /// Opens one incident and session once the window holds THRESHOLD calls; returns the open incident id.
    fn escalate(&mut self, home: &Path, at: f64) -> Result<Option<String>> {
        let open = self.open_incident(home);
        let last = self.dispatches.last();
        let failed_recently = last.is_some_and(|d| d["started"] != true && at - d["at_ms"].as_f64().unwrap_or(0.0) < RETRY_SECS * 1000.0);
        let today = self.dispatches.iter().filter(|d| d["started"] == true && d["at_ms"].as_f64().is_some_and(|t| at - t < DAY_MS)).count();
        if self.window_count < THRESHOLD || open.is_some() || failed_recently || today >= MAX_DISPATCH_PER_DAY {
            return Ok(open);
        }
        // Cut the window where it filled; calls logged while it settles start the next one.
        let (cut, last_ms) = match self.cut {
            Some(cut) => cut,
            None => *self.cut.insert((self.offset, self.window_events(home, self.offset)?.iter().filter_map(|e| e["ts_ms"].as_f64()).fold(0.0, f64::max))),
        };
        if at < last_ms + SETTLE_MS {
            return Ok(None);
        }
        let all = self.window_events(home, cut)?;
        // Labelled synthetic traffic stays in the batch file but out of the score and rates (`synthetic.rs`).
        let events = synthetic::split(&all).0;
        let (from_ms, to_ms) = (events.first().and_then(|e| e["ts_ms"].as_f64()).unwrap_or(0.0), events.last().and_then(|e| e["ts_ms"].as_f64()).unwrap_or(0.0));
        // Antigravity sessions the hook did not log are metered from bridged transcripts (`unhooked.rs`).
        let (antigravity, unhooked) = if cfg!(test) { (Vec::new(), json!({})) } else { unhooked::events(home, from_ms, to_ms, &all) };
        let (scored, scored_all, left_out) = synthetic::scores(&[all.as_slice(), antigravity.as_slice()].concat());
        let quality = quality::measure(home, from_ms, to_ms);
        let previous: Vec<Value> = self.dispatches.iter().rev().filter(|d| d["started"] == true).take(5)
            .map(|d| json!({"incident": d["incident"], "score": d["score"], "grade": d["grade"],
                "report": root(home).join("analysis").join(d["incident"].as_str().unwrap_or("")).join("report.json")})).collect();
        let id = super::diagnostics::incident(home, &format!("search_calls_reached:{}", self.window_start), json!({
            "trigger": "search_calls_reached", "calls": events.len(), "threshold": THRESHOLD,
            "window": {"from_offset": self.window_start, "to_offset": cut, "from_ms": events.first().map(|e| e["ts_ms"].clone()), "to_ms": events.last().map(|e| e["ts_ms"].clone())},
            "score": scored, "score_with_synthetic": scored_all, "synthetic": left_out, "quality": quality.clone(), "unhooked": unhooked,
            "task_pace": task_pace::measure(home, &events, from_ms, to_ms), "previous": previous,
        }))?;
        let batch = root(home).join("search-retro").join(format!("{id}.jsonl"));
        std::os::unix::fs::DirBuilderExt::mode(std::fs::DirBuilder::new().recursive(true), 0o700).create(batch.parent().unwrap_or(Path::new(".")))?;
        write_lines(&batch, &[all.as_slice(), antigravity.as_slice()].concat())?;
        let (points, grade) = (scored["score"].clone(), scored["grade"].clone());
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, &format!("看门狗：检索评分 {} 分（{}）", points, grade.as_str().unwrap_or("?")), &prompt(home, &id, events.len(), &scored, &quality))
        };
        self.dispatches.push(match &result {
            Ok(agent) => json!({"incident": id, "calls": events.len(), "score": points, "grade": grade, "at_ms": at, "started": true, "agent": agent}),
            Err(error) => json!({"incident": id, "calls": events.len(), "score": points, "grade": grade, "at_ms": at, "started": false, "error": format!("{error:#}")}),
        });
        if result.is_err() {
            return Ok(None);
        }
        (self.window_start, self.window_count, self.cut) = (cut, self.local_calls_between(home, cut, self.offset), None);
        super::conversation_retro::notify("看门狗：检索评分", &format!("{} 次检索调用，得分 {}（{}），已开会话做脚本化优化", events.len(), points, grade.as_str().unwrap_or("?")));
        Ok(Some(id))
    }

    fn window_events(&self, home: &Path, end: u64) -> Result<Vec<Value>> {
        let mut file = std::fs::File::open(log_path(home))?;
        file.seek(SeekFrom::Start(self.window_start))?;
        let mut bytes = Vec::new();
        file.take(end.saturating_sub(self.window_start)).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
    }

    fn write(&self, home: &Path, now: f64, open: Option<String>) -> Result<()> {
        write_json(&root(home).join("search-retro.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "log": log_path(home), "offset": self.offset, "window_start": self.window_start, "window_count": self.window_count, "unit": UNIT,
            "total": self.total, "open_incident": open, "dispatches": self.dispatches,
            "cut": self.cut.map(|(offset, to_ms)| json!({"offset": offset, "to_ms": to_ms, "measure_at_ms": to_ms + SETTLE_MS})),
            "threshold": {"calls": THRESHOLD, "sample_s": SAMPLE_SECS, "max_dispatch_per_day": MAX_DISPATCH_PER_DAY}}))
    }
}

/// `cpu-watchdog search-pace <incident-id>`: task pace (searches and minutes to the first edit) of a window.
pub(super) fn pace_cli(home: &Path, args: Vec<String>) -> Result<()> {
    task_pace::cli(home, args)
}

/// `cpu-watchdog search-quality`: the window quality measure for an incident or a time range.
pub(super) fn quality_cli(home: &Path, args: Vec<String>) -> Result<()> {
    quality::cli(home, args)
}

/// `cpu-watchdog search-score <incident-id> | <from_ms> <to_ms>`: the policy score of a window with
/// and without the metered Antigravity searches.
pub(super) fn score_cli(home: &Path, args: Vec<String>) -> Result<()> {
    unhooked::cli(home, args)
}

/// The offset just after the last complete line of a log of `len` bytes.
fn line_end(file: &mut std::fs::File, len: u64) -> Result<u64> {
    let from = len.saturating_sub(1 << 16);
    file.seek(SeekFrom::Start(from))?;
    let mut tail = Vec::new();
    file.take(len - from).read_to_end(&mut tail)?;
    Ok(tail.iter().rposition(|b| *b == b'\n').map_or(len, |last| from + last as u64 + 1))
}

/// Lines of `bytes` that log a local-search call (`script-guard` writes compact JSON); labelled
/// synthetic calls (`synthetic.rs`) never fill a window, nor do replays the meter leaves out of
/// every rate (`"replay":true`, or a clipped command naming LOCAL_SEARCH_NO_LOG in events logged
/// before script-guard set the flag). Founder decision on search-retro 813254723049142099088: 30 of
/// its 89 unlabelled calls were retro replays, so the window metered 36.
fn local_calls(bytes: &[u8]) -> u64 {
    let counted = |l: &&str| l.contains("\"kind\":\"local_search\"") && !l.contains("\"synthetic\":") && !l.contains("\"replay\":true") && !l.contains("LOCAL_SEARCH_NO_LOG");
    String::from_utf8_lossy(bytes).lines().filter(counted).count() as u64
}

fn write_lines(path: &Path, events: &[Value]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut text = String::new();
    for event in events {
        text.push_str(&event.to_string());
        text.push('\n');
    }
    let mut file = std::fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(path)?;
    std::io::Write::write_all(&mut file, text.as_bytes())?;
    Ok(())
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
pub(super) fn prompt(home: &Path, id: &str, calls: usize, scored: &Value, quality: &Value) -> String {
    let goal = &quality["goal"];
    let rate = |k: &str| goal[k].as_f64().map_or("n/a".to_string(), |r| format!("{:.3}%", r * 100.0));
    format!("cpu-watchdog incident {id}: {} local-search calls (threshold {THRESHOLD}) among {calls} search events; answer rate {}, speed rate {}, adoption rate {} against target {:.3}%; local-search policy score {} ({}). {header} \
        Incident: {s}/incidents/{id}.json (quality: answer, speed and adoption with every failure's transcript locator, score components, misses by pattern, agent and session, task_pace: searches and minutes from each founder prompt to its first edit, previous search retros); events: {s}/search-retro/{id}.jsonl; state root {s}. \
        {recall} Classify every failure of the three rates, turn the largest causes into scripted or engineered fixes with measured before/after (cpu-watchdog search-quality), \
        {publish}",
        scored["counts"]["local_search"], rate("answer_rate"), rate("speed_rate"), rate("adoption_rate"), quality::TARGET * 100.0,
        scored["score"], scored["grade"].as_str().unwrap_or("?"), header = crate::dispatch::route_header("search-retro"), recall = crate::retro_recall::instruction(id), publish = super::retro_series::instruction("search", id, &root(home)), s = root(home).display())
}

#[cfg(test)]
#[path = "search_retro/tests.rs"]
mod tests;
#[cfg(test)]
#[path = "search_retro/retry_scope_tests.rs"]
mod retry_scope_tests;
#[cfg(test)]
#[path = "search_retro/filtered_none_tests.rs"]
mod filtered_none_tests;
