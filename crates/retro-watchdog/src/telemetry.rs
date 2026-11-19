//! Supplemental probes never block the resident signal policy on Docker or Xcode commands.
mod cache_watch;
mod crm_ops;
mod crm_turn_retro;
mod risk_retro;
mod canary_retro;
mod cc_restart;
pub(crate) mod command;
mod containers;
mod conversation_retro;
mod crash_dialog;
pub(crate) mod daemon_heal;
mod desktop;
mod diagnostics;
pub(crate) use diagnostics::incident as springboard_incident;
mod disk_guard;
mod esbuild_orphans;
mod gatekeeper_sweep;
mod memory_groups;
mod memory_guard;
mod memory_series;
mod memory_thrash;
mod memory_holders;
mod docker_watch;
mod guest;
mod headless_heal;
mod lease_stall;
mod mic_sessions;
mod network_analysis;
mod oom;
mod port_census;
mod quota_watch;
mod prompt_review;
mod hook_watch;
mod vector_db_watch;
mod cache_trial;
mod release_retro;
mod release_watch;
mod render_probe;
mod runtime_retro;
pub(crate) use runtime_retro::parse::timestamp_ms as transcript_ms;
mod recall_retro;
mod memory_dream;
pub(crate) mod retro_series;
mod search_retro;
mod software_encode;
mod spotlight_guard;
mod storybook_watch;
mod stream_watch;
mod task_stall;
mod tcc_backlog;
mod tasks;
mod transcript_calls;
mod wait_retro;
mod win_guard;
mod win_logs;
mod retro_followup;
mod retro_followup_printed;
mod retro_repeats;
mod retro_clarity;
mod xcode;

use crate::{
    monitor::{root, write_json},
    procs::Proc,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::mpsc::{self, Receiver, TryRecvError},
};

/// PATH of the Mars_CRM package scripts the watchdog runs through `mars` (cache-watch, quota-economics, release recover-disk): cpu-build-gate, cargo
/// (rustup, since those scripts became `cargo run -p mars-ops`), gcloud and cloud-sql-proxy (Homebrew), kubectl (Docker).
pub(super) const MARS_SCRIPT_PATH: &str = "/home/user/.local/bin:/home/user/.docker/bin:/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

#[derive(Default)]
pub struct Telemetry {
    tasks: tasks::Tasks,
    desktop: desktop::Desktop,
    last_probe: f64,
    last_retention: f64,
    worker: Option<Receiver<()>>,
    last_census: f64,
    census: Option<Receiver<()>>,
    mic_started: bool,
    crash_dialog: crash_dialog::CrashDialog,
    daemon_heal: daemon_heal::DaemonHeal,
    headless_heal: headless_heal::HeadlessHeal,
    storybook_watch: storybook_watch::StorybookWatch,
    esbuild_orphans: esbuild_orphans::EsbuildOrphans,
    tcc_backlog: tcc_backlog::TccBacklog,
    software_encode: software_encode::SoftwareEncode,
    conversation_retro: conversation_retro::ConversationRetro,
    runtime_retro: runtime_retro::RuntimeRetro,
    search_retro: search_retro::SearchRetro,
    recall_retro: recall_retro::RecallRetro,
    memory_dream: memory_dream::MemoryDream,
    disk_guard: disk_guard::DiskGuard,
    memory_guard: memory_guard::MemoryGuard,
    spotlight_guard: spotlight_guard::SpotlightGuard,
    docker_watch: docker_watch::DockerWatch,
    win_guard: win_guard::WinGuard,
    win_logs: win_logs::WinLogs,
    stream_watch: stream_watch::StreamWatch,
    quota_watch: quota_watch::QuotaWatch,
    prompt_review: prompt_review::PromptReview,
    hook_watch: hook_watch::HookWatch,
    vector_db_watch: vector_db_watch::VectorDbWatch,
    cache_trial: cache_trial::CacheTrial,
    cache_watch: cache_watch::CacheWatch,
    release_retro: release_retro::ReleaseRetro,
    canary_retro: canary_retro::CanaryRetro,
    crm_turn_retro: crm_turn_retro::CrmTurnRetro,
    crm_costly_turn: crm_turn_retro::costly::CrmCostlyTurn,
    risk_retro: risk_retro::RiskRetro,
    release_watch: release_watch::ReleaseWatch,
    wait_retro: wait_retro::WaitRetro,
    task_stall: task_stall::TaskStall,
}

