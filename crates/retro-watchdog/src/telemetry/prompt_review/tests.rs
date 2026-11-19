use super::*;
use serde_json::json;

const DAY_MS: f64 = 86_400_000.0;
const SGT_MS: f64 = 8.0 * 3_600_000.0;
/// 2026-11-09 is a Monday.
const MONDAY: i64 = 20_766;

fn at(day: i64, minutes: f64) -> f64 {
    day as f64 * DAY_MS + minutes * 60_000.0 - SGT_MS
}

#[test]
fn weeks_start_on_monday() {
    assert_eq!(week_of(MONDAY), MONDAY);
    assert_eq!(week_of(MONDAY + 6), MONDAY);
    assert_eq!(week_of(MONDAY + 7), MONDAY + 7);
    assert!(super::super::quota_watch::is_monday(MONDAY));
}

#[test]
fn due_from_monday_morning_until_one_run_succeeds() {
    let mut review = PromptReview::default();
    assert_eq!(review.due(at(MONDAY, 10.0 * 60.0)), None);
    assert_eq!(review.due(at(MONDAY, WEEKLY_AT_MIN)), Some(MONDAY));
    review.runs.push(json!({"week": MONDAY, "at_ms": at(MONDAY, WEEKLY_AT_MIN), "ok": false}));
    assert_eq!(review.due(at(MONDAY, WEEKLY_AT_MIN + 30.0)), None, "retry waits an hour");
    assert_eq!(review.due(at(MONDAY, WEEKLY_AT_MIN + 60.0)), Some(MONDAY));
    review.runs.push(json!({"week": MONDAY, "at_ms": at(MONDAY, WEEKLY_AT_MIN + 60.0), "ok": true}));
    assert_eq!(review.due(at(MONDAY + 3, 12.0 * 60.0)), None);
    assert_eq!(review.due(at(MONDAY + 7, WEEKLY_AT_MIN)), Some(MONDAY + 7));
    // A host that was off on Monday runs later that week.
    assert_eq!(PromptReview::default().due(at(MONDAY + 2, 1.0)), Some(MONDAY));
}

#[test]
fn ack_gate_needs_a_verdict_per_new_group() {
    let incident = json!({"trigger": "prompt_review_due", "new_groups": [{"id": "S21"}]});
    let mut report = json!({"report_url": "https://reports.example.com/x/", "groups": []});
    assert!(validate::check(&incident, &report).is_err());
    report["groups"] = json!([{"id": "S21", "verdict": "fixed", "reason": "kept the system copy only"}]);
    assert!(validate::check(&incident, &report).unwrap_err().to_string().contains("commit"));
    report["groups"][0]["commit"] = json!("a0fcb414ed");
    assert!(validate::check(&incident, &report).unwrap_err().to_string().contains("after.prefix"));
    report["after"] = json!({"prefix": 212_000, "gate": "pass"});
    validate::check(&incident, &report).unwrap();
    report["groups"] = json!([{"id": "S21", "verdict": "owned_elsewhere", "reason": "the P0 tab edits these files"}]);
    assert!(validate::check(&incident, &report).is_err());
}
