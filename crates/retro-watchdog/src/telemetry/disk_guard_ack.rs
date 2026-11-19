//! Cooldown after a disk-low acknowledgement (founder, 2026-11-01): incident 814592704184004271751
//! opened 9 min after 818320259602176473893 was acknowledged, while that session was still running the
//! cleanup the founder had just approved from its ask list. For ACK_COOLDOWN_SECS after an ack, S0 keeps
//! running but no new disk session opens; a disk still short after the cooldown escalates as before.
use std::path::Path;

pub(super) const ACK_COOLDOWN_SECS: f64 = 1800.0;

/// True while the newest started dispatch was acknowledged less than ACK_COOLDOWN_SECS before `now_s`.
/// The ack time is the `.ack` file's modification time; a missing file or clock skew never blocks.
pub(super) fn cooling(incidents: &Path, dispatches: &[serde_json::Value], now_s: f64) -> bool {
    let Some(id) = dispatches.iter().rev().find(|d| d["started"] == true).and_then(|d| d["incident"].as_str()) else {
        return false;
    };
    let acked = std::fs::metadata(incidents.join(format!("{id}.ack"))).and_then(|m| m.modified()).ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
    acked.is_some_and(|t| (0.0..ACK_COOLDOWN_SECS).contains(&(now_s - t.as_secs_f64())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn blocks_only_within_the_cooldown_after_the_newest_ack() {
        let home = crate::telemetry::test_home("disk-guard-ack");
        let dir = home.join("incidents");
        std::fs::create_dir_all(&dir).unwrap();
        let dispatches = [json!({"incident": "old", "started": true}), json!({"incident": "new", "started": true}),
            json!({"incident": "failed", "started": false})];
        let now = 2_000_000_000.0;
        assert!(!cooling(&dir, &dispatches, now), "no ack yet");
        let ack = dir.join("new.ack");
        std::fs::write(&ack, "{}").unwrap();
        let set = |secs: f64| std::fs::File::options().write(true).open(&ack).unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs_f64(secs)).unwrap();
        set(now - 60.0);
        assert!(cooling(&dir, &dispatches, now));
        set(now - ACK_COOLDOWN_SECS);
        assert!(!cooling(&dir, &dispatches, now), "cooldown over");
        set(now + 60.0);
        assert!(!cooling(&dir, &dispatches, now), "an ack ahead of the clock never blocks");
        assert!(!cooling(&dir, &[], now));
        std::fs::remove_dir_all(home).unwrap();
    }
}
