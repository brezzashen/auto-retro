//! Quota economics schedule (user request, 2026-10-24): check Mars CRM's real usage every day and
//! once a week, and publish the recommended plan quotas as a report page.
//!
//! 1. Every SAMPLE_SECS the schedule is read in Singapore time (UTC+8, no daylight saving).
//! 2. The daily run is due from DAILY_AT_MIN each day; the weekly run from WEEKLY_AT_MIN each Monday,
//!    after that day's daily run. One run at a time, on its own thread.
//! 3. A run is `mars-ops-crm quota-economics --period <p> --state <state>/quota-economics` in Mars_CRM
//!    (`mars-ops` `quota_economics/`, `crm_ops.rs`): read-only canary billing rows, the pooled quota model, an
//!    encrypted report page. Its stdout is one JSON receipt with the page URL.
//! 4. A failed run is retried after RETRY_SECS, at most MAX_ATTEMPTS per period and day. Success and
//!    the final failure each raise one notification.
//!
//! State goes to `quota-economics.json`; each run's receipt and log to `quota-economics/runs/`.
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    os::unix::{fs::{DirBuilderExt, OpenOptionsExt}, process::CommandExt},
    path::Path,
    process::Stdio,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

pub(super) const SAMPLE_SECS: f64 = 300.0;
/// 09:00 and Monday 09:30 Singapore time, in minutes after local midnight.
pub(super) const DAILY_AT_MIN: f64 = 9.0 * 60.0;
pub(super) const WEEKLY_AT_MIN: f64 = 9.0 * 60.0 + 30.0;
pub(super) const RETRY_SECS: f64 = 3600.0;
pub(super) const MAX_ATTEMPTS: usize = 3;
const RUN_TIMEOUT: Duration = Duration::from_secs(1200);
const SINGAPORE_OFFSET_MS: f64 = 8.0 * 3_600_000.0;
const DAY_MS: f64 = 86_400_000.0;
const KEEP_RUNS: usize = 60;

/// Singapore calendar day number since 1970-01-01 and minutes after its midnight.
pub(super) fn singapore(now_ms: f64) -> (i64, f64) {
    let local = now_ms + SINGAPORE_OFFSET_MS;
    let day = (local / DAY_MS).floor();
    (day as i64, (local - day * DAY_MS) / 60_000.0)
}

/// 1970-01-01 was a Thursday, so Monday is the day number with `(day + 3) % 7 == 0`.
pub(super) fn is_monday(day: i64) -> bool {
    (day + 3).rem_euclid(7) == 0
}

#[derive(Default)]
pub(super) struct QuotaWatch {
    loaded: bool,
    last_sample: f64,
    /// One record per run, newest last: `{period, day, at_ms, seconds, exit, ok, receipt, error}`.
    pub(super) runs: Vec<Value>,
    worker: Option<(String, Receiver<Value>)>,
}

impl QuotaWatch {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if !self.loaded {
            self.loaded = true;
            if let Some(state) = super::read(&root(home).join("quota-economics.json")) {
                self.runs = state["runs"].as_array().cloned().unwrap_or_default();
            }
        }
        if let Some((_, worker)) = &self.worker {
            match worker.try_recv() {
                Ok(run) => { self.worker = None; self.absorb(run); }
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS {
            return Ok(());
        }
        self.last_sample = now;
        if self.worker.is_none() && !cfg!(test) {
            if let Some(period) = self.due(now * 1000.0) {
                let (home, period_name) = (home.to_path_buf(), period.to_string());
                let (sender, receiver) = mpsc::sync_channel(1);
                std::thread::Builder::new().name("watchdog-quota".into()).spawn(move || {
                    let _ = sender.send(run(&home, &period_name));
                })?;
                self.worker = Some((period.to_string(), receiver));
            }
        }
        self.write(home, now)
    }

    /// The period to run now, if any: daily before weekly, each once per day or week.
    pub(super) fn due(&self, now_ms: f64) -> Option<&'static str> {
        let (day, minutes) = singapore(now_ms);
        let daily_done = self.succeeded("daily", day);
        if minutes >= DAILY_AT_MIN && !daily_done && self.may_retry("daily", day, now_ms) {
            return Some("daily");
        }
        let weekly_due = is_monday(day) && minutes >= WEEKLY_AT_MIN && (daily_done || self.attempts("daily", day) >= MAX_ATTEMPTS);
        (weekly_due && !self.succeeded("weekly", day) && self.may_retry("weekly", day, now_ms)).then_some("weekly")
    }

