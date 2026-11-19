//! The hourly retro: every finding handed to a tab is followed until its data recovers. A key whose
//! finding is gone moves to `resolved`; one still present after `stale_hours` is flagged, because a
//! fix that waits for promotion or did not work looks the same in the data.

use super::config::{Finding, Resolved, WatchState};
use chrono::{DateTime, FixedOffset};
use serde::Serialize;

const KEEP_RESOLVED: usize = 20;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RetroRow {
    pub key: String,
    pub handed_at: String,
    pub hours: f64,
    /// `open`, `stale` or `resolved`.
    pub status: String,
    pub resolved_at: Option<String>,
}

fn hours_between(from: &str, to: DateTime<FixedOffset>) -> f64 {
    DateTime::parse_from_rfc3339(from).map_or(0.0, |t| (to - t).num_minutes() as f64 / 60.0)
}

/// Updates `state` for this run's findings (only when every source answered) and returns the
/// retro rows plus a P2 finding per stale key.
pub fn reconcile(state: &mut WatchState, findings: &[Finding], now: DateTime<FixedOffset>, complete: bool, stale_hours: f64) -> (Vec<RetroRow>, Vec<Finding>) {
    let present = |key: &str| findings.iter().any(|f| f.key == key);
    let fallback = state.last_fix_at.clone().unwrap_or_else(|| now.to_rfc3339());
    for key in &state.handled { state.handed_at.entry(key.clone()).or_insert_with(|| fallback.clone()); }
    if complete {
        let (gone, kept): (Vec<String>, Vec<String>) = state.handled.drain(..).partition(|k| !present(k));
        state.handled = kept;
        for key in gone {
            let handed_at = state.handed_at.remove(&key).unwrap_or_else(|| fallback.clone());
            state.resolved.insert(0, Resolved { key, handed_at, resolved_at: now.to_rfc3339() });
        }
        state.resolved.truncate(KEEP_RESOLVED);
    }
    let mut rows = Vec::new();
    let mut stale = Vec::new();
    for key in &state.handled {
        let handed_at = state.handed_at.get(key).cloned().unwrap_or_else(|| fallback.clone());
        let hours = hours_between(&handed_at, now);
        let status = if hours >= stale_hours { "stale" } else { "open" };
        if status == "stale" {
            stale.push(Finding::new(format!("retro:stale:{key}"), "P2", format!("已派工 {hours:.0} 小时，`{key}` 仍未恢复"),
                "看修复标签的复盘报告：已到 Canary 而生产未 promote 时等待 founder；否则修复没有生效，需要重新排查"));
        }
        rows.push(RetroRow { key: key.clone(), handed_at, hours, status: status.into(), resolved_at: None });
    }
    rows.extend(state.resolved.iter().take(10).map(|r| RetroRow { key: r.key.clone(), handed_at: r.handed_at.clone(),
        hours: DateTime::parse_from_rfc3339(&r.resolved_at).map_or(0.0, |t| hours_between(&r.handed_at, t)), status: "resolved".into(), resolved_at: Some(r.resolved_at.clone()) }));
    (rows, stale)
}

pub fn section(rows: &[RetroRow]) -> String {
    let mut md = String::from("## 复盘\n\n每小时核对已派工的事项：数据恢复即记为已恢复；超过 48 小时仍在的事项单独标出。\n\n");
    if rows.is_empty() { return md + "还没有派过工。\n\n"; }
    md.push_str("| 事项 | 派工时间 | 已过小时 | 状态 |\n|---|---|---|---|\n");
    for r in rows {
        let status = match r.status.as_str() {
            "open" => "处理中".to_string(),
            "stale" => "超时未恢复".to_string(),
            _ => format!("已恢复（{}）", r.resolved_at.as_deref().and_then(|t| t.get(..16)).unwrap_or("").replace('T', " ")),
        };
        md.push_str(&format!("| `{}` | {} | {:.1} | {status} |\n", r.key, r.handed_at.get(..16).unwrap_or(&r.handed_at).replace('T', " "), r.hours));
    }
    md.push('\n');
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t: &str) -> DateTime<FixedOffset> { DateTime::parse_from_rfc3339(t).unwrap() }

    #[test]
    fn a_recovered_key_moves_to_resolved_and_an_old_one_turns_stale() {
        let mut state = WatchState { handled: vec!["health:a".into(), "health:b".into()], last_fix_at: Some("2026-11-03T15:00:00+08:00".into()), ..Default::default() };
        let still = [Finding::new("health:b", "P1", "", "")];
        let (rows, stale) = reconcile(&mut state, &still, at("2026-11-05T16:00:00+08:00"), true, 48.0);
        assert_eq!(state.handled, vec!["health:b"]);
        assert_eq!(state.resolved[0].key, "health:a");
        assert_eq!(stale[0].key, "retro:stale:health:b");
        assert_eq!(rows.iter().map(|r| r.status.as_str()).collect::<Vec<_>>(), vec!["stale", "resolved"]);
    }

    #[test]
    fn an_incomplete_run_never_marks_anything_resolved() {
        let mut state = WatchState { handled: vec!["health:a".into()], ..Default::default() };
        reconcile(&mut state, &[], at("2026-11-03T16:00:00+08:00"), false, 48.0);
        assert_eq!(state.handled, vec!["health:a"]);
        assert!(state.resolved.is_empty());
    }
}
