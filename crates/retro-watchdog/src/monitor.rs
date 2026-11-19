use crate::procs::Proc;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

const WINDOW_MS: f64 = 300_000.0;
const FRESH_MS: f64 = 15_000.0;

#[derive(Clone, Deserialize, Serialize)]
struct Sample {
    start: f64,
    end: f64,
    percent: f64,
}

#[derive(Deserialize)]
struct History {
    pid: i32,
    updated: f64,
    samples: Vec<Sample>,
}

#[derive(Serialize)]
struct Consumer {
    pid: i32,
    ppid: i32,
    start: i64,
    name: String,
    cpu_percent_one_core: f64,
    program: serde_json::Value,
}

#[derive(Serialize)]
struct NameShare {
    name: String,
    processes: usize,
    cpu_percent_one_core: f64,
}

/// `consumers` keeps the 12 hottest processes; `by_name` sums every measured process by name, so
/// forty compiler processes at half a core each still show up as twenty cores of `swift-frontend`.
#[derive(Serialize)]
struct Frame {
    at_ms: f64,
    consumers: Vec<Consumer>,
    total_cpu_percent_one_core: f64,
    by_name: Vec<NameShare>,
}

#[derive(Default)]
pub struct Monitor {
    collector: Option<i32>,
    end: f64,
    since: Option<f64>,
    latched: bool,
    samples: VecDeque<Sample>,
    frames: VecDeque<Frame>,
    previous: Option<(f64, Vec<Proc>)>,
    telemetry: crate::telemetry::Telemetry,
    programs: crate::program::Programs,
    surge: crate::surge::Surge,
    surge_due: Option<(f64, f64)>,
}

impl Monitor {
    fn reset(&mut self) {
        self.since = None;
        self.latched = false;
        self.samples.clear();
        self.surge.reset();
    }

    fn ingest(&mut self, sample: Sample, now: f64) -> Option<String> {
        if sample.end <= self.end {
            return None;
        }
        let valid = [sample.start, sample.end, sample.percent]
            .iter()
            .all(|n| n.is_finite())
            && sample.percent >= 0.0
            && sample.percent <= 100.0
            && sample.end > sample.start
            && sample.end - sample.start <= FRESH_MS
            && sample.end <= now
            && now - sample.end <= 60_000.0;
        if !valid {
            self.reset();
            self.end = if sample.end.is_finite() && sample.end <= now {
                sample.end
            } else {
                0.0
            };
            return None;
        }
        if self.end != 0.0 && (sample.start - self.end).abs() > 1000.0 {
            self.reset();
        }
        self.end = sample.end;
        if let Some(since) = self.surge.ingest(sample.start, sample.end, sample.percent) {
            self.surge_due = Some((since, sample.end));
        }
        if sample.percent <= 90.0 {
            self.reset();
            return None;
        }
        let since = *self.since.get_or_insert(sample.start);
        self.samples.push_back(sample);
        while self.samples.len() > 120 {
            self.samples.pop_front();
        }
        if !self.latched && self.end - since >= WINDOW_MS {
            self.latched = true;
            return Some(format!("{:.0}", self.end));
        }
        None
    }

    fn frame(&mut self, now: f64, processes: &[Proc]) {
        if let Some((time, previous)) = &self.previous {
            if now > *time && now - time <= 30.0 {
                let by_pid: HashMap<_, _> = previous.iter().map(|p| (p.pid, p)).collect();
                let mut consumers: Vec<_> = processes
                    .iter()
                    .filter_map(|p| {
                        let old = by_pid.get(&p.pid)?;
                        if (p.start - old.start).abs() > 2 || p.cpu_secs < old.cpu_secs {
                            return None;
                        }
                        Some(Consumer {
                            pid: p.pid,
                            ppid: p.ppid,
                            start: p.start,
                            name: p.name.clone(),
                            cpu_percent_one_core: (p.cpu_secs - old.cpu_secs) / (now - time)
                                * 100.0,
                            program: serde_json::Value::Null,
                        })
                    })
                    .collect();
                let mut names: HashMap<&str, (usize, f64)> = HashMap::new();
                for consumer in &consumers {
                    let entry = names.entry(consumer.name.trim_matches(['(', ')'])).or_default();
                    entry.0 += 1;
                    entry.1 += consumer.cpu_percent_one_core;
                }
                let total_cpu_percent_one_core = names.values().map(|(_, cpu)| cpu).sum();
                let mut by_name: Vec<NameShare> = names.into_iter()
                    .map(|(name, (processes, cpu_percent_one_core))| NameShare { name: name.to_string(), processes, cpu_percent_one_core })
                    .collect();
                by_name.sort_by(|a, b| b.cpu_percent_one_core.total_cmp(&a.cpu_percent_one_core));
                by_name.truncate(12);
                consumers.sort_by(|a, b| b.cpu_percent_one_core.total_cmp(&a.cpu_percent_one_core));
                consumers.truncate(12);
                self.frames.push_back(Frame {
                    at_ms: now * 1000.0,
                    consumers,
                    total_cpu_percent_one_core,
                    by_name,
                });
            }
        }
        while self.frames.len() > 24 {
            self.frames.pop_front();
        }
        self.previous = Some((now, processes.to_vec()));
    }

