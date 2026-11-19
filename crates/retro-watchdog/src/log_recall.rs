//! A model diagnosis must start with real log vector retrieval, independent of its route.
//! Deterministic recovery stays independent; this gate applies to analysis completion only.
use crate::monitor::{root, write_json};
use anyhow::{Context, Result, ensure};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, path::{Path, PathBuf}, process::Command};

/// Seconds `local-search sync` waits for another sync of the same corpus. A background watchdog sync
/// held its lock for 15-20 min on 2026-11-03, and the former fail-fast `--require-lock` made nine
/// sessions hand-write retry loops (conversation retro 811769505225643163632).
const SYNC_WAIT_SECS: &str = "2400";

fn read(path: &Path) -> Result<Value> { Ok(serde_json::from_slice(&std::fs::read(path)?)?) }
fn hash(text: &str) -> String { format!("{:x}", Sha256::digest(text.as_bytes())) }

/// Usage and the report contract `log-verify` checks. Five sessions in conversation retro
/// 81740295292268599208 ran `log-verify --help` and got a target error instead.
const VERIFY_USAGE: &str = "usage: cpu-watchdog log-verify <incident-id|report-directory>
Checks, in order: log-recall.json is complete and vector-routed; its raw query files are unchanged;
report.json and report.md both exist and were written after log recall finished (rewrite them, do
not rerun log-recall, after editing); report.json log_recall_evidence lists only locators the recall
returned; report.json duplicate_analysis is ONE non-empty string (not an object or array) covering
repetitions, differences, checked alternatives and uncertainty.";

fn directory(home: &Path, target: &str) -> Result<PathBuf> {
    if !target.is_empty() && target.bytes().all(|b| b.is_ascii_digit()) {
        ensure!(root(home).join(format!("incidents/{target}.json")).is_file(), "unknown incident {target}");
        Ok(root(home).join("analysis").join(target))
    } else {
        // A relative directory resolves against the working directory instead of failing.
        let path = std::path::absolute(target).unwrap_or_else(|_| PathBuf::from(target));
        ensure!(path.is_dir(), "target {target:?} must be an incident id or an existing report directory");
        Ok(path)
    }
}

fn invoke(home: &Path, args: &[&str]) -> Result<String> {
    // Tagged so the recall retro does not count this receipt as an agent recall (`telemetry/recall_retro/batch.rs`).
    let out = Command::new(home.join(".local/bin/local-search")).args(args).env("LOCAL_SEARCH_CALLER", "cpu-watchdog log-recall").output()?;
    ensure!(out.status.success(), "local-search {} failed: {}", args[0], String::from_utf8_lossy(&out.stderr));
    Ok(String::from_utf8(out.stdout)?)
}

/// Absolute `path:line` locators of `corpus` hits in one `--json` query output; the output must come
/// from a real vector search.
fn locators(raw: &str, corpus: &str) -> Result<HashSet<String>> {
    let rows: Vec<Value> = raw.lines().filter(|l| !l.trim().is_empty()).map(serde_json::from_str).collect::<std::result::Result<_, _>>()?;
    ensure!(rows.iter().any(|r| r["route"] == "vector" && r["timing"].is_object()), "log recall must use real vector search");
    Ok(rows.iter().filter(|r| r["corpus"] == corpus).filter_map(|r| r["locator"].as_str())
        .filter(|s| s.rsplit_once(':').is_some_and(|(p,n)| Path::new(p).is_absolute() && n.parse::<usize>().is_ok_and(|n| n > 0)))
        .map(str::to_owned).collect())
}

fn hits(raw: &str) -> Result<HashSet<String>> {
    let found = locators(raw, "watchdog")?;
    ensure!(!found.is_empty(), "no watchdog hits; repair coverage or refine the query before analysis");
    Ok(found)
}

pub fn run(home: &Path, args: Vec<String>) -> Result<()> {
    let target = args.first().context("usage: cpu-watchdog log-recall <incident-id|absolute-report-dir> <query> [query ...]")?;
    let dir = directory(home, target)?;
    capture(&dir, &questions(&args[1..])?, |args| invoke(home, args))?;
    println!("{}", dir.join("log-recall.json").display());
    Ok(())
}

