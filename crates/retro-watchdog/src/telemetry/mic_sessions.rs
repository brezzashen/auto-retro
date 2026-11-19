//! Microphone session ledger that attributes ControlCenter's Mach port growth to an app. On
//! 2026-10-21 every microphone session leaked 20-170 ControlCenter ports (four 3 s ffmpeg
//! captures: +170, +20, +70, +63; idle intervals ~0), and Typeless opens the microphone on every
//! dictation. One thread polls CoreAudio's process objects every POLL. A process that starts
//! input opens a session: its executable, its responsible app (the one the privacy indicator
//! names) and the last ControlCenter port count before it began; SETTLE after input stops the
//! count is taken again. ControlCenter is also counted every COUNT_EVERY_SECS, and every change is
//! a line in `controlcenter-ports/<utc-day-ms>.ndjson`, to join with sessions and unified logs.
//! Each closed session is one line in `mic-sessions/<utc-day-ms>.ndjson` with `cc_port_delta`
//! and the sessions it overlapped; `mic-sessions.json` keeps active sessions and per-app totals
//! for the current UTC day. Counting ports reuses the census `sudo -A lsmp` path.
//! A delta is only taken within one ControlCenter instance: a session that spans a respawn
//! records `cc_port_delta: null` and `cc_restarted` (2026-10-21: 5,024 on the old pid, 1,079 on
//! the new one became -3,945). Overlapping sessions share one growth, so only a session whose
//! overlaps are all WARM_APP (mic-warm's HAL IOProc) is `attributed` and summed into the per-app
//! totals; the others count as `shared_sessions` (replayd +680 and mic-warm +552 were one jump).
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

const POLL: Duration = Duration::from_secs(1);
/// ControlCenter updates its privacy indicator modules a few seconds after input stops.
const SETTLE_SECS: f64 = 10.0;
const HEARTBEAT_SECS: f64 = 30.0;
/// ControlCenter port series resolution; each change is logged to `controlcenter-ports/`.
const COUNT_EVERY_SECS: f64 = 5.0;
const DAY_MS: f64 = 86_400_000.0;
/// The keeper the privacy indicator ignores; overlapping it does not share a session's delta.
const WARM_APP: &str = "mic-warm";

/// ControlCenter count: (time, pid, ports).
type Count = (f64, i32, u64);

#[derive(Clone)]
pub(super) struct Session {
    pub pid: i32,
    pub executable: String,
    pub app: String,
    pub start: f64,
    /// The last ControlCenter count taken before the session started.
    pub cc_before: Option<Count>,
    pub end: Option<f64>,
    pub overlapped: Vec<String>,
}

#[derive(Default)]
pub(super) struct Ledger {
    pub active: HashMap<i32, Session>,
    pub settling: Vec<Session>,
    /// Per app: (sessions, attributed delta, shared sessions).
    pub totals: BTreeMap<String, (u64, i64, u64)>,
    pub day: f64,
    pub last: Option<Count>,
}

fn attributed<'a>(mut overlapped: impl Iterator<Item = &'a str>) -> bool {
    overlapped.all(|app| app == WARM_APP)
}

fn add(total: &mut (u64, i64, u64), attributed: bool, delta: Option<i64>) {
    total.0 += 1;
    if attributed { total.1 += delta.unwrap_or(0) } else { total.2 += 1 }
}

