use super::*;

fn receipt(signature: &str) -> Value {
    json!({"ok": true, "calls": 40, "signature": signature, "zero": if signature.is_empty() { 0 } else { 1 },
        "config": {"minCalls": 5}, "exemptions": [],
        "groups": [{"agent": "relationship-analyst", "model": "gemini-3.8-flash", "status": "zero", "hitRate": 0.0, "calls": 40,
            "samples": [{"threadId": "t", "input": 50000, "cached": 0}]}]})
}

fn absorb(watch: &mut CacheWatch, home: &Path, now: f64, signature: &str) {
    let receipt = receipt(signature);
    let summary = summarize(now * 1000.0, 3, Some(0), false, None, Path::new("/x.json"), &receipt);
    watch.absorb(home, now, summary, &receipt).unwrap();
}

#[test]
fn summary_keeps_rates_and_flags_failures() {
    let ok = summarize(0.0, 3, Some(0), false, None, Path::new("/x.json"), &receipt("a@m"));
    assert_eq!(ok["ok"], true);
    assert_eq!(ok["error"], Value::Null);
    assert_eq!(ok["groups"][0]["group"], "relationship-analyst@gemini-3.8-flash");
    let failed = summarize(0.0, 3, Some(1), false, None, Path::new("/x.json"), &json!({"ok": false, "error": "auth"}));
    assert_eq!(failed["error"], "auth");
    let stuck = summarize(0.0, 300, None, true, None, Path::new("/x.json"), &Value::Null);
    assert_eq!(stuck["error"], "timed out");
}

#[test]
fn a_zero_opens_one_session_until_acknowledged_and_never_repeats_a_signature_within_a_day() {
    let home = crate::telemetry::test_home("cache-watch");
    let now = 20_000.0 * 86_400.0;
    let mut watch = CacheWatch::default();
    absorb(&mut watch, &home, now, "");
    assert!(watch.dispatches.is_empty(), "no zero, no session");
    absorb(&mut watch, &home, now, "relationship-analyst@gemini-3.8-flash");
    assert_eq!(watch.dispatches.len(), 1);
    let id = watch.dispatches[0]["incident"].as_str().unwrap().to_string();
    let incident = crate::telemetry::read(&root(&home).join("incidents").join(format!("{id}.json"))).unwrap();
    assert_eq!(incident["trigger"], "canary_cache_zero");
    assert_eq!(incident["zero"][0]["samples"][0]["threadId"], "t");
    // Open incident: a different zero waits.
    absorb(&mut watch, &home, now + SAMPLE_SECS, "chat@gemini-3.8-flash");
    assert_eq!(watch.dispatches.len(), 1);
    std::fs::write(root(&home).join("incidents").join(format!("{id}.ack")), "").unwrap();
    // Acknowledged: the same signature still waits a day, a new one opens.
    assert!(!watch.may_dispatch(&home, now + 2.0 * SAMPLE_SECS, "relationship-analyst@gemini-3.8-flash"));
    assert!(watch.may_dispatch(&home, now + SAME_SIGNATURE_SECS, "relationship-analyst@gemini-3.8-flash"));
    absorb(&mut watch, &home, now + 2.0 * SAMPLE_SECS, "chat@gemini-3.8-flash");
    assert_eq!(watch.dispatches.len(), 2);
}

#[test]
fn sessions_are_capped_per_day() {
    let home = crate::telemetry::test_home("cache-watch-cap");
    let now = 20_000.0 * 86_400.0;
    let mut watch = CacheWatch::default();
    for index in 0..MAX_DISPATCH_PER_DAY {
        watch.dispatches.push(json!({"incident": format!("done{index}"), "signature": format!("s{index}"), "at_ms": now * 1000.0, "started": true}));
        std::fs::create_dir_all(root(&home).join("incidents")).unwrap();
        std::fs::write(root(&home).join("incidents").join(format!("done{index}.ack")), "").unwrap();
    }
    assert!(!watch.may_dispatch(&home, now + 60.0, "new@model"));
    assert!(watch.may_dispatch(&home, now + 86_400.0, "new@model"));
}

#[test]
fn prompt_is_one_line_and_names_the_route() {
    let text = prompt(Path::new("/Users/test"), "8123", "chat@gemini-3.8-flash");
    assert!(!text.contains('\n'));
    assert!(text.contains("route cache-zero"));
    assert!(text.contains("monitor-ack 8123"));
}

/// The package scripts the watchdog runs must start a program found on MARS_SCRIPT_PATH: on 2026-10-26
/// cache-watch and quota-economics became `cargo run`, cargo was missing and every run exited 127
/// (since 2026-11-16 both run `mars-ops-crm` through `crm_ops.rs`, tested there). The disk guard's S0 (`release recover-disk`, via `cpu-build-gate cargo-run`) failed the same way, so a
/// `cargo-run` wrapper also needs cargo itself on the path.
#[test]
#[ignore = "requires local host workspace checkout"]
fn package_script_programs_resolve_on_the_script_path() {
    let package: Value = serde_json::from_slice(&std::fs::read(format!("{CRM}/package.json")).unwrap()).unwrap();
    let on_path = |program: &str, path: &str| path.split(':').any(|dir| std::path::Path::new(dir).join(program).is_file());
    for (script, path) in [("release", crate::telemetry::disk_guard::S0_PATH)] {
        let command = package["scripts"][script].as_str().unwrap();
        let program = command.split_whitespace().next().unwrap();
        assert!(on_path(program, path), "{script}: {program} is not on its PATH");
        if command.contains("cargo") {
            assert!(on_path("cargo", path), "{script}: cargo is not on its PATH");
        }
    }
}
