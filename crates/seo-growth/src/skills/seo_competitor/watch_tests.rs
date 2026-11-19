use super::*;

fn f(key: &str, severity: &str) -> Finding {
    Finding { key: key.into(), severity: severity.into(), title: String::new(), action: String::new() }
}

fn at(t: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(t).unwrap()
}

#[test]
fn new_health_beats_keyword_work_and_handled_keys_never_repeat() {
    let state = WatchState { handled: vec!["rank:a".into()], ..Default::default() };
    let now = at("2026-11-02T14:00:00+08:00");
    assert_eq!(decide(&[f("health:x", "P0"), f("rank:b", "P1")], &state, (24.0, 6.0), 9, 8, now).unwrap().0, "健康修复");
    assert!(decide(&[f("rank:a", "P1")], &state, (24.0, 6.0), 9, 8, now).is_none());
}

#[test]
fn cooldown_holds_keyword_work_and_the_retro_runs_once_at_its_hour() {
    let now = at("2026-11-02T09:10:00+08:00");
    let state = WatchState { last_optimize_at: Some("2026-11-02T01:00:00+08:00".into()), ..Default::default() };
    assert_eq!(decide(&[f("rank:b", "P1")], &state, (24.0, 6.0), 9, 8, now).unwrap().0, "每日复盘");
    let done = WatchState { last_retro_day: Some("2026-11-02".into()), ..state };
    assert!(decide(&[f("rank:b", "P1")], &done, (24.0, 6.0), 9, 8, now).is_none());
}

#[test]
fn a_traffic_spike_opens_its_own_tab_despite_the_keyword_cooldown() {
    let now = at("2026-11-04T10:17:00+08:00");
    let state = WatchState { last_optimize_at: Some("2026-11-04T09:00:00+08:00".into()), ..Default::default() };
    let (kind, items) = decide(&[f("traffic:spike:2026-11-03", "P1"), f("rank:b", "P1")], &state, (24.0, 6.0), 9, 8, now).unwrap();
    assert_eq!((kind, items.len()), ("流量溯源", 1));
}
