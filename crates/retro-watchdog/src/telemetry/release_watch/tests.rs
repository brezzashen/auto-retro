use super::*;
use snapshot::Git;

/// Sources: `a` and `b` diverge and both change `web/src/app.ts`; `c` carries `a`.
struct FakeGit;
impl Git for FakeGit {
    fn resolve(&self, _cwd: &str, rev: &str) -> Option<String> { Some(rev.to_string()) }
    fn prefix(&self, _cwd: &str) -> String { "Mars_CRM/".into() }
    fn relation(&self, a: &str, b: &str) -> (&'static str, Vec<String>) {
        match (a, b) {
            ("a", "b") | ("b", "a") => ("divergent", vec!["Mars_CRM/web/src/app.ts".into()]),
            ("c", "a") => ("contains", vec![]),
            ("a", "c") => ("contained", vec![]),
            _ => ("unknown", vec![]),
        }
    }
    fn behind(&self, sha: &str) -> Option<u64> { (sha == "a").then_some(16) }
    fn pushed(&self, sha: &str) -> bool { sha.starts_with("pushed") }
    fn carried(&self, sha: &str, paths: &[String]) -> bool { sha.starts_with("copy") && !paths.is_empty() }
}

fn entry(id: &str, target: &str, queue: &str, session: &str, rev: &str, line: &str, pid: u32) -> Value {
    json!({"id": id, "target": target, "queue": queue, "purpose": format!("fix {id}"), "ref": rev, "inputs": [],
        "state": "running", "startedAt": "2026-10-25T14:00:00Z", "updatedAt": "2026-10-25T14:00:00Z",
        "creator": {"agent": "claude", "session": session, "orcaTab": format!("tab-{session}"), "cwd": "/nowhere", "pid": pid},
        "steps": [{"label": "build", "state": "running"}, {"label": "deploy", "state": "pending"}],
        "progress": {"step": "build", "line": line, "at": "2026-10-25T14:00:00Z"}})
}

const NOW_MS: f64 = 1_792_936_800_000.0; // 2026-10-25T14:00:00Z

fn tabs() -> snapshot::Tabs {
    HashMap::from([("tab-s1".to_string(), json!({"title": "删除修复→beta", "terminal": "term_s1"}))])
}

/// The image-map.json shape: beta crm is rebuilt by web sources, beta api by api sources.
fn images() -> Value {
    json!({"images": [
        {"deployments": [{"env": "beta"}], "trigger_paths": ["Mars_CRM/web/src/**", "@intro-media"], "conflict_key": {"rollout": ["beta/deployment/crm"]}},
        {"deployments": [{"env": "beta"}], "trigger_paths": ["Mars_CRM/api/**"], "conflict_key": {"rollout": ["beta/deployment/api"]}},
        {"deployments": [{"env": "canary"}], "trigger_paths": ["Mars_CRM/web/src/**"], "conflict_key": {"rollout": ["canary/deployment/crm"]}},
    ]})
}

#[test]
fn snapshot_orders_each_environment_and_names_collisions() {
    let me = std::process::id();
    let board = vec![
        entry("3", "canary", "Q1", "s3", "c", "waiting for the rollout queue", me),
        entry("2", "beta", "Q2", "s2", "b", "building api", me),
        entry("1", "beta-web", "Q1", "s1", "a", "⏳ waiting up to 600s", me),
    ];
    let mut since = HashMap::new();
    let queue = snapshot::build(&board, &FakeGit, &tabs(), &images(), &Value::Null, NOW_MS, &mut since);
    assert_eq!(queue["agents"], 3);
    let beta = &queue["environments"]["beta"];
    assert_eq!(beta["agents"], 2);
    assert_eq!(beta["releases"][0]["id"], "1");
    assert_eq!(beta["releases"][0]["phase"], "waiting");
    assert_eq!(beta["releases"][0]["overlaps"][0]["relation"], "divergent");
    let line = queue["lines"]["2"].as_str().unwrap();
    assert!(line.starts_with("[queue] beta 2/2 · 2 agent(s) queued · ahead: Q1 \"fix 1\""), "{line}");
    assert!(line.contains("tab \"删除修复→beta\" term_s1"), "the run ahead names its Orca tab: {line}");
    assert!(line.contains("CONFLICT with s1 (divergent, 1 shared: Mars_CRM/web/src/app.ts; rollouts beta/deployment/crm; tab \"删除修复→beta\" term_s1)"), "{line}");
    assert_eq!(queue["contacts"].as_array().unwrap().len(), 3);
    assert!(queue["contacts"].as_array().unwrap().iter().any(|c| c["tab"]["terminal"] == "term_s1" && c["releases"] == json!(["1"])));
    assert!(queue["lines"]["1"].as_str().unwrap().contains("source 16 behind origin/mastra"));
    assert!(queue["lines"]["3"].as_str().unwrap().contains("canary 1/1 · 1 agent(s) queued · nothing ahead"));
    // The wait is measured from the first snapshot that saw the run waiting.
    let later = snapshot::build(&board, &FakeGit, &tabs(), &images(), &Value::Null, NOW_MS + 600_000.0, &mut since);
    assert_eq!(later["environments"]["beta"]["releases"][0]["waiting_s"], 600.0);
}

#[test]
fn triggers_cover_collision_inversion_wait_abandonment_failure_and_docker() {
    let me = std::process::id();
    let runs = Runs::spawn(1);
    let mut q1 = entry("1", "beta-web", "Q1", "s1", "a", "[build-lease] waiting for k8s-build-crm-7001 (pid 7001) to finish its image build", me);
    q1["startedAt"] = json!("2026-10-25T13:50:00Z"); // queued before the Q2 run started
    let board = vec![
        entry("2", "beta", "Q2", "s2", "b", "building api", runs.pid(0)),
        q1,
        entry("0", "canary", "Q2", "s0", "c", "rolling out", 0),
        json!({"id": "f1", "target": "canary", "purpose": "dictation 404", "state": "failed", "finishedAt": "2026-10-25T13:10:00Z"}),
        json!({"id": "f2", "target": "canary", "purpose": "dictation 404", "state": "failed", "finishedAt": "2026-10-25T13:40:00Z"}),
    ];
    let mut since = HashMap::from([("1".to_string(), NOW_MS - 1_000_000.0)]);
    let mut queue = snapshot::build(&board, &FakeGit, &tabs(), &images(), &Value::Null, NOW_MS, &mut since);
    // The build-lease holder 7001 runs under Q2 run "2" and started 60 s ago, long after Q1 began waiting.
    let procs = blocker::Processes { parents: HashMap::from([(7001, runs.pid(0) as i64)]), started_ms: HashMap::from([(7001, NOW_MS - 60_000.0)]) };
    blocker::resolve(&mut queue, &board, &procs);
    let found = triggers::detect(&queue, &board, &FakeGit, &[NOW_MS - 60_000.0], NOW_MS - 120_000.0, NOW_MS);
    let codes: Vec<&str> = found.iter().map(|t| t.code).collect();
    for code in ["T1", "T9", "T8", "T11", "T4", "T5"] { assert!(codes.contains(&code), "{code} missing from {codes:?}"); }
    assert_eq!(found.iter().filter(|t| t.code == "T9").count(), 1, "a collision is reported once per pair");
    // A Docker restart already seen opens nothing.
    assert!(!triggers::detect(&queue, &board, &FakeGit, &[NOW_MS - 60_000.0], NOW_MS - 60_000.0, NOW_MS).iter().any(|t| t.code == "T1"));
}

