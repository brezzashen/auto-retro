//! Conversation retro watch (user request, 2026-10-22): after every THRESHOLD new agent
//! conversations, open one session that finds repeated, redundant and script-replaceable work in
//! them and turns it into scripts, with mandatory CPU/GPU workload and acceleration analysis.
//!
//! Each scope counts from its own baseline: every Claude account (cc1-cc7), Antigravity `ag` (bridged; founder A 2026-11-14),
//! all of them with co1 and co2, and Orca tabs. A conversation is a main transcript created
//! after the baseline (Claude `<uuid>.jsonl`, Codex `rollout-*.jsonl`; subagent logs, Antigravity
//! boot files, one-shot print-mode model calls (bridged Antigravity calls without a tool call too) and harness runs in another scratchpad excluded,
//! `conversation_retro_one_shot.rs`). An Orca tab counts when `orca terminal list` shows a
//! `tabId` it had not shown before. A scope that reaches THRESHOLD (oldest baseline first) opens one
//! `conversation_count_reached` incident and one session on route `conversation-retro` through
//! `dispatch::start_session` (the account `limit-resume route` ranks first); that scope's baseline
//! then moves to the census time. No new incident opens while the last is unacknowledged; at most
//! MAX_DISPATCH_PER_DAY a day. The census runs on its own thread every SAMPLE_SECS.
//!
//! State: `conversation-retro.json`.
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::mpsc::{self, Receiver, TryRecvError},
};

pub(super) const SAMPLE_SECS: f64 = 120.0;
pub(super) const THRESHOLD: usize = 100;
pub(super) const MAX_DISPATCH_PER_DAY: usize = 3;
/// A session that failed to start is retried after this long.
const RETRY_SECS: f64 = 600.0;
const DAY_MS: f64 = 86_400_000.0;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Layout { Claude, Codex }

/// `(alias, transcript root under $HOME, layout)`; aliases follow remaining-range's registry.
pub(super) const SOURCES: [(&str, &str, Layout); 9] = [
    ("cc1", ".claude/projects", Layout::Claude),
    ("cc2", ".claude-profiles/default/projects", Layout::Claude),
    ("cc3", ".claude-profiles/team/projects", Layout::Claude),
    ("cc4", ".claude-profiles/team2/projects", Layout::Claude),
    ("cc5", ".claude-profiles/team3/projects", Layout::Claude),
    ("cc6", ".claude-profiles/team4/projects", Layout::Claude),
    ("cc7", ".claude-profiles/team5/projects", Layout::Claude),
    ("co1", ".codex/sessions", Layout::Codex),
    ("co2", ".codex-profiles/default/sessions", Layout::Codex),
];
/// Checked in this order; the first due scope dispatches.
pub(super) const SCOPES: [&str; 10] = ["all", "cc1", "cc2", "cc3", "cc4", "cc5", "cc6", "cc7", one_shot::BRIDGE_ALIAS, "orca"];

