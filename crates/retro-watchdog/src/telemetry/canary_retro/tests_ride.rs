//! A rider's own upload: a release whose queued build was cancelled to ride a covering commit spent its
//! upload before the ride, and its receipt's `build` is the cover's. Mars_CRM records the cancelled
//! build's upload as `rode.uploadSeconds`.
use super::*;
use serde_json::json;

/// Canary 20261112T033259Z-canary-64843: 1439 s on the board. Its own build 5f230d2a uploaded for 631 s
/// (gcloud 11.33.46: 11:33:48 to 11:44:17 SGT) and was cancelled 16 s after creation to ride 05e22e8b0,
/// then f4c00fc85, whose build 0f31b05a uploaded 208 s, queued 70 s and ran 505 s.
fn rider() -> (Value, Value) {
    let r = json!({"id": "20261112T033259Z-canary-64843", "target": "canary", "queue": "Q1", "purpose": "p", "task": null,
        "state": "succeeded", "startedAt": "2026-11-12T03:32:59.178Z", "finishedAt": "2026-11-12T03:56:57.917Z",
        "updatedAt": "2026-11-12T03:56:57.917Z", "source": "a".repeat(40), "creator": {"pid": 0, "orcaTab": "tab"}, "log": null, "failure": null});
    let step = |id: &str, s: f64| json!({"id": id, "status": "SUCCESS", "seconds": s});
    let rec = json!({"startedAt": "2026-11-12T03:33:20.641Z", "outcome": "live", "tags": {"api": "a"}, "built": ["api", "crm", "agent"],
        "rolled": ["api", "crm", "agent"], "preflight": {"seconds": 342}, "preparedBy": "covering commit f4c00fc85",
        "preparedAt": "2026-11-12T03:54:27.785Z", "queuedAt": "2026-11-12T03:54:27.785Z", "health": {"status": "ok"},
        "rode": {"commit": "f4c00fc85928019090c65180a9e174201af37bd8", "cancelled": "5f230d2a-7196-4bc1-a343-f2aa5a3aa5e1", "build": "0f31b05a-3218-43f4-93ed-99156c9d35ad"},
        "build": {"id": "0f31b05a-3218-43f4-93ed-99156c9d35ad", "status": "SUCCESS", "queuedSeconds": 70, "runSeconds": 505, "uploadSeconds": 208,
            "steps": [step("rust-build", 403.0)]},
        "rollout": {"liveAt": "2026-11-12T03:56:55.232Z", "runSeconds": 111}});
    (r, rec)
}

#[test]
fn a_rider_whose_own_upload_crawled_is_an_upload_cause() {
    let (r, mut rec) = rider();
    rec["rode"]["uploadSeconds"] = json!(631);
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!((row["verdict"].as_str(), row["dominant"].as_str(), row["cause"].as_str()), (Some("over_hard"), Some("prepare"), Some("upload")));
    assert_eq!(row["prepare"]["rode_upload_s"], json!(631.0));
}

#[test]
fn a_ride_without_its_own_upload_seconds_keeps_the_build_cause() {
    let (r, rec) = rider();
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!(row["cause"], "build");
    assert_eq!(row["prepare"]["rode_upload_s"], Value::Null);
}

#[test]
fn a_quick_own_upload_before_a_ride_does_not_name_upload() {
    let (r, mut rec) = rider();
    rec["rode"]["uploadSeconds"] = json!(45);
    let row = analyze::run_row(&r, "succeeded", "", &rec, &Value::Null);
    assert_eq!(row["cause"], "build");
}
