//! Instruction-clarity pass for conversation and runtime retros (founder, 2026-10-26: find where an
//! imprecise instruction, or ambiguity while giving it, caused a misunderstanding, and say so in the
//! retro so the founder can phrase it better next time).
//! Every human instruction of the window is read in session order. A later instruction that corrects
//! the agent, follows an interruption, arrives mid-turn or answers the agent's question marks the
//! instruction before it as a candidate; the retro judges each one from the original lines.
//!
//! `cpu-watchdog retro-clarity <incident> | --hours <n>` prints the candidates; for an incident it writes
//! `analysis/<id>/clarity.json`. Candidates scoring at least REQUIRED_SCORE are `required`: `monitor-ack`
//! on an incident created with `requirements.clarity` needs a report.json `instruction_clarity` verdict
//! for each, plus a Chinese summary for the founder.
use super::transcript_calls;
use crate::monitor::{root, write_json};
use anyhow::{ensure, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path, sync::LazyLock};

#[path = "retro_clarity/carried.rs"]
mod carried;

const REQUIRED_SCORE: u64 = 2;
/// Required reviews per retro; more would turn the pass into reading every turn.
const REQUIRED_MAX: usize = 30;
const KEPT: usize = 80;
const EXCERPT: usize = 400;
const VERDICTS: [&str; 3] = ["instruction", "agent", "not_misunderstanding"];
const CAUSES: [&str; 6] = ["ambiguous", "incomplete", "missing_context", "wrong_term", "conflicting", "changed_mind"];
static CORRECTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(
    r"(?i)不是(这个|这样|那个|让你|我要|说)|我(说|指|要)的是|我的意思|恰恰相反|搞错|理解错|误会|误解|不对|错了|没让你|我没(说|让)|跑偏|答非所问|\bI meant\b|not what I|misunderstood|that's not|\bI said\b|^no[,.!\s]").unwrap());
/// Machine-written prompts (watchdog dispatch, skill bodies, compaction) are not the founder's words.
const MACHINE: [&str; 6] = ["<", "cpu-watchdog incident", "Base directory for this skill", "[Image:", "This session is being continued", "# AGENTS.md"];
/// A `[sender] text` turn is another session's message (`session-send`); `[Image #1]` and `[Pasted text` are the founder's.
fn machine(body: &str) -> bool {
    let body = body.trim_start();
    let relayed = body.starts_with('[') && body.contains(']') && !body.starts_with("[Image #") && !body.starts_with("[Pasted text");
    relayed || MACHINE.iter().any(|m| body.starts_with(m))
}

#[derive(Default)]
struct Session { prior: Option<(String, String)>, assistant: String, interrupted: bool, asked: bool }

fn clip(text: &str) -> String { text.trim().chars().take(EXCERPT).collect() }

/// A correction marker in the prose of a turn; pasted paths and the question `对不对` do not count.
fn corrects(body: &str) -> bool {
    body.lines().map(str::trim_start).filter(|l| !l.starts_with('/') && !l.starts_with("'/"))
        .any(|l| CORRECTION.is_match(&l.replace("对不对", "")))
}

/// `(kind, text)` of one transcript row: `human`, `mid_turn`, `interrupt`, `asked` or `assistant`.
pub(crate) fn classify(row: &Value) -> Option<(&'static str, String)> {
    let text_of = |content: &Value| match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => Some(parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n")),
        _ => None,
    };
    let payload = &row["payload"];
    if payload["type"] == "message" {
        let text = text_of(&payload["content"])?;
        return match payload["role"].as_str()? {
            "user" => Some(("human", text)),
            "developer" if text.contains("<turn_aborted>") => Some(("interrupt", String::new())),
            "assistant" => Some(("assistant", text)),
            _ => None,
        };
    }
    if row["attachment"]["type"] == "queued_command" && row["attachment"]["origin"]["kind"] == "human" {
        return Some(("mid_turn", row["attachment"]["prompt"].as_str()?.to_string()));
    }
    let content = &row["message"]["content"];
    match row["type"].as_str()? {
        "assistant" if content.as_array()?.iter().any(|p| p["type"] == "tool_use" && p["name"] == "AskUserQuestion") => Some(("asked", String::new())),
        "assistant" => Some(("assistant", text_of(&Value::Array(content.as_array()?.iter().filter(|p| p["type"] == "text").cloned().collect()))?)),
        "user" if row["isMeta"] == true || content.as_array().is_some_and(|a| a.iter().any(|p| p["type"] == "tool_result")) => None,
        "user" if row["origin"]["kind"].as_str().is_some_and(|k| k != "human") => None,
        "user" => {
            let text = text_of(content)?;
            if text.starts_with("[Request interrupted") { Some(("interrupt", String::new())) } else { Some(("human", text)) }
        }
        _ => None,
    }
}