impl Ledger {
    /// Rebuilds today's per-app totals from the day file, so a restart keeps the attribution.
    pub(super) fn load(home: &Path, now: f64) -> Self {
        let day = ((now * 1000.0) / DAY_MS).floor() * DAY_MS;
        let mut ledger = Ledger { day, ..Ledger::default() };
        let text = std::fs::read_to_string(root(home).join("mic-sessions").join(format!("{}.ndjson", day as u64))).unwrap_or_default();
        for event in text.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()) {
            // Lines written before `attributed` existed derive it from their overlaps.
            let solo = event["attributed"].as_bool().unwrap_or_else(|| attributed(
                event["overlapped"].as_array().into_iter().flatten().filter_map(Value::as_str)));
            let total = ledger.totals.entry(event["app"].as_str().unwrap_or("?").to_string()).or_default();
            add(total, solo, event["cc_port_delta"].as_i64());
        }
        ledger
    }

    /// Applies one poll: `running` holds the pids running input now; `count` reads ControlCenter.
    /// ControlCenter is counted every COUNT_EVERY_SECS and on every session change; a session
    /// takes the last count before it started, because ControlCenter can leak within the poll.
    pub(super) fn step(&mut self, home: &Path, now: f64, running: &[i32], count: &mut dyn FnMut() -> Option<(i32, u64)>,
        describe: &dyn Fn(i32) -> (String, String)) -> Result<bool> {
        let started: Vec<i32> = running.iter().copied().filter(|pid| !self.active.contains_key(pid)).collect();
        let stopped: Vec<i32> = self.active.keys().copied().filter(|pid| !running.contains(pid)).collect();
        let settled = |s: &Session| s.end.is_some_and(|end| now - end >= SETTLE_SECS);
        let due = self.settling.iter().any(settled);
        let previous = self.last;
        let day = ((now * 1000.0) / DAY_MS).floor() * DAY_MS;
        let current = if !started.is_empty() || due || previous.is_none_or(|(at, _, _)| now - at >= COUNT_EVERY_SECS) {
            count()
        } else {
            None
        };
        if let Some((pid, ports)) = current {
            if previous.map(|(_, p, n)| (p, n)) != Some((pid, ports)) {
                append(home, "controlcenter-ports", day, &json!({"at_ms": now * 1000.0, "pid": pid, "ports": ports}))?;
            }
            self.last = Some((now, pid, ports));
        }
        let changed = !started.is_empty() || !stopped.is_empty() || due;
        // A session overlaps every session that is active or still settling while it runs, both
        // ways: either one's settled count can include the other's growth.
        for pid in started {
            let (executable, app) = describe(pid);
            let overlapped = self.active.values().chain(&self.settling).map(|s| s.app.clone()).collect();
            for other in self.active.values_mut().chain(self.settling.iter_mut()) {
                other.overlapped.push(app.clone());
            }
            self.active.insert(pid, Session { pid, executable, app, start: now, cc_before: previous, end: None, overlapped });
        }
        for pid in stopped {
            let mut session = self.active.remove(&pid).expect("listed above");
            session.end = Some(now);
            session.overlapped.extend(self.active.values().map(|s| s.app.clone()));
            self.settling.push(session);
        }
        if due {
            let (ready, waiting): (Vec<Session>, Vec<Session>) = std::mem::take(&mut self.settling).into_iter().partition(|s| settled(s));
            self.settling = waiting;
            if day != self.day {
                self.day = day;
                self.totals.clear();
            }
            for mut session in ready {
                session.overlapped.sort();
                session.overlapped.dedup();
                let pair = session.cc_before.zip(current);
                let restarted = pair.is_some_and(|((_, before, _), (after, _))| before != after);
                let delta = pair.filter(|_| !restarted).map(|((_, _, before), (_, after))| after as i64 - before as i64);
                let solo = attributed(session.overlapped.iter().map(String::as_str));
                add(self.totals.entry(session.app.clone()).or_default(), solo, delta);
                append(home, "mic-sessions", day, &json!({
                    "app": session.app, "executable": session.executable, "pid": session.pid,
                    "start_ms": session.start * 1000.0, "end_ms": session.end.map(|e| e * 1000.0),
                    "duration_s": session.end.map(|e| e - session.start),
                    "cc_ports_before": session.cc_before.map(|(_, _, p)| p),
                    "cc_before_age_s": session.cc_before.map(|(at, _, _)| session.start - at),
                    "cc_ports_after": current.map(|(_, p)| p), "cc_port_delta": delta,
                    "cc_pid_before": session.cc_before.map(|(_, pid, _)| pid), "cc_pid_after": current.map(|(pid, _)| pid),
                    "cc_restarted": restarted, "attributed": solo, "overlapped": session.overlapped,
                }))?;
            }
        }
        Ok(changed)
    }

    pub(super) fn state(&self, now: f64, error: Option<&str>) -> Value {
        let active: Vec<Value> = self.active.values().map(|s| json!({"app": s.app, "executable": s.executable,
            "pid": s.pid, "start_ms": s.start * 1000.0})).collect();
        let totals: Vec<Value> = self.totals.iter().map(|(app, (sessions, delta, shared))| json!({"app": app,
            "sessions": sessions, "cc_port_delta": delta, "shared_sessions": shared})).collect();
        json!({"at_ms": now * 1000.0, "error": error, "active": active, "today": totals,
            "settle_s": SETTLE_SECS, "poll_ms": POLL.as_millis() as u64})
    }
}

fn append(home: &Path, name: &str, day: f64, event: &Value) -> Result<()> {
    let directory = root(home).join(name);
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(directory.join(format!("{}.ndjson", day as u64)))?;
    file.write_all(format!("{event}\n").as_bytes())?;
    Ok(())
}

/// Apps running microphone input right now, for `cpu-watchdog mic-sessions`.
pub fn live() -> Value {
    match native::running_input() {
        Ok(pids) => json!(pids.iter().map(|pid| { let (executable, app) = native::describe(*pid); json!({"pid": pid, "executable": executable, "app": app}) }).collect::<Vec<_>>()),
        Err(error) => json!({"error": error}),
    }
}

