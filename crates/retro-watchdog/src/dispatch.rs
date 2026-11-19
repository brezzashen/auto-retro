//! Dispatch receipts prove terminal creation or process startup, never a completed model turn.
//! Quota ranking owns account selection; an empty or failed ranking must not default to CC1.
use crate::monitor::{root, write_json};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{path::Path, process::{Command, Stdio}, os::unix::process::CommandExt};

mod launch;
#[path = "dispatch/orca.rs"]
mod orca_cli;
mod queue;
pub(crate) use orca_cli::orca;
use orca_cli::{worktree_selector, OrcaRefused};
pub(crate) use queue::poll as poll_queue;
pub(crate) use queue::opened_for as session_opened_for;

const REPO: &str = ".";
const MAX_PER_DAY: usize = 4;
const MAX_ATTEMPTS: u64 = 3;
const DAY_MS: f64 = 86_400_000.0;

pub fn port_leak(home: &Path, now: f64, id: &str, process: &str) -> Result<()> {
    let path = root(home).join("dispatch.json");
    let mut state: Value = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({"dispatches": []}));
    let list = state["dispatches"].as_array_mut().context("dispatch.json without dispatches")?;
    let previous = list.iter().position(|d| d["incident"] == id);
    if previous.is_some_and(|i| list[i]["started"] == true || list[i]["attempts"].as_u64() >= Some(MAX_ATTEMPTS)) {
        return Ok(());
    }
    // Recursive diagnosis: a known cause closes the incident by script, without a session.
    if process == "ControlCenter" && !cfg!(test) {
        match crate::cause_tree::close_port_leak(home, now, id) {
            Ok(Some(closed)) => {
                list.push(json!({"incident": id, "at_ms": now * 1000.0, "attempts": 0, "started": false,
                    "closed_by": "cause-tree", "closure": closed}));
                write_json(&path, &state)?;
                notify("看门狗：端口泄漏", &format!("已自动重启 ControlCenter；已知原因 {} 解释增长，脚本结案", closed["node"].as_str().unwrap_or("?")));
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => crate::log(&format!("cause tree {id}: {error:#}")),
        }
    }
    let recent = list.iter().filter(|d| d["started"] == true && d["at_ms"].as_f64().is_some_and(|at| now * 1000.0 - at < DAY_MS)).count();
    if recent >= MAX_PER_DAY {
        // The restart already ran; past the cap the incident is logged without an analysis tab.
        list.push(json!({"incident": id, "at_ms": now * 1000.0, "attempts": MAX_ATTEMPTS, "started": false,
            "skipped": format!("{recent} analysis tabs in 24 h reached the limit of {MAX_PER_DAY}")}));
        return write_json(&path, &state);
    }
    let attempts = previous.and_then(|i| list[i]["attempts"].as_u64()).unwrap_or(0) + 1;
    let result = if cfg!(test) { Ok(json!({"mode": "test"})) } else { start(home, id, process) };
    let record = match &result {
        Ok(started) => json!({"incident": id, "at_ms": now * 1000.0, "attempts": attempts, "started": true, "agent": started}),
        Err(error) => json!({"incident": id, "at_ms": now * 1000.0, "attempts": attempts, "started": false, "error": format!("{error:#}")}),
    };
    match previous {
        Some(i) => list[i] = record,
        None => list.push(record),
    }
    write_json(&path, &state)?;
    if result.is_ok() && !cfg!(test) {
        let action = if process == "ControlCenter" { "已自动重启 ControlCenter，并开 Orca 标签分析原因" } else { "已开 Orca 标签分析" };
        notify("看门狗：端口泄漏", &format!("{process} Mach 端口超限，{action}"));
    }
    result.map(|_| ())
}

fn start(home: &Path, id: &str, process: &str) -> Result<Value> {
    start_session(home, id, &format!("看门狗 {process} 端口泄漏"), &prompt(home, id, process))
}

pub(crate) fn start_session(home: &Path, id: &str, title: &str, text: &str) -> Result<Value> {
    queue::enqueue(home, id, title, text)
}

/// A session that must start before every queued analysis (a release past its hard limit).
pub(crate) fn start_urgent_session(home: &Path, id: &str, title: &str, text: &str) -> Result<Value> {
    queue::enqueue_urgent(home, id, title, text)
}

