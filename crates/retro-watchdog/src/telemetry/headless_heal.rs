//! Self-heal for wedged pages in the shared headless Chrome (user request, 2026-10-22). That
//! afternoon Library Storybook stories pinned one renderer core each and stopped answering CDP;
//! the ship-web geometry check waiting on them never returned, so it held the `mars-crm-verify`
//! lease for 25+ min while eight ship-web requests queued. Every SAMPLE_SECS this averages each
//! headless renderer's CPU over HOT_SECS. Only while one is hot (or a page is already suspect) does
//! a worker thread probe every page target with `Runtime.evaluate`; a page that failed every probe
//! for HANG_SECS is closed through `/json/close`, which rejects the calls its client is waiting on.
//! Closures go to `headless-heals/<utc-day-ms>.ndjson`; state, with the lease view from
//! `lease_stall.rs`, to `headless-heal.json`.
use crate::{monitor::{root, write_json}, procs::Proc};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    net::TcpStream,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};
use tungstenite::Message;

const RENDERER: &str = "Google Chrome Helper (Renderer)";
/// The shared headless Chrome runs from this cache directory; the user's own Chrome never does.
const BROWSER_MARK: &str = "/mars-headless-chrome/";
const ACTIVE_PORT: &str = ".codex/shared-browser-mcp/chrome-headless-profile/DevToolsActivePort";
pub(super) const HOT_FLOOR: f64 = 0.8;
pub(super) const HOT_SECS: f64 = 120.0;
/// Above the ship-web per-story deadline (120 s in `storyPage.ts`), so that bound acts first.
pub(super) const HANG_SECS: f64 = 150.0;
pub(super) const SAMPLE_SECS: f64 = 30.0;
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const DAY_MS: f64 = 86_400_000.0;
const MAX_PROBE_GAP: f64 = 90.0;

pub(super) struct Probed { pub id: String, pub url: String, pub responsive: bool }
struct Round { at: f64, pages: Result<Vec<Probed>>, closed: Vec<Value> }

#[derive(Default)]
pub(super) struct HeadlessHeal {
    last_sample: f64,
    renderers: HashMap<(i32, i64), VecDeque<(f64, f64)>>,
    /// Target id → (first failed probe, url).
    pub(super) hung_since: HashMap<String, (f64, String)>,
    worker: Option<Receiver<Round>>,
    last_probe: Value,
}

impl HeadlessHeal {
    pub(super) fn observe(&mut self, home: &Path, now: f64, processes: &[Proc]) -> Result<()> {
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(round) => { self.worker = None; self.absorb(home, round); }
                Err(TryRecvError::Disconnected) => self.worker = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.last_sample > 0.0 && now >= self.last_sample && now - self.last_sample < SAMPLE_SECS {
            return Ok(());
        }
        self.last_sample = now;
        if self.last_probe["at_ms"].as_f64().is_some_and(|at|
            now * 1000.0 < at || now * 1000.0 - at > MAX_PROBE_GAP * 1000.0) {
            self.hung_since.clear();
        }
        let hot = self.sample(now, processes);
        if self.worker.is_none() && !cfg!(test) && (!hot.is_empty() || !self.hung_since.is_empty()) {
            let due = due(&self.hung_since, now);
            let port_file = PathBuf::from(std::env::var("HOME")?).join(ACTIVE_PORT);
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::Builder::new().name("watchdog-headless-heal".into()).spawn(move || {
                let started = Instant::now();
                let pages = probe(&port_file);
                // A recovered or navigated target must never be closed from an old suspicion.
                // A slow probe cannot establish a fresh failure for the entire batch.
                let closed = pages.as_ref().map(|pages| close_candidates(&due, pages,
                    started.elapsed().as_secs_f64()).iter().map(|(id, url)| close(&port_file, id, url)).collect())
                    .unwrap_or_default();
                let _ = sender.send(Round { at: crate::unix_now(), pages, closed });
            })?;
            self.worker = Some(receiver);
        }
        write_json(&root(home).join("headless-heal.json"), &json!({"at_ms": now * 1000.0, "error": null,
            "hot_renderers": hot, "suspect_pages": self.hung_since.iter()
                .map(|(id, (since, url))| json!({"id": id, "url": url, "unresponsive_s": now - since})).collect::<Vec<_>>(),
            "last_probe": self.last_probe, "probe_running": self.worker.is_some(),
            "leases": super::lease_stall::observe(home, now),
            "threshold": {"hot_floor_one_core": HOT_FLOOR, "hot_s": HOT_SECS, "hang_s": HANG_SECS}}))
    }

    /// Records each headless renderer's CPU; returns those averaging HOT_FLOOR over HOT_SECS.
    pub(super) fn sample(&mut self, now: f64, processes: &[Proc]) -> Vec<Value> {
        let live: Vec<&Proc> = processes.iter()
            .filter(|p| p.name == RENDERER && p.args.contains(BROWSER_MARK)).collect();
        self.renderers.retain(|key, _| live.iter().any(|p| (p.pid, p.start) == *key));
        let mut hot = Vec::new();
        for p in live {
            let samples = self.renderers.entry((p.pid, p.start)).or_default();
            if samples.back().is_some_and(|(at, cpu)| now <= *at || now - at > MAX_PROBE_GAP || p.cpu_secs < *cpu) {
                samples.clear();
            }
            samples.push_back((now, p.cpu_secs));
            while samples.len() > 1 && samples.get(1).is_some_and(|(at, _)| now - at >= HOT_SECS) {
                samples.pop_front();
            }
            let usage = samples.iter().rev().find(|(at, _)| now - at >= HOT_SECS)
                .map(|(at, cpu)| (p.cpu_secs - cpu).max(0.0) / (now - at));
            if usage.is_some_and(|u| u >= HOT_FLOOR) {
                hot.push(json!({"pid": p.pid, "usage_one_core": usage, "rss_kib": p.rss_kib}));
            }
        }
        hot
    }

