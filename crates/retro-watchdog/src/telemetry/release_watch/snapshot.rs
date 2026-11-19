//! The release queue snapshot: every unfinished `deliver` run on the release board, grouped by
//! environment, with its position, phase, wait, the agents queued beside it and the paths it
//! shares with them. Board files are written by `Mars_CRM/scripts/release/board.ts`; a reader
//! treats a `running` file whose pid is gone as abandoned, exactly like `deliver status`.
use serde_json::{json, Value};
use std::{collections::{BTreeMap, BTreeSet, HashMap}, path::Path};

/// A progress line that means the run is queued behind someone, not working.
const WAITING: &[&str] = &["waiting", "queued", "⏳", " ahead", "in line"];
/// Lines that say "waiting" for an external step the run itself drives, not for the queue: Apple's
/// processing of an uploaded build took 4222 s of the 4222 s "iOS queue wait" in release retro
/// 84705495676337590512 (`deliver ios` prints "uploaded; waiting for App Store Connect processing").
/// `kubectl rollout status` ("Waiting for deployment \"api\" rollout to finish") is the run's own
/// rollout: judge 818310651505023274466 found it classified as queued and raised a false T8.
/// `cloud build` lines report the run's own Cloud Build (`  cloud build queued 50s, ran 134s`, a
/// superseded build `still queued; … cancelling it`); the canary rollout queue says `rollout build`.
/// The build gate's `[cpu-build-gate] starting after N s of waiting: …` line says the wait is over;
/// the build that follows prints no relayed line for minutes, so release retro 813612464463489159041
/// counted 10 of 38 queue waits (4282 s) through it while the gate itself had printed 150 s.
const NOT_QUEUED: &[&str] = &["waiting for app store connect processing", "waiting for deployment \"", "cloud build", "[cpu-build-gate] starting after"];

/// A Cloud Build step of the run's own build (`mars-ops canary/cloud_build.rs` prints
/// `  build-crm: queued` while polling, then a `  build-crm          queued    -s` timing row).
fn cloud_build_step(line: &str) -> bool {
    static STEP: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    STEP.get_or_init(|| regex::Regex::new(r"^\s+[a-z0-9][a-z0-9-]*:?\s+queued(\s+\S*s)?$").expect("step pattern")).is_match(line)
}

fn queued(line: &str) -> bool {
    static ANSI: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let plain = ANSI.get_or_init(|| regex::Regex::new(r"\x1b\[[0-9;]*m").expect("ansi pattern")).replace_all(line, "");
    let lower = plain.trim_end().to_lowercase();
    WAITING.iter().any(|w| lower.contains(w)) && !NOT_QUEUED.iter().any(|n| lower.contains(n)) && !cloud_build_step(&lower)
}
const MAX_PATHS: usize = 8;
/// Orca tab id → `{"title", "terminal"}` of its writable terminal.
pub(crate) type Tabs = HashMap<String, Value>;

pub(crate) fn environment(target: &str) -> &str {
    match target { "beta" | "beta-web" | "beta-crm" => "beta", other => other }
}