    pub fn observe(
        &mut self,
        home: &Path,
        now: f64,
        processes: &[Proc],
        clock: impl Fn() -> f64,
    ) -> Result<()> {
        self.frame(now, processes);
        if let Some(frame) = self.frames.back_mut().filter(|frame|frame.at_ms == now * 1000.0) {
            for consumer in &mut frame.consumers {
                if let Some(p)=processes.iter().find(|p|p.pid==consumer.pid) {
                    consumer.program=self.programs.record(home,p,processes,now)?;
                }
            }
        }
        self.telemetry.observe(home, now, processes)?;
        crate::stall_guard::step("monitor");
        self.surge.record(now, processes);
        let history = std::fs::read(home.join(".local/state/mars-cpu-build-gate/history.json"))
            .context("read host CPU collector")
            .and_then(|bytes| Ok(serde_json::from_slice::<History>(&bytes)?));
        let now_ms = clock() * 1000.0;
        if self.end > now_ms {
            self.reset();
            self.end = 0.0;
        }
        let mut error = None;
        match history {
            Ok(history)
                if history.updated.is_finite()
                    && now_ms >= history.updated
                    && now_ms - history.updated <= FRESH_MS
                    && !history.samples.is_empty() =>
            {
                if self.collector != Some(history.pid) {
                    self.reset();
                    self.end = 0.0;
                    self.collector = Some(history.pid);
                }
                crate::cpu_history::record(home,now_ms,history.pid,serde_json::to_value(&history.samples)?,
                    serde_json::to_value(self.frames.back())?,processes)?;
                for sample in history.samples {
                    if let Some(id) = self.ingest(sample, now_ms) {
                        let event = serde_json::json!({ "id": id, "trigger": "whole_host_cpu_continuously_gt_90_for_300s",
                            "hot_since_ms": self.since, "detected_at_ms": self.end,
                            "host_samples": self.samples, "process_frames": self.frames,
                            "telemetry": crate::telemetry::evidence(home) });
                        if let Err(failure) = write_json(
                            &root(home).join("incidents").join(format!("{id}.json")),
                            &event,
                        ) {
                            self.latched = false;
                            return Err(failure);
                        }
                        // Join-only condition of the performance trigger: the session analyzing the surge covers it.
                        if let Err(failure) = crate::wake::trigger(home, now_ms, crate::wake::Condition::CpuHot, &id,
                            "whole-host CPU stayed above 90% for 300 s", true, serde_json::json!({})) {
                            crate::log(&format!("performance wake {id}: {failure:#}"));
                        }
                    }
                }
                if let Some((since, end)) = self.surge_due.take() {
                    if let Err(failure) = self.surge.fire(home, since, end, processes, serde_json::to_value(&self.frames)?) {
                        error = Some(format!("CPU surge responder: {failure:#}"));
                    }
                }
                if now_ms - self.end > FRESH_MS {
                    self.reset();
                    error = Some("Host CPU samples are stale".to_string());
                }
            }
            result => {
                self.reset();
                self.end = 0.0;
                error = Some(match result {
                    Err(e) => format!("{e:#}"),
                    _ => "Host CPU collector is stale or empty".into(),
                });
            }
        }
        write_json(
            &root(home).join("monitor.json"),
            &serde_json::json!({
                "at_ms": now_ms, "collector_pid": self.collector, "last_sample_ms": self.end,
                "threshold_percent_whole_host": 90, "required_continuous_ms": WINDOW_MS,
                "hot_since_ms": self.since, "incident_latched": self.latched, "surge": self.surge.state(), "error": error
            }),
        )
    }
}

