//! Reindex follow-ups: pages whose indexing a session requested after a fix, one JSON object per line in
//! `<state>/reindex-followups.jsonl` (`{"url","requestedAt","dueBy","reason","state"?}`, RFC 3339 times). Each run
//! inspects the open follow-ups not checked in the last day ahead of the sitemap rotation, and a follow-up still
//! not indexed after `dueBy` becomes a finding. A follow-up closes once URL Inspection reports it indexed.

use super::watch_analyze::Finding;
use super::watch_config::WatchState;
use chrono::{DateTime, Duration, FixedOffset};
use serde::Deserialize;
use std::collections::BTreeSet;

const INDEXED: &str = "Submitted and indexed";
const RECHECK_HOURS: i64 = 24;

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Followup {
    pub url: String,
    pub requested_at: String,
    pub due_by: String,
    #[serde(default)]
    pub reason: String,
    /// `unsent` (the request never went through; `watch_reindex_resend` re-sends it) or `skipped`
    /// (deliberately not requested); absent once the request went through.
    #[serde(default)]
    pub state: Option<String>,
}

pub fn read(state_dir: &str) -> Vec<Followup> {
    let text = std::fs::read_to_string(format!("{state_dir}/reindex-followups.jsonl")).unwrap_or_default();
    text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
}

fn time(text: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(text).ok()
}

/// Open follow-ups: not yet reported indexed by an inspection made after the request.
fn open<'a>(followups: &'a [Followup], state: &WatchState) -> impl Iterator<Item = &'a Followup> {
    followups.iter().filter(|f| {
        let Some(seen) = state.inspections.get(&f.url) else { return true };
        let after_request = matches!((time(&seen.checked_at), time(&f.requested_at)), (Some(c), Some(r)) if c > r);
        !(after_request && seen.coverage == INDEXED)
    })
}

/// Up to `limit` open follow-ups not inspected since the request or not in the last day, to inspect this run.
pub fn due_for_inspection(followups: &[Followup], state: &WatchState, now: DateTime<FixedOffset>, limit: usize) -> Vec<String> {
    open(followups, state)
        .filter(|f| {
            let checked = state.inspections.get(&f.url).and_then(|i| time(&i.checked_at));
            checked.is_none_or(|at| time(&f.requested_at).is_some_and(|r| at <= r) || now - at >= Duration::hours(RECHECK_HOURS))
        })
        .map(|f| f.url.clone())
        .take(limit)
        .collect()
}

/// URLs of open follow-ups still inside their window: Google has been asked and the answer is pending,
/// so the unindexed and canonical findings for them stay quiet until `dueBy`.
pub fn waiting(followups: &[Followup], state: &WatchState, now: DateTime<FixedOffset>) -> BTreeSet<String> {
    open(followups, state).filter(|f| time(&f.due_by).is_some_and(|due| now <= due)).map(|f| f.url.clone()).collect()
}

/// Open follow-ups past `dueBy`.
pub fn findings(followups: &[Followup], state: &WatchState, now: DateTime<FixedOffset>) -> Vec<Finding> {
    open(followups, state).filter(|f| time(&f.due_by).is_some_and(|due| now > due)).map(|f| {
        let coverage = state.inspections.get(&f.url).map_or("未检查", |i| i.coverage.as_str());
        Finding {
            key: format!("reindex:{}", f.url),
            severity: "P1".into(),
            title: format!("{} 重新编入索引已过期限仍未收录（{coverage}；{}）", f.url, f.reason),
            action: "对比该页与 Google 选定规范页的独有正文，补充正文后再次请求编入索引".into(),
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::seo_competitor::watch_config::Inspection;

    fn at(text: &str) -> DateTime<FixedOffset> { DateTime::parse_from_rfc3339(text).unwrap() }

    fn followup(url: &str) -> Followup {
        Followup { url: url.into(), requested_at: "2026-11-09T18:00:00+08:00".into(), due_by: "2026-11-16T18:00:00+08:00".into(), reason: "duplicate".into(), state: None }
    }

    fn inspected(state: &mut WatchState, url: &str, coverage: &str, checked_at: &str) {
        state.inspections.insert(url.into(), Inspection { coverage: coverage.into(), checked_at: checked_at.into(), ..Default::default() });
    }

    #[test]
    fn inspects_open_followups_once_a_day_and_closes_indexed_ones() {
        let list = [followup("https://example.com/a"), followup("https://example.com/b"), followup("https://example.com/c")];
        let mut state = WatchState::default();
        inspected(&mut state, "https://example.com/a", "Duplicate, Google chose different canonical than user", "2026-11-09T12:00:00+08:00");
        inspected(&mut state, "https://example.com/b", INDEXED, "2026-11-10T09:00:00+08:00");
        inspected(&mut state, "https://example.com/c", "Duplicate, Google chose different canonical than user", "2026-11-10T09:00:00+08:00");
        let due = due_for_inspection(&list, &state, at("2026-11-10T10:00:00+08:00"), 6);
        assert_eq!(due, ["https://example.com/a"], "b is indexed after the request, c was checked within a day");
    }

    #[test]
    fn an_indexed_answer_older_than_the_request_does_not_close_it() {
        let list = [followup("https://example.com/a")];
        let mut state = WatchState::default();
        inspected(&mut state, "https://example.com/a", INDEXED, "2026-11-01T09:00:00+08:00");
        assert_eq!(due_for_inspection(&list, &state, at("2026-11-10T10:00:00+08:00"), 6).len(), 1);
    }

    #[test]
    fn only_open_followups_past_their_due_date_are_findings() {
        let list = [followup("https://example.com/a"), followup("https://example.com/b")];
        let mut state = WatchState::default();
        inspected(&mut state, "https://example.com/b", INDEXED, "2026-11-12T09:00:00+08:00");
        assert!(findings(&list, &state, at("2026-11-15T10:00:00+08:00")).is_empty());
        let late = findings(&list, &state, at("2026-11-17T10:00:00+08:00"));
        assert_eq!(late.iter().map(|f| f.key.as_str()).collect::<Vec<_>>(), ["reindex:https://example.com/a"]);
    }

    #[test]
    fn an_open_followup_waits_until_its_due_date_only() {
        let list = [followup("https://example.com/a"), followup("https://example.com/b")];
        let mut state = WatchState::default();
        inspected(&mut state, "https://example.com/a", "Duplicate, Google chose different canonical than user", "2026-11-10T09:00:00+08:00");
        inspected(&mut state, "https://example.com/b", INDEXED, "2026-11-12T09:00:00+08:00");
        let during = waiting(&list, &state, at("2026-11-13T10:00:00+08:00"));
        assert_eq!(during.into_iter().collect::<Vec<_>>(), ["https://example.com/a"], "b is indexed and closed");
        assert!(waiting(&list, &state, at("2026-11-17T10:00:00+08:00")).is_empty(), "past dueBy it is a reindex finding instead");
    }
}
