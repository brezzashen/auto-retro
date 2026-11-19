use super::*;
use serde_json::json;

#[test]
fn a_trial_opens_once_after_its_due_time_and_retries_a_failed_start() {
    let mut watch = CacheTrial::default();
    watch.trials.push(json!({"id": "a", "due_ms": 1000.0, "since_ms": 0.0, "note": "n", "attempts": 0, "opened": false}));
    assert!(watch.due(999.0).is_empty());
    assert_eq!(watch.due(1000.0), vec![0]);
    watch.trials[0]["attempts"] = json!(1);
    watch.trials[0]["last_attempt_ms"] = json!(1000.0);
    assert!(watch.due(1000.0 + RETRY_SECS * 1000.0 - 1.0).is_empty(), "a failed start waits RETRY_SECS");
    assert_eq!(watch.due(1000.0 + RETRY_SECS * 1000.0), vec![0]);
    watch.trials[0]["attempts"] = json!(MAX_ATTEMPTS);
    assert!(watch.due(1.0e12).is_empty(), "attempts are capped");
    watch.trials[0]["attempts"] = json!(1);
    watch.trials[0]["opened"] = json!(true);
    assert!(watch.due(1.0e12).is_empty(), "an opened trial never opens again");
}

#[test]
fn the_loop_keeps_its_records_and_adopts_new_cli_trials() {
    let mine = vec![json!({"id": "a", "opened": true})];
    let disk = vec![json!({"id": "a", "opened": false}), json!({"id": "b", "opened": false})];
    let merged = merge(mine, disk);
    assert_eq!(merged, vec![json!({"id": "a", "opened": true}), json!({"id": "b", "opened": false})]);
}

#[test]
fn the_ack_gate_needs_both_arms_and_a_verdict() {
    let good = json!({"report_url": "https://reports.example.com/x/", "facts_kept": true, "verdict": "keep_trial",
        "reason": "trial turns too few to judge production yet",
        "arms": {"trial": {"turns": 12, "hit_rate": 0.86, "credits_per_turn": 9.5}, "baseline": {"turns": 80, "hit_rate": 0.81, "credits_per_turn": 11.0}}});
    assert!(check(&good).is_ok());
    let mut bad = good.clone();
    bad["arms"]["baseline"]["hit_rate"] = json!(81.0);
    assert!(check(&bad).is_err(), "hit rate is a 0..1 share");
    let mut bad = good.clone();
    bad["verdict"] = json!("ship");
    assert!(check(&bad).is_err());
    let mut bad = good.clone();
    bad["arms"]["trial"] = json!({});
    assert!(check(&bad).is_err());

    // An empty trial arm reports nulls, not invented rates, and can only keep the trial.
    let mut empty = good;
    empty["arms"]["trial"] = json!({"turns": 0, "hit_rate": null, "credits_per_turn": null});
    empty["facts_kept"] = json!(null);
    assert!(check(&empty).is_ok());
    let mut bad = empty.clone();
    bad["arms"]["trial"]["hit_rate"] = json!(0.0);
    assert!(check(&bad).is_err(), "0 turns cannot have a hit rate");
    let mut bad = empty.clone();
    bad["facts_kept"] = json!(true);
    assert!(check(&bad).is_err(), "nothing was read, so facts_kept stays null");
    let mut bad = empty;
    bad["verdict"] = json!("enable_production");
    assert!(check(&bad).is_err(), "an empty trial cannot enable production");
}