pub(crate) fn root(home: &Path) -> PathBuf {
    home.join(".local/state/cpu-watchdog")
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let parent = path.parent().context("state parent")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

pub fn check(home: &Path) -> Result<()> {
    let state = std::fs::read(root(home).join("monitor.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let cpu_healthy = state.as_ref().is_some_and(|s| {
        s["error"].is_null()
            && s["at_ms"].as_f64().is_some_and(|at| {
                crate::unix_now() * 1000.0 - at >= 0.0 && crate::unix_now() * 1000.0 - at < 60_000.0
            })
    });
    let telemetry = crate::telemetry::health(home, crate::unix_now() * 1000.0);
    let healthy = cpu_healthy
        && telemetry
            .as_object()
            .is_some_and(|sources| sources.values().all(|source| source["healthy"] == true));
    let directory = root(home).join("incidents");
    let mut pending = Vec::new();
    if directory.exists() {
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
                && !path.with_extension("ack").exists()
            {
                pending.push(path);
            }
        }
    }
    pending.sort();
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({ "healthy": healthy, "cpu_healthy":cpu_healthy,"telemetry":telemetry,"state": state, "pending_incidents": pending })
        )?
    );
    if healthy && pending.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

/// Every acknowledgement gate; read-only, so `retro-overdue` can name the refusal.
pub(crate) fn validate(home: &Path, id: &str, path: &Path) -> Result<()> {
    crate::retro_recall::validate_ack(home, id, path)?;
    crate::telemetry::retro_followup_validate(home, id, path)?;
    crate::telemetry::retro_repeats_validate(home, id, path)?;
    crate::telemetry::retro_clarity_validate(home, id, path)?;
    crate::telemetry::wait_retro_validate(home, id, path)?;
    crate::telemetry::task_stall_validate(home, id, path)?;
    crate::telemetry::canary_retro_validate(home, id, path)?;
    crate::telemetry::crm_turn_retro_validate(home, id, path)?;
    crate::telemetry::risk_retro_validate(home, id, path)?;
    crate::telemetry::crm_costly_turn_validate(home, id, path)?;
    crate::telemetry::recall_retro_validate(home, id, path)?;
    crate::telemetry::memory_dream_validate(home, id, path)?;
    crate::telemetry::prompt_review_validate(home, id, path)?;
    crate::telemetry::hook_watch_validate(home, id, path)?;
    crate::telemetry::vector_db_validate(home, id, path)?;
    crate::telemetry::cache_trial_validate(home, id, path)?;
    crate::log_recall::verify(&root(home).join("analysis").join(id))?;
    crate::recovery_retro::validate(home, id, path)?;
    crate::wake::validate(home, id)?;
    crate::crash_repair::batch::validate(home, id)?;
    crate::cause_tree::validate_ack(home, id, path)
}

pub fn ack(home: &Path, id: Option<String>) -> Result<()> {
    let id = id.context("usage: cpu-watchdog monitor-ack <incident-id>")?;
    anyhow::ensure!(
        !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()),
        "invalid incident id"
    );
    let path = root(home).join("incidents").join(format!("{id}.json"));
    anyhow::ensure!(path.is_file(), "incident does not exist");
    let analysis = root(home).join("analysis").join(&id);
    if analysis.is_dir() {
        let sealed = crate::analysis_private::seal(&analysis)?;
        if sealed > 0 {
            eprintln!("monitor-ack: set {sealed} path(s) under analysis/{id} to 0700/0600");
        }
    }
    validate(home, &id, &path)?;
    write_json(
        &path.with_extension("ack"),
        &serde_json::json!({ "acknowledged_at_ms": crate::unix_now() * 1000.0 }),
    )?;
    if let Err(error) = crate::cause_tree::record_ack(home, &id, &path) {
        crate::log(&format!("diagnosis ledger {id}: {error:#}"));
    }
    if let Err(error) = crate::retro_scorecard::record_ack(home, &id, &path) {
        crate::log(&format!("retro ledger {id}: {error:#}"));
    }
    crate::crash_repair::batch::close_covered(home, &id)?;
    Ok(())
}

#[cfg(test)]
#[path = "monitor_tests.rs"]
mod tests;
