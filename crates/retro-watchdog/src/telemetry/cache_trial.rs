//! Scheduled re-measure of a Mars CRM prompt-cache trial (founder 2026-11-15: "这个明天创建一个 watchdog，再复测一下"
//! for the Canary-only cache-aware unloading trial, report crm-cache-om-cold-experiments-20261115).
//!
//! 1. `cpu-watchdog cache-trial schedule --id <slug> (--in-hours N | --due-ms MS) --since-ms MS --note "<what runs>"`
//!    registers one trial; `cpu-watchdog cache-trial` lists them.
//! 2. Every SAMPLE_SECS a trial past its due time that was never opened gets one `cache_trial_due` incident and one
//!    session on route `cache-trial` through the shared analysis queue; it opens once, a failed start is retried
//!    after RETRY_SECS, at most MAX_ATTEMPTS.
//! 3. `monitor-ack` needs report.json with the published page, both arms' measurements and a verdict.
//!
//! State: `cache-trial.json`.
use crate::monitor::{root, write_json};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

pub(super) const SAMPLE_SECS: f64 = 300.0;
const RETRY_SECS: f64 = 1800.0;
const MAX_ATTEMPTS: usize = 3;
pub(super) const VERDICTS: [&str; 3] = ["enable_production", "keep_trial", "revert"];

#[derive(Default)]
pub(super) struct CacheTrial {
    loaded: bool,
    last_sample: f64,
    /// `{id, due_ms, since_ms, note, attempts, opened, incident, agent, error}`, oldest first.
    pub(super) trials: Vec<Value>,
}

impl CacheTrial {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded {
            self.loaded = true;
            self.trials = load(home);
        }
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS {
            return Ok(());
        }
        self.last_sample = now;
        self.trials = merge(std::mem::take(&mut self.trials), load(home));
        for index in self.due(now * 1000.0) {
            self.open(home, index, now * 1000.0)?;
        }
        write(home, &self.trials, now * 1000.0)
    }

    /// Indexes of trials to open now.
    pub(super) fn due(&self, now_ms: f64) -> Vec<usize> {
        self.trials.iter().enumerate().filter(|(_, t)| {
            let attempts = t["attempts"].as_u64().unwrap_or(0) as usize;
            t["opened"] != true && now_ms >= t["due_ms"].as_f64().unwrap_or(f64::MAX) && attempts < MAX_ATTEMPTS
                && t["last_attempt_ms"].as_f64().is_none_or(|last| now_ms - last >= RETRY_SECS * 1000.0)
        }).map(|(i, _)| i).collect()
    }

    fn open(&mut self, home: &Path, index: usize, now_ms: f64) -> Result<()> {
        let trial = self.trials[index].clone();
        let slug = trial["id"].as_str().unwrap_or("trial").to_string();
        let id = super::diagnostics::incident(home, &format!("cache_trial_due:{slug}"), json!({
            "trigger": "cache_trial_due", "trial": trial, "due_ms": trial["due_ms"], "since_ms": trial["since_ms"], "note": trial["note"] }))?;
        let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else {
            crate::dispatch::start_session(home, &id, "看门狗：缓存试验复测", &prompt(home, &id, &trial))
        };
        let entry = &mut self.trials[index];
        entry["attempts"] = json!(entry["attempts"].as_u64().unwrap_or(0) + 1);
        entry["last_attempt_ms"] = json!(now_ms);
        entry["incident"] = json!(id);
        match result {
            Ok(agent) => { entry["opened"] = json!(true); entry["agent"] = agent; entry["error"] = Value::Null; }
            Err(error) => entry["error"] = json!(format!("{error:#}")),
        }
        let opened = if entry["opened"] == true { "已开复测会话" } else { "会话未能打开，见 cache-trial.json" };
        super::conversation_retro::notify("看门狗：缓存试验到期", &format!("{slug}。{opened}。"));
        Ok(())
    }
}

fn load(home: &Path) -> Vec<Value> {
    super::read(&root(home).join("cache-trial.json")).and_then(|s| s["trials"].as_array().cloned()).unwrap_or_default()
}

/// The loop's own records win; a trial the CLI added since the last sample joins them.
pub(super) fn merge(mut mine: Vec<Value>, disk: Vec<Value>) -> Vec<Value> {
    for trial in disk {
        if !mine.iter().any(|m| m["id"] == trial["id"]) {
            mine.push(trial);
        }
    }
    mine
}

