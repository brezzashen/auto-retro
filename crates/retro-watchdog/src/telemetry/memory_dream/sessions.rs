//! Dream session census (founder, 2026-11-10, after Anthropic's "Dreaming" workshop): a dream audits
//! what the window's sessions actually did against what memory and skills told them, instead of
//! only tidying the store. This census picks the sessions worth reading, deterministically:
//!
//! 1. `lesson:<name>`: a lesson a dream wrote, injected again in this window (did it help, or is it
//!    wrong, ignored or obsolete?). The registry is every target of the dream golden set (GOLDEN: one
//!    case per lesson for as long as the lesson lives) plus the lessons of the previous reports.
//! 2. `note:<name>`: a memory note the prompt hook injected into two or more sessions that then hit
//!    failed tool calls (was the note enough, or wrong?).
//! 3. `skill:<name>`: a skill loaded (Skill tool, or a Read under `skills/<name>/`) in two or more
//!    sessions that then hit failed tool calls (what does the skill not say?).
//! 4. `session:<id8>`: the sessions with the most failed calls, whatever they loaded.
//! 5. `best:<id8>`: the rare run that went well. Runs of one recurring task share the shape of their
//!    first request (a family); among FAMILY_RUNS or more runs that delivered a report, the one that
//!    needed at most BEST_SHARE of the family's median tool calls (what did it do differently, and
//!    can every run do that?). Counting failures never finds it.
//! 6. `fast:<release id>`: the rare fast delivery (founder, 2026-11-10: a release that took an especially
//!    short time is a case to copy). Per target, among FAMILY_RUNS or more releases that succeeded in the
//!    window (`mars-deliver` receipts), the one that took at most BEST_SHARE of their median time.
//! 7. `praise:<id8>`: a session where the user answered a result with approval (what worked?).
//!
//! Failures counted against an anchor are the failed results within AFTER_CALLS tool calls after
//! it; the count selects candidates, the audit reads the lines. Note and skill groups also list
//! `clean_at`, sessions that met no failure after the same anchor. Groups the last ROTATE dreams
//! audited are listed but not required again.
//!
//! `cpu-watchdog dream-sessions <incident> | --hours <n>` prints the receipt; for an incident it
//! writes `analysis/<id>/dream-sessions.json`, whose `required` groups the ack gate checks (`audit.rs`).
use super::super::release_retro;
use super::transcript::{self, Session};
use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::{collections::{BTreeMap, BTreeSet}, path::Path, sync::LazyLock};

/// Builds waiting at the CPU gate when a release step started (`[cpu-build-gate] … N gated build(s)`).
static GATED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+) gated build\(s\)").unwrap());

const AFTER_CALLS: usize = 40;
/// Failed calls that make a session a friction candidate on its own.
const STUCK_FAILED: usize = 6;
const FAMILY_RUNS: usize = 4;
const BEST_SHARE: f64 = 0.6;
/// Required groups per kind: lessons, notes, skills, sessions, best runs, fast releases, praised sessions.
const CAPS: [usize; 7] = [3, 2, 2, 2, 2, 2, 1];
const ROWS: usize = 8;
const ROTATE: usize = 2;
const GOLDEN: &str = "automation/crates/agent-memory/eval/golden-memory-dream.jsonl";

fn at(path: &Path, line: usize) -> String {
    format!("{}:{line}", path.display())
}

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

/// One group: every session holding the anchor, with the failures that followed it.
fn group(kind: &str, name: &str, sessions: &BTreeMap<String, Session>, pick: &dyn Fn(&Session) -> Option<(usize, usize)>) -> Option<Value> {
    let mut rows: Vec<(usize, Value)> = sessions.iter().filter_map(|(id, s)| {
        let (anchor, line) = pick(s)?;
        let after: Vec<String> = s.failed.iter().filter(|(i, _)| *i > anchor && *i <= anchor + AFTER_CALLS).map(|(_, l)| at(&s.path, *l)).collect();
        Some((after.len(), json!({"session": id, "title": s.title, "at": at(&s.path, line), "failed_after": after.len(),
            "failed_at": after.iter().take(3).collect::<Vec<_>>()})))
    }).collect();
    if rows.is_empty() { return None; }
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    let clean: Vec<Value> = rows.iter().filter(|r| r.0 == 0).take(3).map(|r| r.1["at"].clone()).collect();
    let (failing, failed): (usize, usize) = (rows.iter().filter(|r| r.0 > 0).count(), rows.iter().map(|r| r.0).sum());
    Some(json!({"group": format!("{kind}:{name}"), "kind": kind, "name": name, "sessions": rows.len(), "failing_sessions": failing,
        "failed_after": failed, "clean_at": clean, "rows": rows.into_iter().take(ROWS).map(|r| r.1).collect::<Vec<_>>()}))
}

