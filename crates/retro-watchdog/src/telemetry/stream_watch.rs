//! Stream watch (founder, 2026-11-14: Moonlight over studio Wi-Fi from the Windows PC's Sunshine to
//! an iPhone felt 400-500 ms late with dropped frames while playing Phantom Blade Zero).
//!
//! Every IDLE_SECS a worker runs `stream_watch/probe.ps1` on the remote host (the tunnel's SSH alias,
//! script piped over stdin because it exceeds the cmd.exe line limit). While Sunshine has a client
//! connected (its log's last CLIENT CONNECTED is newer than the last DISCONNECTED) it samples every
//! ACTIVE_SECS: NVENC fps and latency, GPU load and top process, client ping, NIC rates, Steam
//! download rate, offload jobs, the session's stream format; and from the router's AiMesh client
//! list (`ssh router`), the client's node, band and RSSI. `judge.rs` turns each sample into
//! breaches; one held SUSTAIN samples notifies once per session. When the client leaves, the session
//! summary goes to `stream-watch/sessions.ndjson`. Samples: `stream-watch/<session>.ndjson`; state:
//! `stream-watch.json`; `cpu-watchdog stream-stats` prints both. Nothing on either host is changed,
//! and client decode time stays invisible here (only the client's statistics overlay shows it).
use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

pub(super) const IDLE_SECS: f64 = 60.0;
pub(super) const ACTIVE_SECS: f64 = 15.0;
/// Probe errors in a row that end an open session (about 2 minutes while active).
const LOST_ERRORS: u32 = 8;
const SSH_SECS: u64 = 40;
const ROUTER_ALIAS: &str = "router";
const PROBE: &str = include_str!("stream_watch/probe.ps1");

struct Sample { sample: Value, router: Value, error: Option<String> }

#[derive(Default)]
pub(super) struct StreamWatch { last_sample: f64, worker: Option<Receiver<Sample>>, session: Option<judge::Session>, last: Value, error: Option<String> }

impl StreamWatch {
    pub(super) fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(sample) => { self.worker = None; self.absorb(home, now, sample); }
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let every = if self.session.is_some() { ACTIVE_SECS } else { IDLE_SECS };
        if self.worker.is_none() && (self.last_sample <= 0.0 || now < self.last_sample || now - self.last_sample >= every) {
            self.last_sample = now;
            if let Some(host) = super::docker_watch::remote_host(home) {
                let (sender, receiver) = mpsc::sync_channel(1);
                std::thread::Builder::new().name("watchdog-stream-watch".into())
                    .spawn(move || { let _ = sender.send(round(&host)); })?;
                self.worker = Some(receiver);
            }
        }
        write_json(&root(home).join("stream-watch.json"), &json!({"at_ms": now * 1000.0, "error": self.error,
            "session": self.session.as_ref().map(|s| json!({"key": s.key, "started_ms": s.started_ms, "samples": s.samples})),
            "last": self.last, "threshold": {"idle_s": IDLE_SECS, "active_s": ACTIVE_SECS, "rtt_avg_ms": judge::RTT_AVG_MS,
                "rtt_max_ms": judge::RTT_MAX_MS, "lost_pings": judge::LOST_PINGS, "rssi_min": judge::RSSI_MIN, "fps_ratio": judge::FPS_RATIO,
                "steam_mbps": judge::STEAM_MBPS, "nic_rx_mbps": judge::NIC_RX_MBPS, "sustain": judge::SUSTAIN}}))
    }

    fn absorb(&mut self, home: &Path, now: f64, round: Sample) {
        let now_ms = now * 1000.0;
        self.error = round.error.clone();
        if round.error.is_some() {
            if let Some(session) = &mut self.session {
                session.errors += 1;
                if session.errors >= LOST_ERRORS { self.close(home, "probe_lost"); }
            }
            return;
        }
        let connected = round.sample["connected"] == true;
        let key = round.sample["connected_at"].as_str().unwrap_or("").replace([' ', ':'], "-");
        if self.session.as_ref().is_some_and(|s| !connected || s.key != key) { self.close(home, if connected { "reconnected" } else { "disconnected" }); }
        let breaches = if connected {
            let session = self.session.get_or_insert_with(|| judge::Session::new(&key, now_ms));
            session.errors = 0;
            let fire = session.add(now_ms, &round.sample, &round.router);
            let record = json!({"at_ms": now_ms, "sample": round.sample, "router": round.router,
                "breaches": judge::breaches(&round.sample, &round.router).iter().map(|(k, d)| json!({"kind": k, "detail": d})).collect::<Vec<_>>()});
            if let Err(error) = append(home, &format!("{key}.ndjson"), &record) { crate::log(&format!("stream watch log: {error:#}")); }
            for (kind, detail) in &fire {
                crate::log(&format!("stream watch: {kind} {detail}"));
                crate::dispatch::notify("看门狗：串流卡顿", &format!("{}：{detail}（客户端 {}）", judge::label(kind).0, round.sample["client_ip"].as_str().unwrap_or("?")));
            }
            record["breaches"].clone()
        } else { Value::Array(Vec::new()) };
        self.last = json!({"at_ms": now_ms, "connected": connected, "sample": round.sample, "router": round.router, "breaches": breaches});
    }

    fn close(&mut self, home: &Path, ended_by: &str) {
        let Some(session) = self.session.take() else { return };
        let summary = session.summary(ended_by);
        crate::log(&format!("stream watch session: {summary}"));
        if let Err(error) = append(home, "sessions.ndjson", &summary) { crate::log(&format!("stream watch log: {error:#}")); }
        let top: Vec<String> = summary["breaches"].as_array().into_iter().flatten().take(3)
            .map(|b| format!("{} {}%", b["label"].as_str().unwrap_or(""), b["share_pct"])).collect();
        if !top.is_empty() {
            crate::dispatch::notify("看门狗：串流结束", &format!("{} 分钟，RTT p95 {} ms：{}", summary["minutes"], summary["rtt_ms"]["p95"], top.join("，")));
        }
    }
}