/// One conversation transcript: `(source alias, path, created ms)`.
pub(super) type Found = (&'static str, String, f64);

pub(super) struct Census { pub at: f64, pub files: Vec<Found>, pub tabs: Result<Vec<Value>, String> }

#[derive(Default)]
pub(super) struct ConversationRetro {
    loaded: bool,
    last_sample: f64,
    pub(super) baselines: BTreeMap<String, f64>,
    /// `{tab, title, agent, worktree, first_ms, counted}`; `counted` is false for tabs open at the baseline.
    pub(super) tabs: Vec<Value>,
    /// False until the first answered tab listing, whose tabs become the baseline.
    pub(super) tabs_baselined: bool,
    counts: Value,
    tab_error: Option<String>,
    pub(super) dispatches: Vec<Value>,
    worker: Option<Receiver<Census>>,
}

impl ConversationRetro {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded {
            self.load(home, now);
        }
        let mut open = self.open_incident(home);
        let mut changed = false;
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(census) => { self.worker = None; open = self.absorb(home, census)?; changed = true; }
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.worker.is_none() && (self.last_sample == 0.0 || now < self.last_sample || now - self.last_sample >= SAMPLE_SECS) {
            changed = true;
            self.last_sample = now;
            let home = home.to_path_buf();
            let since = self.since();
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::Builder::new().name("watchdog-conversations".into()).spawn(move || {
                let _ = sender.send(Census { at: crate::unix_now() * 1000.0, files: transcripts(&home, &since), tabs: orca_tabs() });
            })?;
            self.worker = Some(receiver);
        }
        if changed { self.write(home, now, open)?; }
        Ok(())
    }

    fn load(&mut self, home: &Path, now: f64) {
        self.loaded = true;
        if let Some(state) = super::read(&root(home).join("conversation-retro.json")) {
            for (scope, at) in state["baselines"].as_object().into_iter().flatten() {
                if let Some(at) = at.as_f64() { self.baselines.insert(scope.clone(), at); }
            }
            self.tabs = state["orca_tabs"].as_array().cloned().unwrap_or_default();
            self.tabs_baselined = state["orca_baselined"] == true;
            self.dispatches = state["dispatches"].as_array().cloned().unwrap_or_default();
        }
        for scope in SCOPES {
            self.baselines.entry(scope.to_string()).or_insert(now * 1000.0);
        }
    }

    /// Per source, the oldest baseline of any scope that counts it.
    fn since(&self) -> Vec<(&'static str, f64)> {
        SOURCES.iter().map(|(alias, _, _)| {
            let at = SCOPES.iter().filter(|s| covers(s, alias) || (*alias == "cc1" && covers(s, one_shot::BRIDGE_ALIAS))).filter_map(|s| self.baselines.get(*s)).fold(f64::MAX, |a, b| a.min(*b));
            (*alias, at)
        }).collect()
    }

    pub(super) fn absorb(&mut self, home: &Path, census: Census) -> Result<Option<String>> {
        match census.tabs {
            Ok(open) => { self.tab_error = None; self.merge_tabs(&open, census.at); }
            Err(error) => self.tab_error = Some(error),
        }
        let mut counts = serde_json::Map::new();
        for scope in SCOPES {
            counts.insert(scope.into(), json!(self.count(scope, &census.files)));
        }
        self.counts = Value::Object(counts);
        self.escalate(home, &census.files, census.at)
    }

    /// New tabs count; tabs open at the baseline do not. Closed uncounted tabs are forgotten.
    fn merge_tabs(&mut self, open: &[Value], at: f64) {
        let baseline = !self.tabs_baselined;
        self.tabs_baselined = true;
        let ids: Vec<&str> = open.iter().filter_map(|t| t["tab"].as_str()).collect();
        self.tabs.retain(|t| t["counted"] == true || t["tab"].as_str().is_some_and(|id| ids.contains(&id)));
        for tab in open {
            match self.tabs.iter_mut().find(|t| t["tab"] == tab["tab"]) {
                Some(known) => known["title"] = tab["title"].clone(),
                None => {
                    let mut entry = tab.clone();
                    entry["first_ms"] = json!(at);
                    entry["counted"] = json!(!baseline);
                    self.tabs.push(entry);
                }
            }
        }
    }

    fn counted_tabs(&self) -> usize {
        self.tabs.iter().filter(|t| t["counted"] == true).count()
    }

    pub(super) fn count(&self, scope: &str, files: &[Found]) -> usize {
        if scope == "orca" {
            return self.counted_tabs();
        }
        let since = self.baselines.get(scope).copied().unwrap_or(f64::MAX);
        files.iter().filter(|(alias, _, created)| covers(scope, alias) && *created > since).count()
    }

    fn open_incident(&self, home: &Path) -> Option<String> {
        self.dispatches.last().filter(|d| d["started"] == true).and_then(|d| d["incident"].as_str()).map(str::to_string)
            .filter(|id| !root(home).join("incidents").join(format!("{id}.ack")).exists())
    }

    /// Opens one incident and session for the first due scope; returns the open incident id.
    fn escalate(&mut self, home: &Path, files: &[Found], at: f64) -> Result<Option<String>> {
        let open = self.open_incident(home);
        let last = self.dispatches.last();
        let failed_recently = last.is_some_and(|d| d["started"] != true && at - d["at_ms"].as_f64().unwrap_or(0.0) < RETRY_SECS * 1000.0);
        let today = self.dispatches.iter().filter(|d| d["started"] == true && d["at_ms"].as_f64().is_some_and(|t| at - t < DAY_MS)).count();
        // The due scope waiting longest goes first: in SCOPES order `orca` waited 15 days behind the others (retro 88095794513163098809).
        let due = SCOPES.iter().filter(|s| self.count(s, files) >= THRESHOLD).min_by(|a, b| self.baselines.get(**a).unwrap_or(&f64::MAX).total_cmp(self.baselines.get(**b).unwrap_or(&f64::MAX)));
        let (Some(scope), None, false, true) = (due, &open, failed_recently, today < MAX_DISPATCH_PER_DAY) else { return Ok(open) };
        let from = self.baselines[*scope];
        let conversations: Vec<Value> = files.iter().filter(|(alias, _, created)| *created > from && (*scope == "orca" || covers(scope, alias)))
            .map(|(alias, path, created)| json!({"source": alias, "path": path, "created_ms": created})).collect();
        let previous: Vec<Value> = self.dispatches.iter().rev().filter(|d| d["started"] == true).take(5)
            .map(|d| json!({"incident": d["incident"], "scope": d["scope"], "report": root(home).join("analysis").join(d["incident"].as_str().unwrap_or("")).join("report.json")})).collect();
        let count = self.count(scope, files);
        let id = super::diagnostics::incident(home, &format!("conversation_count_reached:{scope}:{from}"), json!({
            "trigger": "conversation_count_reached", "scope": scope, "count": count, "threshold": THRESHOLD,
            "window": {"from_ms": from, "to_ms": at}, "conversations": conversations,
            "orca_tabs": if *scope == "orca" { json!(self.tabs.iter().filter(|t| t["counted"] == true).collect::<Vec<_>>()) } else { Value::Null },
            "previous": previous,
            "requirements": {"resource_analysis": 1, "followups": 1, "repeats": 1, "clarity": 1},
        }))?;
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, &format!("看门狗：{scope} 满 {THRESHOLD} 对话复盘"), &prompt(home, &id, scope, count))
        };
        self.dispatches.push(match &result {
            Ok(agent) => json!({"incident": id, "scope": scope, "count": count, "at_ms": at, "started": true, "agent": agent}),
            Err(error) => json!({"incident": id, "scope": scope, "count": count, "at_ms": at, "started": false, "error": format!("{error:#}")}),
        });
        if result.is_err() {
            return Ok(None);
        }
        self.baselines.insert(scope.to_string(), at);
        if *scope == "orca" {
            // Still-open tabs become the new baseline; closed ones drop out at the next listing.
            for tab in &mut self.tabs { tab["counted"] = json!(false); }
        }
        notify("看门狗：对话复盘", &format!("{scope} 新增 {count} 个对话，已开会话分析重复、脚本复用及 CPU/GPU 优化"));
        Ok(Some(id))
    }

    fn write(&self, home: &Path, now: f64, open: Option<String>) -> Result<()> {
        write_json(&root(home).join("conversation-retro.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "census_running": self.worker.is_some(), "baselines": self.baselines, "counts": self.counts,
            "orca_error": self.tab_error, "orca_tabs": self.tabs, "orca_baselined": self.tabs_baselined, "open_incident": open, "dispatches": self.dispatches,
            "threshold": {"conversations": THRESHOLD, "sample_s": SAMPLE_SECS, "max_dispatch_per_day": MAX_DISPATCH_PER_DAY}}))
    }
}

