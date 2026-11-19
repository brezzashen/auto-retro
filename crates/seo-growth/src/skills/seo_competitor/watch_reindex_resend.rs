//! Re-sends GSC "Request indexing" for follow-ups whose request never went through (`"state": "unsent"`:
//! the per-account quota was exhausted or the console errored). The quota resets at midnight Pacific
//! (16:00 Singapore), so once a Singapore day, at or after `RESEND_HOUR_SGT`, the run sends up to
//! `MAX_PER_DAY` such URLs through `gsc-request-index.js` on debug Chrome 9223 and rewrites each accepted
//! line with the real `requestedAt`. `"state": "skipped"` marks a URL deliberately not requested.

use super::watch_config::WatchState;
use super::watch_reindex::Followup;
use chrono::{DateTime, Duration, FixedOffset, Timelike};
use serde_json::Value;
use std::process::Command;

pub const UNSENT: &str = "unsent";
const RESEND_HOUR_SGT: u32 = 16;
const MAX_PER_DAY: usize = 8;
const SCRIPT: &str = ".claude/skills/seo/scripts/gsc-request-index.js";
const ACCEPTED: &str = "indexing requested";

/// Unsent follow-ups still inside their window, oldest first, at most `MAX_PER_DAY`.
pub fn pending(followups: &[Followup], now: DateTime<FixedOffset>) -> Vec<String> {
    followups.iter()
        .filter(|f| f.state.as_deref() == Some(UNSENT))
        .filter(|f| DateTime::parse_from_rfc3339(&f.due_by).is_ok_and(|due| now <= due))
        .map(|f| f.url.clone())
        .take(MAX_PER_DAY)
        .collect()
}

/// URLs the console confirmed with a visible "Indexing requested" in the script's JSON output.
pub fn accepted(stdout: &str) -> Vec<String> {
    let (Some(start), Some(end)) = (stdout.find("\n[").map(|i| i + 1).or_else(|| stdout.starts_with('[').then_some(0)), stdout.rfind(']')) else { return Vec::new() };
    let Ok(Value::Array(results)) = serde_json::from_str::<Value>(&stdout[start..=end]) else { return Vec::new() };
    results.iter()
        .filter(|r| r["dialog"].as_str().is_some_and(|d| d.to_lowercase().contains(ACCEPTED)))
        .filter_map(|r| r["url"].as_str().map(String::from))
        .collect()
}

/// The follow-up file with each accepted unsent line re-dated to `now`; every other line is kept verbatim.
pub fn rewrite(text: &str, accepted: &[String], now: DateTime<FixedOffset>) -> String {
    let mut out: Vec<String> = text.lines().map(|line| {
        let Ok(mut v) = serde_json::from_str::<Value>(line) else { return line.to_string() };
        let hit = v["state"].as_str() == Some(UNSENT) && v["url"].as_str().is_some_and(|u| accepted.iter().any(|a| a == u));
        if !hit { return line.to_string(); }
        let reason = v["reason"].as_str().unwrap_or_default().to_string();
        v["requestedAt"] = now.to_rfc3339().into();
        v["dueBy"] = (now + Duration::days(7)).to_rfc3339().into();
        v["reason"] = format!("Auto re-requested by the SEO watchdog {}; {reason}", now.format("%Y-%m-%d %H:%M SGT")).into();
        if let Some(map) = v.as_object_mut() { map.remove("state"); }
        v.to_string()
    }).collect();
    if text.ends_with('\n') { out.push(String::new()); }
    out.join("\n")
}

