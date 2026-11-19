//! Diagnosis ledger: one line per closed incident in `diagnosis-ledger.ndjson`, so cause-tree
//! changes can be compared before and after on repetition, time and tokens. A model row carries
//! the cost read from the session transcript (Claude accounts only; Codex rows keep
//! `session: null`). `--backfill` adds `period: baseline` rows for incidents acknowledged before
//! the ledger existed.
use super::attribute::parse_time;
use crate::monitor::root;
use anyhow::Result;
use serde_json::{json, Value};
use std::{collections::{BTreeMap, HashSet}, io::Write, os::unix::fs::OpenOptionsExt, path::{Path, PathBuf}};

const FILE: &str = "diagnosis-ledger.ndjson";
/// Gaps of 30 minutes or more are idle, as in runtime_retro.rs.
const IDLE_MS: f64 = 1_800_000.0;

pub(crate) fn append(home: &Path, row: &Value) -> Result<()> {
    std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(root(home).join(FILE))?.write_all(format!("{row}\n").as_bytes())?;
    Ok(())
}

pub(crate) fn rows(home: &Path) -> Vec<Value> {
    std::fs::read_to_string(root(home).join(FILE)).unwrap_or_default()
        .lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Cost of the Claude sessions whose opening prompt names incident `id`, takeovers included:
/// one session keeps its own row, several are summed with `sessions` and `transcripts`.
pub(crate) fn session_cost(home: &Path, id: &str, since: std::time::SystemTime) -> Value {
    let needle = format!("incident {id}");
    let Ok(profiles) = std::fs::read_dir(home.join(".claude-profiles")) else { return Value::Null };
    let mut costs = Vec::new();
    for profile in profiles.flatten() {
        let dir = profile.path().join("projects/-home-user-workspace");
        let Ok(files) = std::fs::read_dir(&dir) else { continue };
        for file in files.flatten().map(|f| f.path()).filter(|p| p.extension().is_some_and(|e| e == "jsonl")) {
            if std::fs::metadata(&file).and_then(|m| m.modified()).map_or(true, |m| m < since) { continue; }
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            if !text.lines().take(30).any(|l| l.contains(&needle)) { continue; }
            costs.push(cost(&file, &text));
        }
    }
    combine(costs)
}

fn combine(costs: Vec<Value>) -> Value {
    if costs.len() < 2 { return costs.into_iter().next().unwrap_or(Value::Null); }
    let sum = |field: &str| costs.iter().map(|c| c[field].as_f64().unwrap_or(0.0)).sum::<f64>();
    json!({"transcript": costs[0]["transcript"], "transcripts": costs.iter().map(|c| c["transcript"].clone()).collect::<Vec<_>>(),
        "sessions": costs.len(), "turns": sum("turns"), "output_tokens": sum("output_tokens"),
        "cache_read_tokens": sum("cache_read_tokens"), "input_tokens": sum("input_tokens"),
        "active_minutes": (sum("active_minutes") * 10.0).round() / 10.0})
}

fn cost(file: &PathBuf, text: &str) -> Value {
    let (mut seen, mut times) = (HashSet::new(), Vec::new());
    let (mut output, mut cache_read, mut input, mut turns) = (0u64, 0u64, 0u64, 0u64);
    for line in text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
        if let Some(at) = line["timestamp"].as_str().and_then(parse_time) { times.push(at); }
        let (usage, message) = (&line["message"]["usage"], line["message"]["id"].as_str());
        if line["type"] == "assistant" && usage.is_object() && message.is_some_and(|m| seen.insert(m.to_string())) {
            turns += 1;
            output += usage["output_tokens"].as_u64().unwrap_or(0);
            cache_read += usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
            input += usage["input_tokens"].as_u64().unwrap_or(0) + usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
        }
    }
    let active_ms: f64 = times.windows(2).map(|w| w[1] - w[0]).filter(|gap| *gap > 0.0 && *gap < IDLE_MS).sum();
    json!({"transcript": file, "turns": turns, "output_tokens": output, "cache_read_tokens": cache_read,
        "input_tokens": input, "active_minutes": (active_ms / 60_000.0 * 10.0).round() / 10.0})
}