pub(super) fn covers(scope: &str, alias: &str) -> bool {
    scope == "all" || scope == "orca" || scope == alias
}

pub(super) fn is_conversation(layout: Layout, name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".jsonl") else { return false };
    match layout {
        Layout::Claude => stem.len() == 36 && stem.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
        Layout::Codex => stem.starts_with("rollout-"),
    }
}

/// Transcripts created after each source's `since` (ms).
pub(super) fn transcripts(home: &Path, since: &[(&'static str, f64)]) -> Vec<Found> {
    let mut out = Vec::new();
    for (alias, dir, layout) in SOURCES {
        let since = since.iter().find(|(a, _)| *a == alias).map_or(f64::MAX, |(_, at)| *at);
        let depth = match layout { Layout::Claude => 1, Layout::Codex => 3 };
        walk(&home.join(dir), depth, &mut |path, created| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let excluded = || layout == Layout::Claude && (one_shot::is_one_shot_model_call(path) || one_shot::is_harness_run(path));
            if created > since && is_conversation(layout, name) && !excluded() {
                out.push((if one_shot::is_bridged(path) { one_shot::BRIDGE_ALIAS } else { alias }, path.to_string_lossy().into_owned(), created));
            }
        });
    }
    out
}

fn walk(dir: &Path, depth: usize, found: &mut dyn FnMut(&Path, f64)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            if depth > 0 && entry.file_name() != "subagents" { walk(&entry.path(), depth - 1, found); }
        } else if let Ok(created) = meta.created().or_else(|_| meta.modified()) {
            let ms = created.duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0);
            found(&entry.path(), ms);
        }
    }
}