/// Candidates of every main transcript in `from_ms..=to_ms`, highest score first.
pub(crate) fn census(home: &Path, from_ms: f64, to_ms: f64) -> Value { census_with(home, from_ms, to_ms, &HashMap::new()) }

/// `census`, with candidates an acknowledged retro already judged moved from `required` to `judged_before`.
fn census_with(home: &Path, from_ms: f64, to_ms: f64, judged: &HashMap<String, carried::Judged>) -> Value {
    let (mut sessions, mut turns, mut candidates) = (0usize, 0usize, Vec::<Value>::new());
    for (source, path) in transcript_calls::files(home, from_ms) {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        let (mut state, mut seen) = (Session::default(), false);
        for (index, line) in text.lines().enumerate() {
            let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
            let Some(at_ms) = row["timestamp"].as_str().and_then(crate::cause_tree::attribute::parse_time) else { continue };
            if at_ms < from_ms || at_ms > to_ms { continue; }
            let Some((kind, body)) = classify(&row) else { continue };
            match kind {
                "assistant" => { if !body.trim().is_empty() { state.assistant = clip(&body); } continue; }
                "interrupt" => { state.interrupted = true; continue; }
                "asked" => { state.asked = true; continue; }
                _ => {}
            }
            if machine(&body) { continue; }
            let at = format!("{}:{}", path.display(), index + 1);
            let mut signals = Vec::new();
            if corrects(&body) { signals.push(("correction", 3)); }
            if state.interrupted { signals.push(("after_interrupt", 2)); }
            if kind == "mid_turn" { signals.push(("mid_turn", 1)); }
            if state.asked { signals.push(("answered_question", 1)); }
            if let (Some((prior_at, prior)), false) = (&state.prior, signals.is_empty()) {
                candidates.push(json!({"at": at, "session": id, "source": source, "signals": signals.iter().map(|s| s.0).collect::<Vec<_>>(),
                    "score": signals.iter().map(|s| s.1).sum::<u64>(), "text": clip(&body),
                    "prior_at": prior_at, "prior": prior, "agent_before": state.assistant}));
            }
            turns += 1;
            seen = true;
            state = Session { prior: Some((at, clip(&body))), ..Session::default() };
        }
        sessions += usize::from(seen);
    }
    // A resumed or forked session copies its history into a new transcript: keep one copy of each turn.
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|c| seen.insert((c["text"].to_string(), c["prior"].to_string())));
    candidates.sort_by(|a, b| b["score"].as_u64().cmp(&a["score"].as_u64()));
    let (required, judged_before) = carried::split(&candidates, judged, REQUIRED_SCORE, REQUIRED_MAX);
    let total = candidates.len();
    let last_required = required.last().and_then(|at| candidates.iter().position(|c| &c["at"] == at)).map_or(0, |i| i + 1);
    candidates.truncate(KEPT.max(last_required));
    json!({"window": {"from_ms": from_ms, "to_ms": to_ms}, "sessions": sessions, "human_turns": turns, "candidate_count": total,
        "method": "per session, a human turn that corrects (regex), follows an interruption, arrives mid-turn or answers the agent's question marks the instruction before it (prior); score correction 3, after_interrupt 2, mid_turn 1, answered_question 1",
        "required": required, "judged_before": judged_before, "candidates": candidates})
}