/// Rows for acknowledged tree-trigger incidents the ledger does not hold yet.
fn backfill(home: &Path, triggers: &BTreeMap<String, String>) -> Result<usize> {
    let known: HashSet<String> = rows(home).iter().filter_map(|r| r["incident"].as_str().map(String::from)).collect();
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root(home).join("incidents"))?.flatten() {
        let path = entry.path();
        let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        if path.extension().is_none_or(|e| e != "json") || known.contains(&id) || !path.with_extension("ack").exists() { continue; }
        let Some(event) = crate::telemetry::read_state(&path) else { continue };
        let Some(node) = event["trigger"].as_str().and_then(|t| triggers.get(t)) else { continue };
        if !root(home).join("analysis").join(&id).join("report.json").is_file() { continue; }
        let at = entry.metadata()?.modified()?;
        found.push((at, id, event["trigger"].clone(), node.clone()));
    }
    found.sort_by_key(|f| f.0);
    let mut first_of = HashSet::new();
    for (at, id, trigger, node) in &found {
        // Baseline repeat: an earlier report of the same trigger already reached this verdict.
        let repeat = !first_of.insert(trigger.to_string());
        append(home, &json!({"incident": id, "trigger": trigger, "period": "baseline", "closed_by": "model",
            "node": node, "repeat": repeat, "session": session_cost(home, id, *at),
            "at_ms": at.duration_since(std::time::UNIX_EPOCH)?.as_millis() as f64}))?;
    }
    Ok(found.len())
}

/// `cpu-watchdog ledger [--backfill]`: per period and trigger, coverage, repeat rate and cost.
pub(crate) fn run(home: &Path, args: Vec<String>) -> Result<()> {
    if args.iter().any(|a| a == "--backfill") {
        let added = backfill(home, &super::trigger_nodes(&super::load(home)?))?;
        eprintln!("backfilled {added} incident(s)");
    }
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for row in rows(home) {
        let key = format!("{} {}", row["period"].as_str().unwrap_or("live"), row["trigger"].as_str().unwrap_or("?"));
        groups.entry(key).or_default().push(row);
    }
    let summary: BTreeMap<String, Value> = groups.into_iter().map(|(key, rows)| {
        let model: Vec<&Value> = rows.iter().filter(|r| r["closed_by"] == "model").collect();
        let sum = |field: &str| model.iter().map(|r| r["session"][field].as_f64().unwrap_or(0.0)).sum::<f64>();
        let per = |total: f64| if rows.is_empty() { 0.0 } else { (total / rows.len() as f64).round() };
        let repeats = model.iter().filter(|r| r["repeat"] == true).count();
        (key, json!({
            "incidents": rows.len(),
            "script_closed": rows.len() - model.len(),
            "script_coverage": (rows.len() - model.len()) as f64 / rows.len().max(1) as f64,
            "model_sessions": model.len(),
            "repeat_rate": if model.is_empty() { Value::Null } else { json!(repeats as f64 / model.len() as f64) },
            "output_tokens": sum("output_tokens"), "output_tokens_per_incident": per(sum("output_tokens")),
            "cache_read_tokens": sum("cache_read_tokens"), "cache_read_tokens_per_incident": per(sum("cache_read_tokens")),
            "active_minutes": sum("active_minutes"), "active_minutes_per_incident": per(sum("active_minutes") * 10.0) / 10.0,
            "sessions_without_transcript": model.iter().filter(|r| r["session"].is_null()).count(),
        }))
    }).collect();
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_dedupes_messages_and_skips_idle_gaps() {
        let text = [
            r#"{"type":"user","timestamp":"2026-10-24T08:00:00Z","message":{"content":"cpu-watchdog incident 7: x"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-24T08:01:00Z","message":{"id":"m1","usage":{"output_tokens":10,"cache_read_input_tokens":100,"input_tokens":1}}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-24T08:02:00Z","message":{"id":"m1","usage":{"output_tokens":10,"cache_read_input_tokens":100,"input_tokens":1}}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-24T10:00:00Z","message":{"id":"m2","usage":{"output_tokens":5}}}"#,
        ].join("\n");
        let value = cost(&PathBuf::from("/t.jsonl"), &text);
        assert_eq!(value["turns"], 2);
        assert_eq!(value["output_tokens"], 15);
        assert_eq!(value["cache_read_tokens"], 100);
        assert_eq!(value["active_minutes"], 2.0);
    }
}
