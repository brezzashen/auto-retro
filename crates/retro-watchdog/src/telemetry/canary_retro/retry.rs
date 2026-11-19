//! A Canary run whose step deliver executed again after a failed attempt. deliver re-runs the step
//! after a disk-guard recovery or an engine drop (`mars-deliver` `deliver.rs` run loop), so the board
//! run succeeds and its last receipt is live while the first attempt's seconds sit in the `start`
//! segment. 20261113T144257Z-canary-96911 (363 s) and 20261113T151338Z-canary-31669 (312 s) were
//! both a `/System/Volumes/Data` disk-guard refusal followed by a retry, and read as an unexplained
//! `other` stall (canary retro 812155716626541884165).

use super::super::release_retro::stats;

/// Lines of the failed attempt's end that name its class, like a board failure tail.
const TAIL_LINES: usize = 40;

/// The failure class of the attempt before the last one, when the run log holds a re-executed step.
pub(crate) fn retried_attempt(log: &str) -> Option<&'static str> {
    // The step's command is the log's first `$ ` line; deliver writes it again for every execution.
    let command = log.lines().find(|line| line.starts_with("$ "))?;
    let mut offset = 0;
    let mut starts = Vec::new();
    for line in log.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']) == command {
            starts.push(offset);
        }
        offset += line.len();
    }
    let last = *starts.get(1..)?.last()?;
    let earlier: Vec<&str> = log[..last].lines().collect();
    let tail = earlier[earlier.len().saturating_sub(TAIL_LINES)..].join("\n");
    Some(stats::classify(&tail))
}

/// Lines deliver writes when a run rides another run's prepare or rollout instead of building.
const RIDES: [&str; 2] = ["riding it instead of preparing", "absorbed by the canary rollout of"];

/// A rider its creator stopped (`deliver stopped by SIGINT`, class `stopped`) while it waited on the
/// run it rode: its seconds are a queue wait on that leader, not a release error.
/// 20261118T113605Z-canary-12852 rode 85513's prepare of e4b5f2eb1 for 1544 s, then its session sent
/// SIGINT because 85513 already carried its commit (canary retro 84571662472070360849).
pub(crate) fn stopped_rider(class: &serde_json::Value, log: &str) -> bool {
    class == "stopped" && RIDES.iter().any(|line| log.contains(line))
}
