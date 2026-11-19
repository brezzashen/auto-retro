//! The local-search sync steps running when a slow engine row ran. Each sync.log / code-sync.log
//! line ends a step (`watchdog sources … 405.9s (read 142.9s, write lock 262.6s) at <ISO>`,
//! `cache history rebuilt in 315.6s at <ISO>`), so its first duration and `at` give the interval.
//! Search-retro 814125901426638775162: four of five slow rows (23:31:16, 23:31:31, 23:38:40 and
//! 00:30:24 SGT) fell inside a sync whose steps ran 10-700x their usual time (watchdog write lock
//! 262.6 s, watchdog cache rebuild 924.1 s, history cache rebuild 103.0 s against 1.3 s), while
//! rusage read every one as `cpu_starved`; the overlap was matched by hand. Read only: the probe
//! takes no sync lock, since holding one, even briefly, makes a starting sync skip its corpus.
use serde_json::{json, Value};
use std::path::Path;

/// Steps shorter than this are not worth naming beside a slow row.
const MIN_STEP_S: f64 = 5.0;

/// Steps of `sync.log` and `code-sync.log` under `home` that overlap `[at_ms - ms, at_ms]`.
pub(super) fn steps(home: &Path, at_ms: f64, ms: f64) -> Value {
    let dir = home.join("Library/Logs/local-search");
    let mut out: Vec<Value> = Vec::new();
    for log in ["sync.log", "code-sync.log"] {
        let Ok(text) = std::fs::read_to_string(dir.join(log)) else { continue };
        out.extend(overlapping(&text, at_ms - ms, at_ms));
    }
    json!(out)
}

/// Lines of one log whose step interval overlaps `[from_ms, to_ms]`, as `{step, from_ms, s}`.
fn overlapping(text: &str, from_ms: f64, to_ms: f64) -> Vec<Value> {
    text.lines()
        .rev()
        .filter_map(|line| step_of(line).map(|(name, end, s)| (name, end, s)))
        .take_while(|(_, end, _)| *end >= from_ms - 3.6e6)
        .filter(|(_, end, s)| *s >= MIN_STEP_S && *end >= from_ms && end - s * 1e3 <= to_ms)
        .map(|(name, end, s)| json!({"step": name, "from_ms": (end - s * 1e3).round(), "s": s}))
        .collect()
}

/// `(step name, end ms, seconds)` of one log line: its first two words, the first `<n>s` token and
/// the timestamp after `at`.
fn step_of(line: &str) -> Option<(String, f64, f64)> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let s = words.iter().find_map(|w| w.strip_suffix('s').filter(|n| n.contains('.')).and_then(|n| n.parse::<f64>().ok()))?;
    let at = words.iter().position(|w| *w == "at").and_then(|i| words.get(i + 1))?;
    let end = crate::cause_tree::attribute::parse_time(at)?;
    Some((format!("{} {}", words.first()?, words.get(1)?), end, s))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "code     sources  12519  changed      0  removed    0  chunks written       0  kept       0  deleted      0  100.0s (read 100.0s, write lock 0.0s)  at 2026-11-05T15:18:09Z  wrote 0.00 GB (disk 0.00 GB)
cache    history  rebuilt in 315.6s  at 2026-11-05T15:24:10Z  wrote 0.00 GB (disk 0.00 GB)
docs     sources   1214  changed      0  removed    0  chunks written       0  kept       0  deleted      0  5.2s (read 5.2s, write lock 0.0s)  at 2026-11-05T15:26:12Z  wrote 0.00 GB (disk 0.00 GB)
watchdog sources 202318  changed    340  removed   64  chunks written   2329  kept 219782  deleted    337  405.9s (read 142.9s, write lock 262.6s)  at 2026-11-05T15:32:58Z  wrote 0.00 GB (disk 0.00 GB)
cache    watchdog rebuilt in 924.1s  at 2026-11-05T15:48:22Z  wrote 0.00 GB (disk 0.07 GB)
sync     another sync holds /x.sync.history.lock; history skipped";

    #[test]
    fn a_slow_row_inside_a_long_step_names_it() {
        // 23:38:40 SGT, 15.4 s: inside the 924 s watchdog cache rebuild.
        let at = crate::cause_tree::attribute::parse_time("2026-11-05T15:38:40.382Z").unwrap();
        let found = overlapping(LOG, at - 15_423.0, at);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["step"], "cache watchdog");
        assert_eq!(found[0]["s"], 924.1);
        // 23:31:16 SGT: the watchdog sync step (read, then write lock) was still running.
        let at = crate::cause_tree::attribute::parse_time("2026-11-05T15:31:16.490Z").unwrap();
        assert_eq!(overlapping(LOG, at - 1666.0, at)[0]["step"], "watchdog sources");
        // Before the sync started, and short steps, name nothing.
        let at = crate::cause_tree::attribute::parse_time("2026-11-05T15:15:20.142Z").unwrap();
        assert!(overlapping(LOG, at - 1025.0, at).is_empty());
        assert!(step_of("sync     another sync holds /x.sync.history.lock; history skipped").is_none());
    }
}
