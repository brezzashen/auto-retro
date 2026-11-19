//! Search events of Antigravity sessions the hook did not log (search-retro 89311962361037056406,
//! founder decision 1A, 2026-11-05: meter). That retro read Antigravity as hookless; search-retro
//! 814586172752957658158 found `~/.gemini/config/hooks.json` runs PreToolUse hooks and, on founder
//! decision 1A, registered `script-guard antigravity` there, so a session the hook logged (agent
//! "antigravity" in `events.jsonl`) is skipped here and is never counted twice, nor its refused
//! calls as unguarded. Sessions from before the hook, or from a host without it, are still metered
//! from their Orca-bridged transcripts, which also count toward adoption.
//!
//! This reads the Bash calls of those bridged transcripts inside a window, classifies them with
//! `script-guard classify` (the hook's own classifier, so both agree on target, scope and pattern)
//! and returns them as events with agent "antigravity", source "transcript" and the transcript
//! locator. A sweep the hook would have refused ran unguarded: verdict "unguarded", which
//! `score.rs` counts as a sweep that ran.
use super::steps::{self, Kind};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Orca writes Antigravity transcripts as Claude projects under this slug of `~/antigravity-orca-bridge`.
pub(super) fn bridge_dir(home: &Path) -> PathBuf {
    let slug = home.join("antigravity-orca-bridge").to_string_lossy().replace('/', "-");
    home.join(".claude/projects").join(slug)
}

pub(super) struct Call {
    pub session: String,
    pub at_ms: f64,
    pub locator: String,
    pub command: String,
}

/// Antigravity sessions that already have hook events in `hooked`.
pub(super) fn hooked_sessions(hooked: &[Value]) -> std::collections::BTreeSet<String> {
    hooked.iter().filter(|e| e["agent"] == "antigravity").filter_map(|e| e["session"].as_str()).map(str::to_string).collect()
}

/// Bash calls issued from `from_ms` to `to_ms` in the bridged transcripts, in file order.
pub(super) fn calls(dir: &Path, from_ms: f64, to_ms: f64) -> Vec<Call> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect();
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok());
        if modified.is_some_and(|d| d.as_secs_f64() * 1000.0 < from_ms) { continue; }
        let session = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        for step in steps::read(&path, from_ms, to_ms, 0.0).into_iter().filter(|s| s.kind == Kind::Shell) {
            out.push(Call { session: session.clone(), at_ms: step.at_ms, locator: format!("{}:{}", path.display(), step.line), command: step.text });
        }
    }
    out
}

/// Runs `command` with `input` on stdin and returns its stdout. Stdin is fed from its own thread: a
/// filter that answers line by line fills both 64 KiB pipes, and a write before reading stdout then
/// blocks both processes (it froze the whole watchdog tick from 2026-11-11 16:03 to 2026-11-12).
pub(super) fn pipe(mut command: Command, input: String) -> anyhow::Result<Vec<u8>> {
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let mut stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("no stdin"))?;
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let output = child.wait_with_output()?;
    writer.join().map_err(|_| anyhow::anyhow!("stdin writer panicked"))??;
    Ok(output.stdout)
}

/// One event list per call, in order, from `script-guard classify`.
fn classify(guard: &Path, calls: &[Call]) -> anyhow::Result<Vec<Vec<Value>>> {
    let mut command = Command::new(guard);
    command.arg("classify");
    let input: String = calls.iter().map(|call| format!("{}\n", json!({"command": call.command}))).collect();
    let stdout = pipe(command, input)?;
    let lists: Vec<Vec<Value>> = String::from_utf8_lossy(&stdout).lines().map(|l| serde_json::from_str(l).unwrap_or_default()).collect();
    anyhow::ensure!(lists.len() == calls.len(), "script-guard classify returned {} of {} lines", lists.len(), calls.len());
    Ok(lists)
}

