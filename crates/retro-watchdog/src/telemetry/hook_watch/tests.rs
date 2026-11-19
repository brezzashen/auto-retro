use super::*;
use serde_json::json;

const DAY_MS: f64 = 86_400_000.0;
const SGT_MS: f64 = 8.0 * 3_600_000.0;
const DAY: i64 = 20_767;

fn at(day: i64, minutes: f64) -> f64 {
    day as f64 * DAY_MS + minutes * 60_000.0 - SGT_MS
}

#[test]
fn due_daily_from_eleven_until_one_run_succeeds() {
    let mut watch = HookWatch::default();
    assert_eq!(watch.due(at(DAY, 10.0 * 60.0)), None);
    assert_eq!(watch.due(at(DAY, DAILY_AT_MIN)), Some(DAY));
    watch.runs.push(json!({"day": DAY, "at_ms": at(DAY, DAILY_AT_MIN), "ok": false}));
    assert_eq!(watch.due(at(DAY, DAILY_AT_MIN + 30.0)), None, "retry waits an hour");
    watch.runs.push(json!({"day": DAY, "at_ms": at(DAY, DAILY_AT_MIN + 60.0), "ok": true}));
    assert_eq!(watch.due(at(DAY, 23.0 * 60.0)), None);
    assert_eq!(watch.due(at(DAY + 1, DAILY_AT_MIN)), Some(DAY + 1));
}

#[test]
fn a_defect_seen_only_before_its_acknowledged_fix_is_not_fresh() {
    let mut watch = HookWatch::default();
    watch.resolved.insert("unresolved-path|Stop · x|references/a.md".into(), json!(1000.0));
    let receipt = json!({"defects": [
        {"signature": "unresolved-path|Stop · x|references/a.md", "last_seen_ms": 900.0},
        {"signature": "unresolved-path|Stop · x|references/a.md", "last_seen_ms": 1100.0},
        {"signature": "hook-errors|Stop · y|", "last_seen_ms": 10.0}]});
    let fresh: Vec<f64> = watch.fresh(&receipt).iter().map(|d| d["last_seen_ms"].as_f64().unwrap()).collect();
    assert_eq!(fresh, [1100.0, 10.0]);
}

#[test]
fn fresh_defects_open_one_incident_and_the_next_waits_for_its_ack() {
    let home = crate::telemetry::test_home("hook-watch");
    let mut watch = HookWatch::default();
    let run = |day: i64| json!({"day": day, "at_ms": 1.0, "ok": true, "output": "/tmp/x.json",
        "receipt": {"ok": true, "stop_blocks": 3, "defects": [{"signature": "hook-errors|Stop · y|", "last_seen_ms": 5.0}]}});
    watch.absorb(&home, run(DAY)).unwrap();
    assert_eq!(watch.dispatches.len(), 1);
    assert_eq!(watch.dispatches[0]["signatures"], json!(["hook-errors|Stop · y|"]));
    watch.absorb(&home, run(DAY + 1)).unwrap();
    assert_eq!(watch.dispatches.len(), 1, "an open incident blocks a second session");
    assert!(watch.blocked.is_some());
}

#[test]
fn ack_gate_needs_a_verdict_per_defect_and_proof_after_a_fix() {
    let incident = json!({"trigger": "hook_defects_found", "defects": [{"signature": "hook-errors|Stop · y|"}]});
    let url = "https://reports.example.com/x/";
    let none = json!({"report_url": url, "defects": []});
    assert!(validate::check(&incident, &none).unwrap_err().to_string().contains("no verdict"));
    let fixed = json!({"report_url": url, "defects": [{"signature": "hook-errors|Stop · y|", "verdict": "fixed", "reason": "timeout raised after profiling", "commit": "abcdef1"}]});
    assert!(validate::check(&incident, &fixed).unwrap_err().to_string().contains("after.check"));
    let mut proven = fixed.clone();
    proven["after"] = json!({"check": "pass", "replay": "the hook printed nothing in 40 ms"});
    validate::check(&incident, &proven).unwrap();
}
