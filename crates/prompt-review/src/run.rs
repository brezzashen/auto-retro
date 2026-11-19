//! One weekly run. State under `~/.local/state/prompt-review/` (override PROMPT_REVIEW_STATE):
//! `tree/` the capture worktree, `runs/<SGT date>/` one directory per run (body.json, analysis.json,
//! gate.json, semantic.json, report.md, receipt.json), `history.ndjson` one line per run.
//! The previous run's body is the base; the first run (or one whose base is gone) captures mastra as it
//! was BACKFILL_DAYS ago instead, so every run compares one week.
use crate::{metrics, page, semantic, tree};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BACKFILL_DAYS: u64 = 7;
const ANALYZER: &str = "/home/user/.local/bin/mars-skill-tools";
/// Run directories kept (half a year of weekly runs); history.ndjson keeps every run.
const KEEP_RUNS: usize = 26;

fn prune(runs: &Path) {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for old in dirs.iter().take(dirs.len().saturating_sub(KEEP_RUNS)) {
        let _ = std::fs::remove_dir_all(old);
    }
}

fn write(path: &Path, value: &Value) -> Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// `analyze-prefix --json` of `body`, against `base` when given: `(report, gate failures)`.
fn analyze(body: &Path, base: Option<&Path>, out: &Path) -> Result<(Value, Vec<String>)> {
    let mut cmd = Command::new(ANALYZER);
    cmd.args(["prompt-dedup", "analyze-prefix"]).arg(body).arg("--json").arg(out).current_dir(tree::REPO).stdin(Stdio::null());
    if let Some(base) = base {
        // Growth has its own weekly threshold; the gate here reports repetition only.
        cmd.arg("--base").arg(base).args(["--max-growth", "100000000"]);
    }
    let output = cmd.output()?;
    let json = read(out).with_context(|| format!("analyzer wrote no JSON: {}", String::from_utf8_lossy(&output.stderr).lines().last().unwrap_or_default()))?;
    let failures = json["failures"].as_array().into_iter().flatten()
        .map(|f| f.as_str().map(str::to_string).unwrap_or_else(|| f.to_string())).collect();
    Ok((json["report"].clone(), failures))
}

/// Singapore date and time of now: (`YYYY-MM-DD`, `HH:MM`).
pub fn singapore_now() -> (String, String) {
    let out = Command::new("date").env("TZ", "Asia/Singapore").arg("+%Y-%m-%d %H:%M").output().ok();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let (d, t) = text.split_once(' ').unwrap_or(("1970-01-01", "00:00"));
    (d.to_string(), t.to_string())
}