/// A Q1 that arrives after a Q2 run already started waits for that atomic step: no T8
/// (judges 866301798215253410, 81216417729426176291, 83579064678967087910 were this shape).
#[test]
fn q1_arriving_after_a_running_q2_is_no_inversion() {
    let me = std::process::id();
    let runs = Runs::spawn(1);
    let mut q2 = entry("2", "beta", "Q2", "s2", "c", "building api", runs.pid(0));
    q2["startedAt"] = json!("2026-10-25T13:55:00Z");
    let board = vec![q2, entry("1", "beta-web", "Q1", "s1", "c", "[build-lease] waiting for k8s-build-crm-7001 (pid 7001)", me)];
    let mut since = HashMap::from([("1".to_string(), NOW_MS - 60_000.0)]);
    let mut queue = snapshot::build(&board, &FakeGit, &tabs(), &images(), &Value::Null, NOW_MS, &mut since);
    // Even though the Q1 waits on that Q2's own lease, the Q2 started first.
    blocker::resolve(&mut queue, &board, &blocker::Processes::parents(HashMap::from([(7001, runs.pid(0) as i64)])));
    let found = triggers::detect(&queue, &board, &FakeGit, &[], 0.0, NOW_MS);
    assert!(!found.iter().any(|t| t.code == "T8"), "{found:?}");
}

