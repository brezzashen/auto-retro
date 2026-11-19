use super::*;

fn sample(at: f64, ms: f64) -> Sample {
    Sample { at, ms }
}

fn fences(fences: Vec<Sample>) -> Round {
    Round { fences, ..Round::default() }
}

fn incidents(home: &Path) -> Vec<Value> {
    // A joined episode also leaves its `.ack` beside the incident.
    std::fs::read_dir(root(home).join("incidents")).map(|entries| entries
        .map(|entry| entry.unwrap().path()).filter(|path| path.extension().is_some_and(|e| e == "json"))
        .map(|path| serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap())
        .collect()).unwrap_or_default()
}

#[test]
fn quiet_samples_summarize_without_an_incident() {
    let home = super::super::test_home("render-quiet");
    let mut probe = RenderProbe::default();
    let samples = (0..30).map(|i| sample(1000.0 + i as f64 * 0.5, 2.0 + i as f64)).collect();
    let summary = probe.summarize(&home, 1015.0, fences(samples)).unwrap();
    assert_eq!(summary["samples"], 30);
    assert_eq!(summary["stall_count"], 0);
    assert_eq!(summary["fence_ms"]["max"], 31.0);
    assert!(summary["episode_since_ms"].is_null());
    assert!(incidents(&home).is_empty());
    let day = std::fs::read_dir(root(&home).join("render-probe")).unwrap().count();
    assert_eq!(day, 1);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn stalls_list_and_one_incident_per_episode_until_the_gap_closes_it() {
    let home = super::super::test_home("render-stall");
    let mut probe = RenderProbe::default();
    let first = probe.summarize(&home, 1015.0, fences(vec![sample(1001.0, 3.0), sample(1002.0, 400.0), sample(1003.0, 5545.0)])).unwrap();
    assert_eq!(first["stall_count"], 2);
    assert_eq!(first["stalled_ms"], 5945.0);
    assert_eq!(first["episode_since_ms"], 1015000.0);
    // A later severe stall inside the gap extends the same episode: still one incident.
    probe.summarize(&home, 1200.0, fences(vec![sample(1199.0, 1500.0)])).unwrap();
    let found = incidents(&home);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["trigger"], "windowserver_render_stall");
    assert_eq!(found[0]["worst_fence_ms"], 5545.0);
    assert_eq!(found[0]["stalled_request"], "fence");
    // After EPISODE_GAP_SECS of quiet, the next severe stall opens a new episode.
    probe.summarize(&home, 1600.0, fences(vec![sample(1599.0, 3.0)])).unwrap();
    probe.summarize(&home, 1615.0, fences(vec![sample(1614.0, 2000.0)])).unwrap();
    assert_eq!(incidents(&home).len(), 2);
    // The first episode queued a performance session; the second joined it and was acknowledged as such.
    let wake = crate::telemetry::read_state(&root(&home).join("wake-perf.json")).unwrap();
    assert_eq!(wake["episodes"][0]["outcome"], "queued");
    assert_eq!(wake["episodes"][1]["outcome"], "joined");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_request_still_in_flight_counts_as_a_stall_and_errors_are_reported() {
    let home = super::super::test_home("render-inflight");
    let mut probe = RenderProbe::default();
    let summary = probe.summarize(&home, 1015.0, Round { inflight_ms: Some(3200.0), error: Some("render server returned no fence".into()), ..Round::default() }).unwrap();
    assert_eq!(summary["samples"], 0);
    assert!(summary["fence_ms"]["p50"].is_null());
    assert_eq!(summary["error"], "render server returned no fence");
    assert_eq!(incidents(&home)[0]["inflight_ms"], 3200.0);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_stall_right_after_display_sleep_is_a_wake_and_opens_no_incident() {
    let home = super::super::test_home("render-wake");
    let mut probe = RenderProbe::default();
    // 2026-10-29 07:35: the display was asleep at 1000.0 and the next fence, 0.7 s later, took 1.5 s.
    let wake = probe.summarize(&home, 1015.0, Round { fences: vec![sample(1000.7, 1515.0)], asleep_at: Some(1000.0), ..Round::default() }).unwrap();
    assert_eq!(wake["display_wake"], true);
    assert_eq!(wake["stall_count"], 1);
    assert!(wake["episode_since_ms"].is_null());
    // A request still in flight since just after the display slept is the same wake.
    let inflight = probe.summarize(&home, 1030.0, Round { inflight_ms: Some(1500.0), asleep_at: Some(1027.0), ..Round::default() }).unwrap();
    assert_eq!(inflight["display_wake"], true);
    assert!(incidents(&home).is_empty());
    // The same stall long after the display was last asleep is render load again.
    let load = probe.summarize(&home, 1100.0, Round { fences: vec![sample(1090.0, 1515.0)], asleep_at: Some(1000.0), ..Round::default() }).unwrap();
    assert_eq!(load["display_wake"], false);
    assert_eq!(incidents(&home).len(), 1);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_slow_window_list_is_named_as_such_and_never_read_as_a_slow_fence() {
    let home = super::super::test_home("render-list");
    let mut probe = RenderProbe::default();
    // 2026-11-18 23:13 SGT: fences of about 1 ms while a window list had been in flight for 4.3 s.
    // Under the shared clock this opened "render fence took 4291 ms"; below the list level it opens nothing.
    let below = probe.summarize(&home, 1015.0, Round { fences: vec![sample(1014.0, 1.2)], list_inflight_ms: Some(4291.0), ..Round::default() }).unwrap();
    assert!(below["inflight_ms"].is_null());
    assert_eq!(below["window_list_inflight_ms"], 4291.0);
    assert_eq!(below["stall_count"], 0);
    assert!(incidents(&home).is_empty());
    // The same wait finished at 6.6 s: an incident that names the window list and the fence beside it.
    let slow = probe.summarize(&home, 1030.0, Round { fences: vec![sample(1029.0, 0.2)], lists: vec![sample(1014.8, 6622.0)], ..Round::default() }).unwrap();
    assert_eq!(slow["window_list_ms"]["max"], 6622.0);
    assert_eq!(slow["window_list_samples"], 1);
    let found = incidents(&home);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["stalled_request"], "window_list");
    assert_eq!(found[0]["worst_fence_ms"], 0.2);
    assert_eq!(found[0]["window_list_max_ms"], 6622.0);
    assert!(found[0]["inflight_ms"].is_null());
    let wake = crate::telemetry::read_state(&root(&home).join("wake-perf.json")).unwrap();
    assert_eq!(wake["episodes"][0]["detail"], "WindowServer window list (main connection) took 6622 ms; render fence at most 0 ms");
    // A slow fence in the same tick takes the name: it is the request apps freeze on.
    probe.summarize(&home, 1700.0, Round { fences: vec![sample(1690.0, 1200.0)], list_inflight_ms: Some(9000.0), ..Round::default() }).unwrap();
    let both: Vec<Value> = incidents(&home).into_iter().filter(|i| i["stalled_request"] == "fence").collect();
    assert_eq!(both.len(), 1);
    assert_eq!(both[0]["window_list_inflight_ms"], 9000.0);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_window_list_that_waited_while_the_display_slept_opens_no_incident() {
    let home = super::super::test_home("render-list-asleep");
    let mut probe = RenderProbe::default();
    // The list began at 1000.0; the fence thread saw the display asleep until 1017.5, after that start.
    let asleep = probe.summarize(&home, 1018.0, Round { lists: vec![sample(1000.0, 17000.0)], asleep_at: Some(1017.5), ..Round::default() }).unwrap();
    assert_eq!(asleep["display_wake"], true);
    assert!(incidents(&home).is_empty());
    // Awake for a minute before the list began: the founder may be watching.
    let awake = probe.summarize(&home, 1100.0, Round { lists: vec![sample(1080.0, 17000.0)], asleep_at: Some(1017.5), ..Round::default() }).unwrap();
    assert_eq!(awake["display_wake"], false);
    assert_eq!(incidents(&home).len(), 1);
    std::fs::remove_dir_all(home).unwrap();
}