    fn absorb(&mut self, home: &Path, round: Round) {
        for record in &round.closed {
            crate::log(&format!("headless heal: {record}"));
            if let Some(id) = record["id"].as_str() { self.hung_since.remove(id); }
            if let Err(error) = append(home, round.at, record) { crate::log(&format!("headless heal log: {error:#}")); }
            if let Err(error) = crate::recovery_retro::record(home, "headless", record) { crate::log(&format!("headless retro: {error:#}")); }
        }
        match round.pages {
            Ok(pages) => {
                self.hung_since.retain(|id, _| pages.iter().any(|p| &p.id == id && !p.responsive));
                for page in pages.iter().filter(|p| !p.responsive) {
                    self.hung_since.entry(page.id.clone()).or_insert((round.at, page.url.clone()));
                }
                self.last_probe = json!({"at_ms": round.at * 1000.0, "pages": pages.len(),
                    "unresponsive": pages.iter().filter(|p| !p.responsive).count(), "error": null});
            }
            // Chrome down or restarting: nothing to judge, and no page id survives a restart.
            Err(error) => {
                self.hung_since.clear();
                self.last_probe = json!({"at_ms": round.at * 1000.0, "error": format!("{error:#}")});
            }
        }
    }
}

/// Pages that failed every probe for at least HANG_SECS.
pub(super) fn due(hung_since: &HashMap<String, (f64, String)>, now: f64) -> Vec<(String, String)> {
    hung_since.iter().filter(|(_, (since, _))| now - since >= HANG_SECS)
        .map(|(id, (_, url))| (id.clone(), url.clone())).collect()
}

fn close_candidates(due: &[(String, String)], pages: &[Probed], elapsed: f64) -> Vec<(String, String)> {
    if elapsed > MAX_PROBE_GAP { return Vec::new(); }
    due.iter().filter(|(id, url)| pages.iter().any(|page|
        &page.id == id && &page.url == url && !page.responsive)).cloned().collect()
}

fn port(port_file: &Path) -> Result<u16> {
    let text = std::fs::read_to_string(port_file)?;
    Ok(text.lines().next().ok_or_else(|| anyhow!("empty {}", port_file.display()))?.trim().parse()?)
}

fn get(url: &str) -> Result<ureq::Response> {
    Ok(ureq::AgentBuilder::new().timeout(Duration::from_secs(3)).build().get(url).call()?)
}

fn probe(port_file: &Path) -> Result<Vec<Probed>> {
    let list: Value = get(&format!("http://127.0.0.1:{}/json/list", port(port_file)?))?.into_json()?;
    Ok(list.as_array().into_iter().flatten().filter(|t| t["type"] == "page").filter_map(|t| {
        let socket = t["webSocketDebuggerUrl"].as_str()?;
        Some(Probed { id: t["id"].as_str()?.into(), url: t["url"].as_str().unwrap_or_default().chars().take(200).collect(),
            responsive: answers(socket).is_ok() })
    }).collect())
}

/// One `Runtime.evaluate` of `1` on the page's own socket, answered within PROBE_TIMEOUT.
fn answers(socket_url: &str) -> Result<()> {
    let address = socket_url.strip_prefix("ws://").and_then(|rest| rest.split('/').next())
        .ok_or_else(|| anyhow!("unexpected page socket {socket_url}"))?;
    let stream = TcpStream::connect_timeout(&address.parse()?, PROBE_TIMEOUT)?;
    stream.set_read_timeout(Some(PROBE_TIMEOUT))?;
    stream.set_write_timeout(Some(PROBE_TIMEOUT))?;
    let (mut socket, _) = tungstenite::client::client(socket_url, stream).map_err(|e| anyhow!("{e}"))?;
    socket.send(Message::text(json!({"id": 1, "method": "Runtime.evaluate", "params": {"expression": "1"}}).to_string()))?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let answered = loop {
        anyhow::ensure!(Instant::now() < deadline, "no answer within {PROBE_TIMEOUT:?}");
        if let Message::Text(text) = socket.read()? {
            if serde_json::from_str::<Value>(&text).is_ok_and(|m| m["id"] == 1) { break Ok(()); }
        }
    };
    let _ = socket.close(None);
    answered
}

fn close(port_file: &Path, id: &str, url: &str) -> Value {
    let outcome = port(port_file).and_then(|port| get(&format!("http://127.0.0.1:{port}/json/close/{id}")));
    json!({"at_ms": crate::unix_now() * 1000.0, "id": id, "url": url, "hang_s": HANG_SECS,
        "outcome": match outcome { Ok(_) => "closed".into(), Err(error) => format!("failed: {error:#}") }})
}

fn append(home: &Path, now: f64, record: &Value) -> Result<()> {
    let directory = root(home).join("headless-heals");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let day = ((now * 1000.0) / DAY_MS).floor() * DAY_MS;
    std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(directory.join(format!("{}.ndjson", day as u64)))?
        .write_all(format!("{record}\n").as_bytes())?;
    Ok(())
}

#[cfg(test)]
#[path = "headless_heal_tests.rs"]
mod tests;