/// Questions from the CLI words after the target. `--query`/`-q` markers are dropped, so
/// `log-recall <id> --query "a" --query "b"` sends "a" and "b"; any other bare flag is refused.
/// Recall retro 83819043280550941228: a crm-turn-retro session's three `--query` words ran as
/// literal questions, and their 60 unrelated locators passed the gate as recall evidence.
pub(crate) fn questions(words: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for word in words {
        if word == "--query" || word == "-q" { continue; }
        ensure!(!(word.starts_with('-') && !word.contains(char::is_whitespace)),
            "{word:?} is a flag, not a question: pass each question as one quoted argument");
        ensure!(!word.trim().is_empty(), "provide semantic queries about this incident and repeated failures");
        out.push(word.clone());
    }
    ensure!(!out.is_empty(), "provide semantic queries about this incident and repeated failures");
    Ok(out)
}

fn capture(dir: &Path, queries: &[String], mut invoke: impl FnMut(&[&str]) -> Result<String>) -> Result<()> {
    ensure!(!queries.is_empty() && queries.iter().all(|q| !q.trim().is_empty()), "provide semantic queries about this incident and repeated failures");
    write_json(&dir.join("log-recall.json"), &json!({"status":"running","started_at_ms":crate::unix_now()*1000.0}))?;
    eprintln!("log-recall: syncing the watchdog corpus; waits up to {SYNC_WAIT_SECS} s if another sync holds it (no retry loop needed)");
    invoke(&["sync", "--corpus", "watchdog", "--require-lock", "--wait-lock", SYNC_WAIT_SECS])?;
    let coverage: Value = serde_json::from_str(&invoke(&["watchdog-status"])?)?;
    ensure!(coverage["pending"] == 0 && coverage["chunks"].as_u64().is_some_and(|n| n > 0), "watchdog vectors incomplete: {coverage}");
    let manifest = read(Path::new(coverage["manifest"].as_str().context("missing source manifest")?))?;
    write_json(&dir.join("log-sources.json"), &manifest)?;
    let mut recorded = Vec::new();
    for (i, q) in queries.iter().enumerate() {
        let raw = invoke(&["query", q, "--corpus", "watchdog", "--mode", "vector", "--json", "--limit", "30", "--per-source", "2"])?;
        let locators = hits(&raw)?;
        let file = format!("log-recall-{}.json", i + 1);
        write_json(&dir.join(&file), &json!({"stdout":raw}))?;
        let mut locators: Vec<_> = locators.into_iter().collect(); locators.sort();
        recorded.push(json!({"query":q,"file":file,"sha256":hash(&raw),"locators":locators}));
    }
    let perf = perf_recall(dir, queries, &mut invoke)?;
    write_json(&dir.join("log-recall.json"), &json!({"version":1,"status":"complete","mode":"vector","corpus":"watchdog",
        "finished_at_ms":crate::unix_now()*1000.0,"coverage":coverage,"sources_sha256":hash(&serde_json::to_string(&manifest)?),"queries":recorded,"perf":perf}))
}

/// The same queries against the dedicated performance corpus (`local-search` `perf`: CPU and GPU
/// windows, host restarts, panics, crashes, Jetsam, hangs, service restarts, daily counts). Zero
/// hits is a valid answer for a non-performance incident; a failed sync or query is not. Files are
/// named `log-recall-perf-*` so the watchdog corpus never indexes them back (`watchdog.rs`).
fn perf_recall(dir: &Path, queries: &[String], invoke: &mut impl FnMut(&[&str]) -> Result<String>) -> Result<Value> {
    invoke(&["sync", "--corpus", "perf", "--require-lock", "--wait-lock", SYNC_WAIT_SECS])?;
    let mut recorded = Vec::new();
    for (i, q) in queries.iter().enumerate() {
        let raw = invoke(&["query", q, "--corpus", "perf", "--mode", "vector", "--json", "--limit", "20", "--per-source", "1"])?;
        let mut found: Vec<_> = locators(&raw, "perf")?.into_iter().collect(); found.sort();
        let file = format!("log-recall-perf-{}.json", i + 1);
        write_json(&dir.join(&file), &json!({"stdout":raw}))?;
        recorded.push(json!({"query":q,"file":file,"sha256":hash(&raw),"locators":found}));
    }
    Ok(json!({"corpus":"perf","mode":"vector","queries":recorded}))
}

