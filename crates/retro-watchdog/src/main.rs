//! cpu-watchdog: terminates runaway tool processes so they stop holding CPU, run resident by
//! `~/Library/LaunchAgents/local.autoretro.cpu-watchdog.plist`.
//!
//! Every INTERVAL_SECS it samples `ps`. A process is a runaway when all of these hold:
//!   1. it belongs to this user and its executable name is in NAMES (node, bun, swift-frontend, …);
//!   2. neither it nor an ancestor is a build or test run (BUILD_MARKERS: xcodebuild, Xcode's
//!      build service, cargo, cpu-build-gate, sim-lease run, docker build, …) or a shared service
//!      (SHARED_MARKERS: debug Chrome, MCP gateways);
//!   3. its CPU time over the last WINDOW_SECS averages at least THRESHOLD_PCT of one core.
//!
//! A runaway gets SIGTERM; if the same process (pid and start time) is still alive GRACE_SECS
//! later it gets SIGKILL. Each outcome is logged and shown as a macOS notification. System
//! daemons, apps (Xcode, Docker, Orca, ChatGPT), agent CLIs and simulators are never listed.
//!
//! Every SWEEP_SECS the loop also runs the S0 sweep (`sweep.rs`), which closes abandoned
//! scratchpad file servers without asking.
//!
//!   cpu-watchdog run                resident loop (launchd; one instance, `run_lock.rs`)
//!   cpu-watchdog status             latest status: watched processes, usage, pending kills
//!   cpu-watchdog inspect [--seconds 1..30]  read-only CPU/RSS sample and installed watchdog inventory
//!   cpu-watchdog port-census      count ControlCenter and WindowServer Mach ports now (sudo askpass)
//!   cpu-watchdog mic-sessions     apps using the microphone now and the session ledger state
//!   cpu-watchdog surge-now        run the CPU >95% responder once now (`surge.rs`), print its report path
//!   cpu-watchdog stall-selftest [secs]  block a test loop and let the stall guard sample, record and exit 75 (`stall_guard.rs`)
//!   cpu-watchdog stall-drill [--no-wait]  block the resident loop for real and wait for the guard's restart (`stall_guard/drill.rs`)
//!   cpu-watchdog sweep [--dry-run]  S0 sweep now, then a JSON report with the hot processes the
//!                                   `process-cleanup` skill's S1 rules decide on
//!
//! Environment: THRESHOLD_PCT=90 WINDOW_SECS=300 GRACE_SECS=30 INTERVAL_SECS=15 SWEEP_SECS=600
//! IDLE_HOURS=6 DRY_RUN=1, NAMES, BUILD_MARKERS and SHARED_MARKERS as comma-separated lists
//! replacing the defaults in `policy.rs`.

mod inspect;
mod cpu_history;
mod inventory;
mod monitor;
mod analysis_private;
mod policy;
mod procs;
mod program;
mod dispatch;
mod crash_repair;
mod surge;
mod sweep;
mod telemetry;
mod reporting;
mod retention;
mod retro_recall;
mod retro_overdue;
mod retro_scorecard;
mod retro_consolidate;
mod log_recall;
mod resource_usage;
mod recovery_retro;
mod wake;
mod cause_tree;
mod run_lock;
mod stall_guard;
#[path = "../../../shared/app_bundle.rs"]
mod app_bundle;

/// Fixed identity so the Local Network Allow survives rebuilds (`shared/app_bundle.rs`).
const BUNDLE: app_bundle::AppBundle = app_bundle::AppBundle {
    id: "local.autoretro.cpu-watchdog",
    executable: "cpu-watchdog",
    name: "CPUWatchdog",
    version: env!("CARGO_PKG_VERSION"),
    usage: "The CPU watchdog checks local services and network paths while it monitors and repairs this host.",
};

use anyhow::{Context, Result};
use policy::{Policy, Reading, Tracker, DEFAULT_BUILD_MARKERS, DEFAULT_NAMES, DEFAULT_SHARED_SERVICE_MARKERS};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Config {
    policy: Policy,
    grace_secs: f64,
    interval_secs: f64,
    dry_run: bool,
    status_path: PathBuf,
    sweep_secs: f64,
    home: PathBuf,
    idle_hours: f64,
}

#[derive(Clone, Serialize)]
struct Pending {
    pid: i32,
    start: i64,
    name: String,
    usage: f64,
    deadline: f64,
}

#[derive(Serialize)]
struct Status<'a> {
    at: String,
    dry_run: bool,
    threshold_pct: f64,
    window_secs: f64,
    /// Eligible processes at or above half the threshold, busiest first.
    watching: Vec<&'a Reading>,
    pending_kill: Vec<&'a Pending>,
}

