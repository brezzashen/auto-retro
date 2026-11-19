use super::*;

fn heal(home: &Path, directory: &str, at_ms: f64, url: Option<&str>) {
    let path = root(home).join(directory);
    std::fs::create_dir_all(&path).unwrap();
    let day = (at_ms / DAY_MS).floor() * DAY_MS;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path.join(format!("{}.ndjson", day as u64))).unwrap();
    writeln!(file, "{}", json!({"at_ms": at_ms, "url": url, "outcome": "closed"})).unwrap();
}

const STORY: &str = "http://localhost:6006/iframe.html?id=mars-crm-library-backbutton--pops&viewMode=story";

#[test]
fn counts_storybook_heals_inside_the_window_across_midnight() {
    let home = crate::telemetry::test_home("storybook-window");
    // 00:05 UTC: the window reaches back into the previous day's file.
    let now = 20_000.0 * 86_400.0 + 300.0;
    heal(&home, "headless-heals", (now - 600.0) * 1000.0, Some(STORY));
    heal(&home, "headless-heals", (now - 120.0) * 1000.0, Some(STORY));
    heal(&home, "headless-heals", (now - 60.0) * 1000.0, Some("http://example.com/page"));
    heal(&home, "headless-heals", (now - REPEAT_WINDOW_SECS - 1.0) * 1000.0, Some(STORY));
    heal(&home, "storybook-heals", (now - 30.0) * 1000.0, None);
    let events = window_events(&home, now);
    assert_eq!(events.len(), 3);
    assert_eq!(events.iter().filter(|e| e["kind"] == "server_restart").count(), 1);
    assert_eq!(stories(&events), json!([{"story": "mars-crm-library-backbutton--pops", "closed": 2}]));
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn restarts_after_consecutive_failures_and_respects_the_cooldown() {
    assert!(!restart_due(SERVER_FAILS - 1, 0.0, 10_000.0));
    assert!(restart_due(SERVER_FAILS, 0.0, 10_000.0));
    assert!(!restart_due(SERVER_FAILS, 10_000.0 - RESTART_COOLDOWN_SECS + 1.0, 10_000.0));
}

#[test]
fn recovered_server_is_not_restarted_from_old_failures() {
    let round = probe_and_recover(true, || (true, 1, None), || panic!("healthy server must survive"));
    assert!(round.ok && round.restart.is_none());
}

#[test]
fn every_server_restart_is_recorded_for_review_without_repetition_threshold() {
    let home = crate::telemetry::test_home("storybook-recovery");
    let mut attempts = 0;
    let round = probe_and_recover(true, || { attempts += 1; (attempts > 1, 1, None) },
        || json!({"at_ms":1000,"outcome":"restarted"}));
    let mut watch = StorybookWatch::default();
    watch.absorb(&home, 1.0, round);
    // The restart is a condition of the one performance trigger and queues its session.
    let wake = super::super::read(&root(&home).join("wake-perf.json")).unwrap();
    assert_eq!(wake["episodes"].as_array().unwrap().len(), 1);
    assert_eq!(wake["episodes"][0]["condition"], "forced_recovery");
    assert_eq!(wake["episodes"][0]["outcome"], "queued");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn restart_record_keeps_the_failed_probe_error() {
    let mut attempts = 0;
    let round = probe_and_recover(true,
        || { attempts += 1; (attempts > 1, 7, (attempts == 1).then(|| "http://127.0.0.1:6006/index.json: status code 500".to_string())) },
        || json!({"at_ms":1000,"outcome":"restarted"}));
    let record = round.restart.unwrap();
    assert_eq!(record["probe"]["error"], "http://127.0.0.1:6006/index.json: status code 500");
    assert_eq!(record["probe"]["ms"], 7);
    assert_eq!(record["outcome"], "restarted");
    assert!(round.ok && round.error.is_none());
}

#[test]
fn restart_waits_while_the_host_is_busy_by_the_build_gate_rule() {
    let now_ms = 1_000_000.0;
    let history = |percent: f64, age_ms: f64| json!({"samples": [{"start": now_ms - age_ms - 5000.0, "end": now_ms - age_ms, "percent": percent}]});
    assert!(host_busy(&history(97.9, 5_000.0), now_ms));
    assert!(host_busy(&history(95.0, 29_000.0), now_ms));
    assert!(!host_busy(&history(94.1, 5_000.0), now_ms));
    assert!(!host_busy(&history(99.0, 31_000.0), now_ms));
    assert!(!host_busy(&json!({}), now_ms));
}

#[test]
fn hung_capture_parses_ps_fields_and_survives_a_missing_or_dead_server() {
    let fields = hung::parse_ps("S      0.1 718512 01:12:52");
    assert_eq!(fields["stat"], "S");
    assert_eq!(fields["cpu_percent_one_core"], 0.1);
    assert_eq!(fields["rss_kib"], 718512);
    assert_eq!(fields["elapsed"], "01:12:52");
    assert_eq!(hung::parse_ps("")["raw"], "");
    let repo = crate::telemetry::test_home("storybook-hung");
    assert_eq!(hung::capture(&repo, &repo, 1)["error"], "no readable crm.pid");
    // A PID that names no process: the record still forms, with no sample file.
    let locks = repo.join(".claude/locks/storybook-catalogs");
    std::fs::create_dir_all(&locks).unwrap();
    std::fs::write(locks.join("crm.pid"), "99999999\n").unwrap();
    let dead = hung::capture(&repo, &repo, 2);
    assert_eq!(dead["pid"], 99999999);
    assert!(dead["sample"].is_null());
    std::fs::remove_dir_all(repo).unwrap();
}