/// Open Orca tabs, one entry per `tabId`; an error when Orca is not answering.
fn orca_tabs() -> Result<Vec<Value>, String> {
    let listed = crate::dispatch::orca(&["terminal", "list", "--json"]).map_err(|e| format!("{e:#}"))?;
    let mut tabs: Vec<Value> = Vec::new();
    for t in listed["result"]["terminals"].as_array().into_iter().flatten() {
        if t["tabId"].is_string() && !tabs.iter().any(|known| known["tab"] == t["tabId"]) {
            tabs.push(json!({"tab": t["tabId"], "title": t["title"], "agent": t["agentIdentity"], "worktree": t["worktreePath"]}));
        }
    }
    Ok(tabs)
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
pub(super) fn prompt(home: &Path, id: &str, scope: &str, count: usize) -> String {
    format!("cpu-watchdog incident {id}: scope {scope} reached {count} new agent conversations (threshold {THRESHOLD}). {header} \
        Incident: {s}/incidents/{id}.json (scope, window, conversation transcripts, Orca tabs, previous retro reports); state root {s}. \
        {recall} {followup} {repeats} {clarity} {zh} Find the repeated operations, redundant operations and multi-step work a script can replace in those conversations, \
        run cpu-watchdog retro-resources {id}, analyze CPU/GPU workload shares with explicit denominators and coverage, \
        identify GPU acceleration opportunities from original task evidence, implement beneficial candidates and verify correctness with at least three comparable before/after runs; \
        do not add CPU and GPU utilization or infer per-task GPU use from host activity. Include resource_analysis per the route reference; missing data stays unknown. \
        implement and verify the highest-value scripted improvements, \
        {publish}",
        header = crate::dispatch::route_header("conversation-retro"), recall = crate::retro_recall::instruction(id), followup = super::retro_followup::instruction(id), repeats = super::retro_repeats::instruction(id), clarity = super::retro_clarity::instruction(id), zh = super::retro_series::conversation::ZH_FIELDS, publish = super::retro_series::instruction("conversation", id, &root(home)), s = root(home).display())
}

pub(super) fn notify(title: &str, body: &str) {
    if cfg!(test) {
        return;
    }
    crate::dispatch::notify(title, body);
}

#[path = "conversation_retro_one_shot.rs"]
mod one_shot;

#[cfg(test)]
#[path = "conversation_retro_tests.rs"]
mod tests;
