//! Retro analysis must start with real history vector retrieval, with raw receipts retained.
//! The session's own questions run on `local-search`'s default route, scoped to the incident's
//! Singapore days (`dated.rs`); a receipt needs the embedding model, so a `vector` or `mixed` route
//! counts and any other route is rerun with `--mode vector` (founder decision 2026-11-10: the default
//! route found three more of 65 judged real history queries in the top five than `--mode vector`).
//! Seeds from the incident's exact counts run seeds from the incident's exact counts run
//! as exact `local-search tools` scans of the failing commands' tool calls inside the incident
//! window; every hit is read back from its file (`exact.rs`).
//! Acknowledgement revalidates those receipts and each finding's source link; a declared flag
//! alone never proves retrieval. This CLI runs outside the resident sampling loop.
use crate::monitor::{root, write_json};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, path::Path, process::Command};

fn read(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(path).with_context(|| path.display().to_string())?)?)
}

fn retro(incident: &Value) -> bool {
    matches!(incident["trigger"].as_str(), Some("conversation_count_reached" | "model_runtime_reached" | "search_calls_reached" | "release_count_reached" | "agent_wait_reached" | "recall_calls_reached"))
}

fn valid_id(id: &str) -> Result<()> {
    ensure!(!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()), "invalid incident id");
    Ok(())
}

fn invoke(home: &Path, args: &[&str]) -> Result<String> {
    // Tagged so the recall retro does not count this receipt as an agent recall (`telemetry/recall_retro/batch.rs`).
    let output = Command::new(home.join(".local/bin/local-search")).args(args).env("LOCAL_SEARCH_CALLER", "cpu-watchdog retro-recall").output()?;
    ensure!(output.status.success(), "local-search {} failed: {}", args[0], String::from_utf8_lossy(&output.stderr));
    Ok(String::from_utf8(output.stdout)?)
}

const AUTO_ROUTES: [&str; 2] = ["vector", "mixed"];

fn digest(text: &str) -> String { format!("{:x}", Sha256::digest(text.as_bytes())) }

fn hits(text: &str, routes: &[&str]) -> Result<HashSet<String>> {
    let rows: Vec<Value> = text.lines().filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str).collect::<std::result::Result<_, _>>()?;
    ensure!(rows.iter().any(|r| r["route"].as_str().is_some_and(|x| routes.contains(&x)) && r["timing"].is_object()),
        "history retrieval did not use the {} route; keyword fallback is not accepted", routes.join(" or "));
    let found: HashSet<String> = rows.iter().filter(|r| r["corpus"] == "history")
        .filter_map(|r| r["locator"].as_str())
        .filter(|s| s.rsplit_once(':').is_some_and(|(_, n)| n.parse::<usize>().is_ok_and(|n| n > 0)))
        .map(str::to_owned).collect();
    ensure!(!found.is_empty(), "no history vector hits; refine the query or repair index coverage before analysis");
    Ok(found)
}

/// Hits of one recorded retrieval: a seed's tool-call scan may match nothing (the command ran
/// only in an older transcript); a history query must return hits on its own route. `auto` is a
/// session question on the default route: the embedding model ran on `vector` and `mixed`.
fn recorded_hits(text: &str, mode: &str) -> Result<HashSet<String>> {
    match mode {
        "tools" => exact::tool_hits(text),
        "auto" => hits(text, &AUTO_ROUTES),
        _ => hits(text, &[mode]),
    }
}

pub fn run(home: &Path, args: Vec<String>) -> Result<()> {
    run_with(home, args, |args| invoke(home, args))
}

