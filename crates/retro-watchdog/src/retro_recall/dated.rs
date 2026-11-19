//! The incident's Singapore day(s) appended to a session question, so `local-search` ranks only
//! the history turns of those days (`local-search/docs/dated-queries.md`). A retro counts only
//! in-window hits, and a top-20 over every day buried a 50-minute window under older sessions
//! (search-retro 82450840685001354157 needed six rounds). Founder decision 2026-11-10, report
//! history-recall-audit-20261110. A window inside one day also gets its clock range
//! (`2026-11-10 14:52-15:55`), which narrows the ranking to the window when a turn falls in it.
//! A question that names its own date keeps it; the prefix
//! `all-days:` asks for every day.
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

pub const ALL_DAYS: &str = "all-days:";
const SGT_MS: f64 = 8.0 * 3_600_000.0;
const DAY_MS: f64 = 86_400_000.0;
// local-search expands a range of at most 31 days.
const MAX_SPAN: i64 = 30;

static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{4}(?:-\d{1,2}-\d{1,2}|年\d{1,2}月\d{1,2}日)").unwrap());

/// Days since the epoch of the Singapore day holding `ms`.
pub(crate) fn sgt_day(ms: f64) -> i64 {
    ((ms + SGT_MS) / DAY_MS).floor() as i64
}

/// `YYYY-MM-DD` of a day number (Howard Hinnant's civil-from-days).
pub(crate) fn ymd(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + i64::from(m <= 2))
}

/// The question as sent to `local-search query`.
pub fn scoped(question: &str, window: &Value) -> String {
    let q = question.trim();
    if let Some(rest) = q.strip_prefix(ALL_DAYS) {
        return rest.trim().to_string();
    }
    let (Some(from), Some(to)) = (window["from_ms"].as_f64(), window["to_ms"].as_f64()) else { return q.to_string() };
    let (first, last) = (sgt_day(from.min(to)), sgt_day(from.max(to)));
    if DATE.is_match(q) || last - first > MAX_SPAN {
        return q.to_string();
    }
    if first != last {
        return format!("{q} {} to {}", ymd(first), ymd(last));
    }
    // Inside one day the clock range narrows the ranking to the incident window itself.
    let minute = |ms: f64| (ms + SGT_MS).rem_euclid(DAY_MS) / 60_000.0;
    let (start, end) = (minute(from.min(to)).floor() as i64, minute(from.max(to)).ceil() as i64);
    if end <= start || end >= 24 * 60 {
        return format!("{q} {}", ymd(first));
    }
    format!("{q} {} {:02}:{:02}-{:02}:{:02}", ymd(first), start / 60, start % 60, end / 60, end % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_window_days_are_appended_in_singapore_time() {
        // 2026-10-28T17:00Z is 2026-10-29 01:00 SGT.
        let one = json!({"from_ms": 1_793_206_800_000.0, "to_ms": 1_793_210_400_000.0});
        assert_eq!(scoped("Repeated report edits", &one), "Repeated report edits 2026-10-29 01:00-02:00");
        // Seconds round outwards; a window that ends at midnight keeps the day alone.
        let odd = json!({"from_ms": 1_793_206_830_000.0, "to_ms": 1_793_210_430_000.0});
        assert_eq!(scoped("q", &odd), "q 2026-10-29 01:00-02:01");
        let late = json!({"from_ms": 1_793_286_000_000.0, "to_ms": 1_793_289_599_000.0});
        assert_eq!(scoped("q", &late), "q 2026-10-29");
        let two = json!({"from_ms": 1_793_149_220_000.0, "to_ms": 1_793_206_800_000.0});
        assert_eq!(scoped("Release retries", &two), "Release retries 2026-10-28 to 2026-10-29");
    }

    #[test]
    fn a_dated_question_an_all_days_question_and_a_missing_or_long_window_are_left_alone() {
        let one = json!({"from_ms": 1_793_206_800_000.0, "to_ms": 1_793_210_400_000.0});
        assert_eq!(scoped("beta 回迁 2026-11-05", &one), "beta 回迁 2026-11-05");
        assert_eq!(scoped("2026年11月5日 发布", &one), "2026年11月5日 发布");
        assert_eq!(scoped("all-days: same failure in earlier weeks", &one), "same failure in earlier weeks");
        assert_eq!(scoped("no window", &json!(null)), "no window");
        let long = json!({"from_ms": 1_790_000_000_000.0, "to_ms": 1_793_210_400_000.0});
        assert_eq!(scoped("long window", &long), "long window");
    }
}