#[test]
fn one_judge_at_a_time_and_each_signature_once() {
    let home = crate::telemetry::test_home("release-watch");
    let releases = deliver_dir(&home).join("releases");
    std::fs::create_dir_all(&releases).unwrap();
    std::fs::write(releases.join("a.json"), entry("a", "canary", "Q1", "s1", "HEAD", "rolling out", 0).to_string()).unwrap();
    let mut watch = ReleaseWatch::default();
    let now = NOW_MS / 1000.0;
    watch.observe(&home, now).unwrap();
    assert!(deliver_dir(&home).join("queue.json").exists());
    let state = crate::telemetry::read_state(&root(&home).join("release-watch.json")).unwrap();
    assert_eq!(state["dispatches"].as_array().unwrap().len(), 1);
    let id = state["dispatches"][0]["incident"].as_str().unwrap().to_string();
    assert_eq!(state["open_incident"], id.as_str());
    std::fs::write(releases.join("b.json"), entry("b", "beta", "Q1", "s2", "HEAD", "building", 0).to_string()).unwrap();
    watch.observe(&home, now + SAMPLE_SECS).unwrap();
    assert_eq!(watch.dispatches.len(), 1, "no second judge while one is open");
    std::fs::write(root(&home).join(format!("incidents/{id}.ack")), "{}").unwrap();
    watch.observe(&home, now + 2.0 * SAMPLE_SECS).unwrap();
    assert_eq!(watch.dispatches.len(), 2, "the held signal opens after the ack");
    watch.observe(&home, now + 3.0 * SAMPLE_SECS).unwrap();
    assert_eq!(watch.dispatches.len(), 2, "a judged signature does not reopen");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_freeze_protects_its_allowed_release_and_flags_every_other_run() {
    let me = std::process::id();
    // `a` is the allowed candidate; it has waited long and printed nothing for long, yet no judge acts on it.
    let mut allowed = entry("1", "canary", "Q1", "s1", "a", "building api", me);
    allowed["progress"]["at"] = json!("2026-10-25T13:00:00Z");
    let board = vec![entry("2", "canary", "Q2", "s2", "b", "rolling out", me), allowed];
    let freeze = json!({"canary": {"reason": "Q1 acceptance", "allowed": ["a"], "by": "coordinator"}});
    let queue = snapshot::build(&board, &FakeGit, &tabs(), &images(), &freeze, NOW_MS, &mut HashMap::new());
    assert!(queue["lines"]["1"].as_str().unwrap().contains("FROZEN, this run is the allowed release (Q1 acceptance)"));
    assert!(queue["lines"]["2"].as_str().unwrap().contains("FROZEN (Q1 acceptance): only a may release"));
    let found = triggers::detect(&queue, &board, &FakeGit, &[], NOW_MS, NOW_MS);
    assert_eq!(found.iter().map(|t| t.code).collect::<Vec<_>>(), vec!["T10"], "{found:?}");
    assert_eq!(found[0].affected, vec!["2".to_string()]);
}

/// `cargo test -p cpu-watchdog release_preview -- --ignored --nocapture`: this host's live queue
/// and the triggers it would raise, without writing state or opening a judge.
#[test]
#[ignore]
fn release_preview() {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let board = snapshot::board(&deliver_dir(&home).join("releases"));
    let queue = snapshot::build(&board, &git::RepoGit::default(), &git::orca_tabs(), &crate::telemetry::read_state(&deliver_dir(&home).join("image-map.json")).unwrap_or(Value::Null), &crate::telemetry::read_state(&deliver_dir(&home).join("freeze.json")).unwrap_or(Value::Null), crate::unix_now() * 1000.0, &mut HashMap::new());
    for (id, line) in queue["lines"].as_object().unwrap() { println!("{id}: {}", line.as_str().unwrap()); }
    for t in triggers::detect(&queue, &board, &git::RepoGit::default(), &[], f64::MAX, crate::unix_now() * 1000.0) { println!("{} {}", t.code, t.summary); }
}

#[test]
fn an_abandoned_run_never_waits_and_reports_t4_once_while_fresh() {
    // Shape of 20261025T150117Z-beta-web-68517: board state running, creator pid gone, last line a wait.
    let mut stale = entry("20261025T150117Z-beta-web-68517", "beta-web", "Q2", "s1", "a", "⏳ waiting up to 600s for the build lease", 0);
    stale["progress"]["at"] = json!("2026-10-25T07:00:00Z");
    let mut fresh = entry("fresh-dead", "beta", "Q2", "s2", "b", "queued behind 2 runs", 0);
    fresh["progress"]["at"] = json!("2026-10-25T13:50:00Z");
    let board = vec![stale, fresh];
    let mut since = HashMap::from([("20261025T150117Z-beta-web-68517".to_string(), NOW_MS - 30_000_000.0)]);
    let queue = snapshot::build(&board, &FakeGit, &tabs(), &images(), &Value::Null, NOW_MS, &mut since);
    let releases = queue["environments"]["beta"]["releases"].as_array().unwrap();
    assert_eq!(releases.len(), 2);
    for r in releases {
        assert_eq!((r["state"].as_str(), r["phase"].as_str()), (Some("abandoned"), Some("abandoned")), "{}", r["id"]);
    }
    let found = triggers::detect(&queue, &board, &FakeGit, &[], NOW_MS, NOW_MS);
    let signatures: Vec<&str> = found.iter().map(|t| t.signature.as_str()).collect();
    assert_eq!(signatures, vec!["T4:abandoned:fresh-dead"], "no T11 or T9 for dead runs; the 7 h old one stays quiet");
}

#[test]
fn a_later_success_supersedes_failures_and_the_t5_signature_survives_a_sliding_window() {
    // Tonight's Q2-C2 canary runs: four failures, then 33795 succeeded.
    let run = |id: &str, state: &str, at: &str| json!({"id": id, "target": "canary", "queue": "Q2", "task": "Q2-C2",
        "purpose": "Canary catches up to beta-accepted Q2", "state": state, "finishedAt": at});
    let failed = vec![
        run("20261025T182713Z-canary-5964", "failed", "2026-10-25T18:37:46Z"),
        run("20261025T182544Z-canary-2008", "failed", "2026-10-25T18:25:44Z"),
        run("20261025T181536Z-canary-81332", "failed", "2026-10-25T18:23:21Z"),
        run("20261025T180601Z-canary-59702", "failed", "2026-10-25T18:10:10Z"),
    ];
    let ms = |at: &str| crate::cause_tree::attribute::parse_time(at).unwrap();
    let t5 = |board: &[Value], now: &str| triggers::detect(&json!({}), board, &FakeGit, &[], 0.0, ms(now)).into_iter()
        .filter(|t| t.code == "T5").map(|t| t.signature).collect::<Vec<_>>();
    // Before the success: one T5 named after the streak's first failure, unchanged as the window slides.
    assert_eq!(t5(&failed, "2026-10-25T18:40:00Z"), vec!["T5:canary:20261025T180601Z-canary-59702"]);
    assert_eq!(t5(&failed, "2026-10-25T20:15:00Z"), vec!["T5:canary:20261025T180601Z-canary-59702"]);
    // After 33795 succeeded: nothing, at any later time inside the window.
    let mut board = vec![run("20261025T184043Z-canary-33795", "succeeded", "2026-10-25T19:15:04Z")];
    board.extend(failed);
    assert!(t5(&board, "2026-10-25T19:20:00Z").is_empty());
    assert!(t5(&board, "2026-10-25T20:15:00Z").is_empty());
    // A new streak after the success is reported under its own first failure.
    board.insert(0, run("new-b", "failed", "2026-10-25T19:50:00Z"));
    board.insert(0, run("new-c", "failed", "2026-10-25T20:05:00Z"));
    assert_eq!(t5(&board, "2026-10-25T20:10:00Z"), vec!["T5:canary:new-b"]);
}

#[test]
fn a_retry_with_a_reworded_purpose_supersedes_failures_of_its_source() {
    // Judge 816763144392070241090: 12837 (d66f, missing --migrate) and 23628 (f83a, flaky test)
    // failed; 64666 retried f83a with a new purpose and succeeded, yet T5 fired 14 min later.
    let run = |id: &str, source: &str, purpose: &str, state: &str, at: &str| json!({"id": id, "target": "canary",
        "queue": "Q2", "source": source, "purpose": purpose, "state": state, "finishedAt": at});
    let p = "Landing hero: App Store button follows the title capsule tone (founder follow-up, accepted on beta)";
    let mut board = vec![
        run("20261106T011944Z-canary-23628", B, p, "failed", "2026-11-06T01:26:07Z"),
        run("20261106T011846Z-canary-12837", A, p, "failed", "2026-11-06T01:19:16Z"),
    ];
    let ms = |at: &str| crate::cause_tree::attribute::parse_time(at).unwrap();
    let t5 = |board: &[Value]| triggers::detect(&json!({}), board, &FakeGit, &[], 0.0, ms("2026-11-06T01:47:45Z")).into_iter()
        .filter(|t| t.code == "T5").count();
    assert_eq!(t5(&board), 1, "two failures of one item with no success are a T5");
    board.insert(0, run("20261106T012631Z-canary-64666", B, "Landing hero (retry after flaky preview media cache test)", "succeeded", "2026-11-06T01:33:32Z"));
    assert_eq!(t5(&board), 0, "the same-source success supersedes 23628, leaving one failure");
    // A success on another source does not supersede a failure of A when the purpose differs.
    board[0] = run("20261106T012631Z-canary-64666", C, "unrelated purpose", "succeeded", "2026-11-06T01:33:32Z");
    assert_eq!(t5(&board), 1);
}

#[test]
fn a_later_success_whose_source_carries_the_failed_sources_supersedes_them() {
    // Judge 818317066522885911452 (release retro 811322282323378476770): canary 85803 (d70eb69c33) and
    // 24617 (fc99cb95a2) stopped at the disk guard; 53125 shipped d297357211, which carries both, under
    // another item's purpose and succeeded, yet T5 still opened a judge.
    let run = |id: &str, source: &str, purpose: &str, state: &str, at: &str| json!({"id": id, "target": "canary",
        "queue": "Q2", "source": source, "purpose": purpose, "state": state, "finishedAt": at});
    let ja = "i18n(ja): Japanese Web catalog, native strings, onboarding";
    let mut board = vec![
        run("20261107T042500Z-canary-24617", B, ja, "failed", "2026-11-07T04:34:37Z"),
        run("20261107T041616Z-canary-85803", A, ja, "failed", "2026-11-07T04:21:35Z"),
    ];
    let ms = |at: &str| crate::cause_tree::attribute::parse_time(at).unwrap();
    let t5 = |board: &[Value], git: &dyn Git| triggers::detect(&json!({}), board, git, &[], 0.0, ms("2026-11-07T04:45:00Z")).into_iter()
        .filter(|t| t.code == "T5").count();
    assert_eq!(t5(&board, &ShaGit), 1);
    board.insert(0, run("20261107T043458Z-canary-53125", C, "Japanese, Korean and Thai: contracts, server copy", "succeeded", "2026-11-07T04:41:59Z"));
    assert_eq!(t5(&board, &ShaGit), 0, "C carries A and B");
    assert_eq!(t5(&board, &FakeGit), 1, "a success whose source does not carry them leaves the streak");
    // A success that finished before the failures never supersedes them.
    board[0] = run("20261107T043458Z-canary-53125", C, "Japanese, Korean and Thai: contracts, server copy", "succeeded", "2026-11-07T04:10:00Z");
    assert_eq!(t5(&board, &ShaGit), 1);
}

#[test]
fn a_live_retry_on_a_carrying_source_supersedes_failures_until_it_fails_too() {
    // Judge 816077057436881794164: 92284 (A = 1757869ced) and 59914 (B = eeedb1a935) failed the
    // ROUTE_FILE_DOMAIN guard; 83484 was already preparing C = 652a65f47b (the fix) when T5 opened.
    let run = |id: &str, source: &str, state: &str, at: &str| json!({"id": id, "target": "canary", "queue": "Q1",
        "source": source, "purpose": "Android/HarmonyOS /intro App Store download", "state": state,
        "startedAt": at, "finishedAt": at, "aliveAtRead": true});
    let mut board = vec![
        run("20261109T094348Z-canary-59914", B, "failed", "2026-11-09T09:44:52Z"),
        run("20261109T093309Z-canary-92284", A, "failed", "2026-11-09T09:34:41Z"),
    ];
    let ms = |at: &str| crate::cause_tree::attribute::parse_time(at).unwrap();
    let t5 = |board: &[Value], git: &dyn Git| triggers::detect(&json!({}), board, git, &[], 0.0, ms("2026-11-09T09:50:00Z")).into_iter()
        .filter(|t| t.code == "T5").count();
    assert_eq!(t5(&board, &ShaGit), 1, "two failures and no retry are a T5");
    board.insert(0, run("20261109T094634Z-canary-83484", C, "running", "2026-11-09T09:46:34Z"));
    assert_eq!(t5(&board, &ShaGit), 0, "the live retry on C carries A and B");
    assert_eq!(t5(&board, &FakeGit), 1, "a live run whose source does not carry them leaves the streak");
    board[0]["source"] = json!(B);
    assert_eq!(t5(&board, &ShaGit), 0, "a live retry of the same source B (which carries A) supersedes both");
    board[0]["source"] = json!(C);
    board[0]["aliveAtRead"] = json!(false);
    assert_eq!(t5(&board, &ShaGit), 1, "a dead retry supersedes nothing");
    board[0]["aliveAtRead"] = json!(true);
    board[0]["startedAt"] = json!("2026-11-09T09:30:00Z");
    assert_eq!(t5(&board, &ShaGit), 1, "a run that started before the failures is not their retry");
    board[0] = run("20261109T094634Z-canary-83484", C, "failed", "2026-11-09T09:49:00Z");
    assert_eq!(t5(&board, &ShaGit), 1, "a failed retry joins the streak");
}

#[test]
fn judge_prompt_uses_a_lowercase_slug_and_session_send() {
    let trigger = triggers::Trigger { code: "T5", signature: "T5:x".into(), summary: "starved beta".into(), affected: vec![] };
    let text = prompt(Path::new("/nowhere"), "42", &[&trigger]);
    assert!(text.contains("slug release-queue-t5-<YYYYMMDD-HHMM>"), "build.ts refuses an uppercase slug");
    assert!(text.contains("session-send <handle>") && !text.contains("orca terminal send"));
}

/// 40-character sources: `C` carries `B`, which carries `A`.
struct ShaGit;
const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const C: &str = "cccccccccccccccccccccccccccccccccccccccc";
impl Git for ShaGit {
    fn resolve(&self, _cwd: &str, rev: &str) -> Option<String> { Some(rev.to_string()) }
    fn prefix(&self, _cwd: &str) -> String { String::new() }
    fn relation(&self, a: &str, b: &str) -> (&'static str, Vec<String>) {
        let rank = |s: &str| [A, B, C].iter().position(|x| *x == s);
        match (rank(a), rank(b)) {
            (Some(x), Some(y)) if x > y => ("contains", vec![]),
            (Some(x), Some(y)) if x < y => ("contained", vec![]),
            _ => ("unknown", vec![]),
        }
    }
    fn behind(&self, _sha: &str) -> Option<u64> { None }
    fn pushed(&self, _sha: &str) -> bool { false }
}

fn verdict(id: &str, source: &str, verdict: &str, at: &str) -> Value {
    json!({"id": id, "target": "beta-web", "queue": "Q2", "purpose": format!("fix {id}"), "state": "succeeded", "source": source,
        "finishedAt": "2026-10-25T12:00:00Z", "creator": {"session": format!("{id}-session")}, "acceptance": {"verdict": verdict, "at": at}})
}

#[test]
fn a_refused_canary_candidate_raises_t6_once_while_fresh_and_never_t5() {
    let refusal = |id: &str, at: &str| json!({"id": id, "target": "canary", "queue": "Q2", "purpose": "ship origin/mastra", "state": "failed", "finishedAt": at,
        "failure": ["T6 contaminated canary candidate cccccccccc: carries aaaaaaaaaa whose acceptance failed"],
        "candidateCheck": {"contaminated": true, "t6": "T6 contaminated canary candidate cccccccccc: carries aaaaaaaaaa whose acceptance failed"}});
    let board = vec![refusal("r2", "2026-10-25T13:50:00Z"), refusal("r1", "2026-10-25T13:40:00Z"), refusal("old", "2026-10-25T11:00:00Z")];
    let found = triggers::detect(&json!({}), &board, &ShaGit, &[], NOW_MS, NOW_MS);
    let signatures: Vec<&str> = found.iter().map(|t| t.signature.as_str()).collect();
    assert_eq!(signatures, vec!["T6:r2", "T6:r1"], "no T5 for refusals, nothing for the 3 h old one");
    assert!(found[0].summary.contains("carries aaaaaaaaaa whose acceptance failed"));
    let affected = triggers::affected(&json!({}), &board, &found[0].affected);
    assert_eq!(affected[0]["failure"][0].as_str().map(|f| f.starts_with("T6")), Some(true), "the judge reads the T6 line from the board");
}

#[test]
fn a_running_canary_release_carrying_a_failed_acceptance_raises_t6_until_a_later_fix_passes() {
    let me = std::process::id();
    let mut canary = entry("run", "canary", "Q2", "s9", "origin/mastra", "building images", me);
    canary["source"] = json!(C);
    let failed = verdict("bad", A, "failed", "2026-10-25T12:00:00.000Z");
    let board = vec![canary.clone(), failed.clone()];
    let queue = snapshot::build(&board, &ShaGit, &tabs(), &images(), &Value::Null, NOW_MS, &mut HashMap::new());
    assert_eq!(queue["environments"]["canary"]["releases"][0]["source"], json!(C), "the recorded source wins over the moving ref");
    let found = triggers::detect(&queue, &board, &ShaGit, &[], NOW_MS, NOW_MS);
    assert_eq!(found.iter().map(|t| t.signature.as_str()).collect::<Vec<_>>(), vec!["T6:run"], "same signature as the refusal branch");
    assert_eq!(found[0].affected, vec!["run".to_string(), "bad".to_string()]);
    assert!(found[0].summary.contains("carrying aaaaaaaaaa (release bad, session"), "{}", found[0].summary);
    // Two unfixed failures on one run still open one judge.
    let board2 = vec![canary.clone(), failed.clone(), verdict("bad2", A, "failed", "2026-10-25T12:10:00.000Z")];
    let found = triggers::detect(&queue, &board2, &ShaGit, &[], NOW_MS, NOW_MS);
    assert_eq!(found.iter().filter(|t| t.code == "T6").count(), 1);
    assert_eq!(found[0].affected, vec!["run".to_string(), "bad".to_string(), "bad2".to_string()]);
    // B carries A and passed acceptance afterwards: the candidate carries the fix.
    let board = vec![canary, failed, verdict("fix", B, "passed", "2026-10-25T12:30:00.000Z")];
    assert!(triggers::detect(&queue, &board, &ShaGit, &[], NOW_MS, NOW_MS).iter().all(|t| t.code != "T6"));
}

/// Apple's processing of an uploaded build is a step of the run, not a queue wait
/// (release retro 84705495676337590512: all 4222 s of the window's iOS "queue wait").
#[test]
fn app_store_processing_is_running_not_waiting() {
    let me = std::process::id();
    let board = vec![entry("9", "ios", "Q2", "s9", "c", "build 1.0.10050 (10050) uploaded; waiting for App Store Connect processing", me)];
    let mut since = HashMap::new();
    let queue = snapshot::build(&board, &FakeGit, &tabs(), &images(), &Value::Null, NOW_MS, &mut since);
    let run = queue["environments"].as_object().unwrap().values()
        .flat_map(|e| e["releases"].as_array().cloned().unwrap_or_default()).find(|r| r["id"] == "9").unwrap();
    assert_eq!(run["phase"], "running", "{run}");
    assert!(since.is_empty(), "no wait clock starts");
}

/// Release retro 811587194803931207163: divergent sources with no common path are no T9.
#[test]
fn divergent_runs_without_a_common_path_are_no_collision() {
    let run = |id: &str, session: &str, other: &str, shared: u64| json!({"id": id, "state": "running", "creator": {"session": session},
        "overlaps": [{"with": other, "session": if session == "s1" { "s2" } else { "s1" }, "relation": "divergent", "shared": shared}]});
    let snapshot = |shared: u64| json!({"environments": {"beta-web": {"releases": [run("36311", "s1", "43941", shared), run("43941", "s2", "36311", shared)]}}});
    let t9 = |shared: u64| triggers::detect(&snapshot(shared), &[], &FakeGit, &[], 0.0, NOW_MS).into_iter().filter(|t| t.code == "T9").count();
    assert_eq!(t9(0), 0, "an empty intersection merges in one round");
    assert_eq!(t9(1), 1, "one common path is still a collision, reported once per pair");
}

/// Judge 81661975683923041583: two beta-web runs on pushed sources share a path, yet the gateway
/// builds only origin/mastra and refuses a stale copy, so it is no T9; anything else still is.
#[test]
fn pushed_beta_web_sources_sharing_a_path_are_no_collision() {
    let run = |id: &str, session: &str, other: &str, target: &str, source: &str| json!({"id": id, "state": "running",
        "target": target, "source": source, "creator": {"session": session},
        "overlaps": [{"with": other, "session": if session == "s1" { "s2" } else { "s1" }, "relation": "contained", "shared": 1}]});
    let t9 = |target: &str, first: &str, second: &str| {
        let snapshot = json!({"environments": {"beta": {"releases": [run("98344", "s1", "75138", target, first), run("75138", "s2", "98344", target, second)]}}});
        triggers::detect(&snapshot, &[], &FakeGit, &[], 0.0, NOW_MS).into_iter().filter(|t| t.code == "T9").count()
    };
    assert_eq!(t9("beta-web", "pushed-98561", "pushed-b700"), 0, "both on origin/mastra: the gateway ships the tip that carries both");
    assert_eq!(t9("beta-web", "pushed-98561", "local-b700"), 1, "an unpushed source may still need a judge");
    assert_eq!(t9("beta", "pushed-98561", "pushed-b700"), 1, "only the beta-web gateway path is exempt; other targets keep the judge");
}

/// Judges 86365156581115136928 and 81992542804222953903: a beta-web source left behind as a
/// pre-rebase local copy, whose named inputs already equal origin/mastra, ships only the pushed tip.
#[test]
fn pre_rebase_beta_web_copies_whose_inputs_are_on_origin_are_no_collision() {
    let run = |id: &str, session: &str, other: &str, source: &str, inputs: Value| json!({"id": id, "state": "running",
        "target": "beta-web", "source": source, "inputs": inputs, "creator": {"session": session},
        "overlaps": [{"with": other, "session": if session == "s1" { "s2" } else { "s1" }, "relation": "divergent", "shared": 10,
            "rollouts": ["beta/deployment/api"], "mapped": true}]});
    let t9 = |first: &str, second: &str, inputs: Value| {
        let snapshot = json!({"environments": {"beta": {"releases": [
            run("44895", "s1", "59990", first, inputs.clone()), run("59990", "s2", "44895", second, inputs)]}}});
        triggers::detect(&snapshot, &[], &FakeGit, &[], 0.0, NOW_MS).into_iter().filter(|t| t.code == "T9").count()
    };
    let named = json!(["Mars_CRM/web/src/screens/settings/PayStripe.tsx"]);
    assert_eq!(t9("copy-409cd0c566", "copy-5362a3e691", named.clone()), 0, "both copies carry only pushed bytes");
    assert_eq!(t9("pushed-b0712bd186", "copy-5362a3e691", named.clone()), 0, "a pushed source beside a carried copy");
    assert_eq!(t9("copy-409cd0c566", "local-5362a3e691", named), 1, "an input that differs from origin keeps the judge");
    assert_eq!(t9("copy-409cd0c566", "copy-5362a3e691", json!([])), 1, "a run naming no inputs proves nothing");
}

/// Judge 81992542804222953903: the 9 paths a beta-crm and a beta-web run shared were all
/// `.claude/skills/*`, which no beta image deploys; an unmapped environment keeps the judge.
#[test]
fn shared_paths_no_mapped_image_deploys_are_no_collision() {
    let run = |id: &str, session: &str, other: &str, mapped: bool, rollouts: Value| json!({"id": id, "state": "running",
        "target": "beta-crm", "source": "local", "creator": {"session": session},
        "overlaps": [{"with": other, "session": if session == "s1" { "s2" } else { "s1" }, "relation": "divergent", "shared": 9,
            "rollouts": rollouts, "mapped": mapped}]});
    let t9 = |mapped: bool, rollouts: Value| {
        let snapshot = json!({"environments": {"beta": {"releases": [
            run("67677", "s1", "59990", mapped, rollouts.clone()), run("59990", "s2", "67677", mapped, rollouts)]}}});
        triggers::detect(&snapshot, &[], &FakeGit, &[], 0.0, NOW_MS).into_iter().filter(|t| t.code == "T9").count()
    };
    assert_eq!(t9(true, json!([])), 0, "no deployed image carries the shared paths");
    assert_eq!(t9(true, json!(["beta/deployment/crm"])), 1, "a shared deployed path is still a collision");
    assert_eq!(t9(false, json!([])), 1, "without an image map an empty rollout list proves nothing");
}

/// The snapshot marks an overlap `mapped` only where the image map lists the environment.
#[test]
fn overlaps_say_whether_the_environment_is_mapped() {
    let me = std::process::id();
    let board = vec![entry("1", "beta-web", "Q1", "s1", "a", "building", me), entry("2", "beta", "Q2", "s2", "b", "building", me),
        entry("3", "ios", "Q2", "s3", "a", "building", me), entry("4", "ios", "Q2", "s4", "b", "building", me)];
    let mut since = HashMap::new();
    let queue = snapshot::build(&board, &FakeGit, &tabs(), &images(), &Value::Null, NOW_MS, &mut since);
    assert_eq!(queue["environments"]["beta"]["releases"][0]["overlaps"][0]["mapped"], true);
    assert_eq!(queue["environments"]["ios"]["releases"][0]["overlaps"][0]["mapped"], false);
}

/// Live child processes standing in for distinct `deliver` runs (a dead creator pid reads as abandoned).
struct Runs(Vec<std::process::Child>);
impl Runs {
    fn spawn(n: usize) -> Self {
        Runs((0..n).map(|_| std::process::Command::new("/bin/sleep").arg("120").spawn().expect("spawn sleep")).collect())
    }
    fn pid(&self, i: usize) -> u32 { self.0[i].id() }
}
impl Drop for Runs {
    fn drop(&mut self) { for c in &mut self.0 { let _ = c.kill(); let _ = c.wait(); } }
}

#[test]
fn holder_pid_reads_every_queue_wait_shape() {
    let cases = [
        ("⏳ deploy lease (local-k8s) held by 'mars-crm-k8s-deploy-53629' since 14:37:15 (14s ago, 1186s before it expires) — waiting up to 600s", Some(53629)),
        ("⏳ deploy lease (gke-canary-mars-crm) held by 'canary rollout de132e656 pid 96207' since 11:00:00 (5s ago, 3595s before it expires)", Some(96207)),
        ("[build-lease] waiting for k8s-build-crm-83268 (pid 83268) to finish its image build; 0 ahead in queue", Some(83268)),
        ("[ship-web] waiting for ship-web-verify-85532 (pid 85532) to finish its Storybook checks; 0 ahead in queue", Some(85532)),
        ("4671243dc7 is being prepared by pid 4410; waiting for its result", Some(4410)),
        ("queued for a canary rollout behind pid 28069 (checks since 12:48 SGT of 9b403dda4); 1 ahead", Some(28069)),
        ("[ship-web] queued for a ship-web round; 1 ahead", None),
        ("[cpu-build-gate] builds started in the last 30 s leave under 4 idle cores (26 busy); waiting for headroom", None),
    ];
    for (line, pid) in cases { assert_eq!(blocker::holder_pid(line), pid, "{line}"); }
}

/// Kubectl's own rollout status is the run working, not queued (judge 818310651505023274466).
#[test]
fn kubectl_rollout_status_is_not_a_queue_wait() {
    let board = vec![entry("1", "beta", "Q1", "s1", "c", "Waiting for deployment \"api\" rollout to finish: 1 old replicas are pending termination...", std::process::id())];
    let queue = snapshot::build(&board, &FakeGit, &tabs(), &images(), &Value::Null, NOW_MS, &mut HashMap::new());
    assert_eq!(queue["environments"]["beta"]["releases"][0]["phase"], "running");
}

/// The build gate's start line ends the wait: the build that follows is the run working (release
/// retro 813612464463489159041, mac 44719 counted 755 s queued after a 2 s gate wait).
#[test]
fn build_gate_start_line_is_not_a_queue_wait() {
    for line in ["[cpu-build-gate] starting after 2 s of waiting: 0 s for a slot, 0 s for the CPU window",
        "[cpu-build-gate] starting after 7 s of waiting for the CPU window (not a build: no build slot)"] {
        let board = vec![entry("1", "mac", "Q2", "s1", "c", line, std::process::id())];
        let queue = snapshot::build(&board, &FakeGit, &tabs(), &images(), &Value::Null, NOW_MS, &mut HashMap::new());
        assert_eq!(queue["environments"]["mac"]["releases"][0]["phase"], "running", "{line}");
    }
    let board = vec![entry("1", "mac", "Q2", "s1", "c",
        "[cpu-build-gate] builds started in the last 30 s leave under 4 idle cores (27 busy); waiting for headroom or this build's turn", std::process::id())];
    let queue = snapshot::build(&board, &FakeGit, &tabs(), &images(), &Value::Null, NOW_MS, &mut HashMap::new());
    assert_eq!(queue["environments"]["mac"]["releases"][0]["phase"], "waiting");
}

/// Shape of judge 81710544233103865991 (2026-11-04): Q1 beta 46509 waits on the local-k8s deploy lease
/// `mars-crm-k8s-deploy-53629`, held by the OLDER Q2 beta-web 79100 rolling out api, while the younger
/// Q2 beta-web 49101 started 10 s after Q1 and holds nothing. `holder` names the run whose process tree
/// the lease holder pid sits in; `None` leaves it off the board. The Q1 has waited 120 s; the holder
/// process started `holder_started_ms` (unknown when `None`).
fn lease_wait_case(holder: Option<usize>) -> (Runs, Value, Vec<Value>) { lease_wait_since(holder, None) }

fn lease_wait_since(holder: Option<usize>, holder_started_ms: Option<f64>) -> (Runs, Value, Vec<Value>) {
    let runs = Runs::spawn(3);
    let started = |e: &mut Value, at: &str| e["startedAt"] = json!(at);
    let mut old_q2 = entry("20261104T062701Z-beta-web-79100", "beta-web", "Q2", "s-old", "c",
        "Waiting for deployment \"api\" rollout to finish: 1 old replicas are pending termination...", runs.pid(0));
    started(&mut old_q2, "2026-10-25T13:49:08Z");
    let mut q1 = entry("20261104T063645Z-beta-46509", "beta", "Q1", "s-q1", "c",
        "⏳ deploy lease (local-k8s) held by 'mars-crm-k8s-deploy-53629' since 14:37:15 (14s ago, 1186s before it expires) — waiting up to 600s", runs.pid(1));
    started(&mut q1, "2026-10-25T13:58:52Z");
    let mut new_q2 = entry("20261104T063655Z-beta-web-49101", "beta-web", "Q2", "s-new", "c", "", runs.pid(2));
    started(&mut new_q2, "2026-10-25T13:59:02Z");
    let board = vec![new_q2, q1, old_q2];
    let mut since = HashMap::from([("20261104T063645Z-beta-46509".to_string(), NOW_MS - 120_000.0)]);
    let mut queue = snapshot::build(&board, &FakeGit, &tabs(), &images(), &Value::Null, NOW_MS, &mut since);
    // mars-ops (53629) under cpu-build-gate (53600) under the holder run's deliver process.
    let parents: HashMap<i64, i64> = holder.map_or_else(HashMap::new, |i| HashMap::from([(53629, 53600), (53600, runs.pid(i) as i64)]));
    let started_ms = holder_started_ms.map_or_else(HashMap::new, |at| HashMap::from([(53629, at)]));
    assert!(blocker::wanted(&queue));
    blocker::resolve(&mut queue, &board, &blocker::Processes { parents, started_ms });
    (runs, queue, board)
}

fn t8(queue: &Value, board: &[Value]) -> Vec<triggers::Trigger> {
    triggers::detect(queue, board, &FakeGit, &[], 0.0, NOW_MS).into_iter().filter(|t| t.code == "T8").collect()
}

#[test]
fn q1_waiting_on_an_older_q2_lease_is_no_inversion() {
    let (_runs, queue, board) = lease_wait_case(Some(0));
    let beta = queue["environments"]["beta"]["releases"].as_array().unwrap();
    let q1 = beta.iter().find(|r| r["queue"] == "Q1").unwrap();
    assert_eq!(q1["blocker"], json!({"pid": 53629, "run": "20261104T062701Z-beta-web-79100"}));
    assert!(beta.iter().all(|r| r["queue"] == "Q1" || r["phase"] == "running"), "kubectl rollout status is not waiting: {beta:?}");
    assert!(t8(&queue, &board).is_empty(), "{:?}", t8(&queue, &board));
}

#[test]
fn unresolved_lease_holder_is_no_inversion() {
    let (_runs, queue, board) = lease_wait_case(None);
    let q1 = &queue["environments"]["beta"]["releases"].as_array().unwrap().iter().find(|r| r["queue"] == "Q1").unwrap().clone();
    assert_eq!(q1["blocker"], json!({"pid": 53629, "run": null}));
    assert!(t8(&queue, &board).is_empty());
}

#[test]
fn q1_waiting_on_a_younger_q2_lease_is_an_inversion() {
    // The holder started 30 s ago, while the Q1 had already waited 90 s.
    let (_runs, queue, board) = lease_wait_since(Some(2), Some(NOW_MS - 30_000.0));
    let q1 = queue["environments"]["beta"]["releases"].as_array().unwrap().iter().find(|r| r["queue"] == "Q1").unwrap().clone();
    assert_eq!(q1["blocker"]["started_ms"], json!(NOW_MS - 30_000.0));
    let found = t8(&queue, &board);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].signature, "T8:20261104T063655Z-beta-web-49101:20261104T063645Z-beta-46509");
    assert!(found[0].summary.contains("waits on pid 53629"), "{}", found[0].summary);
}