fn run_with(home: &Path, args: Vec<String>, mut invoke: impl FnMut(&[&str]) -> Result<String>) -> Result<()> {
    let id = args.first().context("usage: cpu-watchdog retro-recall <incident-id> <query> [query ...]")?;
    valid_id(id)?;
    let asked = crate::log_recall::questions(&args[1..]).context("at least one meaningful history query is required")?;
    let incident = read(&root(home).join(format!("incidents/{id}.json")))?;
    ensure!(retro(&incident), "incident is not a conversation/runtime/search/release retro");
    let dir = root(home).join(format!("analysis/{id}"));
    crate::analysis_private::seed_raw(&root(home), id)?;
    let started = crate::unix_now() * 1000.0;
    // Invalidate a previous receipt before attempting a new, possibly failing retrieval.
    write_json(&dir.join("history-recall.json"), &json!({"incident": id, "status": "running", "started_at_ms": started}))?;
    let sync = invoke(&["sync", "--corpus", "history"])?;
    let stats = invoke(&["stats"])?;
    let asked = asked.into_iter().map(|q| (None, q, "auto"));
    let seeded = exact::seeds(&incident).into_iter().map(|(source, q)| (Some(source), q, "tools"));
    let mut queries = Vec::new();
    for (i, (seed, query, mode)) in asked.chain(seeded).enumerate() {
        let (raw, mode, sent) = if mode == "tools" {
            // Scan the incident window: output is newest first, so an unbounded scan of a busy
            // command filled its 20 lines after the window (0 of 84 hits in 815473257387631574132).
            let bound = |key: &str| incident["window"][key].as_f64().map(|ms| format!("{}", ms as u64));
            let (from, to) = (bound("from_ms"), bound("to_ms"));
            let mut args: Vec<&str> = vec!["tools", "--limit", "20"];
            if let Some(f) = &from { args.extend(["--from-ms", f]); }
            if let Some(t) = &to { args.extend(["--to-ms", t]); }
            // After `--`, a seed word such as `--no-cone` is a word to match, not a tools flag.
            args.push("--");
            args.extend(exact::seed_args(&query));
            (invoke(&args)?, mode, None)
        } else {
            let sent = dated::scoped(&query, &incident["window"]);
            let ask = ["query", sent.as_str(), "--corpus", "history", "--json", "--limit", "20", "--per-source", "2"];
            let first = invoke(&ask)?;
            // A question full of identifiers may route keyword-first; the receipt needs the embedding model.
            if recorded_hits(&first, "auto").is_ok() {
                (first, "auto", Some(sent))
            } else {
                let mut forced = ask.to_vec();
                forced.extend(["--mode", "vector"]);
                (invoke(&forced)?, "vector", Some(sent))
            }
        };
        let found = recorded_hits(&raw, mode)?;
        let file = format!("history-recall-{}.json", i + 1);
        write_json(&dir.join(&file), &json!({"stdout": raw}))?;
        let mut locators: Vec<_> = found.into_iter().collect();
        locators.sort();
        let read_back = exact::read_back(home, &locators, &incident["window"]);
        queries.push(json!({"query": query, "sent": sent, "mode": mode, "seed": seed, "file": file, "sha256": digest(&raw), "locators": locators, "read_back": read_back}));
    }
    let receipt = json!({"version": 2, "incident": id, "status": "complete", "corpus": "history", "mode": "vector",
        "window": incident["window"],
        "started_at_ms": started, "finished_at_ms": crate::unix_now() * 1000.0,
        "sync_stdout": sync, "stats_stdout": stats, "queries": queries});
    write_json(&dir.join("history-recall.json"), &receipt)?;
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}

fn evidence(value: &Value, locators: &HashSet<String>) -> Result<()> {
    let items = value.as_array().context("recall_evidence must be an array of retrieved path:line locators")?;
    ensure!(!items.is_empty(), "recall_evidence is empty");
    for item in items {
        ensure!(item.as_str().is_some_and(|s| locators.contains(s)), "recall_evidence was not returned by the recorded vector retrieval");
    }
    Ok(())
}