/// Once a Singapore day after the quota reset: re-send the unsent follow-ups and record the result.
pub fn run(followups: &[Followup], state_dir: &str, state: &mut WatchState, now: DateTime<FixedOffset>, dry_run: bool, errors: &mut Vec<String>) {
    let today = now.format("%Y-%m-%d").to_string();
    if dry_run || now.hour() < RESEND_HOUR_SGT || state.last_resend_day.as_deref() == Some(today.as_str()) { return; }
    let urls = pending(followups, now);
    if urls.is_empty() { state.last_resend_day = Some(today); return; }
    let out_dir = format!("{state_dir}/reindex-resend/{today}");
    let _ = std::fs::create_dir_all(&out_dir);
    let result = Command::new("browser-lease").args(["run", "9223", "--", "bun", SCRIPT, &out_dir]).args(&urls).output();
    let Ok(output) = result.map_err(|e| errors.push(format!("reindex resend: {e}"))) else { return };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let sent = accepted(&stdout);
    if !output.status.success() && sent.is_empty() {
        errors.push(format!("reindex resend: gsc-request-index.js exited {} ({} URLs stay unsent)", output.status, urls.len()));
        return;
    }
    state.last_resend_day = Some(today);
    let path = format!("{state_dir}/reindex-followups.jsonl");
    if sent.is_empty() { return; }
    match std::fs::read_to_string(&path) {
        Ok(text) => if let Err(e) = std::fs::write(&path, rewrite(&text, &sent, now)) { errors.push(format!("reindex resend: {path}: {e}")) },
        Err(e) => errors.push(format!("reindex resend: {path}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<FixedOffset> { DateTime::parse_from_rfc3339(text).unwrap() }

    fn followup(url: &str, state: Option<&str>, due_by: &str) -> Followup {
        Followup { url: url.into(), requested_at: "2026-11-11T21:49:24+08:00".into(), due_by: due_by.into(), reason: "r".into(), state: state.map(String::from) }
    }

    #[test]
    fn only_unsent_followups_inside_their_window_are_resent() {
        let list = [
            followup("https://example.com/a", Some(UNSENT), "2026-11-18T21:49:24+08:00"),
            followup("https://example.com/b", Some("skipped"), "2026-11-18T21:49:24+08:00"),
            followup("https://example.com/c", None, "2026-11-18T21:49:24+08:00"),
            followup("https://example.com/d", Some(UNSENT), "2026-11-12T10:00:00+08:00"),
        ];
        assert_eq!(pending(&list, at("2026-11-12T16:17:00+08:00")), ["https://example.com/a"]);
    }

    #[test]
    fn reads_only_visible_indexing_requested_dialogs_from_mixed_output() {
        let stdout = "browser-lease: admitted\n[\n  {\"url\": \"https://example.com/a\", \"status\": \"requested\", \"dialog\": \"Indexing requested\"},\n  {\"url\": \"https://example.com/b\", \"status\": \"requested\", \"dialog\": \"Quota Exceeded\"},\n  {\"url\": \"https://example.com/c\", \"status\": \"requested\", \"dialog\": null}\n]\n";
        assert_eq!(accepted(stdout), ["https://example.com/a"]);
        assert!(accepted("no json here").is_empty());
    }

    #[test]
    fn rewrites_only_the_accepted_unsent_line_and_keeps_the_others_verbatim() {
        let text = "{\"url\":\"https://example.com/a\",\"requestedAt\":\"x\",\"dueBy\":\"y\",\"reason\":\"r\",\"state\":\"unsent\"}\n{\"url\":\"https://example.com/b\", \"requestedAt\":\"x\",\"dueBy\":\"y\",\"reason\":\"r\",\"state\":\"skipped\"}\n";
        let out = rewrite(text, &["https://example.com/a".into(), "https://example.com/b".into()], at("2026-11-12T16:17:00+08:00"));
        let lines: Vec<&str> = out.lines().collect();
        let first: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["requestedAt"], "2026-11-12T16:17:00+08:00");
        assert_eq!(first["dueBy"], "2026-11-19T16:17:00+08:00");
        assert!(first.get("state").is_none() && first["reason"].as_str().unwrap().starts_with("Auto re-requested"));
        assert_eq!(lines[1], "{\"url\":\"https://example.com/b\", \"requestedAt\":\"x\",\"dueBy\":\"y\",\"reason\":\"r\",\"state\":\"skipped\"}");
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn waits_for_the_quota_reset_and_runs_once_a_day() {
        let mut state = WatchState::default();
        let mut errors = Vec::new();
        run(&[], "/nonexistent", &mut state, at("2026-11-12T15:17:00+08:00"), false, &mut errors);
        assert!(state.last_resend_day.is_none(), "before 16:00 nothing runs");
        run(&[], "/nonexistent", &mut state, at("2026-11-12T16:17:00+08:00"), false, &mut errors);
        assert_eq!(state.last_resend_day.as_deref(), Some("2026-11-12"));
        assert!(errors.is_empty());
    }
}
