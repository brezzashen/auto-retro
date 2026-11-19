//! Win logs (founder, 2026-11-07): beta's engine host is the studio Windows PC, but nothing kept its
//! logs on this Mac, so a retro could not read the Windows side of a drop. Docker Desktop rotates its
//! host logs every few minutes (`monitor.log` about every 6 min), and the System log says nothing
//! about a broken SSH session. Every SAMPLE_SECS a worker pulls, past a per-log record cursor:
//! System and Application errors, warnings and boot/shutdown events; OpenSSH lines other than the
//! routine accept/disconnect noise, plus accepted connections per minute (a missing minute is a
//! gap); Docker Desktop host-log error lines past a timestamp cursor; and the CPU, logical processor
//! count and boot time (a line whenever they change). Lines go to `~/.local/state/win-logs/<SGT
//! day>/{events,ssh,docker}.log` and `host.log`, which local-search indexes for log-recall; the
//! cursor and the last error go to `win-logs.json`.
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

pub(super) const SAMPLE_SECS: f64 = 300.0;
/// While a backlog remains (a pull hit MAX_EVENTS), the next pull comes this soon.
const CATCH_UP_SECS: f64 = 60.0;
const MAX_EVENTS: u64 = 3_000;
/// A first pull reaches back this far (7 days).
const BACKFILL_MS: u64 = 604_800_000;
const SSH_SECS: u64 = 180;
const PROBE: &str = include_str!("win_logs/probe.ps1");
const LOGS: [&str; 3] = ["System", "Application", "OpenSSH/Operational"];

#[derive(Default)]
pub(super) struct WinLogs { last_pull: f64, behind: bool, worker: Option<Receiver<Result<Value>>> }

impl WinLogs {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(result) => { self.worker = None; self.absorb(home, now, result)?; }
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => return Ok(()),
            }
        }
        let every = if self.behind { CATCH_UP_SECS } else { SAMPLE_SECS };
        if self.last_pull > 0.0 && now >= self.last_pull && now - self.last_pull < every { return Ok(()); }
        let Some(host) = super::docker_watch::remote_host(home) else { return Ok(()) };
        self.last_pull = now;
        let (cursor, home) = (state(home)["cursor"].clone(), home.to_path_buf());
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new().name("watchdog-win-logs".into())
            .spawn(move || { let _ = sender.send(pull(&home, &host, &cursor)); })?;
        self.worker = Some(receiver);
        Ok(())
    }

    fn absorb(&mut self, home: &Path, now: f64, result: Result<Value>) -> Result<()> {
        let previous = state(home);
        let sample = match result {
            Ok(sample) => sample,
            Err(error) => {
                let mut kept = previous.clone();
                kept["at_ms"] = json!(now * 1000.0);
                kept["error"] = json!(format!("{error:#}"));
                return write_json(&root(home).join("win-logs.json"), &kept);
            }
        };
        let lines = render(&sample, &previous["host"], now);
        write_lines(&dir(home), &lines)?;
        self.behind = LOGS.iter().any(|log| sample["counts"][*log].as_u64().unwrap_or(0) >= MAX_EVENTS);
        write_json(&root(home).join("win-logs.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "cursor": sample["cursor"], "host": sample["host"], "lines": lines.len(), "behind": self.behind,
            "threshold": {"sample_s": SAMPLE_SECS, "max_events": MAX_EVENTS, "backfill_ms": BACKFILL_MS}}))
    }
}

fn state(home: &Path) -> Value {
    std::fs::read_to_string(root(home).join("win-logs.json")).ok()
        .and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null)
}

pub(super) fn dir(home: &Path) -> PathBuf { home.join(".local/state/win-logs") }

/// The probe for `cursor` (absent fields start the backfill).
pub(super) fn script(cursor: &Value) -> String {
    let mut start = serde_json::Map::new();
    for log in LOGS { start.insert(log.into(), json!(cursor[log].as_i64().unwrap_or(0))); }
    start.insert("docker".into(), json!(cursor["docker"].as_str().unwrap_or("")));
    PROBE.replace("__CURSOR__", &Value::Object(start).to_string())
        .replace("__BACKFILL_MS__", &BACKFILL_MS.to_string()).replace("__MAX__", &MAX_EVENTS.to_string())
}

fn pull(home: &Path, host: &str, cursor: &Value) -> Result<Value> {
    let output = super::command::capture(home, "/usr/bin/ssh",
        &["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", host, &super::docker_watch::powershell(&script(cursor))], SSH_SECS)?;
    let start = output.find('{').ok_or_else(|| anyhow::anyhow!("no JSON in the win-logs probe output"))?;
    Ok(serde_json::from_str(output[start..].lines().next().unwrap_or_default())?)
}

/// `(relative file, line)` pairs for one pull; the SGT day of each line's own UTC time picks its file.
pub(super) fn render(sample: &Value, previous_host: &Value, now: f64) -> Vec<(String, String)> {
    let rows = |key: &str| sample[key].as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for e in rows("events") {
        let at = e[0].as_str().unwrap_or_default();
        out.push((format!("{}/events.log", sgt_day(at)), format!("{at} {} {} L{} {} {}", e[1].as_str().unwrap_or("?"),
            e[2], e[3], e[4].as_str().unwrap_or("?"), e[5].as_str().unwrap_or(""))));
    }
    for s in rows("ssh") {
        let at = s[0].as_str().unwrap_or_default();
        out.push((format!("{}/ssh.log", sgt_day(at)), format!("{at} {}", s[1].as_str().unwrap_or(""))));
    }
    for (minute, n) in sample["minutes"].as_object().into_iter().flatten() {
        let at = format!("{minute}:00Z");
        out.push((format!("{}/ssh.log", sgt_day(&at)), format!("{at} sshd accepted {n} connection(s) this minute")));
    }
    for d in rows("docker") {
        let at = d[0].as_str().unwrap_or_default();
        out.push((format!("{}/docker.log", sgt_day(at)), d[1].as_str().unwrap_or("").to_string()));
    }
    let host = &sample["host"];
    if host.is_object() && host != previous_host {
        out.push(("host.log".into(), format!("{} host cpu=\"{}\" logical={} boot={} (was cpu=\"{}\" logical={} boot={})",
            iso(now), host["cpu"].as_str().unwrap_or("?"), host["logical"], host["boot"].as_str().unwrap_or("?"),
            previous_host["cpu"].as_str().unwrap_or("?"), previous_host["logical"], previous_host["boot"].as_str().unwrap_or("?"))));
    }
    out
}

/// `2026-11-07T15:40:01Z` for epoch seconds.
pub(super) fn iso(now: f64) -> String {
    let secs = now.floor() as i64;
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", t / 3600, t % 3600 / 60, t % 60)
}

/// `2026-11-07` (Asia/Singapore) for a UTC `2026-11-06T16:30:00…Z`; `unknown` when unparsable.
pub(super) fn sgt_day(utc: &str) -> String {
    let field = |a: usize, b: usize| utc.get(a..b).and_then(|s| s.parse::<i64>().ok());
    let (Some(y), Some(m), Some(d), Some(h)) = (field(0, 4), field(5, 7), field(8, 10), field(11, 13)) else { return "unknown".into() };
    let days = days_from_civil(y, m, d) + if h + 8 >= 24 { 1 } else { 0 };
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

fn write_lines(base: &Path, lines: &[(String, String)]) -> Result<()> {
    for (file, line) in lines {
        let path = base.join(file);
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path.parent().unwrap_or(base))?;
        std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&path)?.write_all(format!("{line}\n").as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "win_logs/tests.rs"]
mod tests;
