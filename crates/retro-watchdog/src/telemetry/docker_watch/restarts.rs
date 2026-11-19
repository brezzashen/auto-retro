//! Docker restart attribution: for every Docker Desktop VM start, who stopped Docker before it and
//! who started it again, from Docker's own logs, the boot times, the watchdog's records and agent
//! transcripts. Each row names its cause-tree node (N4.*), so repeated causes are counted instead
//! of re-derived by a model session.
//!
//! Markers: `VM has started` and `Requesting VM termination` (com.docker.virtualization.log),
//! `EnsureDockerFinished` (docker-desktop.log*), `[ELECTRON] before-quit` (electron-*.log) and
//! `(cancel cause: terminated signal received)` (com.docker.backend.log*).
use super::super::transcript_calls;
use crate::{cause_tree::attribute::parse_time, monitor::{root, write_json}};
use regex::Regex;
use serde_json::{json, Value};
use std::{path::Path, process::Command, sync::OnceLock};

/// An agent's stop command runs up to this long before Docker logs its stop marker.
const STOP_BEFORE_MS: f64 = 180_000.0;
/// pkill/killall/kill $(...) signals land within a second; Docker logs its stop right after.
const KILL_BEFORE_MS: f64 = 10_000.0;
const START_BEFORE_MS: f64 = 600_000.0;
/// Sessions checking Docker or beta this long after the VM start were still waiting on it.
const BYSTANDER_AFTER_MS: f64 = 120_000.0;
pub(super) const DAYS: f64 = 14.0;
const FILE: &str = "docker-restarts.json";
/// Stop markers closer than this belong to one stop episode.
const EPISODE_GAP_MS: f64 = 120_000.0;
/// A stop the watchdog saw in `ps` is logged up to one sample (30 s) after Docker logs it.
const SEEN_LAG_MS: f64 = 45_000.0;
/// Without a stop marker, sessions checking Docker this long before the start count as waiting.
const UNKNOWN_WINDOW_MS: f64 = 300_000.0;
/// Every line a Kind can match contains one of these.
const NEEDLES: &[&str] = &["ocker", "pkill", "killall", "kill ", "beta.example", "k8s"];
/// A guest-wide OOM kill this long before a watchdog restart or an unmarked outage: VM memory ran out (N4.9).
const OOM_BEFORE_MS: f64 = 900_000.0;
/// `last reboot` has minute resolution: an unattributed quit this close to a boot is its shutdown (2026-11-05 21:09).
const BOOT_MINUTE_MS: f64 = 60_000.0;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Kind { Stop, Start, Kill, Check }

pub(super) struct Hit { pub at: f64, pub session: String, pub locator: String, pub kind: Kind, pub text: String }