fn ranked(mut groups: Vec<Value>) -> Vec<Value> {
    groups.sort_by_key(|g| std::cmp::Reverse((g["failing_sessions"].as_u64(), g["failed_after"].as_u64())));
    groups
}

/// The rare good run of each recurring task: fewest tool calls among its delivered runs, when that
/// is at most BEST_SHARE of their median. Lists the best, a typical and the costliest run to compare.
fn best_runs(all: &BTreeMap<String, Session>) -> Vec<Value> {
    let mut families: BTreeMap<&str, Vec<(&String, &Session)>> = BTreeMap::new();
    for (id, s) in all.iter().filter(|(_, s)| s.delivered && !s.family.is_empty()) { families.entry(&s.family).or_default().push((id, s)); }
    let run = |(id, s): &(&String, &Session)| json!({"session": id, "title": s.title, "path": s.path, "calls": s.calls, "failed": s.failed.len()});
    let mut out: Vec<(f64, Value)> = families.into_iter().filter(|(_, runs)| runs.len() >= FAMILY_RUNS).filter_map(|(family, mut runs)| {
        runs.sort_by_key(|(_, s)| s.calls);
        let (best, median) = (runs[0], runs[runs.len() / 2]);
        let share = best.1.calls as f64 / median.1.calls.max(1) as f64;
        (share <= BEST_SHARE).then(|| (share, json!({"group": format!("best:{}", short(best.0)), "kind": "best", "family": family, "runs": runs.len(),
            "median_calls": median.1.calls, "share_of_median": (share * 100.0).round() / 100.0,
            "best": run(&best), "typical": run(&median), "costliest": run(&runs[runs.len() - 1])})))
    }).collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.into_iter().map(|r| r.1).collect()
}

/// What a release's log says about its speed: whether it rolled nothing (every component `built, unchanged`:
/// the commit was already live, so its seconds measure a health check; or an iOS/Mac TestFlight run that found
/// the commit already in an uploading or shipped build, `…; nothing to publish`) and the most builds that waited at the
/// CPU gate beside it (a slow typical run beside a heavy build is contention, not a step the fast run skipped).
fn release_log(run: &Value) -> (bool, u64) {
    let log = run["log"].as_str().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let noop = (log.contains("built, unchanged") && !log.contains("will roll")) || log.contains("; nothing to publish");
    (noop, GATED.captures_iter(&log).filter_map(|c| c[1].parse().ok()).max().unwrap_or(0))
}

