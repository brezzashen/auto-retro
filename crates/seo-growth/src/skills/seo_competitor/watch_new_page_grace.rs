//! New-page grace for rank findings: a target page first seen in a sitemap less than
//! `INDEX_GRACE_DAYS` days ago has not been crawled and ranked yet, so its `rank:absent:` and
//! `rank:deep:` findings are shown at P2 and never dispatched. On 2026-11-12 at 22:00 eight P1
//! "absent from top 100" words of four pages created that afternoon reached the keyword tab.
//! A target page no sitemap lists yet (its target row ships with the page commit, before Canary)
//! is held from the first run that sees it, for the same number of days, so a target row whose page
//! never ships still comes back as P1: on 2026-11-15 at 15:17 eight words of `/managing-up` and
//! `/career-sponsor`, committed at 15:11 and still on their way to Canary, reached the keyword tab.

use super::watch_analyze::{Finding, INDEX_GRACE_DAYS};
use super::watch_config::{TargetKeyword, WatchState};
use chrono::{DateTime, Duration, FixedOffset};

/// Key suffix of a finding held for a new page: the plain key stays unhandled, so it is new work after the grace.
const NEW: &str = ":new";

/// Holds the P1 absent and deep rank findings of target pages younger than the grace or not yet listed.
pub fn apply(findings: &mut [Finding], targets: &[TargetKeyword], site: &str, state: &mut WatchState, now: DateTime<FixedOffset>) {
    let listed: Vec<String> = targets.iter().map(|t| format!("{site}{}", t.page)).filter(|url| state.first_seen.contains_key(url)).collect();
    state.unlisted_since.retain(|url, _| !listed.contains(url));
    for finding in findings.iter_mut().filter(|f| f.severity == "P1") {
        let Some(word) = finding.key.strip_prefix("rank:absent:").or_else(|| finding.key.strip_prefix("rank:deep:")) else { continue };
        let Some(target) = targets.iter().find(|t| t.keyword == word) else { continue };
        let url = format!("{site}{}", target.page);
        let (since, note) = match state.first_seen.get(&url) {
            Some(seen) => (seen.clone(), format!("{} 上线第 {} 天", target.page, age_days(seen, now))),
            None => {
                let since = state.unlisted_since.entry(url).or_insert_with(|| now.to_rfc3339()).clone();
                let note = format!("{} 尚未进入 sitemap 第 {} 天", target.page, age_days(&since, now));
                (since, note)
            }
        };
        if !DateTime::parse_from_rfc3339(&since).is_ok_and(|t| now - t < Duration::days(INDEX_GRACE_DAYS)) { continue; }
        finding.key.push_str(NEW);
        finding.severity = "P2".into();
        finding.title = format!("{}（{note}，新页 {INDEX_GRACE_DAYS} 天内只报告）", finding.title);
        finding.action = format!("不改文案：新页上线正式站前不请求编入索引，上线后 {INDEX_GRACE_DAYS} 天内只查收录（URL Inspection、请求编入索引、IndexNow）；宽限期满仍无排名或仍未进 sitemap 时本项会重新报出");
    }
}

/// Day number (1-based) of `now` counted from an RFC 3339 start; 1 when the start does not parse.
fn age_days(start: &str, now: DateTime<FixedOffset>) -> i64 {
    DateTime::parse_from_rfc3339(start).map_or(1, |t| (now - t).num_days() + 1)
}

#[cfg(test)]
#[path = "watch_new_page_grace_tests.rs"]
mod tests;