impl Telemetry {
    pub fn observe(&mut self, home: &Path, now: f64, processes: &[Proc]) -> Result<()> {
        crate::stall_guard::step("tasks");
        self.tasks.observe(home, now, processes)?;
        crate::stall_guard::step("desktop");
        if let Err(error) = self.desktop.observe(home, now, processes) {
            write_json(&root(home).join("desktop.json"),
                &json!({"at_ms":now*1000.0,"error":format!("{error:#}")}))?;
        }
        crate::stall_guard::step("diagnostics");
        if let Err(error) = diagnostics::collect(home) {
            write_json(&root(home).join("xcode-errors.json"),
                &json!({"at_ms":now*1000.0,"error":format!("{error:#}")}))?;
        }
        // Run the file tail even when CPU history is stale or a guest probe failed.
        crate::stall_guard::step("oom");
        let oom = oom::observe(home, now);
        write_json(
            &root(home).join("oom-source.json"),
            &match oom {
                Ok(state) => state,
                Err(error) => json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}),
            },
        )?;
        if !cfg!(test) {
            crate::stall_guard::step("poll");
            self.poll(home, now)?;
            crate::stall_guard::step("census");
            self.census(home, now, processes)?;
            crate::stall_guard::step("crash-dialog");
            if let Err(error) = self.crash_dialog.observe(home, now, processes) {
                write_json(&root(home).join("crash-dialog-heal.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("daemon-heal");
            if let Err(error) = self.daemon_heal.observe(home, now, processes) {
                write_json(&root(home).join("daemon-heal.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("headless-heal");
            if let Err(error) = self.headless_heal.observe(home, now, processes) {
                write_json(&root(home).join("headless-heal.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("storybook-watch");
            if let Err(error) = self.storybook_watch.observe(home, now) {
                write_json(&root(home).join("storybook-watch.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("esbuild-orphans");
            if let Err(error) = self.esbuild_orphans.observe(home, now, processes) {
                crate::log(&format!("esbuild orphan: {error:#}"));
            }
            crate::stall_guard::step("tcc-backlog");
            if let Err(error) = self.tcc_backlog.observe(home, now, processes) {
                crate::log(&format!("tcc backlog: {error:#}"));
            }
            crate::stall_guard::step("software-encode");
            if let Err(error) = self.software_encode.observe(home, now, processes) {
                crate::log(&format!("software encode: {error:#}"));
            }
            crate::stall_guard::step("conversation-retro");
            if let Err(error) = self.conversation_retro.observe(home, now) {
                write_json(&root(home).join("conversation-retro.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("runtime-retro");
            if let Err(error) = self.runtime_retro.observe(home, now) {
                write_json(&root(home).join("runtime-retro.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("search-retro");
            if let Err(error) = self.search_retro.observe(home, now) {
                write_json(&root(home).join("search-retro.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("recall-retro");
            if let Err(error) = self.recall_retro.observe(home, now) {
                write_json(&root(home).join("recall-retro.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("memory-dream");
            if let Err(error) = self.memory_dream.observe(home, now) {
                write_json(&root(home).join("memory-dream.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("disk-guard");
            if let Err(error) = self.disk_guard.observe(home, now) {
                write_json(&root(home).join("disk-guard.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("memory-guard");
            if let Err(error) = self.memory_guard.observe(home, now) {
                write_json(&root(home).join("memory-guard.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("spotlight-guard");
            if let Err(error) = self.spotlight_guard.observe(home, now) {
                write_json(&root(home).join("spotlight-guard.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("docker-watch");
            if let Err(error) = self.docker_watch.observe(home, now) {
                write_json(&root(home).join("docker-watch.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("win-guard");
            if let Err(error) = self.win_guard.observe(home, now) {
                write_json(&root(home).join("win-guard.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("win-logs");
            if let Err(error) = self.win_logs.observe(home, now) {
                crate::log(&format!("win logs: {error:#}"));
            }
            crate::stall_guard::step("stream-watch");
            if let Err(error) = self.stream_watch.observe(home, now) {
                write_json(&root(home).join("stream-watch.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("quota-watch");
            if let Err(error) = self.quota_watch.observe(home, now) {
                write_json(&root(home).join("quota-economics.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("prompt-review");
            if let Err(error) = self.prompt_review.observe(home, now) {
                write_json(&root(home).join("prompt-review.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("hook-watch");
            if let Err(error) = self.hook_watch.observe(home, now) {
                write_json(&root(home).join("hook-watch.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("vector-db-watch");
            if let Err(error) = self.vector_db_watch.observe(home, now) {
                write_json(&root(home).join("vector-db-watch.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("cache-trial");
            if let Err(error) = self.cache_trial.observe(home, now) {
                write_json(&root(home).join("cache-trial.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("cache-watch");
            if let Err(error) = self.cache_watch.observe(home, now) {
                write_json(&root(home).join("cache-watch.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("release-watch");
            if let Err(error) = self.release_watch.observe(home, now) {
                write_json(&root(home).join("release-watch.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("release-retro");
            if let Err(error) = self.release_retro.observe(home, now) {
                write_json(&root(home).join("release-retro.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("canary-retro");
            if let Err(error) = self.canary_retro.observe(home, now) {
                write_json(&root(home).join("canary-retro.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("crm-turn-retro");
            if let Err(error) = self.crm_turn_retro.observe(home, now) {
                write_json(&root(home).join("crm-turn-retro.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("crm-costly-turn");
            if let Err(error) = self.crm_costly_turn.observe(home, now) {
                write_json(&root(home).join("crm-costly-turn.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("risk-retro");
            if let Err(error) = self.risk_retro.observe(home, now) {
                write_json(&root(home).join("risk-retro.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("wait-retro");
            if let Err(error) = self.wait_retro.observe(home, now) {
                write_json(&root(home).join("wait-retro.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            crate::stall_guard::step("task-stall");
            if let Err(error) = self.task_stall.observe(home, now) {
                write_json(&root(home).join("task-stall.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}))?;
            }
            if !self.mic_started {
                mic_sessions::spawn(home.to_path_buf())?;
                self.mic_started = true;
            }
        }
        Ok(())
    }

    /// Runs the root Mach port census on its own thread every port_census::INTERVAL_SECS.
    fn census(&mut self, home: &Path, now: f64, processes: &[Proc]) -> Result<()> {
        if let Some(census) = &self.census {
            match census.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => self.census = None,
                Err(TryRecvError::Empty) => return Ok(()),
            }
        }
        if self.last_census > 0.0 && now >= self.last_census && now - self.last_census < port_census::INTERVAL_SECS {
            return Ok(());
        }
        let targets: Vec<Proc> = processes.iter().filter(|p| port_census::TARGETS.iter().any(|t| t.name == p.name)).cloned().collect();
        let (sender, receiver) = mpsc::sync_channel(1);
        let home = home.to_path_buf();
        std::thread::Builder::new().name("watchdog-port-census".into()).spawn(move || {
            if let Err(error) = port_census::collect(&home, now, &targets) {
                let _ = write_json(&root(&home).join("port-census.json"),
                    &json!({"at_ms": now * 1000.0, "error": format!("{error:#}")}));
            }
            let _ = sender.send(());
        })?;
        self.last_census = now;
        self.census = Some(receiver);
        Ok(())
    }

    fn poll(&mut self, home: &Path, now: f64) -> Result<()> {
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => return Ok(()),
            }
        }
        if self.last_probe > 0.0 && now >= self.last_probe && now - self.last_probe < 60.0 {
            return Ok(());
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let retain = self.last_retention == 0.0 || now - self.last_retention >= 3600.0;
        let home = home.to_path_buf();
        std::thread::Builder::new().name("watchdog-evidence".into()).spawn(move || {
            if let Err(error) = crate::dispatch::poll_queue(&home, crate::unix_now()*1000.0) {
                let _ = write_json(&root(&home).join("analysis-queue-health.json"),
                    &json!({"at_ms":crate::unix_now()*1000.0,"error":format!("{error:#}")}));
            } else {
                let state = read(&root(&home).join("analysis-queue.json")).unwrap_or(Value::Null);
                let _ = write_json(&root(&home).join("analysis-queue-health.json"),
                    &json!({"at_ms":crate::unix_now()*1000.0,"error":state["error"],"waiting":state["waiting"]}));
            }
            if let Err(error) = crate::retro_overdue::escalate(&home, crate::unix_now()*1000.0) {
                crate::log(&format!("retro overdue: {error:#}"));
            }
            if let Err(error) = crate::resource_usage::collect(&home) {
                let _ = write_json(&root(&home).join("resource-usage.json"),
                    &json!({"at_ms":crate::unix_now()*1000.0,"error":format!("{error:#}")}));
            }
            if retain {
                if let Err(error) = crate::retention::maintain(&home, std::time::SystemTime::now(), false) {
                    let _ = write_json(&root(&home).join("retention.json"),
                        &json!({"at_ms":crate::unix_now()*1000.0,"error":format!("{error:#}")}));
                }
            }
            for (name, result) in [("guest", guest::collect(&home)), ("xcode", xcode::collect(&home))] {
                if let Err(error) = result {
                    let _ = write_json(&root(&home).join(format!("{name}.json")),
                        &json!({"at_ms": crate::unix_now()*1000.0, "error": format!("{error:#}")}));
                }
            }
            if let Err(error) = crate::crash_repair::poll(&home, crate::unix_now() * 1000.0) {
                let _ = write_json(&root(&home).join("crash-repair-health.json"),
                    &json!({"at_ms":crate::unix_now()*1000.0,"error":format!("{error:#}")}));
            }
            if let Err(error)=network_analysis::poll(&home) {
                let _=write_json(&root(&home).join("network-analysis.json"),
                    &json!({"at_ms":crate::unix_now()*1000.0,"account":"CO2","error":format!("{error:#}")}));
            }
            let _ = sender.send(());
        })?;
        self.last_probe = now;
        if retain { self.last_retention = now; }
        self.worker = Some(receiver);
        Ok(())
    }
}

pub fn evidence(home: &Path) -> Value {
    let mut result = serde_json::Map::new();
    for name in ["guest", "tasks", "container-peaks", "xcode", "xcode-errors", "oom-source", "desktop", "port-census", "mic-sessions", "resource-usage"] {
        result.insert(
            name.into(),
            read(&root(home).join(format!("{name}.json")))
                .unwrap_or_else(|| json!({"error": "Evidence unavailable"})),
        );
    }
    Value::Object(result)
}

pub fn health(home: &Path, now_ms: f64) -> Value {
    let mut result = serde_json::Map::new();
    for (name, max_age) in [
        ("tasks", 60_000.0),
        ("oom-source", 60_000.0),
        ("guest", 180_000.0),
        ("resource-usage", 180_000.0),
        ("analysis-queue-health", 180_000.0),
        ("xcode", 180_000.0),
        ("xcode-errors", 60_000.0),
        ("desktop", 60_000.0),
        // One census every 300 s; three missed runs are unhealthy.
        ("port-census", 900_000.0),
        // The ledger thread writes a heartbeat every 30 s.
        ("mic-sessions", 120_000.0),
        // Written every daemon_heal::SAMPLE_SECS (30 s).
        ("daemon-heal", 120_000.0),
        // Written every crash_dialog::CHECK_INTERVAL_SECS (60 s).
        ("crash-dialog-heal", 180_000.0),
        // Written every headless_heal::SAMPLE_SECS (30 s).
        ("headless-heal", 120_000.0),
        // Written every storybook_watch::SAMPLE_SECS (30 s).
        ("storybook-watch", 120_000.0),
        // Written when each conversation_retro::SAMPLE_SECS (120 s) census starts and ends.
        ("conversation-retro", 360_000.0),
        // Written when each runtime_retro::SAMPLE_SECS (600 s) scan starts and ends.
        ("runtime-retro", 1_800_000.0),
        // Written every search_retro::SAMPLE_SECS (60 s).
        ("search-retro", 300_000.0),
        // Written every recall_retro::SAMPLE_SECS (60 s).
        ("recall-retro", 300_000.0),
        // Written every memory_dream::SAMPLE_SECS (600 s).
        ("memory-dream", 1_800_000.0),
        // Written every disk_guard::SAMPLE_SECS (300 s).
        ("disk-guard", 900_000.0),
        // Written every memory_guard::SAMPLE_SECS (5 s).
        ("memory-guard", 60_000.0),
        // Written every docker_watch::SAMPLE_SECS (30 s).
        ("docker-watch", 120_000.0),
        // Written every win_guard::SAMPLE_SECS (60 s).
        ("win-guard", 300_000.0),
        // Written every stream_watch::IDLE_SECS (60 s), every ACTIVE_SECS (15 s) while streaming.
        ("stream-watch", 300_000.0),
        // Written every quota_watch::SAMPLE_SECS (300 s).
        ("quota-economics", 900_000.0),
        // Written every prompt_review::SAMPLE_SECS (300 s).
        ("prompt-review", 900_000.0),
        // Written every hook_watch::SAMPLE_SECS (300 s).
        ("hook-watch", 900_000.0),
        // Written every vector_db_watch::SAMPLE_SECS (600 s).
        ("vector-db-watch", 1_800_000.0),
        // Written every cache_trial::SAMPLE_SECS (300 s).
        ("cache-trial", 900_000.0),
        // Written when each cache_watch::SAMPLE_SECS (900 s) run starts and ends.
        ("cache-watch", 2_700_000.0),
        // Written every release_watch::SAMPLE_SECS (30 s).
        ("release-watch", 120_000.0),
        // Written every release_retro::SAMPLE_SECS (300 s).
        ("release-retro", 900_000.0),
        // Written every canary_retro::SAMPLE_SECS (300 s).
        ("canary-retro", 900_000.0),
        // Written when each crm_turn_retro::SAMPLE_SECS (900 s) count starts and ends.
        ("crm-turn-retro", 2_700_000.0),
        // Written when each crm_turn_retro::costly::SAMPLE_SECS (300 s) read starts and ends.
        ("crm-costly-turn", 900_000.0),
        // Written when each risk_retro::SAMPLE_SECS (120 s) read starts and ends.
        ("risk-retro", 600_000.0),
        // Written when each wait_retro::SAMPLE_SECS (600 s) scan starts and ends.
        ("wait-retro", 1_800_000.0),
        // Written when each task_stall::SAMPLE_SECS (120 s) scan starts and ends.
        ("task-stall", 360_000.0),
        ("crash-repair-health", 180_000.0),
        ("network-analysis", 180_000.0),
    ] {
        if name == "network-analysis" && read(&root(home).join("network-analysis-policy.json")).is_none_or(|p|p["enabled"]!=true) {continue;}
        if name == "crash-repair-health" &&
            read(&root(home).join("crash-repair-policy.json")).is_none_or(|p|p["enabled"] != true) { continue; }
        let state = read(&root(home).join(format!("{name}.json"))).unwrap_or(Value::Null);
        let age = state["at_ms"].as_f64().map(|at| now_ms - at);
        let healthy = state.is_object()
            && age.is_some_and(|age| age >= 0.0 && age <= max_age)
            && state["error"].is_null()
            && state["stats_error"].is_null()
            && state["probe_errors"]
                .as_array()
                .is_none_or(|errors| errors.is_empty())
            && state["evidence_errors"].as_array().is_none_or(|errors| errors.is_empty());
        result.insert(name.into(),json!({"healthy":healthy,"age_ms":age,"max_age_ms":max_age,
            "error":state["error"],"stats_error":state["stats_error"],"probe_errors":state["probe_errors"],
            "evidence_errors":state["evidence_errors"],
            "coverage_gap":state["coverage_gap"],"pending_changed_manifests":state["pending_changed_manifests"]}));
    }
    Value::Object(result)
}

fn read(path: &Path) -> Option<Value> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > 4 * 1024 * 1024 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(4 * 1024 * 1024).read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Live microphone users plus the resident ledger state.
pub fn mic_sessions_now(home: &Path) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&json!({"running_input": mic_sessions::live(),
        "ledger": read(&root(home).join("mic-sessions.json"))}))?);
    Ok(())
}

/// Whether this program may list Xcode's open Swift files (the grant relief's canvas park needs).
pub fn xcode_canvas_check(home: &Path) -> Result<()> {
    memory_guard::xcode_canvas_check(home)
}

/// One census now, with the resident evaluation (an over-limit count still opens its incident).
pub fn port_census_now(home: &Path) -> Result<()> {
    let (now, processes) = crate::procs::snapshot(crate::unix_now)?;
    port_census::collect(home, now, &processes)?;
    println!("{}", serde_json::to_string_pretty(&read(&root(home).join("port-census.json")))?);
    Ok(())
}

pub fn xcode_errors(home: &Path) -> Result<()> {
    diagnostics::collect(home)?;
    println!("{}", serde_json::to_string_pretty(&json!({
        "runtime":read(&root(home).join("xcode-errors.json")),
        "builds":read(&root(home).join("xcode.json"))
    }))?);
    Ok(())
}

/// A census now for the CPU surge responder, with the resident evaluation; returns the new state.
pub(crate) fn census_for_surge(home: &Path, now: f64, processes: &[Proc]) -> Result<Value> {
    port_census::collect(home, now, processes)?;
    read(&root(home).join("port-census.json")).ok_or_else(|| anyhow::anyhow!("port-census.json unreadable"))
}

/// The rate-limited ControlCenter restart, for the surge responder's CPU rule.
pub(crate) fn restart_controlcenter(home: &Path, now: f64, pid: i32, start: i64, ports: u64, incident: &str) -> Result<Value> {
    cc_restart::restart(home, now, pid, start, ports, incident)
}

/// `cpu-watchdog docker-pause <minutes> [reason ...]`: hold Docker recovery during maintenance.
pub fn docker_pause(home: &Path, args: Vec<String>) -> Result<()> {
    docker_watch::pause_cli(home, args)
}

/// `cpu-watchdog remote-reboot [--dry-run] <reason ...>`: planned reboot of the remote beta host.
pub fn remote_reboot(home: &Path, args: Vec<String>) -> Result<()> {
    docker_watch::remote_reboot_cli(home, args)
}

pub fn docker_restarts(home: &Path, args: Vec<String>) -> Result<()> {
    docker_watch::restarts_cli(home, args)
}

pub fn retro_followup(home: &Path, args: Vec<String>) -> Result<()> {
    retro_followup::cli(home, args)
}

pub fn retro_followup_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    retro_followup::validate_ack(home, id, incident)
}

pub fn runtime_breakdown(home: &Path, args: Vec<String>) -> Result<()> {
    runtime_retro::replay::cli(home, args)
}

pub fn retro_repeats(home: &Path, args: Vec<String>) -> Result<()> {
    retro_repeats::cli(home, args)
}

pub fn retro_repeats_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    retro_repeats::validate_ack(home, id, incident)
}

pub fn retro_clarity(home: &Path, args: Vec<String>) -> Result<()> {
    retro_clarity::cli(home, args)
}

pub fn release_stats(home: &Path, args: Vec<String>) -> Result<()> {
    release_retro::cli(home, args)
}

pub fn canary_stats(home: &Path, args: Vec<String>) -> Result<()> {
    canary_retro::cli(home, args)
}

pub fn canary_retro_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    canary_retro::validate_ack(home, id, incident)
}

/// `cpu-watchdog crm-turn-stats`: waste metrics of stored Mars CRM production chat turns.
pub fn crm_turn_stats(home: &Path, args: Vec<String>) -> Result<()> {
    crm_turn_retro::cli(home, args)
}

pub fn crm_turn_retro_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    crm_turn_retro::validate_ack(home, id, incident)
}

/// `cpu-watchdog crm-costly-turns`: Mars CRM production turns that settled more than the costly-turn threshold.
pub fn crm_costly_turns(home: &Path, args: Vec<String>) -> Result<()> {
    crm_turn_retro::costly::cli(home, args)
}

pub fn crm_costly_turn_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    crm_turn_retro::costly::validate_ack(home, id, incident)
}

/// `cpu-watchdog risk-events`: Mars CRM risk warnings, reviews and suspensions of non-exempt accounts.
pub fn risk_events(home: &Path, args: Vec<String>) -> Result<()> {
    risk_retro::cli(home, args)
}

pub fn risk_retro_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    risk_retro::validate_ack(home, id, incident)
}

pub fn recall_retro_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    recall_retro::validate::validate_ack(home, id, incident)
}

/// `cpu-watchdog dream-sessions <incident> | --hours <n>`: the sessions a memory dream audits.
pub fn dream_sessions(home: &Path, args: Vec<String>) -> Result<()> {
    memory_dream::sessions::cli(home, args)
}

pub fn memory_dream_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    memory_dream::validate::validate_ack(home, id, incident)
}

pub fn prompt_review_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    prompt_review::validate::validate_ack(home, id, incident)
}

pub fn hook_watch_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    hook_watch::validate::validate_ack(home, id, incident)
}

pub fn vector_db_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    vector_db_watch::validate::validate_ack(home, id, incident)
}

/// `cpu-watchdog vector-db-stats`: vector database size, baseline and distance to the next retro.
pub fn vector_db_stats(home: &Path) -> Result<()> {
    vector_db_watch::cli(home)
}

pub fn cache_trial_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    cache_trial::validate_ack(home, id, incident)
}

pub fn cache_trial_cli(home: &Path, args: Vec<String>) -> Result<()> {
    cache_trial::cli(home, args)
}

pub fn wait_stats(home: &Path, args: Vec<String>) -> Result<()> {
    wait_retro::cli(home, args)
}

pub fn wait_retro_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    wait_retro::validate_ack(home, id, incident)
}

pub fn stream_stats(home: &Path, args: Vec<String>) -> Result<()> {
    stream_watch::cli(home, args)
}

pub fn task_stall_cli(home: &Path, args: Vec<String>) -> Result<()> {
    task_stall::cli(home, args)
}

pub fn search_quality_cli(home: &Path, args: Vec<String>) -> Result<()> {
    search_retro::quality_cli(home, args)
}

pub fn search_score_cli(home: &Path, args: Vec<String>) -> Result<()> {
    search_retro::score_cli(home, args)
}

pub fn retro_series_cli(home: &Path, args: Vec<String>) -> Result<()> {
    retro_series::cli(home, args)
}

pub fn search_pace_cli(home: &Path, args: Vec<String>) -> Result<()> {
    search_retro::pace_cli(home, args)
}

pub fn task_stall_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    task_stall::validate_ack(home, id, incident)
}

pub fn retro_clarity_validate(home: &Path, id: &str, incident: &Path) -> Result<()> {
    retro_clarity::validate_ack(home, id, incident)
}

pub(crate) fn read_state(path: &Path) -> Option<Value> {
    read(path)
}

#[cfg(test)]
pub(crate) fn test_home(name: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let home = std::env::temp_dir().join(format!(
        "watchdog-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&home).unwrap();
    home
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn health_rejects_stale_failed_or_missing_supplemental_sources() {
        let home = test_home("health");
        assert!(health(&home, 100000.0)
            .as_object()
            .unwrap()
            .values()
            .all(|value| value["healthy"] == false));
        for name in ["tasks", "oom-source", "guest", "xcode", "xcode-errors", "desktop", "port-census", "mic-sessions", "daemon-heal", "crash-dialog-heal", "headless-heal", "storybook-watch", "conversation-retro", "runtime-retro", "search-retro", "recall-retro", "memory-dream", "disk-guard", "memory-guard", "docker-watch", "win-guard", "stream-watch", "quota-economics", "prompt-review", "hook-watch", "vector-db-watch", "cache-trial", "cache-watch", "release-watch", "release-retro", "canary-retro", "crm-turn-retro", "crm-costly-turn", "risk-retro", "wait-retro", "task-stall", "resource-usage", "analysis-queue-health"] {
            write_json(
                &root(&home).join(format!("{name}.json")),
                &json!({"at_ms":100000.0,"error":null}),
            )
            .unwrap();
        }
        assert!(health(&home, 100000.0)
            .as_object()
            .unwrap()
            .values()
            .all(|value| value["healthy"] == true));
        assert_eq!(health(&home, 161000.0)["tasks"]["healthy"], false);
        assert_eq!(health(&home, 161000.0)["guest"]["healthy"], true);
        write_json(
            &root(&home).join("guest.json"),
            &json!({"at_ms":100000.0,"error":null,"stats_error":"timeout"}),
        )
        .unwrap();
        assert_eq!(health(&home, 100000.0)["guest"]["healthy"], false);
        std::fs::remove_dir_all(home).unwrap();
    }
}
