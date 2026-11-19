//! Release command review (founder, 2026-10-28): the release commands agents typed inside one
//! retro window — Docker (beta, beta-web), Canary and Xcode/TestFlight alike — read from every
//! Claude and Codex transcript, so the retro finds instruction errors (a wrong or missing flag, a
//! release started from the wrong place, a quoting slip, a guard refusal, flag-by-flag retries)
//! and replaces them with deterministic commands.
//!
//! A call counts when one of its shell segments starts a release entry (`classify::release_segment`), so
//! a grep, a commit message or a heredoc body naming `deliver canary` never counts. Its result is
//! matched by call id; a Claude background run resolves at its `<task-notification>`, read from
//! its output file. A call seen twice (a session resumed under another account copies its
//! transcript) counts once. A failed call followed in the same session by the same entry with
//! other flags within RETRY_MS is a flag retry.
use super::classify::{self, board_run, codex_command, flags, lints, outcome, refusal, release_segment, RELEASE_FAILED};
use super::outputs::Outputs;
use super::super::runtime_retro::parse::timestamp_ms;
use super::super::wait_retro::parse::each;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::{collections::{BTreeMap, BTreeSet}, io::BufRead, path::Path, sync::LazyLock};

const RETRY_MS: f64 = 30.0 * 60_000.0;
const EXAMPLES: usize = 6;
const COMMAND_CHARS: usize = 300;
/// Cheap prefilter: a tool-call line without one of these is never parsed.
const KEYWORDS: [&str; 9] = ["deliver", "xcodebuild", "docker", "canary", "k8s", "release", "gcloud", "kubectl", "asc"];
const BACKGROUND: &str = "Command running in background";
static NOTIFY: LazyLock<Regex> = LazyLock::new(|| Regex::new(
    r"<tool-use-id>([^<]+)</tool-use-id>(?:\\n|\s)*<output-file>([^<]*)</output-file>(?:\\n|\s)*<status>(\w+)</status>(?:\\n|\s)*<summary>([^<]*)</summary>").unwrap());
static SUMMARY_EXIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"exit code (-?\d+)").unwrap());

fn excerpt(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(COMMAND_CHARS).collect()
}

/// An instruction error shows in the output's head; any other failure in its tail.
fn evidence(output: &str, class: &str) -> String {
    if let Some(refused) = refusal(output) { return excerpt(&refused); }
    if class != "other" && class != RELEASE_FAILED { return excerpt(output); }
    let words = output.split_whitespace().collect::<Vec<_>>().join(" ");
    words.chars().skip(words.chars().count().saturating_sub(COMMAND_CHARS)).collect()
}

struct Call { at: f64, entry: String, segment: String, command: String, id: String, background: bool }

#[derive(Default)]
pub(super) struct Tally { pub calls: Vec<Value>, pub retries: Vec<Value>, pub outputs: Outputs, seen: BTreeSet<String>, failed: BTreeMap<(String, String), (f64, Vec<String>, &'static str)> }

impl Tally {
    fn finish(&mut self, session: &str, alias: &str, call: Call, text: &str, error: bool, exit: Option<i64>) {
        if !self.seen.insert(call.id.clone()) { return; }
        let (is_failed, output) = outcome(text, error, exit);
        if call.background && is_failed { self.outputs.keep(&call.id, call.at, text); }
        let class = if is_failed { classify::classify(&output) } else { "ok" };
        let call_flags = flags(&call.segment);
        let key = (session.to_string(), call.entry.clone());
        if let Some((at, before, first)) = self.failed.get(&key).filter(|(t, _, _)| call.at - t < RETRY_MS && call.at >= *t) {
            if *before != call_flags {
                self.retries.push(json!({"session": session, "source": alias, "entry": call.entry, "gap_s": ((call.at - at) / 1000.0).round(), "after": first,
                    "added": call_flags.iter().filter(|f| !before.contains(f)).collect::<Vec<_>>(), "removed": before.iter().filter(|f| !call_flags.contains(f)).collect::<Vec<_>>()}));
            }
        }
        if is_failed { self.failed.insert(key, (call.at, call_flags, class)); } else { self.failed.remove(&key); }
        self.calls.push(json!({"session": session, "source": alias, "at_ms": call.at, "call": call.id, "entry": call.entry, "command": excerpt(&call.command),
            "lints": lints(&call.entry, &call.command), "background": call.background, "failed": is_failed, "class": class, "run": is_failed.then(|| board_run(&output)).flatten(),
            "output": is_failed.then(|| evidence(&output, class))}));
    }
}

pub(super) fn read(path: &Path, alias: &str, codex: bool, from: f64, to: f64, tally: &mut Tally) {
    let Ok(file) = std::fs::File::open(path) else { return };
    let session = path.file_stem().map_or(String::new(), |s| s.to_string_lossy().into_owned());
    let mut pending: Vec<Call> = Vec::new();
    for line in std::io::BufReader::new(file).split(b'\n').map_while(Result::ok) {
        let text = String::from_utf8_lossy(&line);
        let answers = pending.iter().any(|c| text.contains(&c.id));
        let calls = KEYWORDS.iter().any(|k| text.contains(k)) && ["tool_use", "function_call", "custom_tool_call"].iter().any(|k| text.contains(k));
        if !answers && !calls { continue; }
        if answers {
            for c in NOTIFY.captures_iter(&text) {
                let Some(i) = pending.iter().position(|p| p.background && p.id == c[1]) else { continue };
                let call = pending.remove(i);
                let output = tally.outputs.read(&c[1], &c[2]).unwrap_or_else(|| c[4].to_string());
                let exit = SUMMARY_EXIT.captures(&c[4]).and_then(|m| m[1].parse().ok());
                tally.finish(&session, alias, call, &output, &c[3] == "failed", exit);
            }
        }
        let Ok(value) = serde_json::from_str::<Value>(&text) else { continue };
        let Some(at) = value["timestamp"].as_str().and_then(timestamp_ms) else { continue };
        let mut results: Vec<(String, String, bool)> = Vec::new();
        let mut add = |command: String, id: Option<&str>| {
            let (Some(id), Some((entry, segment))) = (id.filter(|i| !i.is_empty()), release_segment(&command)) else { return };
            if (from..=to).contains(&at) { pending.push(Call { at, entry, segment, command, id: id.to_string(), background: false }); }
        };
        if codex {
            let p = &value["payload"];
            match p["type"].as_str() {
                Some("function_call" | "custom_tool_call") => add(codex_command(p), p["call_id"].as_str()),
                Some("function_call_output" | "custom_tool_call_output") =>
                    results.push((p["call_id"].as_str().unwrap_or("").into(), p["output"].as_str().map_or_else(|| p["output"].to_string(), str::to_string), false)),
                _ => {}
            }
        } else {
            for b in value["message"]["content"].as_array().into_iter().flatten() {
                match b["type"].as_str() {
                    Some("tool_use") if b["name"] == "Bash" => add(b["input"]["command"].as_str().unwrap_or("").into(), b["id"].as_str()),
                    Some("tool_result") => results.push((b["tool_use_id"].as_str().unwrap_or("").into(), b["content"].as_str().map(str::to_string).unwrap_or_else(||
                        b["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")), b["is_error"] == true)),
                    _ => {}
                }
            }
        }
        for (id, output, error) in results {
            let Some(i) = pending.iter().position(|c| c.id == id && !c.background) else { continue };
            if output.starts_with(BACKGROUND) { pending[i].background = true; continue; }
            let call = pending.remove(i);
            tally.finish(&session, alias, call, &output, error, None);
        }
    }
}

fn examples<'a>(rows: impl Iterator<Item = &'a Value>) -> Vec<Value> {
    rows.take(EXAMPLES).map(|r| json!({"session": r["session"], "source": r["source"], "at_ms": r["at_ms"], "call": r["call"], "command": r["command"], "output": r["output"]})).collect()
}

/// Release calls between `from` and `to` (Unix ms) across every agent transcript.
pub(crate) fn review(home: &Path, from: f64, to: f64) -> Value {
    let mut tally = Tally { outputs: Outputs::load(home), ..Tally::default() };
    each(home, &mut |alias, codex, path, _, modified, _| { if modified >= from { read(path, alias, codex, from, to, &mut tally); } });
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64);
    std::mem::take(&mut tally.outputs).save(now);
    summarize(&tally.calls, &tally.retries)
}