/// Starts the resident ledger thread once.
pub fn spawn(home: PathBuf) -> Result<()> {
    std::thread::Builder::new().name("watchdog-mic-sessions".into()).spawn(move || {
        let mut ledger = Ledger::load(&home, crate::unix_now());
        let mut written = 0.0;
        loop {
            let now = crate::unix_now();
            let (running, mut error) = match native::running_input() {
                Ok(running) => (running, None),
                Err(error) => (Vec::new(), Some(error.to_string())),
            };
            let mut count = || super::port_census::controlcenter_ports();
            let changed = match ledger.step(&home, now, &running, &mut count, &native::describe) {
                Ok(changed) => changed,
                Err(e) => { error = Some(format!("{e:#}")); true }
            };
            if changed || error.is_some() || now - written >= HEARTBEAT_SECS {
                let _ = write_json(&root(&home).join("mic-sessions.json"), &ledger.state(now, error.as_deref()));
                written = now;
            }
            std::thread::sleep(POLL);
        }
    })?;
    Ok(())
}

mod native {
    use std::ffi::c_void;
    const SYSTEM_OBJECT: u32 = 1;
    const fn fourcc(code: &[u8; 4]) -> u32 { u32::from_be_bytes(*code) }

    #[repr(C)]
    struct Address { selector: u32, scope: u32, element: u32 }

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectGetPropertyDataSize(object: u32, address: *const Address, qualifier_size: u32, qualifier: *const c_void, size: *mut u32) -> i32;
        fn AudioObjectGetPropertyData(object: u32, address: *const Address, qualifier_size: u32, qualifier: *const c_void, size: *mut u32, data: *mut c_void) -> i32;
    }
    extern "C" {
        // libquarantine (part of libSystem): the app macOS holds responsible for a helper process.
        fn responsibility_get_pid_responsible_for_pid(pid: i32) -> i32;
    }

    fn address(selector: &[u8; 4]) -> Address {
        Address { selector: fourcc(selector), scope: fourcc(b"glob"), element: 0 }
    }

    /// Pids of CoreAudio clients whose input is running (kAudioProcessPropertyIsRunningInput).
    pub fn running_input() -> Result<Vec<i32>, &'static str> {
        unsafe {
            let list = address(b"prs#");
            let mut size = 0u32;
            if AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &list, 0, std::ptr::null(), &mut size) != 0 {
                return Err("CoreAudio process object list unavailable");
            }
            let mut objects = vec![0u32; size as usize / 4];
            if AudioObjectGetPropertyData(SYSTEM_OBJECT, &list, 0, std::ptr::null(), &mut size, objects.as_mut_ptr().cast()) != 0 {
                return Err("CoreAudio process object list unreadable");
            }
            let mut pids = Vec::new();
            for object in objects {
                let (mut input, mut pid, mut four) = (0u32, 0i32, 4u32);
                if AudioObjectGetPropertyData(object, &address(b"piri"), 0, std::ptr::null(), &mut four, (&mut input as *mut u32).cast()) == 0
                    && input != 0
                {
                    four = 4;
                    if AudioObjectGetPropertyData(object, &address(b"ppid"), 0, std::ptr::null(), &mut four, (&mut pid as *mut i32).cast()) == 0 {
                        pids.push(pid);
                    }
                }
            }
            Ok(pids)
        }
    }

    fn path(pid: i32) -> Option<String> {
        let mut buffer = [0u8; 4096];
        // SAFETY: the buffer is writable and its capacity is passed.
        let n = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        (n > 0).then(|| String::from_utf8_lossy(&buffer[..n as usize]).into_owned())
    }

    /// Executable basename and the responsible app's bundle path (`Typeless.app`) or basename.
    pub fn describe(pid: i32) -> (String, String) {
        let name = |p: Option<String>| p.map(|p| {
            p.split('/').find(|part| part.ends_with(".app")).map(str::to_string)
                .unwrap_or_else(|| p.rsplit('/').next().unwrap_or_default().to_string())
        }).unwrap_or_else(|| format!("pid {pid}"));
        // SAFETY: a pure query by pid.
        let responsible = unsafe { responsibility_get_pid_responsible_for_pid(pid) };
        let executable = path(pid).map(|p| p.rsplit('/').next().unwrap_or_default().to_string()).unwrap_or_else(|| format!("pid {pid}"));
        (executable, name(path(if responsible > 0 { responsible } else { pid })))
    }
}

#[cfg(test)]
#[path = "mic_sessions_tests.rs"]
mod tests;