fn main() -> Result<()> {
    let config = Config::from_env()?;
    match std::env::args().nth(1).as_deref() {
        Some("run") => run(&config),
        None => anyhow::bail!("no command; the resident loop is `cpu-watchdog run` under launchd, see the header for the CLI"),
        Some("status") => {
            let text = std::fs::read_to_string(&config.status_path)
                .with_context(|| format!("no status yet at {}", config.status_path.display()))?;
            println!("{text}");
            Ok(())
        }
        Some("sweep") => {
            let dry_run = config.dry_run || std::env::args().any(|argument| argument == "--dry-run");
            let report = config.sweep(dry_run, 10.0).run(unix_now)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Some("inspect") => inspect::run(&config.home, std::env::args().skip(2).collect()),
        Some("process-info") => program::inspect(std::env::args().nth(2)
            .context("process-info requires a PID")?.parse().context("invalid PID")?),
        Some("monitor-check") => monitor::check(&config.home),
        Some("stall-selftest") => stall_guard::selftest(std::env::args().skip(2).collect()),
        Some("stall-drill") => stall_guard::drill::cli(&config.home, std::env::args().skip(2).collect()),
        Some("surge-now") => surge::run_now(&config.home),
        Some("xcode-errors") => telemetry::xcode_errors(&config.home),
        Some("port-census") => telemetry::port_census_now(&config.home),
        Some("xcode-canvas") => telemetry::xcode_canvas_check(&config.home),
        Some("docker-pause") => telemetry::docker_pause(&config.home, std::env::args().skip(2).collect()),
        Some("remote-reboot") => telemetry::remote_reboot(&config.home, std::env::args().skip(2).collect()),
        Some("docker-restarts") => telemetry::docker_restarts(&config.home, std::env::args().skip(2).collect()),
        Some("retro-followup") => telemetry::retro_followup(&config.home, std::env::args().skip(2).collect()),
        Some("retro-repeats") => telemetry::retro_repeats(&config.home, std::env::args().skip(2).collect()),
        Some("runtime-breakdown") => telemetry::runtime_breakdown(&config.home, std::env::args().skip(2).collect()),
        Some("dream-sessions") => telemetry::dream_sessions(&config.home, std::env::args().skip(2).collect()),
        Some("retro-clarity") => telemetry::retro_clarity(&config.home, std::env::args().skip(2).collect()),
        Some("release-stats") => telemetry::release_stats(&config.home, std::env::args().skip(2).collect()),
        Some("canary-stats") => telemetry::canary_stats(&config.home, std::env::args().skip(2).collect()),
        Some("crm-turn-stats") => telemetry::crm_turn_stats(&config.home, std::env::args().skip(2).collect()),
        Some("crm-costly-turns") => telemetry::crm_costly_turns(&config.home, std::env::args().skip(2).collect()),
        Some("risk-events") => telemetry::risk_events(&config.home, std::env::args().skip(2).collect()),
        Some("wait-stats") => telemetry::wait_stats(&config.home, std::env::args().skip(2).collect()),
        Some("cache-trial") => telemetry::cache_trial_cli(&config.home, std::env::args().skip(2).collect()),
        Some("task-stall") => telemetry::task_stall_cli(&config.home, std::env::args().skip(2).collect()),
        Some("vector-db-stats") => telemetry::vector_db_stats(&config.home),
        Some("stream-stats") => telemetry::stream_stats(&config.home, std::env::args().skip(2).collect()),
        Some("search-quality") => telemetry::search_quality_cli(&config.home, std::env::args().skip(2).collect()),
        Some("search-score") => telemetry::search_score_cli(&config.home, std::env::args().skip(2).collect()),
        Some("retro-series") => telemetry::retro_series_cli(&config.home, std::env::args().skip(2).collect()),
        Some("search-retro-series") => telemetry::retro_series_cli(&config.home, std::iter::once("search".to_string()).chain(std::env::args().skip(2)).collect()),
        Some("search-pace") => telemetry::search_pace_cli(&config.home, std::env::args().skip(2).collect()),
        Some("mic-sessions") => telemetry::mic_sessions_now(&config.home),
        Some("attribute") => cause_tree::attribute_cli(&config.home, std::env::args().nth(2)),
        Some("ledger") => cause_tree::ledger::run(&config.home, std::env::args().skip(2).collect()),
        Some("monitor-ack") => monitor::ack(&config.home, std::env::args().nth(2)),
        Some("retro-recall") => retro_recall::run(&config.home, std::env::args().skip(2).collect()),
        Some("retro-overdue") => retro_overdue::run(&config.home),
        Some("retro-scorecard") => retro_scorecard::run(&config.home, std::env::args().skip(2).collect()),
        Some("insight-consolidate") => retro_consolidate::run(&config.home, std::env::args().skip(2).collect()),
        Some("retro-resources") => resource_usage::run(&config.home, std::env::args().skip(2).collect()),
        Some("log-recall") => log_recall::run(&config.home, std::env::args().skip(2).collect()),
        Some("log-verify") => log_recall::verify_cli(&config.home, std::env::args().skip(2).collect()),
        Some("report-check") => reporting::check(&config.home),
        Some("report-ack") => reporting::ack(&config.home,std::env::args().nth(2)),
        Some("retention") => {
            let dry_run = std::env::args().any(|arg| arg == "--dry-run");
            println!("{}", retention::maintain(&config.home, SystemTime::now(), dry_run)?);
            Ok(())
        }
        Some(other) => anyhow::bail!("unknown command {other}; use run, status, inspect or sweep"),
    }
}

impl Config {
    fn from_env() -> Result<Self> {
        let number = |name: &str, default: f64| -> Result<f64> {
            std::env::var(name).map_or(Ok(default), |value| value.parse().with_context(|| format!("{name}={value}")))
        };
        let list = |name: &str, default: &[&str]| -> Vec<String> {
            std::env::var(name).map_or_else(
                |_| default.iter().map(|item| item.to_string()).collect(),
                |value| value.split(',').map(str::trim).filter(|item| !item.is_empty()).map(String::from).collect(),
            )
        };
        let home = std::env::var_os("HOME").map(PathBuf::from).context("HOME")?;
        Ok(Self {
            policy: Policy {
                // SAFETY: getuid has no preconditions and cannot fail.
                uid: unsafe { libc::getuid() },
                threshold: number("THRESHOLD_PCT", 90.0)? / 100.0,
                window_secs: number("WINDOW_SECS", 300.0)?,
                names: list("NAMES", DEFAULT_NAMES),
                build_markers: list("BUILD_MARKERS", DEFAULT_BUILD_MARKERS),
                shared_markers: list("SHARED_MARKERS", DEFAULT_SHARED_SERVICE_MARKERS),
            },
            grace_secs: number("GRACE_SECS", 30.0)?,
            interval_secs: number("INTERVAL_SECS", 15.0)?,
            dry_run: std::env::var("DRY_RUN").is_ok_and(|value| value == "1"),
            status_path: home.join(".local/state/cpu-watchdog/status.json"),
            sweep_secs: number("SWEEP_SECS", 600.0)?,
            home,
            idle_hours: number("IDLE_HOURS", 6.0)?,
        })
    }

    fn sweep(&self, dry_run: bool, sample_secs: f64) -> sweep::Sweep {
        sweep::Sweep {
            uid: self.policy.uid,
            home: self.home.clone(),
            idle_hours: self.idle_hours,
            dry_run,
            sample_secs,
            hot_pct: 50.0,
        }
    }
}

fn run(config: &Config) -> Result<()> {
    // Returns only when already bundled or adoption failed; success re-executes from the bundle.
    if let Err(error) = BUNDLE.adopt() {
        log(&format!("app bundle adoption failed, running bare: {error}"));
    }
    if let Some(parent) = config.status_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _lock = run_lock::acquire(config.status_path.parent().context("state directory")?)?;
    log(&format!(
        "start threshold={}% window={}s grace={}s names={} dryRun={}",
        config.policy.threshold * 100.0,
        config.policy.window_secs,
        config.grace_secs,
        config.policy.names.join("|"),
        config.dry_run
    ));
    let mut tracker = Tracker::default();
    let mut pending: HashMap<i32, Pending> = HashMap::new();
    let mut reported: HashSet<(i32, i64)> = HashSet::new();
    let mut next_sweep = unix_now();
    let mut monitor = monitor::Monitor::default();
    stall_guard::report_pending(&config.home);
    stall_guard::guard_resident(&config.home)?;
    loop {
        stall_guard::beat(stall_guard::Stage::Rotate);
        if let Err(error) = retention::rotate(&config.home) {
            log(&format!("log rotation failed: {error:#}"));
        }
        stall_guard::beat(stall_guard::Stage::Tick);
        stall_guard::drill::take(&config.home);
        if let Err(error) = tick(config, &mut tracker, &mut pending, &mut reported, &mut monitor) {
            log(&format!("tick failed: {error:#}"));
        }
        if unix_now() >= next_sweep {
            next_sweep = unix_now() + config.sweep_secs;
            stall_guard::beat(stall_guard::Stage::Sweep);
            match config.sweep(config.dry_run, 0.0).run(unix_now) {
                Ok(report) => log_sweep(&report),
                Err(error) => log(&format!("sweep failed: {error:#}")),
            }
        }
        stall_guard::beat(stall_guard::Stage::Sleep);
        let now = unix_now();
        let next_deadline = pending.values().map(|item| item.deadline - now).fold(config.interval_secs, f64::min);
        std::thread::sleep(Duration::from_secs_f64(next_deadline.clamp(1.0, config.interval_secs)));
    }
}

fn tick(
    config: &Config,
    tracker: &mut Tracker,
    pending: &mut HashMap<i32, Pending>,
    reported: &mut HashSet<(i32, i64)>,
    monitor: &mut monitor::Monitor,
) -> Result<()> {
    let (now, snapshot) = procs::snapshot(unix_now)?;
    if let Err(error) = monitor.observe(&config.home, now, &snapshot, unix_now) {
        log(&format!("host monitor failed: {error:#}"));
    }
    stall_guard::step("policy");
    let alive = |pid: i32, start: i64| snapshot.iter().any(|proc| proc.pid == pid && (proc.start - start).abs() <= 2);

    pending.retain(|_, item| {
        if !alive(item.pid, item.start) {
            report(item, "exited after SIGTERM", "已终止");
            return false;
        }
        if now < item.deadline {
            return true;
        }
        if !procs::still_same_process(item.pid, item.start, unix_now) {
            log(&format!("skip SIGKILL {} pid={}: pid exited or was reused", item.name, item.pid));
            return false;
        }
        signal(item.pid, libc::SIGKILL);
        report(item, "still alive after grace; sent SIGKILL", "已强制结束");
        false
    });

    let readings = tracker.observe(now, &snapshot, &config.policy);
    for reading in readings.iter().filter(|reading| reading.runaway) {
        if pending.contains_key(&reading.pid) {
            continue;
        }
        let summary = format!(
            "{} pid={} usage={:.0}% over {:.0}s args={}",
            reading.name,
            reading.pid,
            reading.usage * 100.0,
            reading.observed_secs,
            truncate(&reading.args, 200)
        );
        if config.dry_run {
            if reported.insert((reading.pid, reading.start)) {
                log(&format!("dry run: would terminate {summary}"));
            }
            continue;
        }
        if !procs::still_same_process(reading.pid, reading.start, unix_now) {
            log(&format!("skip SIGTERM {summary}: pid exited or was reused"));
            continue;
        }
        log(&format!("SIGTERM {summary}"));
        signal(reading.pid, libc::SIGTERM);
        tracker.forget(reading.pid);
        pending.insert(
            reading.pid,
            Pending {
                pid: reading.pid,
                start: reading.start,
                name: reading.name.clone(),
                usage: reading.usage,
                deadline: now + config.grace_secs,
            },
        );
    }

    let mut watching: Vec<&Reading> =
        readings.iter().filter(|reading| reading.usage >= config.policy.threshold / 2.0).collect();
    watching.sort_by(|left, right| right.usage.total_cmp(&left.usage));
    let status = Status {
        at: local_time(now as i64),
        dry_run: config.dry_run,
        threshold_pct: config.policy.threshold * 100.0,
        window_secs: config.policy.window_secs,
        watching,
        pending_kill: pending.values().collect(),
    };
    let temporary = config.status_path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_string_pretty(&status)?)?;
    std::fs::rename(&temporary, &config.status_path)?;
    Ok(())
}

