use super::*;

fn job(pid: i64, tag: &str, age_s: i64) -> Job {
    Job { pid, tag: tag.into(), age_s, commit_mb: 7_000 }
}

const PLENTY: i64 = 60_000;

#[test]
fn an_orphan_is_reaped_on_its_second_round() {
    let jobs = [job(1, "p/held", 900), job(2, "p/orphan", 900)];
    let held = vec!["p/held".to_string()];
    let first = decide(&jobs, &held, &[], PLENTY, 2);
    assert_eq!(first, Plan { reap: vec![], relieve: vec![], suspects: vec![2] });
    let second = decide(&jobs, &held, &first.suspects, PLENTY, 2);
    assert_eq!(second, Plan { reap: vec![2], relieve: vec![], suspects: vec![] });
}

#[test]
fn a_young_or_held_job_is_never_an_orphan() {
    let jobs = [job(1, "p/held", 900), job(3, "p/starting", 30)];
    let plan = decide(&jobs, &["p/held".into()], &[1, 3], PLENTY, 2);
    assert_eq!(plan, Plan::default());
}

#[test]
fn short_of_commit_orphans_go_at_once_and_the_newest_render_makes_room() {
    // 2026-11-03: 7.7 GB free with seven orphans and two held renders.
    let mut jobs: Vec<Job> = (10..17).map(|p| job(p, &format!("p/o{p}"), 3_000)).collect();
    jobs.extend([job(1, "p/a", 900), job(2, "p/b", 300)]);
    let held = vec!["p/a".to_string(), "p/b".to_string()];
    let plan = decide(&jobs, &held, &[], 7_700, 2);
    assert_eq!(plan.reap, (10..17).collect::<Vec<_>>());
    assert!(plan.relieve.is_empty(), "reaping frees 49 GB, enough");
    let tight = decide(&[job(1, "p/a", 900), job(2, "p/b", 300)], &held, &[], 7_700, 2);
    assert_eq!(tight.relieve, vec![2]);
    assert!(decide(&[job(1, "p/a", 900)], &held, &[], 7_700, 0).relieve.is_empty(), "hourly cap");
}

#[test]
fn reads_one_or_many_jobs() {
    let one = json!({"pid": 5, "tag": "p/x", "age_s": 10, "commit_mb": 1});
    assert_eq!(jobs(&one).len(), 1);
    assert_eq!(jobs(&json!([one, {"pid": 6, "tag": "p/y"}])).len(), 2);
    assert!(jobs(&Value::Null).is_empty());
}

#[test]
fn held_jobs_need_a_live_owner() {
    let tmp = std::env::temp_dir().join(format!("win-guard-test-{}", std::process::id()));
    let live = tmp.as_path().join("win-gpu/slot-0");
    std::fs::create_dir_all(&live).unwrap();
    std::fs::write(live.join("owner.json"), json!({"pid": std::process::id(), "job": "p/live"}).to_string()).unwrap();
    let dead = tmp.as_path().join("win-gpu/slot-1");
    std::fs::create_dir_all(&dead).unwrap();
    std::fs::write(dead.join("owner.json"), r#"{"pid": 999999, "job": "p/dead"}"#).unwrap();
    assert_eq!(held_jobs(tmp.as_path(), "win"), vec!["p/live".to_string()]);
    let _ = std::fs::remove_dir_all(&tmp);
}