/// The model family that leads crash retros (`crash_repair.rs`, route `xcode`) and system
/// high-usage retros (`wake.rs`, route `perf`) instead of the account's default (founder, 2026-11-10).
pub(crate) const LEAD_FABLE: &str = "fable";

/// A session `lead` must lead: it starts on that model on a Claude account with room in the
/// model's own window. When no account has room it still starts, on the fallback
/// `limit-resume route --model` ranks next (Opus 5.5, then AG1 or Codex), and says so.
pub(crate) fn start_led_session(home: &Path, id: &str, title: &str, text: &str, lead: &str) -> Result<Value> {
    queue::enqueue_led(home, id, title, text, lead)
}

fn start_now(home: &Path, id: &str, title: &str, text: &str, lead: Option<&str>) -> Result<Value> {
    let account = best_account(home, lead).map_err(|error| anyhow::Error::new(queue::Preflight(format!("{error:#}"))))?;
    let profile = account["alias"].as_str().context("ranked account has no alias")?;
    let name = format!("watchdog-{id}");
    let led = launch::led(&account, lead);
    if led["lead_fallback"] == true {
        crate::log(&format!("watchdog dispatch {id}: no account has room for {}; {profile} leads on {}", lead.unwrap_or("?"), led["model"].as_str().or(account["agent"].as_str()).unwrap_or("?")));
        notify("看门狗：主导模型回退", &format!("{title}：没有账号剩余 {} 额度，改由 {profile} 的 {} 主导", lead.unwrap_or("?"), led["model"].as_str().or(account["agent"].as_str()).unwrap_or("?")));
    }
    let orca_up = orca(&["status", "--json"])
        .is_ok_and(|s| s["result"]["app"]["running"] == true && s["result"]["runtime"]["reachable"] == true);
    if orca_up {
        let command = launch::shell(home, &account, &name, text, true)?;
        match orca(&["terminal", "create", "--worktree", &worktree_selector(REPO), "--title", title, "--command", &command, "--json"]) {
            Ok(created) => {
                let handle = created["result"]["terminal"]["handle"].as_str().context("orca terminal create returned no handle")?;
                return Ok(launch::receipt(json!({"mode": "orca", "handle": handle, "profile": profile,
                    "agent": account["agent"], "verification": "terminal-created"}), &led));
            }
            // A refusal created nothing, so a headless start cannot duplicate a session.
            Err(error) if error.is::<OrcaRefused>() => crate::log(&format!("watchdog dispatch {id}: {error:#}; starting headless")),
            Err(error) => return Err(error),
        }
    }
    std::fs::create_dir_all(root(home).join("analysis"))?;
    use std::os::unix::fs::OpenOptionsExt;
    let log = std::fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600)
        .open(root(home).join("analysis").join(format!("{id}-agent.log")))?;
    let mut command = Command::new("/bin/zsh");
    command.args(["-lc", &launch::shell(home, &account, &name, text, false)?]).current_dir(REPO)
        .stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log).process_group(0);
    let mut child = command.spawn().context("spawn headless watchdog agent")?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    if let Some(status) = child.try_wait()? {
        if !status.success() { bail!("{} exited during startup ({status}); inspect {id}-agent.log", account["agent"]); }
    }
    Ok(launch::receipt(json!({"mode": "headless", "pid": child.id(), "profile": profile,
        "agent": account["agent"], "verification": "process-started"}), &led))
}

/// The opening every woken session gets: its route reference plus the shared session contract
/// (gated other skills, invariants). The full SKILL.md is a manual-use index and is not loaded.
pub(crate) fn route_header(route: &str) -> String {
    format!("Watchdog route {route}: read ~/.codex/skills/watchdog/references/{route}.md and ~/.codex/skills/watchdog/references/session-contract.md and follow them exactly; do not load the whole watchdog SKILL.md. \
        For disk space, memory or process release, simulators, sudo, system dialogs or the report page, use the 'Other skills' table in the session contract and respect its Gate column. \
        After a forced recovery, review resource contention, concurrency, error prevention and recurrence; fix only evidenced causes, run scoped checks and publish the report. Preserve active tasks. {}", crate::log_recall::instruction())
}