/// Shape of judge 87261054405244924163 (2026-11-05): the younger Q2 took the free lease 161 s before
/// the older Q1, still building its image, asked for it. Waiting for that atomic step is by design.
#[test]
fn younger_q2_holding_before_the_q1_waited_is_no_inversion() {
    let (_runs, queue, board) = lease_wait_since(Some(2), Some(NOW_MS - 281_000.0));
    assert!(t8(&queue, &board).is_empty(), "{:?}", t8(&queue, &board));
    // Unknown holder start: no T8 either (T11 still covers a long wait).
    let (_runs, queue, board) = lease_wait_case(Some(2));
    assert!(t8(&queue, &board).is_empty(), "{:?}", t8(&queue, &board));
}

#[test]
fn ps_elapsed_times_give_process_starts() {
    for (text, secs) in [("05:07", Some(307.0)), ("01:02:03", Some(3723.0)), ("2-00:00:01", Some(172_801.0)), ("x", None), ("1:2:3:4", None)] {
        assert_eq!(blocker::etime_secs(text), secs, "{text}");
    }
    let procs = blocker::parse_ps("  69787 69700    02:41\n    1     0 3-04:05:06\nbad line\n", NOW_MS);
    assert_eq!(procs.parents.get(&69787), Some(&69700));
    assert_eq!(procs.started_ms.get(&69787), Some(&(NOW_MS - 161_000.0)));
}