pub(crate) fn summarize(calls: &[Value], retries: &[Value]) -> Value {
    let instruction = |r: &&Value| r["failed"] == true && r["class"] != "other" && r["class"] != RELEASE_FAILED;
    let mut by_entry: BTreeMap<String, [usize; 3]> = BTreeMap::new();
    for r in calls {
        let e = by_entry.entry(r["entry"].as_str().unwrap_or("?").into()).or_default();
        e[0] += 1;
        if r["failed"] == true { e[1] += 1; }
        if instruction(&r) { e[2] += 1; }
    }
    let group = |rows: Vec<&Value>| json!({"n": rows.len(), "examples": examples(rows.into_iter())});
    let classes: Map<String, Value> = classify::CLASSES.iter().map(|c| (c.to_string(), calls.iter().filter(|r| r["failed"] == true && r["class"] == *c).collect::<Vec<_>>()))
        .filter(|(_, rows)| !rows.is_empty()).map(|(c, rows)| (c, group(rows))).collect();
    let lint_out: Map<String, Value> = classify::LINTS.iter().map(|l| (l.to_string(), calls.iter().filter(|r| r["lints"].as_array().is_some_and(|x| x.iter().any(|v| v == *l))).collect::<Vec<_>>()))
        .filter(|(_, rows)| !rows.is_empty()).map(|(l, rows)| (l, group(rows))).collect();
    let errors = calls.iter().filter(instruction).count();
    let started: Vec<&Value> = calls.iter().filter(|r| r["failed"] == true && r["class"] == RELEASE_FAILED).collect();
    json!({"calls": calls.len(), "sessions": calls.iter().filter_map(|r| r["session"].as_str()).collect::<BTreeSet<_>>().len(),
        "background": calls.iter().filter(|r| r["background"] == true).count(), "failed": calls.iter().filter(|r| r["failed"] == true).count(),
        "instruction_errors": errors, "instruction_error_rate": if calls.is_empty() { Value::Null } else { json!((errors as f64 * 1000.0 / calls.len() as f64).round() / 10.0) },
        "by_entry": by_entry.into_iter().map(|(k, [n, f, i])| (k, json!({"calls": n, "failed": f, "instruction_errors": i}))).collect::<Map<_, _>>(),
        "classes": classes, "unclassified": group(calls.iter().filter(|r| r["failed"] == true && r["class"] == "other").collect()),
        "release_failed": {"n": started.len(), "runs": started.iter().filter_map(|r| r["run"].as_str()).collect::<BTreeSet<_>>()},
        "lints": lint_out, "flag_retries": {"n": retries.len(), "after_instruction_error": retries.iter().filter(|r| !matches!(r["after"].as_str(), Some("other" | RELEASE_FAILED))).count(), "examples": retries.iter().filter(|r| !matches!(r["after"].as_str(), Some("other" | RELEASE_FAILED))).chain(retries.iter().filter(|r| matches!(r["after"].as_str(), Some("other" | RELEASE_FAILED)))).take(EXAMPLES * 2).collect::<Vec<_>>()}})
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