/// One line, so the shell Orca types the command into never sees a continuation prompt.
fn prompt(home: &Path, id: &str, process: &str) -> String {
    let state = root(home);
    format!("cpu-watchdog incident {id}: {process} crossed its Mach port limit, or its dead-name rights grew faster than the early-warning rate (the incident's trigger says which). {header} \
        Incident: {s}/incidents/{id}.json; state root {s}. \
        The watchdog has already restarted ControlCenter when it was the leaking process (outcome in {s}/controlcenter-restart.json); verify that, then find the cause. \
        Known causes did not close this incident by script: read the cause tree ~/.codex/skills/watchdog/references/cause-tree.json and {s}/analysis/{id}/attribution.json (cpu-watchdog attribute {id} recomputes it), and work only on the residual the known nodes leave. \
        Add each new finding as a node to cause-tree.json; report.json needs cause_tree {{node, new_nodes or no_new_node_reason}}. \
        Write {s}/analysis/{id}/report.md in Chinese and report.json, then run cpu-watchdog monitor-ack {id}.",
        header = route_header("port-leak"), s = state.display())
}

/// The account `limit-resume route` ranks first; with `lead`, the ranking of a task that model
/// family must lead, whose entries name the model to start (`model` `{family, id}`).
fn best_account(home: &Path, lead: Option<&str>) -> Result<Value> {
    let program = home.join(".local/bin/limit-resume");
    let mut args = vec!["route", "--json"];
    if let Some(lead) = lead { args.extend(["--model", lead]); }
    let text = crate::telemetry::command::capture(home, program.to_str().context("account tool path")?,
        &args, 30).context("read watchdog account ranking")?;
    launch::select(&serde_json::from_str::<Value>(&text).context("account ranking is not JSON")?)
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

pub(crate) fn notify(title: &str, body: &str) {
    let clean = |s: &str| s.replace(['"', '\\'], "");
    let script = format!("display notification \"{}\" with title \"{}\"", clean(body), clean(title));
    let child = Command::new("/usr/bin/osascript").args(["-e", &script]).stdin(Stdio::null())
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    // The resident loop never waits on this child; reap it on a thread or it stays a zombie.
    if let Ok(mut child) = child {
        std::thread::spawn(move || { let _ = child.wait(); });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_dispatch_per_incident_and_a_daily_cap() {
        let home = crate::telemetry::test_home("dispatch");
        for n in 0..MAX_PER_DAY {
            port_leak(&home, 1000.0 + n as f64, &format!("8{n}"), "ControlCenter").unwrap();
        }
        // The same incident never dispatches twice.
        port_leak(&home, 1100.0, "80", "ControlCenter").unwrap();
        let state: Value = serde_json::from_slice(&std::fs::read(root(&home).join("dispatch.json")).unwrap()).unwrap();
        assert_eq!(state["dispatches"].as_array().unwrap().len(), MAX_PER_DAY);
        // A fifth incident within 24 h is logged as skipped; a new incident a day later starts.
        port_leak(&home, 1200.0, "89", "ControlCenter").unwrap();
        let state: Value = serde_json::from_slice(&std::fs::read(root(&home).join("dispatch.json")).unwrap()).unwrap();
        assert!(state["dispatches"][MAX_PER_DAY]["skipped"].is_string());
        port_leak(&home, 1000.0 + 86_400.0 + 10.0, "90", "ControlCenter").unwrap();
        let state: Value = serde_json::from_slice(&std::fs::read(root(&home).join("dispatch.json")).unwrap()).unwrap();
        assert_eq!(state["dispatches"].as_array().unwrap().last().unwrap()["started"], true);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn quoting_survives_single_quotes() {
        assert_eq!(quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn prompts_name_a_skill_route_that_exists() {
        let skill = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".codex/skills/watchdog");
        for route in ["port-leak", "xcode", "perf", "cache-zero"] {
            let header = route_header(route);
            assert!(header.contains(&format!("route {route}")) && header.contains("session-contract.md"));
            assert!(!header.contains("Load the watchdog skill"), "woken sessions skip the full SKILL.md");
            // The installed skill is host state; check it only where it exists.
            if skill.exists() {
                assert!(skill.join("references").join(format!("{route}.md")).exists(), "missing reference for {route}");
                assert!(skill.join("references/session-contract.md").exists(), "missing session contract");
                let index = std::fs::read_to_string(skill.join("SKILL.md")).unwrap();
                assert!(index.contains(&format!("`{route}`")), "route {route} missing from SKILL.md");
            }
        }
        assert!(prompt(Path::new("/h"), "7", "ControlCenter").contains("route port-leak"));
    }
}
