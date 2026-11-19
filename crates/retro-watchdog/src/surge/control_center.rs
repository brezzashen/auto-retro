//! ControlCenter check during a surge: CPU over the last minute, RSS, a Mach port census taken
//! now (the existing census evaluation restarts it over its port limit and opens the port-leak
//! incident), and WindowServer render-fence stalls over the last ten minutes. A ControlCenter
//! holding HOT_ONE_CORE of a core is restarted through the same rate-limited restart.
use super::history::History;
use crate::{monitor::root, procs::Proc};
use serde_json::{json, Value};
use std::path::Path;

const HOT_ONE_CORE: f64 = 0.8;
const RENDER_WINDOW_MS: f64 = 600_000.0;

pub fn check(home: &Path, now: f64, processes: &[Proc], history: &History, incident: &str) -> Value {
    let Some(cc) = processes.iter().filter(|p| p.name == "ControlCenter").min_by_key(|p| p.start) else {
        return json!({"abnormal": ["ControlCenter not running"], "render_10min": render(home, now)});
    };
    let usage = history.usage(cc, now, 60.0);
    let mut abnormal = Vec::new();
    let census = crate::telemetry::census_for_surge(home, now, processes);
    let ports = census.as_ref().ok().and_then(|state| state["controlcenter"]["ports"].as_u64());
    let limit = census.as_ref().ok().and_then(|state| state["controlcenter"]["limit"].as_u64());
    if let (Some(ports), Some(limit)) = (ports, limit) {
        if ports >= limit {
            abnormal.push(format!("{ports} Mach ports ≥ limit {limit}"));
        }
    }
    let latest = crate::telemetry::read_state(&root(home).join("controlcenter-restart.json"));
    let mut restart = latest.filter(|r| r["at_ms"].as_f64().is_some_and(|at| at >= now * 1000.0));
    if usage.is_some_and(|u| u >= HOT_ONE_CORE) {
        abnormal.push(format!("CPU {:.0}% of one core over 60 s", usage.unwrap_or(0.0) * 100.0));
        if restart.is_none() {
            restart = Some(crate::telemetry::restart_controlcenter(home, now, cc.pid, cc.start, ports.unwrap_or(0), incident)
                .unwrap_or_else(|error| json!({"outcome": format!("failed: {error:#}")})));
        }
    }
    // Render stalls are WindowServer-side evidence (a saturated host stalls them too), so they are
    // reported beside ControlCenter but never make it abnormal on their own.
    let render = render(home, now);
    json!({"pid": cc.pid, "uptime_s": now as i64 - cc.start, "rss_mib": cc.rss_kib / 1024,
        "cpu_one_core_60s": usage, "ports": ports, "port_limit": limit,
        "census_error": census.err().map(|error| format!("{error:#}")),
        "restart": restart, "render_10min": render, "abnormal": abnormal})
}

/// Stall totals from today's `render-probe/<utc-day-ms>.ndjson` within RENDER_WINDOW_MS.
fn render(home: &Path, now: f64) -> Value {
    let day = ((now * 1000.0) / 86_400_000.0).floor() * 86_400_000.0;
    let path = root(home).join("render-probe").join(format!("{}.ndjson", day as u64));
    let Some(lines) = super::launchd::tail_lines(&path, 512 * 1024) else {
        return json!({"error": "render probe log unavailable"});
    };
    let (mut ticks, mut stalls, mut long, mut stalled_ms, mut worst) = (0, 0, 0, 0.0, 0.0f64);
    for tick in lines.iter().filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|tick| tick["at_ms"].as_f64().is_some_and(|at| now * 1000.0 - at <= RENDER_WINDOW_MS)) {
        ticks += 1;
        stalls += tick["stall_count"].as_u64().unwrap_or(0);
        stalled_ms += tick["stalled_ms"].as_f64().unwrap_or(0.0).max(0.0);
        let max = tick["fence_ms"]["max"].as_f64().unwrap_or(0.0);
        worst = worst.max(max);
        long += u64::from(max >= 1000.0);
    }
    json!({"ticks": ticks, "stalls_ge_250ms": stalls, "stalls_ge_1000ms": long, "stalled_ms": stalled_ms, "worst_fence_ms": worst})
}
