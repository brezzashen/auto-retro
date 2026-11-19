use super::*;
use serde_json::json;

/// Lines of ~/.local/state/mars-deliver/canary-2026-11-13T14-42-57-576Z.log (20261113T144257Z-canary-96911).
const RETRIED: &str = "\n$ bun run canary release --ref=468f2e35ea --preflight=fast
$ cpu-build-gate cargo-run --shared-cache -- canary release \"--ref=468f2e35ea\"
cloud build 0de23a28: build [api crm agent] and validate
[cpu-build-gate] Disk-space guard refused build: Error: /System/Volumes/Data has 172.3 GiB free; minimum is 200 GiB
cancelled cloud build 0de23a28
receipt /a/2026-11-13T14-43-11.868Z-468f2e35e.json
error: script \"canary\" exited with code 1

$ bun run canary release --ref=468f2e35ea --preflight=fast
$ cpu-build-gate cargo-run --shared-cache -- canary release \"--ref=468f2e35ea\"
cloud build 046e3d2d: build [api crm agent] and validate
receipt /a/2026-11-13T14-49-00.968Z-468f2e35e.json
";

fn live(start: &str) -> Value {
    json!({"startedAt": start, "outcome": "live", "tags": {}, "built": [], "rolled": ["api"], "preflight": {"seconds": 175},
        "queuedAt": "2026-11-13T14:54:02.000Z", "health": {"status": "ok"},
        "build": {"status": "SUCCESS", "queuedSeconds": 58, "runSeconds": 188, "uploadSeconds": 11, "steps": []},
        "rollout": {"liveAt": "2026-11-13T14:56:21.000Z", "runSeconds": 113}})
}

fn board() -> Value {
    json!({"id": "20261113T144257Z-canary-96911", "target": "canary", "state": "succeeded", "startedAt": "2026-11-13T14:42:57.576Z",
        "finishedAt": "2026-11-13T14:56:24.174Z", "updatedAt": "2026-11-13T14:56:24.174Z", "source": "a".repeat(40), "failure": null})
}

#[test]
fn a_disk_guard_attempt_deliver_retried_is_an_error_not_an_unexplained_stall() {
    assert_eq!(retry::retried_attempt(RETRIED), Some("disk_guard"));
    let row = analyze::run_row(&board(), "succeeded", RETRIED, &live("2026-11-13T14:49:00.968Z"), &Value::Null);
    assert_eq!(row["segments"]["start"], json!(363.0));
    assert_eq!((row["dominant"].as_str(), row["cause"].as_str()), (Some("start"), Some("error")));
    assert_eq!(row["errors"], json!([{"kind": "retried_attempt", "class": "disk_guard"}]));
}

#[test]
fn a_single_execution_names_no_retried_attempt() {
    let once = RETRIED.split("\n\n$ bun").next().unwrap();
    assert_eq!(retry::retried_attempt(once), None);
    assert_eq!(retry::retried_attempt(""), None);
}

/// Lines of ~/.local/state/mars-deliver/canary-2026-11-18T11-36-05-517Z.log (20261118T113605Z-canary-12852).
const STOPPED_RIDER: &str = "\n$ bun run canary release --ref=ef445fb03e0c6ba35bb9167514b73faa78ed5a7d --preflight=fast
api: 7d099db7af2cf64f to build, will roll
e4b5f2eb1 contains ef445fb03 and is prepared by pid 90313; riding it instead of preparing
waiting for pid 90313 to finish preparing e4b5f2eb1
";

#[test]
fn a_rider_its_creator_stopped_is_a_queue_wait_not_an_error() {
    let run = json!({"id": "20261118T113605Z-canary-12852", "target": "canary", "state": "failed", "startedAt": "2026-11-18T11:36:05.517Z",
        "finishedAt": "2026-11-18T12:01:49.119Z", "updatedAt": "2026-11-18T12:01:49.119Z", "source": "e".repeat(40), "failure": ["deliver stopped by SIGINT"]});
    let row = analyze::run_row(&run, "failed", STOPPED_RIDER, &Value::Null, &Value::Null);
    assert_eq!((row["verdict"].as_str(), row["cause"].as_str()), (Some("over_hard"), Some("queue")));
    assert_eq!(row["errors"], json!([]));
    assert_eq!(row["queue"], json!([{"kind": "stopped_rider", "s": 1544.0}]));
    // A stopped run that rode nothing stays an error.
    let alone = STOPPED_RIDER.replace("riding it instead of preparing", "preparing");
    let row = analyze::run_row(&run, "failed", &alone, &Value::Null, &Value::Null);
    assert_eq!(row["cause"], "error");
}
