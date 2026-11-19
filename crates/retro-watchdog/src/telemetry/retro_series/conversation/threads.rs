//! Problem threads of the conversation retro series (founder 2026-11-15: follow each problem and its
//! fix versions instead of per-window counts). A thread is one recurring repeat signature
//! (`orca terminal`) with every probe a retro registered against it as one fix version, or a probe no
//! signature names, on its own. Each version carries its follow-up verdicts window by window and its
//! lifecycle status (`probe-lifecycle.json`); each thread carries its occurrence rate per window.
//! Lifecycle ids the registry lacks (`C1`) are clarity probes, not fixes, and stay off the page.

use super::super::Window;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// One shipped fix: a probe with its change line, commit and follow-up history.
pub struct Version {
    pub probe: String,
    pub date: String,
    pub zh: Option<String>,
    pub change: String,
    pub sha: String,
    pub verdicts: Vec<String>,
    pub status: String,
    pub last: String,
    pub streak: u64,
    /// The newest followups.json measurement after the change: (date, calls per 100 sessions).
    pub rate: Option<(String, f64)>,
}

impl Version {
    /// The lifecycle status in a few Chinese characters, for diagram nodes.
    pub fn status_short(&self) -> &'static str {
        match self.status.as_str() {
            "graduated" => "已解决",
            "escalate" => "需升级",
            "redefine" => "口径待改",
            _ if self.last == "regressed" => "跟进中，最近退化",
            _ if self.last == "ineffective" => "跟进中，最近无效",
            _ => "跟进中",
        }
    }
}

/// Where a thread stands, worst first; the order is the page order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Escalate,
    Failing,
    NoFix,
    Active,
    Redefine,
    Resolved,
    Quiet,
    Necessary,
}

pub struct Thread {
    pub key: String,
    pub signature: Option<String>,
    pub zh: Option<String>,
    pub first: usize,
    pub seen: Vec<usize>,
    pub last_verdict: String,
    pub versions: Vec<Version>,
    /// (window index, calls per 100 sessions).
    pub rate: Vec<(usize, f64)>,
    pub state: State,
}

/// Inputs outside the windows: the probe registry and the lifecycle file.
pub struct Sources<'a> {
    pub registry: &'a Value,
    pub lifecycle: &'a Value,
}

fn date_of(shipped: &str) -> String {
    shipped.get(..10).unwrap_or(shipped).to_string()
}

fn version(id: &str, src: &Sources, windows: &[&Window], history: &BTreeMap<String, Vec<String>>) -> Version {
    let entry = src.registry["probes"].as_array().and_then(|l| l.iter().find(|p| p["id"] == id)).cloned().unwrap_or(Value::Null);
    let finding = entry["finding"].as_str().and_then(|f| f.split_once('#')).and_then(|(wid, idx)| {
        let w = windows.iter().find(|w| w.id == wid)?;
        Window::list(&w.report, "findings").get(idx.parse::<usize>().ok()?).cloned()
    });
    let finding = finding.unwrap_or(Value::Null);
    let zh = entry["zh"].as_str().or(finding["zh"]["solution"].as_str()).filter(|s| !s.is_empty()).map(str::to_string);
    let life = &src.lifecycle["probes"][id];
    Version {
        probe: id.to_string(),
        date: date_of(entry["shipped"].as_str().unwrap_or("")),
        zh,
        change: entry["change"].as_str().unwrap_or("").to_string(),
        sha: super::sha(if finding["commit"].is_string() { &finding["commit"] } else { &entry["change"] }),
        verdicts: history.get(id).cloned().unwrap_or_default(),
        status: life["status"].as_str().unwrap_or("active").to_string(),
        last: life["last"].as_str().unwrap_or("").to_string(),
        streak: life["streak"].as_u64().unwrap_or(0),
        rate: windows.iter().rev().find_map(|w| probe_rate(w, id).map(|r| (w.date(), r))),
    }
}

fn state_of(versions: &[Version], verdicts: &[String], seen: &[usize], latest: usize) -> State {
    if !verdicts.is_empty() && verdicts.iter().all(|v| v == "necessary") {
        return State::Necessary;
    }
    let any = |f: &dyn Fn(&Version) -> bool| versions.iter().any(f);
    if any(&|v| v.status == "escalate") {
        State::Escalate
    } else if any(&|v| v.status == "active" && (v.last == "regressed" || v.last == "ineffective")) {
        State::Failing
    } else if versions.is_empty() {
        if seen.iter().any(|&i| i + 3 > latest) { State::NoFix } else { State::Quiet }
    } else if any(&|v| v.status == "active") {
        State::Active
    } else if any(&|v| v.status == "redefine") {
        State::Redefine
    } else {
        State::Resolved
    }
}