pub fn history(state: &Path) -> Vec<Value> {
    std::fs::read_to_string(state.join("history.ndjson")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

struct Base { sha: String, body: PathBuf, metrics: Value, semantic: Value, source: &'static str }

/// The previous run's capture, or a backfilled capture of mastra BACKFILL_DAYS ago.
fn base(state: &Path, run_dir: &Path, repo: &Path, tree_dir: &Path, week_sha: &str) -> Result<Base> {
    if let Some(last) = history(state).into_iter().rev().find(|r| r["ok"] == true && r["run_dir"].as_str() != Some(&run_dir.to_string_lossy())) {
        let dir = PathBuf::from(last["run_dir"].as_str().unwrap_or_default());
        if let (true, Some(sem)) = (dir.join("body.json").exists(), read(&dir.join("semantic.json"))) {
            return Ok(Base { sha: last["sha"].as_str().unwrap_or_default().into(), body: dir.join("body.json"), metrics: last["metrics"].clone(), semantic: sem, source: "previous_run" });
        }
    }
    tree::checkout(repo, tree_dir, week_sha)?;
    let body_path = run_dir.join("base-body.json");
    let body = tree::capture(tree_dir, &body_path)?;
    let (report, _) = analyze(&body_path, None, &run_dir.join("base-analysis.json"))?;
    let pass = semantic::run(semantic::collect(&body, tree_dir)?, tree_dir, None)?;
    let sem = pass.to_json();
    write(&run_dir.join("base-semantic.json"), &sem)?;
    Ok(Base { sha: week_sha.into(), body: body_path, metrics: metrics::of_report(&report), semantic: sem, source: "backfill" })
}

pub fn main(publish: bool) -> Result<()> {
    let receipt = match execute(publish) {
        Ok(receipt) => receipt,
        Err(error) => json!({"ok": false, "error": format!("{error:#}")}),
    };
    println!("{}", serde_json::to_string(&receipt)?);
    if receipt["ok"] != true {
        std::process::exit(1);
    }
    Ok(())
}

fn execute(publish: bool) -> Result<Value> {
    let (state, repo) = (tree::state_dir(), PathBuf::from(tree::REPO));
    let tree_dir = state.join("tree");
    let (date, time) = singapore_now();
    let run_dir = state.join("runs").join(&date);
    // A run already finished today (by hand or an earlier attempt) is the answer; PROMPT_REVIEW_RERUN=1 measures again.
    if let Some(done) = read(&run_dir.join("receipt.json")).filter(|r| r["ok"] == true && r["url"].is_string()) {
        if std::env::var_os("PROMPT_REVIEW_RERUN").is_none() {
            return Ok(done);
        }
    }
    std::fs::create_dir_all(&run_dir)?;
    let (head, week_sha) = tree::heads(&repo, BACKFILL_DAYS)?;
    let base = base(&state, &run_dir, &repo, &tree_dir, &week_sha)?;
    tree::checkout(&repo, &tree_dir, &head)?;
    let body_path = run_dir.join("body.json");
    let body = tree::capture(&tree_dir, &body_path)?;
    let (report, _) = analyze(&body_path, None, &run_dir.join("analysis.json"))?;
    let (_, gate_failures) = analyze(&body_path, Some(&base.body), &run_dir.join("gate.json"))?;
    let mut prev_pairs = semantic::previous_pairs(&base.semantic);
    prev_pairs.extend(reviewed_pairs(&state.join("reviewed")));
    let pass = semantic::run(semantic::collect(&body, &tree_dir)?, &tree_dir, Some(&prev_pairs))?;
    let sem = pass.to_json();
    write(&run_dir.join("semantic.json"), &sem)?;
    let cur = metrics::of_report(&report);
    let mut cur_full = cur.clone();
    cur_full["semantic_standing_waste"] = json!(pass.standing_waste());
    cur_full["semantic_on_demand_waste"] = json!(pass.on_demand_waste());
    cur_full["semantic_groups"] = json!(pass.groups.len());
    cur_full["semantic_new_groups"] = json!(pass.groups.iter().filter(|g| g.new_pairs > 0).count());
    let prev_standing = base.semantic["standing_waste"].as_i64().unwrap_or(0);
    let mut prev_full = base.metrics.clone();
    prev_full["semantic_standing_waste"] = json!(prev_standing);
    prev_full["semantic_groups"] = json!(base.semantic["groups"].as_array().map(Vec::len));
    let reasons = metrics::reasons(&cur, &base.metrics, &gate_failures, &sem, prev_standing);
    let ctx = page::Context { date: &date, time: &time, head: &head, base_sha: &base.sha, base_source: base.source,
        cur: &cur_full, prev: &prev_full, gate_failures: &gate_failures, semantic: &sem, reasons: &reasons, history: &history(&state), run_dir: &run_dir };
    let published = page::render(&ctx, publish)?;
    let receipt = json!({"ok": true, "date": date, "time_sgt": time, "sha": head, "base_sha": base.sha, "base_source": base.source,
        "run_dir": run_dir, "metrics": cur_full, "base_metrics": prev_full, "gate_failures": gate_failures,
        "escalate": !reasons.is_empty(), "reasons": reasons, "url": published.url, "slug": published.slug,
        "groups": sem["groups"].as_array().map(|g| g.iter().map(|g| json!({"id": g["id"], "places": g["places"], "standing_waste": g["standing_waste"],
            "new_standing_waste": g["new_standing_waste"], "new_pairs": g["new_pairs"]})).collect::<Vec<_>>())});
    write(&run_dir.join("receipt.json"), &receipt)?;
    let line = json!({"ok": true, "date": date, "sha": head, "run_dir": run_dir, "metrics": cur_full, "escalate": !reasons.is_empty(),
        "reasons": reasons.iter().map(|r| &r["code"]).collect::<Vec<_>>(), "url": published.url});
    // A rehearsal (--no-publish) leaves no history line, so it never becomes next week's base.
    // A same-day rerun (PROMPT_REVIEW_RERUN) replaces that day's line instead of adding a second point.
    if publish {
        let mut lines: Vec<String> = history(&state).into_iter().filter(|r| r["run_dir"] != line["run_dir"]).map(|r| r.to_string()).collect();
        lines.push(line.to_string());
        std::fs::write(state.join("history.ndjson"), lines.join("\n") + "\n")?;
    }
    prune(&state.join("runs"));
    Ok(receipt)
}

/// Pairs a dedup session measured on its delivered change (`reviewed/<incident>.json`, written by
/// `prompt-review semantic --out`). The previous run saw the text before that session's edit, so without
/// these a group it fixed or kept, whose wording it changed, came back as new the next week.
fn reviewed_pairs(dir: &Path) -> Vec<(crate::units::Unit, crate::units::Unit)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    for old in files.iter().take(files.len().saturating_sub(KEEP_RUNS)) {
        let _ = std::fs::remove_file(old);
    }
    files.iter().rev().take(KEEP_RUNS).filter_map(|f| read(f)).flat_map(|v| semantic::previous_pairs(&v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewed_pairs_read_every_session_file() {
        let dir = std::env::temp_dir().join(format!("prompt-review-reviewed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let unit = |place: &str, text: &str| json!({"surface": "system", "place": place, "path": "", "text": text, "standing": true, "src": []});
        write(&dir.join("1.json"), &json!({"pairs": [{"a": unit("system:a", "Confirm first."), "b": unit("tool:x", "Always confirm."), "score": 0.9}]})).unwrap();
        write(&dir.join("2.json"), &json!({"pairs": []})).unwrap();
        std::fs::write(dir.join("note.txt"), "not a pass").unwrap();
        let pairs = reviewed_pairs(&dir);
        assert_eq!(pairs.len(), 1);
        assert_eq!((pairs[0].0.place.as_str(), pairs[0].1.text.as_str()), ("system:a", "Always confirm."));
        assert!(reviewed_pairs(&dir.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
