use super::*;
use serde_json::Value;

#[test]
fn a_silent_loop_is_stalled_only_once_the_limit_has_passed() {
    let start = Instant::now();
    let limit = Duration::from_secs(600);
    let mut watch = Watch { seen: 7, since: start };
    assert_eq!(watch.stalled(7, start + Duration::from_secs(599), limit), None);
    assert_eq!(watch.stalled(7, start + Duration::from_secs(600), limit), Some(Duration::from_secs(600)));
}

#[test]
fn a_beat_restarts_the_clock() {
    let start = Instant::now();
    let limit = Duration::from_secs(600);
    let mut watch = Watch { seen: 7, since: start };
    assert_eq!(watch.stalled(8, start + Duration::from_secs(590), limit), None);
    assert_eq!(watch.stalled(8, start + Duration::from_secs(1100), limit), None);
    assert_eq!(watch.stalled(8, start + Duration::from_secs(1190), limit), Some(Duration::from_secs(600)));
}

#[test]
fn the_record_names_the_stage_and_counts_as_a_recovery() {
    let directory = std::env::temp_dir().join(format!("cpu-watchdog-stall-test-{}", std::process::id()));
    let path = capture(&directory, Duration::from_secs(612), Duration::from_secs(600), "tick", "search-retro", 1_794_384_255_000.0);
    let record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(path.file_name().unwrap(), "1794384255000.json");
    assert_eq!(record["outcome"], "restarted");
    assert_eq!(record["stage"], "tick");
    assert_eq!(record["step"], "search-retro");
    assert_eq!(record["drill"], false);
    assert_eq!(record["children"], Value::Null);
    assert_eq!(record["children_error"], "not listed in tests");
    assert_eq!(record["stalled_secs"], 612);
    assert_eq!(record["limit_secs"], 600);
    assert_eq!(record["sample"], Value::Null);
    assert_eq!(record["sample_error"], "not sampled in tests");
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn a_step_is_a_beat_and_the_next_stage_clears_it() {
    let before = BEATS.load(Ordering::Relaxed);
    step("search-retro");
    assert_eq!(current_step(), "search-retro");
    beat(Stage::Sleep);
    assert_eq!(current_step(), "");
    assert!(BEATS.load(Ordering::Relaxed) >= before + 2);
}

#[test]
fn only_the_children_of_the_stalled_process_are_listed() {
    let listing = "  23444 23071 21:35:02 /home/user/.local/bin/script-guard classify\n  501 1 03:10 /bin/zsh -l\n 23445 230710 00:01 ps\n 23446 23071 00:00 /bin/ps -axo pid=,ppid=,etime=,command=\nbroken line\n";
    let rows = child_rows(listing, 23071);
    assert_eq!(rows, vec![json!({"pid": 23444, "etime": "21:35:02", "command": "/home/user/.local/bin/script-guard classify"})]);
}

#[test]
fn a_drill_record_is_marked_without_a_forced_recovery() {
    let home = crate::telemetry::test_home("stall-drill-record");
    let records = root(&home).join(RECORDS);
    std::fs::create_dir_all(&records).unwrap();
    std::fs::write(records.join("1794470000000.json"), br#"{"at_ms":1794470000000.0,"outcome":"restarted","name":"cpu-watchdog","drill":true}"#).unwrap();
    report_pending(&home);
    assert!(records.join("1794470000000.reported").exists());
    assert!(!root(&home).join("incidents").exists() || std::fs::read_dir(root(&home).join("incidents")).unwrap().next().is_none());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn stage_codes_have_names() {
    assert_eq!(stage_name(Stage::Rotate as u8), "rotate");
    assert_eq!(stage_name(Stage::Sweep as u8), "sweep");
    assert_eq!(stage_name(Stage::Sleep as u8), "sleep");
    assert_eq!(stage_name(0), "start");
}