/// The rare fast delivery of each target, with a typical and the slowest one to compare. `session` is the
/// tab that ran the release; `receipt` holds its inputs, purpose and steps. A release that rolled nothing is
/// left out (2026-11-11: a 35 s re-release of a live commit was audited as the fast canary).
fn fast_releases(home: &Path, from_ms: f64, to_ms: f64) -> Vec<Value> {
    let ms = |v: &Value| v.as_str().and_then(crate::cause_tree::attribute::parse_time);
    let mut targets: BTreeMap<String, Vec<(f64, Value)>> = BTreeMap::new();
    for (mut run, outcome, end) in release_retro::finished(home) {
        if outcome != "succeeded" || end < from_ms || end > to_ms { continue; }
        let (noop, gated) = release_log(&run);
        if noop { continue; }
        run["gated"] = json!(gated);
        // A release a session started returns after its foreground part (iOS: the ad hoc install) and leaves the
        // rest to a background publisher: it ended when that exited (`closed.at`), and is not measured before.
        let done = if run["backgroundPid"].is_null() { &run["finishedAt"] } else { &run["closed"]["at"] };
        let (Some(target), Some(from), Some(to)) = (run["target"].as_str(), ms(&run["startedAt"]), ms(done)) else { continue };
        targets.entry(target.to_string()).or_default().push(((to - from) / 1000.0, run));
    }
    let row = |(seconds, r): &(f64, Value)| json!({"release": r["id"], "seconds": seconds.round(), "gated_builds": r["gated"], "purpose": r["purpose"], "queue": r["queue"],
        "session": r["creator"]["session"], "receipt": home.join(".local/state/mars-deliver/releases").join(format!("{}.json", r["id"].as_str().unwrap_or_default()))});
    let mut out: Vec<(f64, Value)> = targets.into_iter().filter(|(_, runs)| runs.len() >= FAMILY_RUNS).filter_map(|(target, mut runs)| {
        runs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (fast, median) = (&runs[0], &runs[runs.len() / 2]);
        let share = fast.0 / median.0.max(1.0);
        (share <= BEST_SHARE).then(|| (share, json!({"group": format!("fast:{}", fast.1["id"].as_str().unwrap_or_default()), "kind": "fast", "target": target,
            "runs": runs.len(), "median_seconds": median.0.round(), "share_of_median": (share * 100.0).round() / 100.0,
            "busier_typical": median.1["gated"].as_u64() > fast.1["gated"].as_u64(),
            "fast": row(fast), "typical": row(median), "slowest": row(&runs[runs.len() - 1])})))
    }).collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.into_iter().map(|r| r.1).collect()
}

fn praised(all: &BTreeMap<String, Session>) -> Vec<Value> {
    let mut out: Vec<Value> = all.iter().filter(|(_, s)| !s.praise.is_empty()).map(|(id, s)| {
        json!({"group": format!("praise:{}", short(id)), "kind": "praise", "name": id, "title": s.title, "calls": s.calls,
            "praised": s.praise.len(), "praised_at": s.praise.iter().take(ROWS).map(|(l, _)| at(&s.path, *l)).collect::<Vec<_>>(),
            "said": s.praise.iter().take(3).map(|(_, t)| t.chars().take(60).collect::<String>()).collect::<Vec<_>>()})
    }).collect();
    out.sort_by_key(|g| std::cmp::Reverse(g["praised"].as_u64()));
    out
}

fn stuck(all: &BTreeMap<String, Session>) -> Vec<Value> {
    let mut out: Vec<Value> = all.iter().filter(|(_, s)| s.failed.len() >= STUCK_FAILED).map(|(id, s)| {
        json!({"group": format!("session:{}", short(id)), "kind": "session", "name": id, "title": s.title,
            "calls": s.calls, "failed": s.failed.len(), "failed_at": s.failed.iter().take(ROWS).map(|(_, l)| at(&s.path, *l)).collect::<Vec<_>>()})
    }).collect();
    out.sort_by_key(|g| std::cmp::Reverse(g["failed"].as_u64()));
    out
}

/// What earlier dreams left behind: the lesson notes they wrote and the groups the last ROTATE audited.
fn earlier(home: &Path, previous: &[Value]) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut lessons: BTreeSet<String> = std::fs::read_to_string(home.join(GOLDEN)).unwrap_or_default().lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok()).flat_map(|v| v["targets"].as_array().cloned().unwrap_or_default())
        .filter_map(|t| t.as_str().map(str::to_string)).collect();
    let mut audited = BTreeSet::new();
    for (i, p) in previous.iter().enumerate() {
        let Some(report) = p["report"].as_str().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
        lessons.extend(report["lessons"].as_array().into_iter().flatten().filter_map(|l| l["note"].as_str().map(str::to_string)));
        if i < ROTATE {
            audited.extend(report["audits"].as_array().into_iter().flatten().filter_map(|a| a["group"].as_str().map(str::to_string)));
        }
    }
    (lessons, audited)
}

