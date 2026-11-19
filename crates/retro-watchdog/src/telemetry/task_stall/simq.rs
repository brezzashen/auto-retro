//! A blocked call that sits in the simulator lease queue (incident 818084050210556831742: a
//! `cpu-build-gate run -- … simRun --device <udid>` call waited 22 min behind another session's
//! lease and was reported as `build_test`). sim-lease writes one ticket per waiting request under
//! `~/.local/state/sim-lease/queue/<ticket>.json` with the requesting agent session as `owner`; the
//! holder of a named device is `leases/<UDID>.json`. Read only; nothing here changes a lease.
use serde_json::{json, Value};
use std::path::Path;

/// The agent session id a transcript belongs to, the id sim-lease records as `owner`: a Claude
/// lead `<id>.jsonl`, its subagents `<id>/subagents/agent-*.jsonl`, a Codex `rollout-…-<id>.jsonl`.
pub(super) fn session_of(path: &str) -> String {
    let p = Path::new(path);
    let name = |q: Option<&Path>| q.and_then(|q| q.file_name()).and_then(|s| s.to_str()).unwrap_or("").to_string();
    if name(p.parent()) == "subagents" { return name(p.parent().and_then(Path::parent)); }
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(path);
    match stem.strip_prefix("rollout-") {
        Some(rest) if rest.len() >= 36 => rest[rest.len() - 36..].to_string(),
        _ => stem.to_string(),
    }
}

/// The lease queue ticket of `session` whose requesting process still lives, with the holder of
/// the device it names; None when the session is not queued.
pub(super) fn waiting(home: &Path, session: &str, now_s: f64) -> Option<Value> {
    let dir = home.join(".local/state/sim-lease");
    let read = |p: &Path| std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let ticket = std::fs::read_dir(dir.join("queue")).ok()?.flatten().filter_map(|e| read(&e.path()))
        .find(|t| t["owner"] == session && t["pid"].as_i64().is_some_and(|pid| cfg!(test) || super::heal::alive(pid)))?;
    let device = ticket["device"].as_str().map(str::to_ascii_uppercase);
    let holder = device.as_ref().and_then(|d| read(&dir.join("leases").join(format!("{d}.json")))).map(|l| json!({
        "owner": l["owner"], "purpose": l["purpose"],
        "expires_in_s": l["expires"].as_f64().map(|e| (e - now_s).round()),
        "held_by_self": l["owner"] == session}));
    Some(json!({"kind": "sim-lease", "ticket": ticket["ticket"], "device": device, "family": ticket["family"],
        "purpose": ticket["purpose"], "holder": holder, "ended": false}))
}