/// Cloud Build status of the run's own build is working, not queued; the canary rollout queue is.
#[test]
fn cloud_build_status_lines_are_not_queue_waits() {
    let me = std::process::id();
    let own = [
        "\u{1b}[2m  build-crm: queued\u{1b}[0m",
        "\u{1b}[2m  offline-validation: queued\u{1b}[0m",
        "\u{1b}[2m  build-api          queued    -s\u{1b}[0m",
        "\u{1b}[2m  cloud build queued 50s, ran 134s\u{1b}[0m",
        "\u{1b}[2mcloud build 1a2b3c4d-f00d-4555-b7e1-5e6f7a8b9c0d is still queued; 9b403dda4 contains 1a2b3c4d5 and is prepared by pid 91842; cancelling it\u{1b}[0m",
        "\u{1b}[2mriding 9b403dda4: cancelled queued cloud build 1a2b3c4d-f00d-4555-b7e1-5e6f7a8b9c0d\u{1b}[0m",
    ];
    let waits = [
        "\u{1b}[2mqueued for a canary rollout behind pid 62025 (rollout build 1a2b-b905 since 14:50 SGT of 9b403dda4); 1 ahead\u{1b}[0m",
        "[ship-web] queued for a ship-web round; 1 ahead",
    ];
    let board: Vec<Value> = own.iter().chain(waits.iter()).enumerate()
        .map(|(i, line)| entry(&format!("r{i}"), "canary", "Q1", "s1", "c", line, me)).collect();
    let queue = snapshot::build(&board, &FakeGit, &tabs(), &images(), &Value::Null, NOW_MS, &mut HashMap::new());
    for r in queue["environments"]["canary"]["releases"].as_array().unwrap() {
        let wait = waits.iter().any(|w| r["progress"] == w.chars().take(160).collect::<String>());
        assert_eq!(r["phase"], if wait { "waiting" } else { "running" }, "{}", r["progress"]);
    }
}