fn alive(pid: i64) -> bool {
    pid > 0 && (unsafe { libc::kill(pid as i32, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

fn parse_ms(text: &str) -> Option<f64> {
    crate::cause_tree::attribute::parse_time(text)
}

/// Board entries newest first; unreadable files are skipped like `boardStatus` does.
pub(crate) fn board(dir: &Path) -> Vec<Value> {
    let mut names: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    names.sort();
    names.reverse();
    names.iter().filter_map(|p| read_settled(p)).collect()
}

/// Whether a `running` entry's creator was alive when `board` read it; `live_state` trusts this
/// stamp, so one snapshot never reads a run as running and later, after its pid exited, as abandoned.
const ALIVE_AT_READ: &str = "aliveAtRead";

/// One board file. `deliver` writes its final state, then exits; a `running` file whose pid is
/// already gone is read once more, and the pid is checked only here (judge 87281007128583980004:
/// canary 26782 succeeded at 01:47:24.584Z, the snapshot read it as running at 01:47:23.980Z and
/// a later pid check in the same sample raised a false T4; earlier judge 87509402574383064309).
fn read_settled(path: &Path) -> Option<Value> {
    let read = || std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let mut entry = read()?;
    if entry["state"] == "running" {
        let living = alive(entry["creator"]["pid"].as_i64().unwrap_or(0));
        if !living {
            entry = read().unwrap_or(entry);
        }
        if entry["state"] == "running" {
            entry[ALIVE_AT_READ] = json!(living);
        }
    }
    Some(entry)
}

/// The state a reader should believe: `abandoned` when a running run's process is gone (as
/// `board` saw it, else checked now).
pub(crate) fn live_state(entry: &Value) -> &'static str {
    let living = || entry[ALIVE_AT_READ].as_bool().unwrap_or_else(|| alive(entry["creator"]["pid"].as_i64().unwrap_or(0)));
    match entry["state"].as_str().unwrap_or_default() {
        "running" if !living() => "abandoned",
        "running" => "running",
        "detached" if alive(entry["backgroundPid"].as_i64().unwrap_or(0)) => "detached",
        "failed" => "failed",
        _ => "finished",
    }
}

fn step(entry: &Value) -> String {
    let steps = entry["steps"].as_array().map_or(&[][..], Vec::as_slice);
    let done = steps.iter().filter(|s| s["state"] == "ok").count();
    let current = steps.iter().find(|s| s["state"] == "running").or_else(|| steps.get(done.saturating_sub(1)));
    format!("{} {done}/{}", current.and_then(|s| s["label"].as_str()).unwrap_or("none"), steps.len())
}

/// Source commit and shipped paths of one run, as git sees them from its creator's checkout.
pub(crate) trait Git {
    fn resolve(&self, cwd: &str, rev: &str) -> Option<String>;
    fn prefix(&self, cwd: &str) -> String;
    fn relation(&self, a: &str, b: &str) -> (&'static str, Vec<String>);
    fn behind(&self, sha: &str) -> Option<u64>;
    /// Whether `sha` is already on `origin/mastra`.
    fn pushed(&self, sha: &str) -> bool;
    /// Whether every one of `paths` at `sha` has the same bytes on `origin/mastra` (a pre-rebase
    /// copy of pushed work); empty `paths` carry nothing.
    fn carried(&self, _sha: &str, _paths: &[String]) -> bool { false }
}

fn release(entry: &Value, git: &dyn Git, tabs: &Tabs, now_ms: f64, waiting_since: &mut HashMap<String, f64>) -> Value {
    let id = entry["id"].as_str().unwrap_or_default().to_string();
    let cwd = entry["creator"]["cwd"].as_str().unwrap_or_default();
    let line = entry["progress"]["line"].as_str().unwrap_or_default();
    let state = live_state(entry);
    // A dead run waits for nothing: its last progress line may still say "waiting".
    let waiting = state != "abandoned" && queued(line);
    let since = if waiting { *waiting_since.entry(id.clone()).or_insert(now_ms) } else { waiting_since.remove(&id); now_ms };
    let last = entry["progress"]["at"].as_str().or(entry["updatedAt"].as_str()).and_then(parse_ms).unwrap_or(now_ms);
    let rev = entry["ref"].as_str().filter(|r| !r.is_empty()).unwrap_or("HEAD");
    // `deliver` records the exact commit it ships; a symbolic ref such as origin/mastra moves after it opened.
    let source = entry["source"].as_str().filter(|s| s.len() == 40).map(str::to_string).or_else(|| git.resolve(cwd, rev));
    let prefix = git.prefix(cwd);
    let inputs: Vec<String> = entry["inputs"].as_array().into_iter().flatten().filter_map(Value::as_str)
        .filter(|p| !p.starts_with('-')).map(|p| format!("{prefix}{p}")).collect();
    let started = entry["startedAt"].as_str().and_then(parse_ms).unwrap_or(now_ms);
    json!({"id": id, "target": entry["target"], "queue": entry["queue"], "purpose": entry["purpose"], "task": entry["task"],
        "state": state, "phase": if state == "abandoned" { "abandoned" } else if waiting { "waiting" } else { "running" }, "step": step(entry),
        "waiting_s": ((now_ms - since) / 1000.0).round(), "idle_s": ((now_ms - last) / 1000.0).round(),
        "age_s": ((now_ms - started) / 1000.0).round(), "progress": line.chars().take(160).collect::<String>(),
        "source": source, "behind_origin": source.as_deref().and_then(|s| git.behind(s)), "inputs": inputs,
        "deploys": super::deploys::deploys(entry),
        "creator": {"agent": entry["creator"]["agent"], "session": entry["creator"]["session"],
            "orcaTab": entry["creator"]["orcaTab"], "lookup": entry["creator"]["lookup"], "cwd": cwd,
            "tab": entry["creator"]["orcaTab"].as_str().and_then(|t| tabs.get(t)).cloned().unwrap_or(Value::Null)}})
}

/// Rollout keys (`<env>/<kind>/<name>`) of the images in `env` whose trigger paths cover any of
/// `paths`, from `image-map.json` (exact paths, `dir/**` prefixes; `@` tokens are not paths).
fn rollouts(images: &Value, env: &str, paths: &BTreeSet<String>) -> Vec<String> {
    let covers = |pattern: &str| paths.iter().any(|p| match pattern.strip_suffix("/**") {
        Some(dir) => p.starts_with(&format!("{dir}/")),
        None => p == pattern,
    });
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for image in images["images"].as_array().into_iter().flatten()
        .filter(|i| i["deployments"].as_array().into_iter().flatten().any(|d| d["env"] == env))
        .filter(|i| i["trigger_paths"].as_array().into_iter().flatten().filter_map(Value::as_str).any(covers)) {
        keys.extend(image["conflict_key"]["rollout"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
    }
    keys.into_iter().collect()
}

/// Shared paths between two runs of one environment, how their sources relate, and the rollouts
/// both would replace.
fn overlap(a: &Value, b: &Value, git: &dyn Git, env: &str, images: &Value) -> Option<Value> {
    let set = |r: &Value| r["inputs"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect::<BTreeSet<_>>();
    let mut paths: BTreeSet<String> = set(a).intersection(&set(b)).cloned().collect();
    let relation = match (a["source"].as_str(), b["source"].as_str()) {
        (Some(x), Some(y)) if x == y => "same",
        (Some(x), Some(y)) => { let (rel, shared) = git.relation(x, y); paths.extend(shared); rel }
        _ => "unknown",
    };
    // `mapped`: the image map knows this environment, so an empty `rollouts` means no deployed image.
    let mapped = images["images"].as_array().into_iter().flatten()
        .any(|i| i["deployments"].as_array().into_iter().flatten().any(|d| d["env"] == env));
    (relation == "divergent" || !paths.is_empty()).then(|| json!({"with": b["id"], "queue": b["queue"], "relation": relation,
        "session": b["creator"]["session"], "orcaTab": b["creator"]["orcaTab"], "tab": b["creator"]["tab"],
        "paths": paths.iter().take(MAX_PATHS).collect::<Vec<_>>(), "shared": paths.len(),
        "rollouts": super::deploys::narrow(rollouts(images, env, &paths), [a, b], images, env),
        "mapped": mapped}))
}

fn short(session: &Value) -> String {
    session.as_str().map_or("unknown session".into(), |s| s.chars().take(8).collect())
}

/// Where to message the run's agent: its Orca tab title and writable terminal handle.
fn contact(tab: &Value) -> String {
    match (tab["title"].as_str(), tab["terminal"].as_str()) {
        (Some(title), Some(terminal)) => format!("tab \"{title}\" {terminal}"),
        _ => "tab unknown".into(),
    }
}

/// The one line `deliver` prints to the waiting agent whenever it changes.
fn line(env: &str, list: &[Value], index: usize, agents: usize) -> String {
    let me = &list[index];
    // An abandoned run holds no turn: it is neither ahead of anyone nor counted in the queue.
    let live: Vec<&Value> = list.iter().filter(|r| r["state"] == "running").collect();
    let position = live.iter().position(|r| r["id"] == me["id"]);
    let ahead: Vec<String> = live[..position.unwrap_or(0)].iter().map(|r| format!("{} \"{}\" ({} {}, {} {}, {} min, {})",
        r["queue"].as_str().unwrap_or("?"), r["purpose"].as_str().unwrap_or("").chars().take(60).collect::<String>(),
        r["creator"]["agent"].as_str().unwrap_or("?"), short(&r["creator"]["session"]), r["phase"].as_str().unwrap_or("?"),
        r["step"].as_str().unwrap_or("?"), (r["age_s"].as_f64().unwrap_or(0.0) / 60.0).round(), contact(&r["creator"]["tab"]))).collect();
    let overlaps: Vec<String> = me["overlaps"].as_array().into_iter().flatten().map(|o| format!("{} with {} ({}, {} shared: {}{}; {})",
        if o["relation"] == "divergent" { "CONFLICT" } else { "overlap" }, short(&o["session"]), o["relation"].as_str().unwrap_or("?"),
        o["shared"], o["paths"].as_array().into_iter().flatten().filter_map(Value::as_str).take(3).collect::<Vec<_>>().join(", "),
        o["rollouts"].as_array().filter(|r| !r.is_empty()).map_or(String::new(), |r| format!("; rollouts {}",
            r.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))),
        contact(&o["tab"]))).collect();
    let behind = me["behind_origin"].as_u64().filter(|n| *n > 0).map_or(String::new(), |n| format!(" · source {n} behind origin/mastra"));
    let place = position.map_or("abandoned".to_string(), |p| format!("{}/{}", p + 1, live.len()));
    let frozen = match (me["freeze"]["frozen"] == true, me["freeze"]["allowed"] == true) {
        (false, _) => String::new(),
        (true, true) => format!(" FROZEN, this run is the allowed release ({})", me["freeze"]["reason"].as_str().unwrap_or("")),
        (true, false) => format!(" FROZEN ({}): only {} may release, stop before upload", me["freeze"]["reason"].as_str().unwrap_or(""),
            me["freeze"]["only"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")),
    };
    format!("[queue] {env}{frozen} {place} · {agents} agent(s) queued · {}{}{}",
        if ahead.is_empty() { "nothing ahead".into() } else { format!("ahead: {}", ahead.join("; ")) },
        if overlaps.is_empty() { String::new() } else { format!(" · {}", overlaps.join("; ")) }, behind)
}

/// The whole snapshot that `queue.json` holds; `entries` is the board, newest first.
/// The freeze of `env` from `freeze.json` (`{"<env>": {"reason", "allowed": [sha...], "by", "at"}}`),
/// and whether `source` is one of its allowed commits (either side may be abbreviated).
fn freeze_of(freeze: &Value, env: &str, source: Option<&str>) -> Value {
    let rule = &freeze[env];
    if !rule.is_object() { return json!({"frozen": false}); }
    let only: Vec<&str> = rule["allowed"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let allowed = source.is_some_and(|s| only.iter().any(|a| !a.is_empty() && (s.starts_with(a) || a.starts_with(s))));
    json!({"frozen": true, "allowed": allowed, "only": only, "reason": rule["reason"], "by": rule["by"]})
}

pub(crate) fn build(entries: &[Value], git: &dyn Git, tabs: &Tabs, images: &Value, freeze: &Value, now_ms: f64, waiting_since: &mut HashMap<String, f64>) -> Value {
    let mut envs: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for entry in entries.iter().rev().filter(|e| matches!(live_state(e), "running" | "abandoned")) {
        let env = environment(entry["target"].as_str().unwrap_or("?")).to_string();
        envs.entry(env).or_default().push(release(entry, git, tabs, now_ms, waiting_since));
    }
    let mut lines = serde_json::Map::new();
    let mut contacts: BTreeMap<String, Value> = BTreeMap::new();
    let mut out = serde_json::Map::new();
    for (env, mut list) in envs {
        let found: Vec<Vec<Value>> = (0..list.len()).map(|i| (0..list.len()).filter(|j| *j != i)
            .filter_map(|j| overlap(&list[i], &list[j], git, &env, images)).collect()).collect();
        for (release, overlaps) in list.iter_mut().zip(found) {
            release["overlaps"] = json!(overlaps);
            release["freeze"] = freeze_of(freeze, &env, release["source"].as_str());
        }
        let agents = list.iter().filter(|r| r["state"] == "running").map(|r| r["creator"]["session"].to_string()).collect::<BTreeSet<_>>().len();
        for (i, release) in list.iter().enumerate() {
            let who = contacts.entry(release["creator"]["session"].to_string()).or_insert_with(|| json!({
                "session": release["creator"]["session"], "agent": release["creator"]["agent"], "tab": release["creator"]["tab"], "releases": []}));
            who["releases"].as_array_mut().into_iter().for_each(|ids| ids.push(release["id"].clone()));
            lines.insert(release["id"].as_str().unwrap_or_default().into(), json!(line(&env, &list, i, agents)));
        }
        out.insert(env, json!({"agents": agents, "releases": list}));
    }
    waiting_since.retain(|id, _| lines.contains_key(id));
    // `contacts`: every agent with a run in the queue and the Orca tab to message it in.
    json!({"at_ms": now_ms, "agents": live_agents(&out), "contacts": contacts.into_values().collect::<Vec<_>>(), "environments": out, "lines": lines})
}

/// Agents with a live (not abandoned) run anywhere in the queue.
fn live_agents(envs: &serde_json::Map<String, Value>) -> usize {
    envs.values().flat_map(|e| e["releases"].as_array().into_iter().flatten())
        .filter(|r| r["state"] == "running").map(|r| r["creator"]["session"].to_string()).collect::<BTreeSet<_>>().len()
}