/// `cpu-watchdog retro-clarity <incident> | --hours <n>`.
pub(crate) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let now = crate::unix_now() * 1000.0;
    let value = match args.first().map(String::as_str) {
        Some("--hours") => {
            let hours: f64 = args.get(1).and_then(|h| h.parse().ok()).context("--hours needs a number")?;
            census(home, now - hours * 3_600_000.0, now)
        }
        Some(id) if !id.starts_with('-') => {
            let event: Value = serde_json::from_slice(&std::fs::read(root(home).join("incidents").join(format!("{id}.json"))).context("incident does not exist")?)?;
            let from = event["window"]["from_ms"].as_f64().context("incident has no window")?;
            let mut value = census_with(home, from, event["window"]["to_ms"].as_f64().unwrap_or(now), &carried::judged(home, id));
            value["incident"] = json!(id);
            write_json(&root(home).join("analysis").join(id).join("clarity.json"), &value)?;
            value
        }
        _ => anyhow::bail!("usage: cpu-watchdog retro-clarity <incident-id> | --hours <n>"),
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

/// The prompt sentence of a retro that reviews the founder's instructions.
pub(super) fn instruction(id: &str) -> String {
    format!("Instruction clarity (founder, 2026-10-26): run cpu-watchdog retro-clarity {id}; read the original lines of every analysis/{id}/clarity.json required[] candidate \
        (its prior instruction, the agent's reading and the correction; judged_before[] lists candidates an acknowledged retro already judged, which need no new verdict) and give report.json instruction_clarity {{summary (Chinese, for the founder), reviewed[] {{at, verdict instruction|agent|not_misunderstanding, analysis}}}}. \
        instruction (the wording caused it) also needs cause ambiguous|incomplete|missing_context|wrong_term|conflicting|changed_mind, instruction (quote), intended, interpretation, cost and suggestion (a better wording); \
        agent (the agent misread a clear instruction) needs fix. Show the instruction findings and suggestions in a 指令清晰度 section of report.md and the report page.")
}

/// monitor-ack gate for retros created with `requirements.clarity`.
pub(crate) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    if event["requirements"]["clarity"].is_null() { return Ok(()); }
    let analysis = root(home).join("analysis").join(id);
    let receipt: Value = serde_json::from_slice(&std::fs::read(analysis.join("clarity.json"))
        .context("run cpu-watchdog retro-clarity <id> first: analysis/<id>/clarity.json is missing")?)?;
    ensure!(receipt["incident"] == id, "clarity.json belongs to another incident");
    let report: Value = serde_json::from_slice(&std::fs::read(analysis.join("report.json")).context("report.json missing")?)?;
    let section = &report["instruction_clarity"];
    let text = |v: &Value| v.as_str().is_some_and(|s| !s.trim().is_empty());
    ensure!(text(&section["summary"]), "report.json instruction_clarity.summary (Chinese, for the founder) is empty");
    let markdown = std::fs::read_to_string(analysis.join("report.md")).unwrap_or_default();
    ensure!(markdown.contains("指令清晰度"), "report.md needs a 指令清晰度 section");
    let reviewed = section["reviewed"].as_array().cloned().unwrap_or_default();
    for at in receipt["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let entry = reviewed.iter().find(|r| r["at"] == at).with_context(|| format!("instruction_clarity.reviewed[] has no verdict for {at}"))?;
        let verdict = entry["verdict"].as_str().unwrap_or_default();
        ensure!(VERDICTS.contains(&verdict), "clarity {at}: verdict must be one of {VERDICTS:?}");
        ensure!(text(&entry["analysis"]), "clarity {at}: analysis is empty");
        match verdict {
            "instruction" => {
                ensure!(CAUSES.contains(&entry["cause"].as_str().unwrap_or_default()), "clarity {at}: cause must be one of {CAUSES:?}");
                for field in ["instruction", "intended", "interpretation", "cost", "suggestion"] {
                    ensure!(text(&entry[field]), "clarity {at}: instruction verdict needs {field}");
                }
            }
            "agent" => ensure!(text(&entry["fix"]), "clarity {at}: agent verdict needs fix"),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "retro_clarity/tests.rs"]
mod tests;
