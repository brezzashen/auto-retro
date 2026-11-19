use super::*;
use serde_json::json;

const DAY_MS: f64 = 86_400_000.0;
/// 2026-11-07 as a day number since 1970-01-01 (2026-10-26 Monday is 20_752).
const DAY: i64 = 20_764;

/// Singapore wall time `day` at `hh:mm`, as UTC seconds (what `observe` takes).
fn sgt(day: i64, hh: u32, mm: u32) -> f64 {
    (day as f64 * DAY_MS + (hh * 60 + mm) as f64 * 60_000.0 - 8.0 * 3_600_000.0) / 1000.0
}

fn commit(dir: &Path, n: usize, subject: &str) {
    for i in 0..n {
        std::fs::write(dir.join(format!("{}-{i}.md", subject.replace([' ', ':'], "-"))), format!("{subject} {i}")).unwrap();
        git(dir, &["add", "-A"]).unwrap();
        git(dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", &format!("{subject} {i}")]).unwrap();
    }
}

fn store_home(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = crate::telemetry::test_home(name);
    let dir = home.join(".agent-memory");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]).unwrap();
    commit(&dir, 1, "memory: seed");
    (home, dir)
}

fn ack(home: &Path, id: &str) {
    std::fs::write(root(home).join("incidents").join(format!("{id}.ack")), "").unwrap();
}

#[test]
fn slot_is_singapore_06_for_one_hour() {
    let ms = |day, hh, mm| sgt(day, hh, mm) * 1000.0;
    assert_eq!(slot_at(ms(DAY, 5, 59)), None);
    assert_eq!(slot_at(ms(DAY, 6, 0)), Some((format!("{DAY}:morning"), "morning")));
    assert_eq!(slot_at(ms(DAY, 6, 59)).map(|s| s.1), Some("morning"));
    assert_eq!(slot_at(ms(DAY, 7, 0)), None);
    assert_eq!(slot_at(ms(DAY, 18, 30)), None, "the 18:00 evening slot was dropped on 2026-11-18");
    // 06:00 Singapore is 22:00 UTC of the previous day, never a US-time 06:00.
    assert_eq!(slot_at((DAY as f64 * DAY_MS - 2.0 * 3_600_000.0) + 1.0).map(|s| s.1), Some("morning"));
    assert_eq!(slot_at(DAY as f64 * DAY_MS + 6.0 * 3_600_000.0), None, "06:00 UTC is 14:00 Singapore");
}

#[test]
fn each_slot_dreams_once_and_waits_for_the_previous_ack() {
    let (home, dir) = store_home("memory-dream-slots");
    let mut watch = MemoryDream::default();
    watch.observe(&home, sgt(DAY, 5, 50)).unwrap();
    assert!(watch.dispatches.is_empty(), "outside a slot nothing opens");

    commit(&dir, 3, "memory: add");
    watch.observe(&home, sgt(DAY, 6, 5)).unwrap();
    assert_eq!(watch.dispatches.len(), 1);
    assert_eq!(watch.dispatches[0]["slot"], format!("{DAY}:morning"));
    assert_eq!(watch.dispatches[0]["commits"], 3);
    let morning = watch.dispatches[0]["incident"].as_str().unwrap().to_string();
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents").join(format!("{morning}.json"))).unwrap()).unwrap();
    assert_eq!(incident["trigger"], "memory_dream_due");

    watch.observe(&home, sgt(DAY, 6, 40)).unwrap();
    assert_eq!(watch.dispatches.len(), 1, "one dream per slot");

    // No evening dream: 18:00 opens nothing.
    watch.observe(&home, sgt(DAY, 18, 20)).unwrap();
    assert_eq!(watch.dispatches.len(), 1, "one dream a day");

    // Next morning while the previous dream is unacknowledged: blocked, then opens after the ack.
    watch.observe(&home, sgt(DAY + 1, 6, 0)).unwrap();
    assert_eq!(watch.dispatches.len(), 1);
    assert_eq!(watch.blocked.as_ref().unwrap()["by"], morning.as_str());
    commit(&dir, 2, "memory: dream merge");
    commit(&dir, 4, "memory: add later");
    ack(&home, &morning);
    watch.observe(&home, sgt(DAY + 1, 6, 20)).unwrap();
    assert_eq!(watch.dispatches.len(), 2);
    assert_eq!(watch.dispatches[1]["slot"], format!("{}:morning", DAY + 1));
    assert_eq!(watch.dispatches[1]["commits"], 0, "the ack restarts the count at HEAD");
    assert!(watch.dispatches[0]["closed_head"].is_string());
    assert!(watch.blocked.is_none());

    // The morning after opens a new dream once the previous one is acknowledged.
    ack(&home, watch.dispatches[1]["incident"].as_str().unwrap());
    watch.observe(&home, sgt(DAY + 2, 6, 0)).unwrap();
    assert_eq!(watch.dispatches.len(), 3);
    assert_eq!(watch.dispatches[2]["slot"], format!("{}:morning", DAY + 2));
}