fn write(home: &Path, trials: &[Value], now_ms: f64) -> Result<()> {
    write_json(&root(home).join("cache-trial.json"), &json!({"at_ms": now_ms, "error": null, "trials": trials,
        "schedule": {"sample_s": SAMPLE_SECS, "retry_s": RETRY_SECS, "max_attempts": MAX_ATTEMPTS}}))
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

pub(super) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let mut trials = load(home);
    if args.first().map(String::as_str) == Some("schedule") {
        let id = flag(&args, "--id").context("--id <slug> is required")?;
        let now_ms = crate::unix_now() * 1000.0;
        let due_ms = match (flag(&args, "--in-hours"), flag(&args, "--due-ms")) {
            (Some(h), None) => now_ms + h.parse::<f64>().context("--in-hours takes a number")? * 3_600_000.0,
            (None, Some(ms)) => ms.parse::<f64>().context("--due-ms takes epoch milliseconds")?,
            _ => bail!("give exactly one of --in-hours N or --due-ms MS"),
        };
        let since_ms: f64 = flag(&args, "--since-ms").context("--since-ms MS (trial start) is required")?.parse()?;
        let note = flag(&args, "--note").context("--note \"<what the trial runs and how to read it>\" is required")?;
        ensure!(note.chars().count() >= 20, "--note must say what runs and where (20+ characters)");
        ensure!(!trials.iter().any(|t| t["id"] == id && t["opened"] != true), "trial {id} is already scheduled");
        trials.push(json!({"id": id, "due_ms": due_ms, "since_ms": since_ms, "note": note, "attempts": 0, "opened": false, "scheduled_ms": now_ms}));
        write(home, &trials, now_ms)?;
    } else if !args.is_empty() {
        bail!("usage: cpu-watchdog cache-trial [schedule --id <slug> (--in-hours N | --due-ms MS) --since-ms MS --note \"…\"]");
    }
    println!("{}", serde_json::to_string_pretty(&json!({"trials": trials}))?);
    Ok(())
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
fn prompt(home: &Path, id: &str, trial: &Value) -> String {
    format!("cpu-watchdog incident {id}: the scheduled prompt-cache trial {slug} is due ({note}). {header} \
        Incident: {s}/incidents/{id}.json (since_ms is the trial start). Measure both arms from the trial start to now, write {s}/analysis/{id}/report.md in Chinese and report.json \
        {{report_url, arms: {{trial: {{turns, hit_rate, credits_per_turn}}, baseline: {{turns, hit_rate, credits_per_turn}}}}, facts_kept, verdict: one of {verdicts:?}, reason}}, \
        update the founder's report named in the note with the result and a confirm item for the production decision, publish it encrypted, then run cpu-watchdog monitor-ack {id}.",
        slug = trial["id"].as_str().unwrap_or(""), note = trial["note"].as_str().unwrap_or(""), verdicts = VERDICTS,
        header = crate::dispatch::route_header("cache-trial"), s = root(home).display())
}

/// `monitor-ack` gate: the page, both arms with turns, hit rate and credits per turn, and a verdict with a reason.
pub(super) fn check(report: &Value) -> Result<()> {
    ensure!(report["report_url"].as_str().is_some_and(|u| u.starts_with("https://reports.example.com")), "report.json report_url is missing");
    for arm in ["trial", "baseline"] {
        let a = &report["arms"][arm];
        let turns = a["turns"].as_u64().with_context(|| format!("report.json arms.{arm}.turns must be a count"))?;
        // An empty arm has no rate to report: null, never an invented 0.0 (trial 87032278175550895089).
        if turns == 0 {
            ensure!(a["hit_rate"].is_null() && a["credits_per_turn"].is_null(),
                "report.json arms.{arm} has 0 turns: hit_rate and credits_per_turn must be null");
            continue;
        }
        ensure!(a["hit_rate"].as_f64().is_some_and(|v| (0.0..=1.0).contains(&v)), "report.json arms.{arm}.hit_rate must be 0..1");
        ensure!(a["credits_per_turn"].as_f64().is_some_and(|v| v >= 0.0), "report.json arms.{arm}.credits_per_turn must be a number");
    }
    let empty_trial = report["arms"]["trial"]["turns"].as_u64() == Some(0);
    if empty_trial {
        ensure!(report["facts_kept"].is_null(), "report.json facts_kept must be null when the trial arm has 0 turns (nothing was read)");
        ensure!(report["verdict"] == "keep_trial", "report.json verdict must be keep_trial when the trial arm has 0 turns");
    } else {
        ensure!(report["facts_kept"].is_boolean(), "report.json facts_kept must say whether answers kept their facts");
    }
    ensure!(report["verdict"].as_str().is_some_and(|v| VERDICTS.contains(&v)), "report.json verdict must be one of {VERDICTS:?}");
    ensure!(report["reason"].as_str().is_some_and(|r| r.trim().chars().count() >= 20), "report.json reason needs 20+ characters");
    Ok(())
}

pub(super) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    if event["trigger"] != "cache_trial_due" {
        return Ok(());
    }
    let path = root(home).join("analysis").join(id).join("report.json");
    check(&serde_json::from_slice(&std::fs::read(&path).with_context(|| format!("{} missing", path.display()))?)?)
}

#[cfg(test)]
#[path = "cache_trial/tests.rs"]
mod tests;
