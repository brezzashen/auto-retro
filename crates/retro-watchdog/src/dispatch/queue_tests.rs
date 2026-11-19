use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn pressure_requires_complete_fresh_cool_window_and_normal_memory() {
    let mut history = json!({"updated":100_000,"samples":[
        {"start":70_000,"end":85_000,"percent":70},
        {"start":85_000,"end":100_000,"percent":80}]});
    assert!(pressure::evaluate(&history, 100_000.0, 1).is_ok());
    assert!(pressure::evaluate(&history, 100_000.0, 2).is_err());
    assert!(pressure::evaluate(&history, 116_000.0, 1).is_err());
    history["samples"][1]["percent"] = json!(85);
    assert!(pressure::evaluate(&history, 100_000.0, 1).is_err());
    history["samples"][1]["percent"] = json!(80);
    history["samples"][1]["start"] = json!(87_000);
    assert!(pressure::evaluate(&history, 100_000.0, 1).is_err());
    assert!(pressure::evaluate(&json!({"updated":100_000,"samples":[]}), 100_000.0, 1).is_err());
}

#[test]
fn concurrent_triggers_persist_but_only_max_active_start_until_ack() {
    let home = crate::telemetry::test_home("analysis-queue");
    std::thread::scope(|scope| {
        for n in 0..8 { let home = &home; scope.spawn(move || { enqueue(home, &n.to_string(), "test", "test").unwrap(); }); }
    });
    enqueue(&home, "0", "duplicate", "duplicate").unwrap();
    assert_eq!(read(&home).unwrap()["jobs"].as_array().unwrap().len(), 8);
    let started = AtomicUsize::new(0);
    let launch = |_: &Value| { started.fetch_add(1, Ordering::SeqCst); Ok(json!({"mode":"test"})) };
    poll_with(&home, 1000.0, Err("busy".into()), &launch).unwrap();
    assert_eq!(started.load(Ordering::SeqCst), 0);
    for at in [2000.0, 3000.0, 3500.0] { poll_with(&home, at, Ok(()), &launch).unwrap(); }
    assert_eq!(started.load(Ordering::SeqCst), MAX_ACTIVE, "one start per poll, then the cap holds");
    let state = read(&home).unwrap();
    assert_eq!(state["waiting"], format!("{MAX_ACTIVE} analyses already active (limit {MAX_ACTIVE})"));
    let active = state["jobs"].as_array().unwrap().iter().find(|j| j["state"] == "launched").unwrap();
    write_json(&root(&home).join(format!("incidents/{}.ack", active["incident"].as_str().unwrap())), &json!({})).unwrap();
    poll_with(&home, 4000.0, Ok(()), &launch).unwrap();
    assert_eq!(started.load(Ordering::SeqCst), MAX_ACTIVE + 1);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn ambiguous_launch_survives_reload_without_replay() {
    let home = crate::telemetry::test_home("analysis-ambiguous");
    for id in ["1", "2", "3"] { enqueue(&home, id, "test", "test").unwrap(); }
    poll_with(&home, 1000.0, Ok(()), |_| anyhow::bail!("terminal response lost")).unwrap();
    // The uncertain job keeps its slot and is never replayed; the free slot takes the next job.
    poll_with(&home, 2000.0, Ok(()), |job: &Value| { assert_eq!(job["incident"], "2", "must not replay job 1"); Ok(json!({"mode":"test"})) }).unwrap();
    poll_with(&home, 3000.0, Ok(()), |_| panic!("both slots are held")).unwrap();
    let state = read(&home).unwrap();
    assert_eq!((state["jobs"][0]["state"].as_str(), state["jobs"][2]["state"].as_str()), (Some("uncertain"), Some("queued")));
    assert!(state["error"].is_string());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn quota_failure_before_any_launch_remains_retryable() {
    let home = crate::telemetry::test_home("analysis-preflight");
    enqueue(&home, "1", "test", "test").unwrap();
    poll_with(&home, 1000.0, Ok(()), |_| Err(anyhow::Error::new(Preflight("no quota".into())))).unwrap();
    assert_eq!(read(&home).unwrap()["jobs"][0]["state"], "queued");
    poll_with(&home, 2000.0, Ok(()), |_| Ok(json!({"mode":"test"}))).unwrap();
    assert_eq!(read(&home).unwrap()["jobs"][0]["state"], "launched");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn ended_session_is_taken_over_once_then_abandoned_freeing_the_slot() {
    let home = crate::telemetry::test_home("analysis-liveness");
    enqueue(&home, "1", "test", "original task").unwrap();
    enqueue(&home, "2", "test", "next task").unwrap();
    let launched = std::cell::RefCell::new(Vec::new());
    let launch = |job: &Value| { launched.borrow_mut().push(job["prompt"].as_str().unwrap().to_string()); Ok(json!({"mode":"test"})) };
    poll_with(&home, 1000.0, Ok(()), &launch).unwrap();
    let dead = json!({"path":"/t.jsonl","idle_min":90,"live":null});
    let hour = 3_600_000.0;
    reap(&home, 1000.0 + hour, |_, _| panic!("too young to probe")).unwrap();
    reap(&home, 1000.0 + 3.0 * hour, |_, _| Ok(Some(json!({"path":"/t.jsonl","idle_min":5,"live":"running"})))).unwrap();
    reap(&home, 1000.0 + 3.0 * hour + 1000.0, |_, _| panic!("probed again inside the interval")).unwrap();
    reap(&home, 1000.0 + 4.0 * hour, |_, _| Ok(Some(dead.clone()))).unwrap();
    assert_eq!(read(&home).unwrap()["jobs"][0]["state"], "queued");
    poll_with(&home, 1000.0 + 4.0 * hour, Ok(()), &launch).unwrap();
    assert!(launched.borrow()[1].starts_with("TAKEOVER 1 of cpu-watchdog incident 1") && launched.borrow()[1].ends_with("original task"));
    reap(&home, 1000.0 + 7.0 * hour, |_, _| Ok(Some(dead.clone()))).unwrap();
    let state = read(&home).unwrap();
    assert_eq!(state["jobs"][0]["state"], "abandoned");
    assert_eq!(state["jobs"][0]["ended_sessions"].as_array().unwrap().len(), 2);
    poll_with(&home, 1000.0 + 7.0 * hour, Ok(()), &launch).unwrap();
    assert_eq!(launched.borrow().last().unwrap(), "next task");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn urgent_jobs_start_before_every_queued_job_in_their_own_order() {
    let home = crate::telemetry::test_home("analysis-queue-urgent");
    for id in ["a", "b"] { enqueue(&home, id, "test", "test").unwrap(); }
    for id in ["u1", "u2"] { enqueue_urgent(&home, id, "test", "test").unwrap(); }
    let order: Vec<String> = read(&home).unwrap()["jobs"].as_array().unwrap().iter()
        .map(|j| j["incident"].as_str().unwrap().to_string()).collect();
    assert_eq!(order, ["u1", "u2", "a", "b"]);
    let launched = std::sync::Mutex::new(String::new());
    poll_with(&home, 1000.0, Ok(()), |job: &Value| { *launched.lock().unwrap() = job["incident"].as_str().unwrap().into(); Ok(json!({"mode":"test"})) }).unwrap();
    assert_eq!(*launched.lock().unwrap(), "u1");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn an_urgent_job_jumps_a_requeued_takeover_and_takes_the_reserved_slot() {
    let home = crate::telemetry::test_home("analysis-queue-urgent-slot");
    for id in ["a", "b", "c"] { enqueue(&home, id, "test", "test").unwrap(); }
    let launched = std::cell::RefCell::new(Vec::new());
    let launch = |job: &Value| { launched.borrow_mut().push(job["incident"].as_str().unwrap().to_string()); Ok(json!({"mode":"test"})) };
    for at in [1000.0, 2000.0] { poll_with(&home, at, Ok(()), &launch).unwrap(); }
    enqueue_urgent(&home, "u1", "test", "test").unwrap();
    enqueue_urgent(&home, "u2", "test", "test").unwrap();
    // A takeover requeued in place can sit ahead of the urgent jobs.
    let mut state = read(&home).unwrap();
    state["jobs"].as_array_mut().unwrap().insert(0, json!({"incident":"t","title":"t","prompt":"takeover","state":"queued"}));
    write_json(&root(&home).join("analysis-queue.json"), &state).unwrap();
    poll_with(&home, 3000.0, Err("busy".into()), &launch).unwrap();
    assert_eq!(launched.borrow().len(), 2, "the CPU gate still applies to urgent jobs");
    for at in [4000.0, 5000.0] { poll_with(&home, at, Ok(()), &launch).unwrap(); }
    assert_eq!(*launched.borrow(), ["a", "b", "u1"], "u1 takes the reserved slot ahead of the takeover; u2 waits");
    assert_eq!(read(&home).unwrap()["waiting"], format!("{} analyses already active (limit {})", MAX_ACTIVE + URGENT_RESERVE, MAX_ACTIVE + URGENT_RESERVE));
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn uncertain_launch_is_requeued_only_when_no_session_opened() {
    let home = crate::telemetry::test_home("analysis-uncertain");
    enqueue(&home, "1", "test", "original task").unwrap();
    enqueue(&home, "2", "test", "next task").unwrap();
    poll_with(&home, 1000.0, Ok(()), |_| anyhow::bail!("terminal response lost")).unwrap();
    let hour = 3_600_000.0;
    reap(&home, 1000.0 + hour, |_, _| panic!("too young to probe")).unwrap();
    reap(&home, 1000.0 + 3.0 * hour, |_, _| Ok(None)).unwrap();
    let state = read(&home).unwrap();
    assert_eq!(state["jobs"][0]["state"], "queued");
    assert_eq!(state["jobs"][0]["prompt"], "original task");
    assert!(state["jobs"][0]["error"].is_null());
    let launched = std::cell::RefCell::new(Vec::new());
    poll_with(&home, 1000.0 + 3.0 * hour, Ok(()), |job: &Value| {
        launched.borrow_mut().push(job["incident"].as_str().unwrap().to_string()); Ok(json!({"mode":"test"})) }).unwrap();
    assert_eq!(*launched.borrow(), ["1"]);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn uncertain_launch_with_a_live_session_counts_as_launched() {
    let home = crate::telemetry::test_home("analysis-uncertain-live");
    enqueue(&home, "1", "test", "original task").unwrap();
    poll_with(&home, 1000.0, Ok(()), |_| anyhow::bail!("terminal response lost")).unwrap();
    reap(&home, 1000.0 + 3.0 * 3_600_000.0, |_, _| Ok(Some(json!({"path":"/t.jsonl","idle_min":2,"live":"running"})))).unwrap();
    assert_eq!(read(&home).unwrap()["jobs"][0]["state"], "launched");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_led_job_keeps_its_lead_model_through_a_takeover() {
    let home = crate::telemetry::test_home("analysis-queue-lead");
    enqueue_led(&home, "1", "test", "crash retro", "fable").unwrap();
    enqueue(&home, "2", "test", "ordinary retro").unwrap();
    let leads = std::cell::RefCell::new(Vec::new());
    let launch = |job: &Value| { leads.borrow_mut().push((job["incident"].as_str().unwrap().to_string(), job["lead"].as_str().map(str::to_string))); Ok(json!({"mode":"test"})) };
    for at in [1000.0, 2000.0] { poll_with(&home, at, Ok(()), &launch).unwrap(); }
    let hour = 3_600_000.0;
    reap(&home, 1000.0 + 4.0 * hour, |_, _| Ok(Some(json!({"path":"/t.jsonl","idle_min":90,"live":null})))).unwrap();
    poll_with(&home, 1000.0 + 4.0 * hour, Ok(()), &launch).unwrap();
    assert_eq!(*leads.borrow(), [("1".to_string(), Some("fable".to_string())), ("2".to_string(), None), ("1".to_string(), Some("fable".to_string()))]);
    std::fs::remove_dir_all(home).unwrap();
}
