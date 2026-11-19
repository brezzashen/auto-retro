use super::*;

fn hit(at: f64, session: &str, kind: Kind) -> Hit {
    Hit { at, session: session.into(), locator: format!("/t/{session}.jsonl:1"), kind, text: String::new() }
}

fn inputs(stops: Vec<(f64, &'static str)>, hits: Vec<Hit>, actions: Vec<(f64, String)>) -> Inputs {
    Inputs { markers: Markers { starts: vec![], stops }, boots: vec![500.0], actions, pauses: vec![], hits, ooms: vec![] }
}

#[test]
fn a_watchdog_restart_after_guest_oom_kills_is_memory_exhaustion() {
    // 2026-10-25 22:21-22:30 SGT: 23 guest_global OOM kills, then the engine_hung restart at 22:30.
    let mut i = inputs(vec![(1_000_000.0, "cli_stop")], vec![], vec![(999_000.0, "restart".into())]);
    i.ooms = vec![430_000.0, 990_000.0];
    assert_eq!(attribute(&i, 1_000.0, Some(1_050_000.0), 1_100_000.0)["node"], "N4.9");
    // Detection time: no stop marker yet, Docker still up but hung.
    let mut hung = inputs(vec![], vec![], vec![]);
    hung.ooms = vec![990_000.0];
    hung.boots.clear();
    assert_eq!(attribute(&hung, 600.0, None, 1_000_000.0)["node"], "N4.9");
    // An OOM older than the window leaves the watchdog's own restart on N4.4.
    i.ooms = vec![50_000.0];
    assert_eq!(attribute(&i, 1_000.0, Some(1_050_000.0), 1_100_000.0)["node"], "N4.4");
}

#[test]
fn kinds_skip_searches_and_split_segments() {
    assert_eq!(kinds("tmutil stopbackup && docker desktop stop 2>&1 | tail -1"), vec![Kind::Stop]);
    assert_eq!(kinds("docker desktop restart"), vec![Kind::Stop, Kind::Start]);
    assert_eq!(kinds("open -a Docker; for i in 1 2; do docker info; done"), vec![Kind::Start, Kind::Check]);
    assert_eq!(kinds(r#"osascript -e 'quit app "Docker Desktop"'"#), vec![Kind::Stop]);
    assert_eq!(kinds(r#"rg -e "docker desktop restart" ~/automation/crates"#), Vec::<Kind>::new());
    assert_eq!(kinds(r#"pkill -f "cat" -P 4242"#), vec![Kind::Kill]);
    assert_eq!(kinds("kill $(lsof -nP -t -iTCP:8047 -sTCP:LISTEN -a -i@127.0.0.1) 2>/dev/null"), vec![Kind::Kill]);
    assert_eq!(kinds(r#"tools.exec_command({cmd:\"open -a Docker\"})"#), vec![Kind::Start]);
}

#[test]
fn kinds_skip_free_text_queries_inside_harness_wrappers() {
    // 2026-10-25 22:25 and 22:36 SGT: both were recorded as running Docker stops.
    let wrapped = |cmd: &str| format!("/bin/zsh -c source /s.sh 2>/dev/null || true && export X='a'\\012: && eval '{cmd}' < /dev/null && pwd -P");
    let status = wrapped("uptime; ~/.local/bin/session-status docker desktop restart 2>&1 | grep -E title");
    assert!(!kinds(&status).contains(&Kind::Stop));
    let recall = wrapped(r#"~/.local/bin/cpu-watchdog log-recall 869 "Kubernetes API down, watchdog docker desktop restart" 2>&1"#);
    assert!(!kinds(&recall).contains(&Kind::Stop));
    let memo = wrapped(r#"~/.local/bin/agent-memory write --name n4 --description "watchdog docker desktop restart" <<'EOF'"#);
    assert!(!kinds(&memo).contains(&Kind::Stop));
    // 2026-10-26 20:23 SGT (pid 65535): sed/bun prose with `;` and `|` inside quotes was recorded as docker_stop_seen.
    let prose = wrapped(r#"sed -i '' 's|old|raised with docker-pause + docker desktop stop/start; ~5 min|' m.md; NEW=$(bun -e 'const a=1;k.actual="20:16 → cpu-watchdog docker-pause 15 + docker desktop stop → docker-pause 0 + docker desktop start。";')"#);
    assert!(kinds(&prose).is_empty());
    assert_eq!(kinds(&wrapped("timeout 60 docker desktop stop 2>&1")), vec![Kind::Stop]);
    assert_eq!(kinds(&wrapped("docker desktop stop; open -a Docker")), vec![Kind::Stop, Kind::Start]);
}

#[test]
fn maintenance_stop_restarted_by_watchdog_is_a_collision() {
    // 2026-10-24 10:27: an agent stops Docker to repartition, the watchdog restarts it at 10:45.
    let i = inputs(vec![(10_000.0, "cli_stop")], vec![hit(9_000.0, "ce1b11a8", Kind::Stop), hit(20_000.0, "dc3190b8", Kind::Check)],
        vec![(30_000.0, "restart".into())]);
    let row = attribute(&i, 1_000.0, Some(40_000.0), 50_000.0);
    assert_eq!(row["stopped_by"]["session"], "ce1b11a8");
    assert_eq!(row["started_by"]["actor"], "watchdog");
    assert_eq!(row["collision"], true);
    assert_eq!(row["node"], "N4.2.1");
    assert_eq!(row["downtime_s"], 30.0);
    assert_eq!(row["bystanders"]["sessions"], 1);
}

#[test]
fn a_kill_right_before_the_quit_wins_and_a_bare_marker_stays_unattributed() {
    let i = inputs(vec![(10_000.0, "app_quit")], vec![hit(9_200.0, "0c090497", Kind::Kill)], vec![(12_000.0, "start".into())]);
    let row = attribute(&i, 1_000.0, Some(20_000.0), 30_000.0);
    assert_eq!(row["node"], "N4.3");
    assert_eq!(row["collision"], false);
    // 2026-10-26 18:53:04 and 19:27:17 SGT: `kill $(lsof ... -a -i@127.0.0.1)` sent SIGTERM to com.docker.backend.
    let sigterm = inputs(vec![(10_400.0, "backend_sigterm")], vec![hit(8_000.0, "other", Kind::Stop), hit(10_000.0, "f5dbdc15", Kind::Kill)], vec![(90_000.0, "start".into())]);
    let sigterm_row = attribute(&sigterm, 1_000.0, Some(100_000.0), 110_000.0);
    assert_eq!(sigterm_row["node"], "N4.3");
    assert_eq!(sigterm_row["stop_marker"], "backend_sigterm");
    assert_eq!(sigterm_row["stopped_by"]["actor"], "agent_kill");
    assert_eq!(sigterm_row["stopped_by"]["session"], "f5dbdc15");
    let bare = attribute(&inputs(vec![(10_000.0, "cli_stop")], vec![], vec![]), 1_000.0, Some(20_000.0), 30_000.0);
    assert_eq!(bare["stopped_by"]["actor"], "unattributed_cli_stop");
    assert_eq!(bare["node"], "N4.5");
}

#[test]
fn a_start_after_a_boot_without_markers_is_the_boot() {
    let row = attribute(&inputs(vec![], vec![hit(900.0, "3dba09c2", Kind::Start)], vec![]), 100.0, Some(1_000.0), 2_000.0);
    assert_eq!(row["node"], "N4.1");
    assert_eq!(row["started_by"]["session"], "3dba09c2");
    // 2026-10-26 16:02 SGT: login-item boot failed before VM start; watchdog restart at 16:06 wrote cli_stop after boot.
    let boot_heal = attribute(&inputs(vec![(309_905.0, "cli_stop")], vec![], vec![(300_532.0, "restart".into())]), 100.0, Some(316_047.0), 400_000.0);
    assert_eq!(boot_heal["node"], "N4.1");
    assert_eq!(boot_heal["stopped_by"]["actor"], "host_boot");
    assert_eq!(boot_heal["started_by"]["actor"], "watchdog");
    assert_eq!(boot_heal["later_stops"][0]["actor"], "watchdog");
    assert_eq!(boot_heal["downtime_s"], 316.0);
    let none = attribute(&inputs(vec![], vec![], vec![]), 600.0, Some(1_000.0), 2_000.0);
    assert_eq!(none["node"], "N4.6");
}

#[test]
fn the_same_session_restarting_is_not_a_collision() {
    let i = inputs(vec![(10_000.0, "cli_stop")], vec![hit(9_000.0, "6f2a0986", Kind::Stop), hit(9_000.0, "6f2a0986", Kind::Start)], vec![]);
    let row = attribute(&i, 1_000.0, Some(20_000.0), 30_000.0);
    assert_eq!(row["node"], "N4.2");
    assert_eq!(row["started_by"]["session"], "6f2a0986");
}

#[test]
fn a_later_agent_stop_in_a_long_outage_makes_the_watchdog_start_a_collision() {
    // 2026-10-24: an unattributed stop at 09:02, the cutover agent's stop at 10:27, the watchdog's
    // restart at 10:45 and the VM start at 11:28.
    let i = inputs(vec![(10_000.0, "cli_stop"), (10_500.0, "cli_stop"), (400_000.0, "cli_stop"), (600_000.0, "cli_stop")],
        vec![hit(399_000.0, "ce1b11a8", Kind::Stop)], vec![(599_000.0, "restart".into())]);
    let row = attribute(&i, 1_000.0, Some(900_000.0), 950_000.0);
    assert_eq!(row["stopped_by"]["actor"], "unattributed_cli_stop");
    assert_eq!(row["later_stops"].as_array().unwrap().len(), 2);
    assert_eq!(row["later_stops"][0]["session"], "ce1b11a8");
    assert_eq!(row["started_by"]["actor"], "watchdog");
    assert_eq!(row["collision"], true);
    assert_eq!(row["node"], "N4.2.1");
}

#[test]
fn a_stop_command_wins_over_a_kill_in_the_same_command() {
    let i = inputs(vec![(10_000.0, "cli_stop")], vec![hit(9_000.0, "3e8bf392", Kind::Stop), hit(9_000.0, "3e8bf392", Kind::Kill)], vec![]);
    assert_eq!(attribute(&i, 1_000.0, Some(20_000.0), 30_000.0)["stopped_by"]["actor"], "agent");
}

#[test]
fn a_shutdown_quit_within_the_boot_minute_is_the_boot() {
    // 2026-11-05 21:09:21 SGT: the founder's restart quit Docker; `last reboot` prints 21:09, boot 21:09:51.
    let mut i = inputs(vec![(30_500.0, "app_quit")], vec![], vec![(290_000.0, "start".into())]);
    let row = attribute(&i, 100.0, Some(300_000.0), 400_000.0);
    assert_eq!(row["node"], "N4.1");
    assert_eq!(row["stopped_by"]["shutdown_marker"], "app_quit");
    assert_eq!(row["started_by"]["actor"], "watchdog");
    assert_eq!(row["downtime_s"], 270.0);
    // A quit two minutes before the boot minute stays unattributed.
    i.boots = vec![150_500.0];
    assert_eq!(attribute(&i, 100.0, Some(300_000.0), 400_000.0)["node"], "N4.5");
}