/// Tags the classifier's events with who ran them; a would-be block becomes "unguarded".
pub(super) fn tag(calls: &[Call], lists: Vec<Vec<Value>>) -> Vec<Value> {
    let mut out = Vec::new();
    for (call, events) in calls.iter().zip(lists) {
        for mut event in events {
            if event["verdict"] == "blocked" { event["verdict"] = json!("unguarded"); }
            event["agent"] = json!("antigravity");
            event["profile"] = json!("bridge");
            event["session"] = json!(call.session);
            event["ts_ms"] = json!(call.at_ms);
            event["source"] = json!("transcript");
            event["locator"] = json!(call.locator);
            out.push(event);
        }
    }
    out
}

/// The window's Antigravity events of sessions `hooked` does not cover, plus a summary for the
/// incident; a classifier failure leaves the events out and says why, so the score stays the hooked
/// agents' score.
pub(super) fn events(home: &Path, from_ms: f64, to_ms: f64, hooked: &[Value]) -> (Vec<Value>, Value) {
    let skip = hooked_sessions(hooked);
    let calls: Vec<Call> = calls(&bridge_dir(home), from_ms, to_ms).into_iter().filter(|c| !skip.contains(&c.session)).collect();
    if calls.is_empty() {
        return (Vec::new(), json!({"calls": 0, "events": 0}));
    }
    match classify(&home.join(".local/bin/script-guard"), &calls) {
        Ok(lists) => {
            let events = tag(&calls, lists);
            let count = |verdict: &str| events.iter().filter(|e| e["verdict"] == verdict).count();
            let sessions: std::collections::BTreeSet<&str> = calls.iter().map(|c| c.session.as_str()).collect();
            let summary = json!({"calls": calls.len(), "events": events.len(), "sessions": sessions,
                "local_search": events.iter().filter(|e| e["kind"] == "local_search").count(),
                "unguarded": count("unguarded"), "allowed": count("allowed"),
                "unguarded_locators": events.iter().filter(|e| e["verdict"] == "unguarded").filter_map(|e| e["locator"].as_str()).take(20).collect::<Vec<_>>()});
            (events, summary)
        }
        Err(error) => (Vec::new(), json!({"calls": calls.len(), "events": 0, "error": format!("{error:#}")})),
    }
}

/// `cpu-watchdog search-score <incident-id> | <from_ms> <to_ms>`: the window's policy score from the
/// hooked events alone (`hooked`) and with Antigravity metered in (`metered`), for before/after.
pub(super) fn cli(home: &Path, args: Vec<String>) -> anyhow::Result<()> {
    let root = crate::monitor::root(home);
    let (from_ms, to_ms, hooked) = match args.as_slice() {
        [id] => {
            let incident: Value = serde_json::from_str(&std::fs::read_to_string(root.join("incidents").join(format!("{id}.json")))?)?;
            let text = std::fs::read_to_string(root.join("search-retro").join(format!("{id}.jsonl")))?;
            let events: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|e| e["source"] != "transcript").collect();
            (incident["window"]["from_ms"].as_f64().unwrap_or(0.0), incident["window"]["to_ms"].as_f64().unwrap_or(0.0), events)
        }
        [from, to] => {
            let (from_ms, to_ms): (f64, f64) = (from.parse()?, to.parse()?);
            let text = std::fs::read_to_string(super::log_path(home))?;
            let events = text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .filter(|e| e["ts_ms"].as_f64().is_some_and(|t| t >= from_ms && t <= to_ms)).collect();
            (from_ms, to_ms, events)
        }
        _ => anyhow::bail!("usage: cpu-watchdog search-score <incident-id> | <from_ms> <to_ms>"),
    };
    let (antigravity, unhooked) = events(home, from_ms, to_ms, &hooked);
    // Without labelled synthetic events (`synthetic.rs`); `metered_with_synthetic` keeps them.
    let (metered, metered_all, left_out) = super::synthetic::scores(&[hooked.as_slice(), antigravity.as_slice()].concat());
    println!("{}", serde_json::to_string_pretty(&json!({"from_ms": from_ms, "to_ms": to_ms,
        "hooked": super::score::score(&super::synthetic::split(&hooked).0), "metered": metered,
        "metered_with_synthetic": metered_all, "synthetic": left_out, "unhooked": unhooked}))?);
    Ok(())
}

#[cfg(test)]
#[path = "unhooked_tests.rs"]
mod tests;