fn log_sweep(report: &sweep::Report) {
    for item in &report.closed {
        let verb = if report.dry_run { "dry run: would close" } else { "closed" };
        log(&format!("sweep {verb} {} pid={}: {}", item.name, item.pid, item.reason));
    }
}

fn report(item: &Pending, outcome: &str, label: &str) {
    log(&format!("{} pid={} {outcome}", item.name, item.pid));
    let message = format!(
        "{} (pid {}) 5 分钟平均占用 {:.0}% 单核，{label}，资源已释放",
        item.name,
        item.pid,
        item.usage * 100.0
    );
    let script = format!(
        "display notification \"{}\" with title \"CPU 看门狗\"",
        message.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let _ = std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).output();
}

fn signal(pid: i32, signal: libc::c_int) {
    // SAFETY: kill only reads its arguments; a stale pid yields ESRCH, which is fine here.
    if unsafe { libc::kill(pid, signal) } != 0 {
        log(&format!("kill({pid}, {signal}) failed: {}", std::io::Error::last_os_error()));
    }
}

fn truncate(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((index, _)) => format!("{}…", &text[..index]),
        None => text.to_string(),
    }
}

fn unix_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

fn log(message: &str) {
    println!("{} {message}", local_time(unix_now() as i64));
}

fn local_time(seconds: i64) -> String {
    let time = seconds as libc::time_t;
    // SAFETY: localtime_r writes into the zeroed tm we own; strftime is bounded by the buffer size.
    unsafe {
        let mut parts: libc::tm = std::mem::zeroed();
        libc::localtime_r(&time, &mut parts);
        let mut buffer = [0u8; 32];
        let length = libc::strftime(buffer.as_mut_ptr().cast(), buffer.len(), c"%Y-%m-%dT%H:%M:%S%z".as_ptr(), &parts);
        String::from_utf8_lossy(&buffer[..length]).into_owned()
    }
}