pub fn verify(dir: &Path) -> Result<()> {
    let receipt = read(&dir.join("log-recall.json")).context("run cpu-watchdog log-recall before diagnosing logs")?;
    ensure!(receipt["version"] == 1 && receipt["status"] == "complete" && receipt["mode"] == "vector" && receipt["corpus"] == "watchdog", "incomplete log vector receipt");
    ensure!(receipt["coverage"]["pending"] == 0, "incomplete log vectors");
    let manifest = read(&dir.join("log-sources.json"))?;
    ensure!(receipt["sources_sha256"] == hash(&serde_json::to_string(&manifest)?), "source coverage changed after recall");
    let queries = receipt["queries"].as_array().filter(|q| !q.is_empty()).context("no log queries")?;
    let mut found = HashSet::new();
    for (i, q) in queries.iter().enumerate() {
        let file = format!("log-recall-{}.json", i + 1);
        ensure!(q["file"] == file, "invalid log artifact path");
        let raw = read(&dir.join(file))?;
        let raw = raw["stdout"].as_str().context("missing raw query output")?;
        ensure!(q["sha256"] == hash(raw), "log query output changed");
        found.extend(hits(raw)?);
    }
    // Receipts written before the perf corpus have no `perf` block.
    for (i, q) in receipt["perf"]["queries"].as_array().into_iter().flatten().enumerate() {
        let file = format!("log-recall-perf-{}.json", i + 1);
        ensure!(q["file"] == file, "invalid perf recall artifact path");
        let raw = read(&dir.join(file))?;
        let raw = raw["stdout"].as_str().context("missing raw perf query output")?;
        ensure!(q["sha256"] == hash(raw), "perf query output changed");
        found.extend(locators(raw, "perf")?);
    }
    let finished = receipt["finished_at_ms"].as_f64().context("missing recall time")?;
    ensure!(finished.is_finite() && finished <= crate::unix_now()*1000.0, "invalid recall time");
    for name in ["report.json", "report.md"] {
        let path = dir.join(name);
        let meta = std::fs::metadata(&path).with_context(|| format!("missing {}: write report.json and report.md (Chinese) there", path.display()))?;
        let written = meta.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_secs_f64()*1000.0;
        ensure!(meta.len()>0, "{} is empty", path.display());
        ensure!(written >= finished, "write reports after log recall: {name} was written {:.0} s before log recall finished; rewrite it now (rerunning log-recall moves the finish time again)", (finished - written) / 1000.0);
    }
    let report = read(&dir.join("report.json"))?;
    let locators = report["log_recall_evidence"].as_array().filter(|v| !v.is_empty()).context("report needs log_recall_evidence (locators returned by log-recall)")?;
    let foreign: Vec<_> = locators.iter().filter(|v| !v.as_str().is_some_and(|s| found.contains(s))).map(|v| v.to_string()).collect();
    ensure!(foreign.is_empty(), "report cites logs not returned by vector recall: {} (copy locators from log-recall-*.json / log-recall-perf-*.json; other evidence goes elsewhere)", foreign.join(", "));
    let dup = &report["duplicate_analysis"];
    ensure!(dup.as_str().is_some_and(|s| !s.trim().is_empty()), "report needs duplicate_analysis as ONE non-empty string explaining repeated patterns, differences and uncertainty (found {})",
        match dup { Value::Null => "nothing".into(), Value::String(_) => "an empty string".into(), Value::Object(_) => "an object: join its parts into one string".into(), Value::Array(_) => "an array: join its parts into one string".into(), other => other.to_string() });
    Ok(())
}

pub fn verify_cli(home: &Path, args: Vec<String>) -> Result<()> {
    if args.first().is_some_and(|a| a == "--help" || a == "-h") { println!("{VERIFY_USAGE}"); return Ok(()); }
    let dir = directory(home, args.first().context("log-verify requires incident id or report directory")?)?;
    verify(&dir)?;
    println!("log vector evidence verified: {}", dir.display());
    Ok(())
}

pub fn instruction() -> &'static str {
    "MANDATORY before any log diagnosis: run cpu-watchdog log-recall <incident-id-or-absolute-report-directory> with semantic queries for the symptom and repeated failures. It also queries the dedicated performance corpus (perf: CPU/GPU load, host restarts, panics, crashes, Jetsam, hangs, service restarts, daily counts) into log-recall-perf-N.json. Read log-sources.json coverage and original path:line hits from both, compare repeated patterns and differences before inferring causes. Report.json must include log_recall_evidence (returned locators) and duplicate_analysis. Similarity is not causality; retain gaps and uncertainty. Run cpu-watchdog log-verify on the report directory before completion. Missing or failed vector retrieval blocks analysis acknowledgement, never deterministic emergency recovery."
}

#[cfg(test)]
pub(crate) mod tests;