#[test]
fn a_run_alive_when_the_board_was_read_stays_running_for_that_snapshot() {
    // Judge 87281007128583980004: canary 26782 exited between the board read and a later pid check
    // in the same sample, which raised a false T4. The pid is checked once, at read time.
    let dir = std::env::temp_dir().join(format!("release-watch-settled-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut run = std::process::Command::new("/bin/sleep").arg("120").spawn().expect("spawn sleep");
    let live = entry("20261105T014117Z-canary-26782", "canary", "Q1", "s1", "a", "rollout", run.id());
    let dead = entry("20261105T014000Z-canary-1", "canary", "Q1", "s2", "b", "rollout", 0);
    for e in [&live, &dead] {
        std::fs::write(dir.join(format!("{}.json", e["id"].as_str().unwrap())), e.to_string()).unwrap();
    }
    let board = snapshot::board(&dir);
    let _ = run.kill();
    let _ = run.wait();
    let state = |id: &str| snapshot::live_state(board.iter().find(|e| e["id"] == id).unwrap());
    assert_eq!(state("20261105T014117Z-canary-26782"), "running", "exited after the read: the next sample sees its final state");
    assert_eq!(state("20261105T014000Z-canary-1"), "abandoned", "dead at read time and still running on the re-read");
    assert_eq!(snapshot::live_state(&live), "abandoned", "an entry not read by board checks its pid now");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Judge 87201392560521810218: an agent-only `deliver beta` run shared web paths with a beta-web
/// run through a pre-rebase copy, but only the beta-web run deploys the api image those paths
/// map to. Rollouts count only images both runs deploy; a plan that deploys api keeps the judge.
#[test]
fn rollouts_count_only_services_both_runs_deploy() {
    let dir = std::env::temp_dir().join(format!("release-watch-deploys-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let named = json!({"images": [
        {"name": "mars-crm/api:<image id>", "deployments": [{"env": "beta"}], "trigger_paths": ["Mars_CRM/web/src/**"],
            "conflict_key": {"rollout": ["beta/deployment/api"]}},
        {"name": "mars-crm/agent:<image id>", "deployments": [{"env": "beta"}], "trigger_paths": ["Mars_CRM/agent/**"],
            "conflict_key": {"rollout": ["beta/deployment/agent"]}},
    ]});
    let me = std::process::id();
    let t9 = |services: Value| {
        let plan = dir.join(format!("plan-{}.json", services.to_string().len()));
        std::fs::write(&plan, json!({"services": services}).to_string()).expect("plan");
        let mut beta = entry("19177", "beta", "Q1", "s1", "a", "building", me);
        beta["inputs"] = json!([plan.to_string_lossy()]);
        let board = vec![entry("45686", "beta-web", "Q2", "s2", "b", "building", me), beta];
        let queue = snapshot::build(&board, &FakeGit, &tabs(), &named, &Value::Null, NOW_MS, &mut HashMap::new());
        let rollouts = queue["environments"]["beta"]["releases"][0]["overlaps"][0]["rollouts"].clone();
        (rollouts, triggers::detect(&queue, &[], &FakeGit, &[], 0.0, NOW_MS).into_iter().filter(|t| t.code == "T9").count())
    };
    assert_eq!(t9(json!(["agent"])), (json!([]), 0), "the beta run keeps the api image the shared paths map to");
    assert_eq!(t9(json!(["api", "agent"])), (json!(["beta/deployment/api"]), 1), "both runs replace the api image");
    let _ = std::fs::remove_dir_all(&dir);
}
