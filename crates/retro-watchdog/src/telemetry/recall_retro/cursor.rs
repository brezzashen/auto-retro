//! Where the window resumes after `queries.jsonl` was replaced or shrank.
//!
//! Incident 817029181313800309049 (2026-11-07): the local-search directory moved from
//! /Volumes/MarsBuild back to the internal disk. The copy of `queries.jsonl` was taken at 10:43:16
//! and swapped in about 10:51, so the new file was 19.5 KB shorter than the sampled offset. The old
//! rule reset the offset to 0, recounted all 28414 lines since 2026-10-24 as one window and opened
//! a retro of 7564 "recalls", of which only ~300 were new. The cursor now resumes after the last
//! line it already counted, found by `ts_ms`, so a rotation, copy or restore never replays history.
use serde_json::Value;

/// The `ts_ms` of one telemetry line, 0 when it has none.
pub(super) fn ts_of(line: &[u8]) -> f64 {
    serde_json::from_slice::<Value>(line).ok().and_then(|v| v["ts_ms"].as_f64()).unwrap_or(0.0)
}

/// Byte offset of the first complete line whose `ts_ms` is at least `from_ms` (or above it when
/// `strict`), else `end`, the end of the complete lines. Lines without a timestamp are skipped.
pub(super) fn first_at(bytes: &[u8], end: usize, from_ms: f64, strict: bool) -> u64 {
    let mut at = 0usize;
    for line in bytes[..end].split(|b| *b == b'\n') {
        let ts = ts_of(line);
        if ts > 0.0 && (ts > from_ms || (!strict && ts >= from_ms)) {
            return at as u64;
        }
        at += line.len() + 1;
    }
    end as u64
}

/// (offset, window_start) in a replaced or shorter log: after the last counted line and at the
/// window's first line. Without a recorded timestamp both go to the end, losing at most one
/// partial window instead of replaying the whole log.
pub(super) fn resume(bytes: &[u8], last_ms: f64, window_from_ms: f64) -> (u64, u64) {
    let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |e| e + 1);
    if last_ms <= 0.0 {
        return (end as u64, end as u64);
    }
    let offset = first_at(bytes, end, last_ms, true);
    let start = if window_from_ms > 0.0 { first_at(bytes, end, window_from_ms, false).min(offset) } else { offset };
    (offset, start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(ts: &[u64]) -> Vec<u8> {
        ts.iter().map(|t| format!("{{\"sub\":\"query\",\"ts_ms\":{t}}}\n")).collect::<String>().into_bytes()
    }

    #[test]
    fn resumes_after_the_last_counted_line_and_at_the_window_start() {
        let bytes = log(&[10, 20, 30, 40, 50]);
        let line = bytes.len() as u64 / 5;
        assert_eq!(resume(&bytes, 30.0, 20.0), (3 * line, line));
        assert_eq!(resume(&bytes, 50.0, 0.0), (5 * line, 5 * line), "nothing new: both at the end");
        assert_eq!(resume(&bytes, 0.0, 0.0), (5 * line, 5 * line), "no timestamp recorded: skip to the end");
    }

    #[test]
    fn a_partial_last_line_and_untimed_lines_are_not_resume_points() {
        let mut bytes = b"not json\n".to_vec();
        bytes.extend(log(&[10, 20]));
        bytes.extend(b"{\"ts_ms\":30");
        let (offset, start) = resume(&bytes, 10.0, 10.0);
        assert_eq!(&bytes[offset as usize..offset as usize + 10], b"{\"sub\":\"qu");
        assert_eq!(start, 9, "window starts at the ts 10 line after the untimed one");
        assert_eq!(resume(&bytes, 20.0, 0.0).0, bytes.len() as u64 - 11);
    }
}
