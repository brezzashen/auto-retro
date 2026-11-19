use super::*;
use serde_json::json;

fn event(key: &str, kind: &str, user: &str, detail: Value) -> Value {
    json!({"kind": kind, "key": key, "at_ms": 1_000.0, "user_id": user, "email_masked": "ab***@example.com", "detail": detail})
}

const EXEMPT_SRC: &str = r#"
/// Internal QA accounts.
pub const RISK_EXEMPT_EMAILS: &[&str] = &[
    "demo@example.com",
    "Lin@example.com",
];
"#;

const FILTER_SRC: &str = "pub const WINDOW_MS: i64 = 24 * 3_600_000;\npub const WARN_FROM: u32 = 3;\npub const SUSPEND_AT: u32 = 5;\n";

#[test]
fn the_policy_is_read_from_the_crm_source() {
    let policy = fetch::parse_policy(EXEMPT_SRC, FILTER_SRC).unwrap();
    assert_eq!(policy, fetch::Policy { exempt: vec!["demo@example.com".into(), "lin@example.com".into()], warn_from: 3, suspend_at: 5, window_ms: 86_400_000.0 });
    let sql = fetch::events_sql(&policy, 0.0, 1000.0);
    assert!(sql.contains("array['demo@example.com','lin@example.com']"));
    assert!(sql.contains("cf.n >= 3 and cf.n < 5"));
    assert!(sql.contains("interval '86400000 milliseconds'"));
    // A changed shape fails the sample instead of guessing.
    assert!(fetch::parse_policy("", FILTER_SRC).is_err());
    assert!(fetch::parse_policy(EXEMPT_SRC, "pub const WARN_FROM: u32 = 3;").is_err());
    assert!(fetch::parse_policy(&EXEMPT_SRC.replace("demo@", "x'; drop@"), FILTER_SRC).is_err());
}

#[test]
fn a_new_event_opens_one_session_and_later_ones_wait_for_its_ack() {
    let home = crate::telemetry::test_home("risk-retro");
    let mut retro = RiskRetro { loaded: true, cursor_ms: 0.0, ..Default::default() };
    let first = event("rv:1", "risk_review", "u1", json!({"behaviour": "access_probing", "status": "pending"}));
    retro.absorb(&home, 10_000.0, Ok((5_000.0, vec![first.clone()]))).unwrap();
    assert_eq!(retro.dispatches.len(), 1);
    assert!(retro.pending.is_empty());
    assert_eq!(retro.cursor_ms, 5_000.0);
    let id = retro.dispatches[0]["incident"].as_str().unwrap().to_string();
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents").join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(incident["trigger"], "risk_warning_found");
    assert_eq!(incident["requirements"]["risk_review"]["events"], json!(["rv:1"]));
    assert_eq!(incident["summary"]["kinds"]["risk_review:access_probing:pending"], 1);

    // The same row read again (overlap) is not a new event; a new one waits while the session is open.
    let second = event("cw:u2:9", "cancellation_warning", "u2", json!({}));
    retro.absorb(&home, 20_000.0, Ok((15_000.0, vec![first, second]))).unwrap();
    assert_eq!(retro.dispatches.len(), 1);
    assert_eq!(retro.pending.len(), 1);

    // Once acknowledged, the waiting event opens the next session at once.
    std::fs::write(root(&home).join("incidents").join(format!("{id}.ack")), "{}").unwrap();
    retro.observe(&home, 30.0).unwrap();
    assert_eq!(retro.dispatches.len(), 2);
    assert!(retro.pending.is_empty());
}

#[test]
fn a_failed_read_keeps_the_cursor() {
    let home = crate::telemetry::test_home("risk-retro-error");
    let mut retro = RiskRetro { loaded: true, cursor_ms: 7.0, ..Default::default() };
    retro.absorb(&home, 10_000.0, Err("timeout".into())).unwrap();
    assert_eq!(retro.cursor_ms, 7.0);
    assert!(retro.dispatches.is_empty());
}

#[test]
fn the_ack_gate_needs_a_grounded_verdict_for_every_event() {
    let review = json!({"events": ["rv:1", "cf:u:f"]});
    let md = "## 判定\n## 证据\n## 处置\n";
    let real = json!({"key": "rv:1", "verdict": "real_risk", "evidence": "12 distinct 403 paths in 4 min", "reason": "enumerating ids"});
    let mut report = json!({"report_url": "https://reports.example.com/x/", "events": [real]});
    assert!(validate::validate_report(&review, &report, md).unwrap_err().to_string().contains("cf:u:f"));

    let mut fp = json!({"key": "cf:u:f", "verdict": "false_positive", "evidence": "medical question", "reason": "provider over-blocks"});
    report["events"].as_array_mut().unwrap().push(fp.clone());
    assert!(validate::validate_report(&review, &report, md).unwrap_err().to_string().contains("file:line"));

    fp["rule"] = json!("rust/crates/mars-crm-domain/src/risk/content_filter.rs:17");
    fp["proposal"] = json!("count only repeated blocks of one thread");
    fp["account"] = json!("none: still under the warning band");
    report["events"][1] = fp;
    validate::validate_report(&review, &report, md).unwrap();
    assert!(validate::validate_report(&review, &report, "## 判定\n").is_err());

    report["events"][0]["verdict"] = json!("inconclusive");
    assert!(validate::validate_report(&review, &report, md).unwrap_err().to_string().contains("next"));
}