fn regexes() -> &'static [(Kind, Regex); 4] {
    static R: OnceLock<[(Kind, Regex); 4]> = OnceLock::new();
    R.get_or_init(|| [
        (Kind::Stop, Regex::new(r#"(^|cmd:\\?"|(?:^|\s)(?:eval|sudo|nohup|command|do|then|else|until|while|if)\s+['"]*|timeout\s+\S+\s+)(\S*/)?docker\s+desktop\s+(stop|restart)(\s|$|['"\\])|quit\s+\\?"?Docker|quit\s+app\s+\\?"Docker|killall\s+(-\S+\s+)*\\?"?(Docker|com\.docker)"#).unwrap()),
        (Kind::Start, Regex::new(r#"(^|cmd:\\?"|(?:^|\s)(?:eval|sudo|nohup|command|do|then|else|until|while|if)\s+['"]*|timeout\s+\S+\s+)(\S*/)?docker\s+desktop\s+(start|restart)(\s|$|['"\\])|open\s+(-\S+\s+)*-a\s+\\?"?Docker|open\s+\S*Docker\.app"#).unwrap()),
        (Kind::Kill, Regex::new(r"(^|\s|/)(pkill|killall)\s|(^|\s|/)kill\s+(-[A-Za-z0-9]+\s+)*\$\(").unwrap()),
        (Kind::Check, Regex::new(r"docker\s+(info|ps|version)|kubectl\s.*docker-desktop|beta\.example\.com|session-status[^;|&]*(docker|Docker|beta)|k8s\s+status").unwrap()),
    ])
}

/// Kinds a command has, ignoring segments that only search for or print the words. `ps` shows a
/// harness-wrapped command's newlines as `\012` and its body as `eval '<command>'`; session-status
/// and cpu-watchdog take free-text queries (2026-10-25: `session-status docker desktop restart` and
/// a `log-recall` query were recorded as running stops).
pub(super) fn kinds(text: &str) -> Vec<Kind> {
    let mut found = Vec::new();
    let text = text.replace("\\012", "\n");
    for segment in text.split(['\n', ';', '|', '&']).map(str::trim) {
        let mut words = segment.split_whitespace().map(|w| w.trim_start_matches(['\'', '"']));
        let first = match words.next() { Some("eval") => words.next(), other => other }.unwrap_or_default();
        let first = first.rsplit('/').next().unwrap_or_default();
        if ["rg", "grep", "echo", "printf", "cat", "local-search", "git", "log", "jq", "sed", "session-status", "cpu-watchdog", "agent-memory"]
            .contains(&first) { continue; }
        for (kind, regex) in regexes() {
            if regex.is_match(segment) && !found.contains(kind) { found.push(*kind); }
        }
    }
    found
}

pub(super) struct Markers { pub starts: Vec<f64>, pub stops: Vec<(f64, &'static str)> }

fn markers(home: &Path) -> Markers {
    let dir = home.join(super::heal::LOG_DIR);
    let (mut starts, mut stops) = (Vec::new(), Vec::new());
    let bracket = |line: &str| line.strip_prefix('[').and_then(|l| l.split(']').next()).and_then(parse_time);
    for line in std::fs::read_to_string(dir.join("com.docker.virtualization.log")).unwrap_or_default().lines() {
        if line.contains("VM has started") { starts.extend(bracket(line)); }
        if line.contains("Requesting VM termination") { stops.extend(bracket(line).map(|t| (t, "vm_terminate"))); }
    }
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let (needle, marker) = if name.starts_with("docker-desktop.log") { ("EnsureDockerFinished", "cli_stop") }
            else if name.starts_with("electron-") { ("[ELECTRON] before-quit", "app_quit") }
            else if name.starts_with("com.docker.backend.log") { ("(cancel cause: terminated signal received)", "backend_sigterm") } else { continue };
        for line in std::fs::read_to_string(entry.path()).unwrap_or_default().lines().filter(|l| l.contains(needle)) {
            let time = if marker == "app_quit" { line.split_whitespace().next().and_then(parse_time) } else { bracket(line) };
            stops.extend(time.map(|t| (t, marker)));
        }
    }
    starts.sort_by(f64::total_cmp);
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    Markers { starts, stops }
}

/// Boot times (ms) from `last reboot`, which prints local time without a year.
fn boots() -> Vec<f64> {
    let year = String::from_utf8_lossy(&Command::new("/bin/date").arg("+%Y").output().map(|o| o.stdout).unwrap_or_default()).trim().to_string();
    let out = Command::new("/usr/bin/last").arg("reboot").output().map(|o| o.stdout).unwrap_or_default();
    String::from_utf8_lossy(&out).lines().filter(|l| l.starts_with("reboot time")).filter_map(|l| {
        let when = format!("{} {year} 00", l["reboot time".len()..].trim());
        let out = Command::new("/bin/date").args(["-j", "-f", "%a %b %d %H:%M %Y %S", &when, "+%s"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok().map(|s| s * 1000.0)
    }).collect()
}

/// Watchdog recoveries `(started ms, action)`, pauses `(from ms, until ms, reason)` and the stop
/// commands the watchdog saw running (`docker_stop_seen`), as transcript-like hits.
fn watchdog(home: &Path) -> (Vec<(f64, String)>, Vec<(f64, f64, String)>, Vec<Hit>) {
    let (mut actions, mut pauses, mut seen) = (Vec::new(), Vec::new(), Vec::new());
    for entry in std::fs::read_dir(root(home).join("docker-heals")).into_iter().flatten().flatten() {
        for row in std::fs::read_to_string(entry.path()).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
            let at = row["at_ms"].as_f64().unwrap_or_default();
            match row["kind"].as_str() {
                Some("docker_recovery") => if let (Some(at), Some(action)) = (row["started_at_ms"].as_f64(), row["steps"][0]["action"].as_str()) {
                    actions.push((at, action.to_string()));
                },
                Some("docker_pause") => if let Some(until) = row["until_ms"].as_f64() {
                    pauses.push((at, until, row["reason"].as_str().unwrap_or_default().to_string()));
                },
                Some("docker_stop_seen") => seen.push(Hit { at, session: row["session"].as_str().unwrap_or("?").to_string(),
                    locator: format!("ps pid {}", row["pid"]), kind: Kind::Stop, text: row["chain"].to_string().chars().take(240).collect() }),
                _ => {}
            }
        }
    }
    (actions, pauses, seen)
}

/// Docker-related tool calls of every transcript written since `from_ms`.
fn hits(home: &Path, from_ms: f64, to_ms: f64) -> Vec<Hit> {
    let mut out = Vec::new();
    for (_, path) in transcript_calls::files(home, from_ms) {
        transcript_calls::scan(&path, from_ms, to_ms, NEEDLES, &mut |call| {
            if call.result { return; }
            for kind in kinds(&call.text) {
                out.push(Hit { at: call.at_ms, session: call.session.to_string(), locator: format!("{}:{}", call.path.display(), call.line),
                    kind, text: call.text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(240).collect() });
            }
        });
    }
    out
}

fn agent(hit: &Hit, actor: &str) -> Value {
    json!({"actor": actor, "session": hit.session, "at_ms": hit.at, "locator": hit.locator, "command": hit.text})
}

pub(super) struct Inputs { pub markers: Markers, pub boots: Vec<f64>, pub actions: Vec<(f64, String)>, pub pauses: Vec<(f64, f64, String)>, pub hits: Vec<Hit>, pub ooms: Vec<f64> }

impl Inputs {
    fn oom_before(&self, at: f64) -> bool {
        self.ooms.iter().any(|o| *o <= at && at - o <= OOM_BEFORE_MS)
    }

    fn latest(&self, kind: Kind, from: f64, to: f64) -> Option<&Hit> {
        self.hits.iter().rev().find(|h| h.kind == kind && h.at >= from && h.at <= to)
    }

    /// Who caused the stop marker at `at`: an explicit stop command wins over a kill near it, except on direct backend SIGTERM.
    fn stopper(&self, at: f64, marker: &str) -> Value {
        if let Some(hit) = (marker != "backend_sigterm").then(|| self.latest(Kind::Stop, at - STOP_BEFORE_MS, at + SEEN_LAG_MS)).flatten() { agent(hit, "agent") }
        else if let Some(hit) = self.latest(Kind::Kill, at - KILL_BEFORE_MS, at + 1_000.0) { agent(hit, "agent_kill") }
        else if let Some((t, action)) = self.actions.iter().find(|(t, _)| (t - at).abs() < 60_000.0) { json!({"actor": "watchdog", "at_ms": t, "action": action}) }
        else { json!({"actor": format!("unattributed_{marker}"), "at_ms": at}) }
    }
}

/// One restart: the VM start at `start` (None while Docker is still down) after `previous`.
pub(super) fn attribute(i: &Inputs, previous: f64, start: Option<f64>, now: f64) -> Value {
    let end = start.unwrap_or(now);
    let mut episodes: Vec<(f64, &str)> = Vec::new();
    for (t, marker) in i.markers.stops.iter().filter(|(t, _)| *t > previous && *t <= end) {
        if episodes.last().is_none_or(|(last, _)| t - last > EPISODE_GAP_MS) { episodes.push((*t, marker)); }
    }
    let boot = i.boots.iter().copied().find(|b| *b > previous && *b <= end);
    let stops: Vec<Value> = episodes.iter().map(|(at, marker)| { let mut s = i.stopper(*at, marker); s["marker"] = json!(marker); s["at_ms"] = json!(at); s }).collect();
    let shutdown = boot.zip(stops.first()).filter(|(at, s)| (s["at_ms"].as_f64().unwrap_or(0.0) - at).abs() < BOOT_MINUTE_MS && s["actor"].as_str().is_some_and(|a| a.starts_with("unattributed_")));
    let (stopped_by, stop_ms, later) = match (stops.first(), boot) {
        (Some(first), Some(at)) if shutdown.is_some() => (json!({"actor": "host_boot", "at_ms": at, "shutdown_marker": first["marker"]}), first["at_ms"].as_f64(), &stops[1..]),
        (_, Some(at)) if episodes.first().is_none_or(|(first, _)| at <= *first) => (json!({"actor": "host_boot", "at_ms": at}), Some(at), &stops[..]),
        (Some(first), _) => (first.clone(), episodes.first().map(|s| s.0), &stops[1..]),
        (None, _) => (json!({"actor": "unknown"}), None, &stops[..]),
    };
    let from = stop_ms.unwrap_or(end - UNKNOWN_WINDOW_MS);
    let started_by = match start {
        None => Value::Null,
        Some(s) => if let Some((t, action)) = i.actions.iter().rev().find(|(t, _)| *t >= from - 60_000.0 && *t <= s) { json!({"actor": "watchdog", "at_ms": t, "action": action}) }
            else if let Some(hit) = i.latest(Kind::Start, (s - START_BEFORE_MS).max(from - 5_000.0), s + 5_000.0) { agent(hit, "agent") }
            else if boot.is_some() { json!({"actor": "login_item"}) } else { json!({"actor": "docker_or_user"}) },
    };
    let pause = i.pauses.iter().find(|(p, until, _)| *p <= end && *until >= from).map(|p| json!({"reason": p.2, "until_ms": p.1}));
    // The last session that deliberately stopped Docker in this outage holds it down.
    let holder = stops.iter().rev().find(|s| s["actor"] == "agent").map(|s| s["session"].clone());
    let collision = holder.as_ref().is_some_and(|h| started_by["actor"].as_str().is_some_and(|a| a == "watchdog" || a == "agent") && &started_by["session"] != h);
    let ours: Vec<&Value> = stops.iter().map(|s| &s["session"]).chain([&started_by["session"]]).collect();
    let mut bystanders: Vec<&str> = i.hits.iter().filter(|h| h.kind == Kind::Check && h.at >= from && h.at <= end + BYSTANDER_AFTER_MS)
        .filter(|h| !ours.iter().any(|s| s.as_str() == Some(&h.session))).map(|h| h.session.as_str()).collect();
    let calls = bystanders.len();
    bystanders.sort_unstable();
    bystanders.dedup();
    let node = match stopped_by["actor"].as_str().unwrap_or_default() {
        _ if collision => "N4.2.1", "host_boot" => "N4.1", "agent" => "N4.2", "agent_kill" => "N4.3",
        "watchdog" if i.oom_before(stopped_by["at_ms"].as_f64().unwrap_or(end)) => "N4.9", "watchdog" => "N4.4",
        "unknown" if i.oom_before(end) => "N4.9", "unknown" => "N4.6", _ => "N4.5",
    };
    json!({"start_ms": start, "stop_ms": stop_ms, "stop_marker": (later.len() != stops.len()).then(|| episodes.first().map(|s| s.1)).flatten(),
        "downtime_s": stop_ms.map(|s| ((end - s) / 1000.0).round()), "stopped_by": stopped_by, "later_stops": later,
        "started_by": started_by, "paused": pause, "collision": collision, "node": node,
        "bystanders": {"sessions": bystanders.len(), "calls": calls, "ids": bystanders.iter().take(12).collect::<Vec<_>>()}})
}

pub(super) fn inputs(home: &Path, from_ms: f64, now: f64) -> Inputs {
    let (actions, pauses, seen) = watchdog(home);
    let mut hits = hits(home, from_ms - START_BEFORE_MS, now);
    hits.extend(seen);
    hits.sort_by(|a, b| a.at.total_cmp(&b.at));
    Inputs { markers: markers(home), boots: boots(), actions, pauses, hits, ooms: super::super::oom::guest_global_times(home, from_ms - OOM_BEFORE_MS) }
}

/// Every VM start of the last `days` days, attributed; written to `docker-restarts.json`.
pub(super) fn all(home: &Path, days: f64, now_ms: f64) -> anyhow::Result<Value> {
    let from = now_ms - days * 86_400_000.0;
    let i = inputs(home, from, now_ms);
    let starts: Vec<f64> = i.markers.starts.clone();
    let rows: Vec<Value> = starts.iter().enumerate().filter(|(_, s)| **s >= from)
        .map(|(n, s)| attribute(&i, if n == 0 { 0.0 } else { starts[n - 1] }, Some(*s), now_ms)).collect();
    let mut by_node = serde_json::Map::new();
    for row in &rows {
        let entry = by_node.entry(row["node"].as_str().unwrap_or("?")).or_insert_with(|| json!({"restarts": 0, "downtime_s": 0.0, "bystander_sessions": 0}));
        entry["restarts"] = json!(entry["restarts"].as_u64().unwrap_or(0) + 1);
        entry["downtime_s"] = json!(entry["downtime_s"].as_f64().unwrap_or(0.0) + row["downtime_s"].as_f64().unwrap_or(0.0));
        entry["bystander_sessions"] = json!(entry["bystander_sessions"].as_u64().unwrap_or(0) + row["bystanders"]["sessions"].as_u64().unwrap_or(0));
    }
    let value = json!({"at_ms": now_ms, "days": days, "latest_start_ms": starts.last(), "restarts": rows.len(),
        "collisions": rows.iter().filter(|r| r["collision"] == true).count(), "by_node": by_node, "rows": rows});
    write_json(&root(home).join(FILE), &value)?;
    Ok(value)
}

/// Refreshes `docker-restarts.json` when Docker logged a VM start it does not hold yet and the
/// bystander window after it has passed. Returns the new rows' collisions.
pub(super) fn refresh(home: &Path, now_ms: f64) -> Vec<Value> {
    let latest = markers(home).starts.last().copied();
    let known = super::super::read(&root(home).join(FILE)).and_then(|v| v["latest_start_ms"].as_f64());
    let Some(latest) = latest.filter(|l| known.is_none_or(|k| *l > k) && now_ms - l > BYSTANDER_AFTER_MS) else { return Vec::new() };
    match all(home, DAYS, now_ms) {
        Ok(value) => value["rows"].as_array().into_iter().flatten()
            .filter(|r| r["collision"] == true && r["start_ms"].as_f64().is_some_and(|s| known.is_none_or(|k| s > k) && s <= latest)).cloned().collect(),
        Err(error) => { crate::log(&format!("docker restarts: {error:#}")); Vec::new() }
    }
}

/// Docker stop commands running now that the watchdog did not start and has not recorded
/// (`seen` pids), as `docker_stop_seen` rows: the topmost matching process, its parent chain and
/// the agent session id the chain names (a transcript or scratchpad path). Scripts an agent runs in
/// the background stop Docker without the command in any transcript (2026-10-24 09:02 cutover.zsh).
pub(super) fn running_stops(seen: &[i64], now_ms: f64) -> Vec<Value> {
    static SESSION: OnceLock<Regex> = OnceLock::new();
    let session = SESSION.get_or_init(|| Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap());
    let out = super::heal::run(Path::new("/bin/ps"), &["-axo", "pid=,ppid=,command="], super::heal::PROBE_TIMEOUT).text;
    let procs: Vec<(i64, i64, &str)> = out.lines().filter_map(|l| {
        let (pid, rest) = l.trim_start().split_once(' ')?;
        let (ppid, command) = rest.trim_start().split_once(' ')?;
        Some((pid.parse().ok()?, ppid.parse().ok()?, command.trim_start()))
    }).collect();
    let stopping = |command: &str| kinds(command).contains(&Kind::Stop);
    let by_pid = |pid: i64| procs.iter().find(|p| p.0 == pid);
    procs.iter().filter(|(pid, ppid, command)| stopping(command) && !seen.contains(pid) && !by_pid(*ppid).is_some_and(|p| stopping(p.2)))
        .filter_map(|(pid, ppid, command)| {
            let mut full = vec![*command];
            let mut next = *ppid;
            while let Some(parent) = by_pid(next).filter(|_| next > 1 && full.len() < 12) {
                full.push(parent.2);
                next = parent.1;
            }
            let chain: Vec<String> = full.iter().map(|c| c.chars().take(200).collect()).collect();
            if chain.iter().any(|c| c.contains("cpu-watchdog")) { return None; }
            // Search the full commands: a wrapper's session id can sit past the 200-character cut.
            let id = full.iter().find_map(|c| session.find(c)).map(|m| m.as_str().to_string());
            Some(json!({"kind": "docker_stop_seen", "at_ms": now_ms, "pid": pid, "session": id, "chain": chain}))
        }).collect()
}

/// Who stopped Docker in the outage still running at `now_ms`.
pub(super) fn current(home: &Path, now_ms: f64) -> Value {
    let i = inputs(home, now_ms - 3_600_000.0, now_ms);
    let previous = i.markers.starts.last().copied().unwrap_or(0.0);
    attribute(&i, previous, None, now_ms)
}

/// `cpu-watchdog docker-restarts [days]`.
pub(super) fn cli(home: &Path, args: Vec<String>) -> anyhow::Result<()> {
    let days = args.first().and_then(|d| d.trim_end_matches('d').parse().ok()).unwrap_or(DAYS);
    let value = all(home, days, crate::unix_now() * 1000.0)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

#[cfg(test)]
#[path = "restarts_tests.rs"]
mod tests;