/// Every thread, worst state first, then by how many windows it appeared in.
pub fn build(windows: &[&Window], src: &Sources) -> Vec<Thread> {
    let mut history: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for w in windows {
        for f in Window::list(&w.report, "followups") {
            if let (Some(p), Some(v)) = (f["probe"].as_str(), f["verdict"].as_str()) {
                history.entry(p.to_string()).or_default().push(v.to_string());
            }
        }
    }
    // signature -> (windows seen, verdicts, probes, latest zh)
    let mut sigs: BTreeMap<String, (Vec<usize>, Vec<String>, BTreeSet<String>, Option<String>)> = BTreeMap::new();
    for (i, w) in windows.iter().enumerate() {
        for r in Window::list(&w.report, "repeats") {
            let Some(sig) = r["signature"].as_str() else { continue };
            let e = sigs.entry(sig.to_string()).or_default();
            if e.0.last() != Some(&i) {
                e.0.push(i);
            }
            e.1.push(r["verdict"].as_str().unwrap_or("").to_string());
            e.2.extend(r["probe"].as_str().map(str::to_string));
            if let Some(z) = r["zh"].as_str().filter(|z| !z.is_empty()) {
                e.3 = Some(z.to_string());
            }
        }
    }
    let latest = windows.len().saturating_sub(1);
    let linked: BTreeSet<String> = sigs.values().flat_map(|e| e.2.iter().cloned()).collect();
    let mut threads: Vec<Thread> = sigs
        .into_iter()
        .map(|(sig, (seen, verdicts, probes, zh))| {
            let mut versions: Vec<Version> = probes.iter().map(|p| version(p, src, windows, &history)).collect();
            versions.sort_by(|a, b| a.date.cmp(&b.date));
            let rate = seen.iter().filter_map(|&i| signature_rate(windows[i], &sig).map(|r| (i, r))).collect();
            let state = state_of(&versions, &verdicts, &seen, latest);
            Thread { key: format!("S-{}", key(&sig)), first: seen[0], last_verdict: verdicts.last().cloned().unwrap_or_default(), signature: Some(sig), zh, seen, versions, rate, state }
        })
        .collect();
    for p in src.registry["probes"].as_array().into_iter().flatten() {
        let Some(id) = p["id"].as_str().filter(|id| !linked.contains(*id)) else { continue };
        let v = version(id, src, windows, &history);
        // A probe whose change names a signature first ("session-status prints …") is a version of
        // that signature's thread even when no repeats row registered it there.
        if let Some(t) = threads.iter_mut().find(|t| t.signature.as_deref().is_some_and(|s| names_first(&v.change, s))) {
            t.versions.push(v);
            t.versions.sort_by(|a, b| a.date.cmp(&b.date));
            continue;
        }
        let seen: Vec<usize> = windows.iter().enumerate().filter(|(_, w)| Window::list(&w.report, "followups").iter().any(|f| f["probe"] == id)).map(|(i, _)| i).collect();
        let first = p["finding"].as_str().and_then(|f| f.split('#').next()).and_then(|wid| windows.iter().position(|w| w.id == wid)).or(seen.first().copied()).unwrap_or(0);
        let rate = windows.iter().enumerate().filter_map(|(i, w)| probe_rate(w, id).map(|r| (i, r))).collect();
        let state = state_of(std::slice::from_ref(&v), &[], &seen, latest);
        threads.push(Thread { key: id.to_string(), signature: None, zh: None, first, seen, last_verdict: String::new(), versions: vec![v], rate, state });
    }
    // Threads that gained a version above are judged again; an all-necessary signature stays so.
    for t in threads.iter_mut().filter(|t| t.signature.is_some() && t.state != State::Necessary) {
        t.state = state_of(&t.versions, &[], &t.seen, latest);
    }
    threads.sort_by(|a, b| a.state.cmp(&b.state).then(b.seen.len().cmp(&a.seen.len())).then(a.key.cmp(&b.key)));
    threads
}

/// Whether `change` starts with the signature as a whole word (`session-status prints …`).
fn names_first(change: &str, sig: &str) -> bool {
    sig.len() >= 4 && change.strip_prefix(sig).is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', ':', ',', '(']))
}

/// A case-id-safe key: ASCII letters, digits and dashes.
fn key(sig: &str) -> String {
    let k: String = sig.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    k.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

/// Calls of `sig` per 100 sessions in one window, from its repeats.json counts.
fn signature_rate(w: &Window, sig: &str) -> Option<f64> {
    let sessions = w.counts["sessions"].as_f64().filter(|s| *s > 0.0)?;
    let calls = w.counts["top"].as_array()?.iter().find(|t| t["signature"] == sig)?["calls"].as_f64()?;
    Some(calls * 100.0 / sessions)
}

/// A probe's measured rate after its change in one window, from followups.json.
fn probe_rate(w: &Window, id: &str) -> Option<f64> {
    w.probes["probes"].as_array()?.iter().find(|p| p["probe"] == id)?["after"]["calls_per_100_sessions"].as_f64()
}
