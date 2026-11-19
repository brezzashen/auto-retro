//! Early warning for a WindowServer Mach port leak, below the hard limit. WindowServer cannot be
//! restarted, so the hard limit alone only opens a session once the leak is already large. On
//! 2026-11-05 WindowServer reached 44,800 ports with ControlCenter holding only 232 of them;
//! 34,052 of its rights were `dead-name` (senders of clients that had exited) and they grew about
//! 1,600 an hour. The founder chose (option A) to notify once at `notify_at` ports and to open the
//! port-leak session when dead names grow faster than `dead_per_hour` over at least WINDOW_MIN_MS.
use super::{Count, Target};
use crate::telemetry::diagnostics::incident;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

pub struct Early {
    pub notify_at: u64,
    pub dead_per_hour: f64,
}

/// Samples older than this leave the rate window.
const WINDOW_MAX_MS: f64 = 75.0 * 60_000.0;
/// A rate needs at least this span, so one burst between two 5-minute samples does not fire.
const WINDOW_MIN_MS: f64 = 50.0 * 60_000.0;

/// The dead-name samples of this process instance carried over from the previous census.
pub(super) fn window(previous: &Value, pid: i32, start: i64, now_ms: f64, dead: u64) -> Vec<[f64; 2]> {
    let same = previous["pid"] == json!(pid) && previous["process_start"] == json!(start);
    let mut samples: Vec<[f64; 2]> = if same {
        previous["dead_window"].as_array().into_iter().flatten()
            .filter_map(|s| Some([s[0].as_f64()?, s[1].as_f64()?]))
            .filter(|s| now_ms - s[0] <= WINDOW_MAX_MS).collect()
    } else {
        Vec::new()
    };
    samples.push([now_ms, dead as f64]);
    samples
}

/// Dead names gained per hour across the window, once it spans WINDOW_MIN_MS.
pub(super) fn rate(samples: &[[f64; 2]]) -> Option<f64> {
    let (first, last) = (samples.first()?, samples.last()?);
    let span = last[0] - first[0];
    (span >= WINDOW_MIN_MS).then(|| (last[1] - first[1]) / (span / 3_600_000.0))
}

pub(super) struct Instance<'a> {
    pub target: &'a Target,
    pub pid: i32,
    pub start: i64,
    pub count: &'a Count,
    pub peers: &'a [Value],
    pub previous: &'a Value,
}

/// Runs both early checks; returns the fields to keep in the census entry and any dispatch error.
pub(super) fn check(home: &Path, now: f64, at: &Instance, samples: &[[f64; 2]]) -> Result<(Value, Option<String>)> {
    let Some(early) = &at.target.early else { return Ok((Value::Null, None)) };
    let same = at.previous["pid"] == json!(at.pid) && at.previous["process_start"] == json!(at.start);
    let mut notified = same && at.previous["early"]["notified"] == true;
    let mut error = None;
    if at.count.total >= early.notify_at && !notified {
        notified = true;
        if !cfg!(test) {
            crate::dispatch::notify("看门狗：WindowServer 端口偏高", &format!(
                "{} 个端口（提醒线 {}，上限 {}），其中 dead-name {} 个", at.count.total, early.notify_at, at.target.limit, at.count.dead_names));
        }
    }
    let per_hour = rate(samples);
    let mut growth = same.then(|| at.previous["early"]["growth_incident"].clone()).filter(|v| !v.is_null());
    if growth.is_none() && per_hour.is_some_and(|r| r >= early.dead_per_hour) && at.count.total < at.target.limit {
        let id = incident(home, &format!("{}_port_growth:{}:{}", at.target.key, at.pid, at.start), json!({
            "trigger": format!("{}_port_growth", at.target.key), "process": at.target.name, "pid": at.pid,
            "process_start": at.start, "uptime_s": now as i64 - at.start, "ports": at.count.total,
            "dead_names": at.count.dead_names, "dead_names_per_hour": per_hour, "dead_window": samples,
            "top_peers": at.peers, "threshold": {"dead_names_per_hour": early.dead_per_hour,
                "notify_at": early.notify_at, "limit": at.target.limit},
        }))?;
        if let Err(e) = crate::dispatch::port_leak(home, now, &id, at.target.name) {
            error = Some(format!("dispatch {id}: {e:#}"));
        }
        growth = Some(json!(id));
    }
    Ok((json!({"notify_at": early.notify_at, "notified": notified, "dead_per_hour_limit": early.dead_per_hour,
        "dead_per_hour": per_hour, "growth_incident": growth}), error))
}