    fn of_day<'a>(&'a self, period: &'a str, day: i64) -> impl Iterator<Item = &'a Value> + 'a {
        self.runs.iter().filter(move |r| r["period"] == period && r["day"].as_i64() == Some(day))
    }

    fn succeeded(&self, period: &str, day: i64) -> bool {
        self.of_day(period, day).any(|r| r["ok"] == true)
    }

    fn attempts(&self, period: &str, day: i64) -> usize {
        self.of_day(period, day).count()
    }

    fn may_retry(&self, period: &str, day: i64, now_ms: f64) -> bool {
        let last = self.of_day(period, day).filter_map(|r| r["at_ms"].as_f64()).fold(0.0, f64::max);
        self.attempts(period, day) < MAX_ATTEMPTS && now_ms - last >= RETRY_SECS * 1000.0
    }

    pub(super) fn absorb(&mut self, run: Value) {
        crate::log(&format!("quota economics: {run}"));
        let period = if run["period"] == "weekly" { "周报" } else { "日报" };
        let day = run["day"].as_i64().unwrap_or(0);
        let final_failure = run["ok"] != true && self.attempts(run["period"].as_str().unwrap_or(""), day) + 1 >= MAX_ATTEMPTS;
        if run["ok"] == true {
            let plans = run["receipt"]["plans"].as_array().map(|plans| plans.iter()
                .map(|p| format!("{} {}→{}", p["plan"].as_str().unwrap_or("?"), p["current"], p["recommended"])).collect::<Vec<_>>().join("，")).unwrap_or_default();
            super::conversation_retro::notify(&format!("看门狗：额度{period}"), &format!("{plans}。{}", run["receipt"]["url"].as_str().unwrap_or("")));
        } else if final_failure {
            super::conversation_retro::notify(&format!("看门狗：额度{period}失败"), run["error"].as_str().unwrap_or("见 quota-economics.json"));
        }
        self.runs.push(run);
        let excess = self.runs.len().saturating_sub(KEEP_RUNS);
        self.runs.drain(..excess);
    }

    fn write(&self, home: &Path, now: f64) -> Result<()> {
        write_json(&root(home).join("quota-economics.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "running": self.worker.as_ref().map(|(period, _)| period.clone()), "runs": self.runs,
            "schedule": {"timezone": "Asia/Singapore", "daily_at_min": DAILY_AT_MIN, "weekly_monday_at_min": WEEKLY_AT_MIN,
                "retry_s": RETRY_SECS, "max_attempts": MAX_ATTEMPTS, "timeout_s": RUN_TIMEOUT.as_secs(), "sample_s": SAMPLE_SECS}}))
    }
}

/// Runs `quota-economics` once and returns its record; never panics on a failed or stuck run.
fn run(home: &Path, period: &str) -> Value {
    let started_ms = crate::unix_now() * 1000.0;
    let started = Instant::now();
    let state = root(home).join("quota-economics");
    let runs = state.join("runs");
    let output = runs.join(format!("{}-{period}.json", started_ms as u64));
    let result = (|| -> Result<(Option<i32>, bool)> {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&runs)?;
        let file = |path: &Path| std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(path);
        let mut child = super::crm_ops::command(home, &["quota-economics", "--period", period, "--state"]).arg(&state)
            .stdin(Stdio::null()).stdout(file(&output)?)
            .stderr(file(&output.with_extension("log"))?).process_group(0).spawn()?;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok((status.code(), false));
            }
            if started.elapsed() >= RUN_TIMEOUT {
                unsafe { libc::kill(-(child.id() as i32), libc::SIGTERM) };
                let _ = child.wait();
                return Ok((None, true));
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    })();
    let receipt = receipt(&std::fs::read_to_string(&output).unwrap_or_default());
    let (exit, timed_out, spawn_error) = match result {
        Ok((exit, timed_out)) => (exit, timed_out, None),
        Err(error) => (None, false, Some(format!("{error:#}"))),
    };
    let ok = exit == Some(0) && receipt["ok"] == true;
    let error = spawn_error.or_else(|| timed_out.then(|| "timed out".to_string()))
        .or_else(|| (!ok).then(|| receipt["error"].as_str().unwrap_or("no receipt").to_string()));
    json!({"period": period, "day": singapore(started_ms).0, "at_ms": crate::unix_now() * 1000.0, "started_at_ms": started_ms,
        "seconds": started.elapsed().as_secs(), "exit": exit, "ok": ok, "error": error, "output": output, "receipt": receipt})
}

/// The last JSON line of the run's stdout (`mars` and `cpu-build-gate` print their banners to stderr).
pub(super) fn receipt(stdout: &str) -> Value {
    stdout.lines().rev().find_map(|line| serde_json::from_str::<Value>(line.trim()).ok().filter(Value::is_object)).unwrap_or(Value::Null)
}

#[cfg(test)]
#[path = "quota_watch_tests.rs"]
mod tests;