#[test]
fn ack_gate_needs_measurements_and_listed_gaps() {
    let (home, _) = store_home("memory-dream-ack");
    let id = "md1";
    let incidents = root(&home).join("incidents");
    std::fs::create_dir_all(&incidents).unwrap();
    let incident = incidents.join(format!("{id}.json"));
    std::fs::write(&incident, json!({"trigger": "memory_dream_due"}).to_string()).unwrap();
    let dir = root(&home).join("analysis").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(validate::validate_ack(&home, id, &incident).is_err(), "nothing measured");

    let eval = |top5: f64| json!({"sets": {"single": {"top5_pct": top5, "hook_injected_pct": 92.7}, "multi": {"coverage_top5_pct": 82.1}}});
    let scan = |broken: u64| json!({"stats": {"broken_link": broken, "superseded_link": 5, "missing_path": 60, "near_duplicate": 78}});
    let w = |name: &str, v: &Value| std::fs::write(dir.join(name), v.to_string()).unwrap();
    w("eval-before.json", &eval(94.7));
    w("eval-after.json", &eval(95.3));
    w("scan-before.json", &scan(60));
    w("scan-after.json", &scan(2));
    let mut actions: serde_json::Map<String, Value> = validate::ACTIONS.iter().map(|k| (k.to_string(), json!(0))).collect();
    actions.insert("gaps_recorded".into(), json!(1));
    actions.insert("hubs_updated".into(), json!(1));
    let mut report = json!({"report_url": "https://reports.example.com/x/",
        "eval": {"before": eval(94.7)["sets"], "after": eval(95.3)["sets"]},
        "scan": {"before": scan(60)["stats"], "after": scan(2)["stats"]}, "actions": actions, "misses_reviewed": 12,
        "gaps": [{"name": "recall-gap-windows-perf", "asked": "Windows Docker VM CPU history", "instead": "win-desktop perf counters on the ROG PC"}],
        "hubs": [{"name": "hub-seo-automation"}]});
    w("report.json", &report);
    validate::validate_ack(&home, id, &incident).unwrap();

    // A gap without where to get it instead, or a count that differs from the list, is refused.
    report["gaps"][0]["instead"] = json!("");
    w("report.json", &report);
    assert!(validate::validate_ack(&home, id, &incident).is_err());
    report["gaps"][0]["instead"] = json!("ask the founder");
    report["actions"]["hubs_updated"] = json!(2);
    w("report.json", &report);
    assert!(validate::validate_ack(&home, id, &incident).is_err());
    report["actions"]["hubs_updated"] = json!(1);

    // A declared number that differs from the measurement is refused.
    report["eval"]["after"]["single"]["top5_pct"] = json!(99.0);
    w("report.json", &report);
    assert!(validate::validate_ack(&home, id, &incident).is_err());

    // A real drop needs a reason.
    report["eval"]["after"]["single"]["top5_pct"] = json!(90.0);
    w("eval-after.json", &eval(90.0));
    w("report.json", &report);
    assert!(validate::validate_ack(&home, id, &incident).is_err());
    report["regression_reason"] = json!("merged notes moved two golden targets into one hub note");
    w("report.json", &report);
    validate::validate_ack(&home, id, &incident).unwrap();
}