fn round(host: &str) -> Sample {
    let sample = match ssh_stdin(host, "powershell -NoProfile -NonInteractive -Command \"iex ([Console]::In.ReadToEnd())\"", PROBE)
        .and_then(|text| last_json(&text)) {
        Ok(value) => value,
        Err(error) => return Sample { sample: Value::Null, router: Value::Null, error: Some(format!("{error:#}")) },
    };
    let router = match sample["client_ip"].as_str() {
        Some(ip) if sample["connected"] == true => client_link(ip),
        _ => Value::Null,
    };
    Sample { sample, router, error: None }
}

/// The last line of `text` that parses as a JSON object (PowerShell may print CLIXML noise first).
pub(super) fn last_json(text: &str) -> Result<Value> {
    text.lines().rev().find_map(|l| serde_json::from_str::<Value>(l.trim()).ok().filter(Value::is_object))
        .with_context(|| format!("no JSON in probe output: {}", text.chars().take(200).collect::<String>()))
}

/// The client's AiMesh node, band and RSSI from the main router's client list.
fn client_link(ip: &str) -> Value {
    let command = "cat /tmp/clientlist.json; echo; nvram get cfg_device_list";
    match ssh_stdin(ROUTER_ALIAS, command, "") {
        Ok(text) => locate(&text, ip),
        Err(error) => json!({"error": format!("{error:#}")}),
    }
}

/// Find `ip` in the router output: line 1 the client list JSON, line 2 `cfg_device_list`.
pub(super) fn locate(text: &str, ip: &str) -> Value {
    let mut lines = text.lines();
    let Some(list) = lines.next().and_then(|l| serde_json::from_str::<Value>(l).ok()) else { return json!({"error": "unreadable clientlist"}) };
    let devices = lines.next().unwrap_or("");
    let node_ip = |mac: &str| devices.split('<').find_map(|d| { let f: Vec<&str> = d.split('>').collect(); (f.len() >= 3 && f[2].eq_ignore_ascii_case(mac)).then(|| f[1].to_string()) });
    for (node, bands) in list.as_object().into_iter().flatten() {
        for (band, clients) in bands.as_object().into_iter().flatten() {
            for (mac, client) in clients.as_object().into_iter().flatten() {
                if client["ip"].as_str() == Some(ip) {
                    let rssi = client["rssi"].as_str().and_then(|r| r.parse::<i64>().ok());
                    return json!({"node": node, "node_ip": node_ip(node), "band": band, "mac": mac, "rssi": rssi});
                }
            }
        }
    }
    json!({"error": format!("{ip} not in the AiMesh client list")})
}

/// Run `command` on `host` with `input` on stdin, killed after SSH_SECS.
fn ssh_stdin(host: &str, command: &str, input: &str) -> Result<String> {
    let mut child = Command::new("/usr/bin/ssh").args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", host, command])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().context("start ssh")?;
    let mut stdin = child.stdin.take().context("ssh stdin")?;
    let input = input.to_string();
    let writer = std::thread::spawn(move || { let _ = stdin.write_all(input.as_bytes()); });
    let mut stdout = child.stdout.take().context("ssh stdout")?;
    let reader = std::thread::spawn(move || { let mut out = Vec::new(); let _ = stdout.by_ref().take(1 << 20).read_to_end(&mut out); out });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? { break status; }
        if start.elapsed() >= Duration::from_secs(SSH_SECS) {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("ssh {host} timed out after {SSH_SECS}s");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = writer.join();
    let out = String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned();
    anyhow::ensure!(status.success(), "ssh {host} exited {status}");
    Ok(out)
}

fn append(home: &Path, name: &str, record: &Value) -> Result<()> {
    let directory = root(home).join("stream-watch");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(directory.join(name))?.write_all(format!("{record}\n").as_bytes())?;
    Ok(())
}

/// `cpu-watchdog stream-stats [--last N]`: the live state and the last N session summaries.
pub(super) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let last = match args.as_slice() {
        [] => 5,
        [flag, n] if flag == "--last" => n.parse().context("--last takes a number")?,
        _ => anyhow::bail!("usage: cpu-watchdog stream-stats [--last N]"),
    };
    let state: Value = std::fs::read_to_string(root(home).join("stream-watch.json")).ok()
        .and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let text = std::fs::read_to_string(root(home).join("stream-watch/sessions.ndjson")).unwrap_or_default();
    let sessions: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let tail = &sessions[sessions.len().saturating_sub(last)..];
    println!("{}", serde_json::to_string_pretty(&json!({"state": state, "sessions": tail}))?);
    Ok(())
}

#[path = "stream_watch/judge.rs"]
mod judge;

#[cfg(test)]
#[path = "stream_watch/tests.rs"]
mod tests;
