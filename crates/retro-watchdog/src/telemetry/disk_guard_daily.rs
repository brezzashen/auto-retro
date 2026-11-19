//! Fixed daily S0 (user request, 2026-11-01): with room everywhere, S0 still runs once every DAILY_SECS,
//! so its safe steps (snapshots, old release outputs, aged temp caches, unused images) never pile up.

pub(super) const DAILY_SECS: f64 = 86_400.0;

/// `last_run_ms` is the newest S0 run (0 when none); `since_s` is the first sample after start (0 in a fresh
/// guard that has not sampled). An unmeasured host never triggers.
pub(super) fn due(last_run_ms: f64, since_s: f64, now_s: f64, free: Option<f64>) -> bool {
    let anchor = if last_run_ms > 0.0 { last_run_ms } else { since_s * 1000.0 };
    free.is_some() && anchor > 0.0 && now_s * 1000.0 - anchor >= DAILY_SECS * 1000.0
}