/// The receipt: ranked groups per kind and the `required` ones a dream must audit.
pub(crate) fn census(home: &Path, from_ms: f64, to_ms: f64, previous: &[Value]) -> Value {
    let mut all = transcript::sessions(home, from_ms, to_ms);
    let harness = all.values().filter(|s| s.harness).count();
    all.retain(|_, s| !s.harness);
    let (lessons, audited) = earlier(home, previous);
    let names = |of: &dyn Fn(&Session) -> &Vec<(String, usize, usize)>| -> BTreeSet<String> { all.values().flat_map(|s| of(s).iter().map(|n| n.0.clone())).collect() };
    let anchor = |list: &[(String, usize, usize)], name: &str| list.iter().find(|n| n.0 == name).map(|n| (n.1, n.2));
    let note = |kind: &str, name: &String| group(kind, name, &all, &|s| anchor(&s.notes, name));
    let followups: Vec<Value> = ranked(names(&|s| &s.notes).iter().filter(|n| lessons.contains(*n)).filter_map(|n| note("lesson", n)).collect());
    let recurring = |g: &Value| g["sessions"].as_u64() >= Some(2) && g["failed_after"].as_u64() > Some(0);
    let notes: Vec<Value> = ranked(names(&|s| &s.notes).iter().filter(|n| !lessons.contains(*n)).filter_map(|n| note("note", n)).filter(recurring).collect());
    let skills: Vec<Value> = ranked(names(&|s| &s.skills).iter().filter_map(|n| group("skill", n, &all, &|s| anchor(&s.skills, n))).filter(recurring).collect());
    let (stuck, best, fast, praised) = (stuck(&all), best_runs(&all), fast_releases(home, from_ms, to_ms), praised(&all));
    let open = |list: &[Value], cap: usize| -> Vec<Value> { list.iter().map(|g| g["group"].clone()).filter(|g| !audited.contains(g.as_str().unwrap_or_default())).take(cap).collect() };
    let mut required: Vec<Value> = Vec::new();
    for (list, cap) in [(&followups, CAPS[0]), (&notes, CAPS[1]), (&skills, CAPS[2]), (&stuck, CAPS[3]), (&best, CAPS[4]), (&fast, CAPS[5]), (&praised, CAPS[6])] { required.extend(open(list, cap)); }
    let top = |list: Vec<Value>| list.into_iter().take(12).collect::<Vec<_>>();
    json!({"window": {"from_ms": from_ms, "to_ms": to_ms}, "sessions": all.len(), "harness_sessions": harness, "calls": all.values().map(|s| s.calls).sum::<usize>(),
        "method": format!("failed_after = failed tool results within {AFTER_CALLS} calls after the injection or load (heuristic; read the lines); note and skill groups need 2 sessions; a best run needs {FAMILY_RUNS} delivered runs of its task and at most {BEST_SHARE} of their median calls, a fast release {FAMILY_RUNS} succeeded releases of its target and at most {BEST_SHARE} of their median seconds; sessions started inside another session's scratchpad (harness runs) and releases that rolled nothing are left out; busier_typical = the typical release waited beside more gated builds than the fast one; groups audited by the last {ROTATE} dreams are not required again"),
        "lessons": top(followups), "notes": top(notes), "skills": top(skills), "stuck": top(stuck), "best": top(best), "fast": top(fast), "praised": top(praised),
        "rotated": audited.into_iter().collect::<Vec<_>>(), "required": required})
}

/// `cpu-watchdog dream-sessions <incident> | --hours <n>`.
pub(crate) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let now = crate::unix_now() * 1000.0;
    let value = match args.first().map(String::as_str) {
        Some("--hours") => {
            let hours: f64 = args.get(1).and_then(|h| h.parse().ok()).context("--hours needs a number")?;
            census(home, now - hours * 3_600_000.0, now, &[])
        }
        Some(id) if !id.starts_with('-') => {
            let event: Value = serde_json::from_slice(&std::fs::read(root(home).join("incidents").join(format!("{id}.json"))).context("incident does not exist")?)?;
            let from = event["window"]["from_ms"].as_f64().context("incident has no window (a dream opened before the audit contract)")?;
            let mut value = census(home, from, event["window"]["to_ms"].as_f64().unwrap_or(now), event["previous"].as_array().map_or(&[][..], Vec::as_slice));
            value["incident"] = json!(id);
            write_json(&root(home).join("analysis").join(id).join("dream-sessions.json"), &value)?;
            value
        }
        _ => anyhow::bail!("usage: cpu-watchdog dream-sessions <incident-id> | --hours <n>"),
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
