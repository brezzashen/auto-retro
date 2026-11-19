//! Mars CRM host leases that stop moving while others wait (2026-10-22: the `mars-crm-verify`
//! holder sat 25+ min on wedged headless pages, the oldest of eight ship-web waiters 70 min).
//! Read-only: a holder past its limit with at least one live waiter opens one `lease_stalled`
//! incident per holding; nothing is signalled or removed. Layout is `buildLease.ts`: `<name>/owner.json`
//! (`holder`, `pid`, `acquiredAt` ms) and `<name>.queue/<ms>-<pid>-<seq>` tickets.
use super::diagnostics::incident;
use serde_json::{json, Value};
use std::path::Path;

const LOCKS: &str = "Mars_agent/.claude/locks";
/// Lease name and the holding time that counts as stalled; build matches its own 45 min TTL.
pub(super) const LEASES: &[(&str, f64)] = &[("mars-crm-verify", 1_800.0), ("mars-crm-build", 2_700.0)];

fn alive(pid: i32) -> bool {
    pid > 0 && (unsafe { libc::kill(pid, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

pub(super) fn observe(home: &Path, now: f64) -> Value {
    let locks = std::env::var("HOME").map(|h| Path::new(&h).join(LOCKS)).unwrap_or_default();
    observe_in(home, &locks, now)
}

pub(super) fn observe_in(home: &Path, locks: &Path, now: f64) -> Value {
    let mut state = serde_json::Map::new();
    for &(name, limit) in LEASES {
        let owner = super::read(&locks.join(name).join("owner.json"));
        let waiters: Vec<f64> = std::fs::read_dir(locks.join(format!("{name}.queue"))).into_iter().flatten().flatten()
            .filter_map(|entry| {
                let file = entry.file_name().to_string_lossy().into_owned();
                let mut parts = file.split('-');
                let at: f64 = parts.next()?.parse().ok()?;
                alive(parts.next()?.parse().ok()?).then_some(at / 1000.0)
            }).collect();
        let oldest_wait = waiters.iter().map(|at| now - at).fold(None, |max: Option<f64>, w| Some(max.map_or(w, |m| m.max(w))));
        let held = owner.as_ref().and_then(|o| o["acquiredAt"].as_f64()).map(|at| now - at / 1000.0);
        let mut error = Value::Null;
        if let (Some(owner), Some(held)) = (&owner, held) {
            if held >= limit && !waiters.is_empty() {
                let key = format!("lease_stalled:{name}:{}", owner["acquiredAt"]);
                if let Err(e) = incident(home, &key, json!({"trigger": "lease_stalled", "lease": name,
                    "holder": owner["holder"], "pid": owner["pid"], "holder_alive": owner["pid"].as_i64().is_some_and(|p| alive(p as i32)),
                    "held_s": held, "waiters": waiters.len(), "oldest_wait_s": oldest_wait, "threshold_s": limit})) {
                    error = json!(format!("{e:#}"));
                }
            }
        }
        state.insert(name.into(), json!({"holder": owner.as_ref().map(|o| o["holder"].clone()), "held_s": held,
            "waiters": waiters.len(), "oldest_wait_s": oldest_wait, "limit_s": limit, "error": error}));
    }
    Value::Object(state)
}