pub fn validate_ack(home: &Path, id: &str, incident_path: &Path) -> Result<()> {
    if !retro(&read(incident_path)?) { return Ok(()); }
    let dir = root(home).join(format!("analysis/{id}"));
    let receipt = read(&dir.join("history-recall.json")).context("run cpu-watchdog retro-recall before analyzing this retro")?;
    ensure!((receipt["version"] == 1 || receipt["version"] == 2) && receipt["incident"] == id && receipt["status"] == "complete"
        && receipt["mode"] == "vector" && receipt["corpus"] == "history", "history vector receipt is incomplete or belongs to another incident");
    let queries = receipt["queries"].as_array().filter(|q| !q.is_empty()).context("no recorded vector queries")?;
    ensure!(queries.iter().any(|q| q["seed"].is_null() && q["mode"] != "hybrid"), "no session question was recalled by vector");
    let seeds = exact::seeds(&read(incident_path)?).len();
    // A receipt written before 2026-11-02 ran its seeds as hybrid history queries; both count.
    ensure!(queries.iter().filter(|q| !q["seed"].is_null() && (q["mode"] == "tools" || q["mode"] == "hybrid")).count() >= seeds,
        "rerun cpu-watchdog retro-recall: the incident's exact counts seed {seeds} tool-call scans this receipt lacks");
    let mut locators = HashSet::new();
    for (i, query) in queries.iter().enumerate() {
        ensure!(query["query"].as_str().is_some_and(|s| !s.trim().is_empty()), "empty recorded query");
        let file = format!("history-recall-{}.json", i + 1);
        ensure!(query["file"] == file, "invalid retrieval artifact path");
        let raw = read(&dir.join(file))?;
        let raw = raw["stdout"].as_str().context("missing retrieval output")?;
        ensure!(query["sha256"] == digest(raw), "retrieval output changed after capture");
        let mode = match (query["mode"].as_str(), query["seed"].is_null()) { (Some(m @ ("tools" | "hybrid")), false) => m, (Some("auto"), true) => "auto", _ => "vector" };
        locators.extend(recorded_hits(raw, mode)?);
    }
    let report_path = dir.join("report.json");
    let report = read(&report_path)?;
    ensure!(report["incident"] == id, "report belongs to another incident");
    crate::resource_usage::validate(home, id, &read(incident_path)?, &report)?;
    let finished = receipt["finished_at_ms"].as_f64().context("missing retrieval completion time")?;
    ensure!(finished.is_finite() && finished <= crate::unix_now() * 1000.0, "invalid retrieval time");
    for file in [&report_path, &dir.join("report.md")] {
        let metadata = std::fs::metadata(file)?;
        ensure!(metadata.len() > 0 && metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_secs_f64() * 1000.0 >= finished,
            "write report.md and report.json after completing history vector retrieval");
    }
    let findings = report["findings"].as_array().context("report findings must be an array")?;
    if findings.is_empty() {
        ensure!(report["no_findings_reason"].as_str().is_some_and(|s| !s.trim().is_empty()), "explain a no-findings conclusion");
        evidence(&report["recall_evidence"], &locators)?;
    }
    for finding in findings {
        evidence(&finding["recall_evidence"], &locators)?;
        ensure!(finding["analysis"].as_str().is_some_and(|s| !s.trim().is_empty()), "each finding needs grounded analysis, not only a command count");
        ensure!(finding["evidence"].as_array().is_some_and(|v| !v.is_empty() && v.iter().all(|s| s.as_str().is_some_and(|s| exact::exists(home, s)))),
            "each finding needs original path:line evidence naming a real line of a real file");
    }
    Ok(())
}

/// A cited `path:line` names a real line of a real file (absolute or `~/`).
pub(crate) fn cited_line_exists(home: &Path, locator: &str) -> bool {
    exact::exists(home, locator)
}

pub(crate) fn instruction(id: &str) -> String {
    format!("MANDATORY before analysis or edits: run cpu-watchdog retro-recall {id} with semantic queries about this incident; it syncs history, runs each question through the embedding model scoped to the incident's Singapore days (prefix a question with all-days: to rank every day), adds exact local-search tools scans for the failing commands in the incident's exact counts (unclassified errors first) and reads every hit back with its in_window flag. Read the returned transcript path:line hits, count only in_window hits as occurrences (older ones are background), distinguish necessary repeats from avoidable work and verified problems, then analyze and measure. Every report.json finding must include recall_evidence (returned locators), evidence (original path:line) and analysis. Keyword search or command counts alone do not satisfy this. monitor-ack rejects missing/invalid recall evidence; retrieval failure must remain unacknowledged.")
}

pub(crate) mod dated;
mod exact;

#[cfg(test)]
mod tests;
