//! WindowServer responsiveness, two requests timed apart. On 2026-10-21 Orca and Typeless main
//! threads froze for 1-4.5 s inside `+[CAFenceHandle newFenceFromDefaultServer]`, waiting on
//! WindowServer's `com.apple.CARenderServer` port, while host CPU stayed under 50% and `CGWindowList`
//! answered in under 5 ms. One resident thread therefore times that fence request every PERIOD, and a
//! second one times a window-list round-trip over the main connection. Each tick drains the samples
//! into `desktop.json`, appends the tick summary to `render-probe/<utc-day-ms>.ndjson`, and opens one
//! `windowserver_render_stall` incident per episode. The probe never signals a process.
//! Both threads run at user-interactive QoS, the class of an app's main thread: the resident
//! process otherwise runs at priority 20, below every build process, and on 2026-10-24 a probe at
//! that priority reported second-long "stalls" that were its own scheduling delay.
//! A request that starts within WAKE_GRACE_SECS of any online display being seen asleep is a display
//! waking, not render load: 5 of 8 stall episodes from 2026-10-27 to 10-31 began within 1 s of
//! powerd's "Display is turned on" on a near-idle host. Such a tick is logged with `display_wake`
//! and opens no incident; so is one while the display is still asleep, which nobody sees.
//! Until 2026-11-19 one thread ran both requests in turn under one in-flight clock: 11 of the 15
//! incidents opened by a request in flight were a slow window list behind a fence of 0-27 ms, reported
//! as "render fence took N ms" (2 were a slow fence, 2 left no following tick to tell), and no fence was requested while the window list waited (2026-11-18
//! 23:18 SGT: WindowServer logged a render callback 5.6 s late inside an 18 s window-list wait).
//! Now each request has its own thread and clock, the incident names the request that stalled
//! (`stalled_request`), and a window list counts from LIST_INCIDENT_MS, finished or in flight.
use super::diagnostics::incident;
use crate::monitor::root;
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const PERIOD: Duration = Duration::from_millis(500);
/// A fence round-trip at or above this is a stall, listed with its time.
const STALL_MS: f64 = 250.0;
/// A fence round-trip (or a fence request still in flight) at or above this opens an incident.
const INCIDENT_MS: f64 = 1000.0;
/// A window-list round-trip (or one still in flight) at or above this opens an incident. 29 days of
/// ticks to 2026-11-18 hold 16 awake episodes at this level, against 15 incidents the shared in-flight
/// clock opened by chance; 1 s would have opened 193.
const LIST_INCIDENT_MS: f64 = 5000.0;
/// An episode ends after this long without another incident-level stall.
const EPISODE_GAP_SECS: f64 = 300.0;
/// A stall starting this soon after the display was last seen asleep is a display wake.
const WAKE_GRACE_SECS: f64 = 5.0;
/// Bounds the samples kept between ticks if a tick is late.
const MAX_SAMPLES: usize = 1024;
const MAX_LISTED: usize = 32;
const DAY_MS: f64 = 86_400_000.0;

/// One finished request, stamped at its start.
#[derive(Clone, Copy)]
pub(super) struct Sample {
    pub at: f64,
    pub ms: f64,
}

/// What one tick drains: finished requests of both kinds and the age of any still in flight.
#[derive(Default)]
pub(super) struct Round {
    pub fences: Vec<Sample>,
    pub lists: Vec<Sample>,
    pub inflight_ms: Option<f64>,
    pub list_inflight_ms: Option<f64>,
    pub error: Option<String>,
    pub asleep_at: Option<f64>,
}

#[derive(Default)]
struct Shared {
    fences: Vec<Sample>,
    lists: Vec<Sample>,
    inflight_since: Option<f64>,
    list_inflight_since: Option<f64>,
    error: Option<String>,
    /// Last request start at which any online display reported asleep.
    asleep_at: Option<f64>,
}

#[derive(Default)]
pub struct RenderProbe {
    shared: Arc<Mutex<Shared>>,
    started: bool,
    episode: Option<(f64, f64)>,
}

impl RenderProbe {
    pub fn drain(&mut self, home: &Path, now: f64) -> Result<Value> {
        if !self.started && !cfg!(test) {
            let (fence, list) = (Arc::clone(&self.shared), Arc::clone(&self.shared));
            std::thread::Builder::new().name("windowserver-probe".into()).spawn(move || fence_loop(&fence))?;
            std::thread::Builder::new().name("windowserver-list-probe".into()).spawn(move || list_loop(&list))?;
            self.started = true;
        }
        let round = {
            let mut shared = self.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            // A request that began after this tick's `now` was taken is not yet late.
            let age = |since: Option<f64>| since.map(|since| ((now - since) * 1000.0).max(0.0));
            Round {
                fences: std::mem::take(&mut shared.fences), lists: std::mem::take(&mut shared.lists),
                inflight_ms: age(shared.inflight_since), list_inflight_ms: age(shared.list_inflight_since),
                error: shared.error.clone(), asleep_at: shared.asleep_at,
            }
        };
        self.summarize(home, now, round)
    }

    pub(super) fn summarize(&mut self, home: &Path, now: f64, round: Round) -> Result<Value> {
        let Round { fences, lists, inflight_ms, list_inflight_ms, error, asleep_at } = round;
        let sorted = |samples: &[Sample]| { let mut ms: Vec<f64> = samples.iter().map(|s| s.ms).collect(); ms.sort_by(f64::total_cmp); ms };
        let (fence, window) = (sorted(&fences), sorted(&lists));
        let stalls: Vec<Value> = fences.iter().filter(|s| s.ms >= STALL_MS)
            .map(|s| json!({"at_ms": s.at * 1000.0, "fence_ms": s.ms})).collect();
        let worst = |finished: &[f64], inflight: Option<f64>| finished.last().copied().into_iter().chain(inflight)
            .fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.max(b))));
        let (worst_fence, worst_list) = (worst(&fence, inflight_ms), worst(&window, list_inflight_ms));
        if self.episode.is_some_and(|(_, last)| now - last > EPISODE_GAP_SECS) {
            self.episode = None;
        }
        let (fence_severe, list_severe) = (worst_fence.is_some_and(|ms| ms >= INCIDENT_MS), worst_list.is_some_and(|ms| ms >= LIST_INCIDENT_MS));
        // Start of the first incident-level request of a kind: a finished sample's stamp, or the one in flight.
        let start = |samples: &[Sample], inflight: Option<f64>, level: f64| samples.iter().filter(|s| s.ms >= level).map(|s| s.at)
            .chain(inflight.filter(|&ms| ms >= level).map(|ms| now - ms / 1000.0)).fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.min(b))));
        // The fence thread stamps the display asleep at its own request start, so a fence wake starts at or
        // after that stamp. A window list waits on its own thread while the stamp moves on: asleep at any
        // time since its start is a display nobody saw.
        let woke = |started: Option<f64>, since_start: bool| started.zip(asleep_at)
            .is_some_and(|(s, a)| (since_start || s >= a) && s - a <= WAKE_GRACE_SECS);
        let fence_real = fence_severe && !woke(start(&fences, inflight_ms, INCIDENT_MS), false);
        let list_real = list_severe && !woke(start(&lists, list_inflight_ms, LIST_INCIDENT_MS), true);
        let display_wake = (fence_severe || list_severe) && !fence_real && !list_real;
        if fence_real || list_real {
            let since = self.episode.map_or(now, |(since, _)| since);
            self.episode = Some((since, now));
            let (request, detail) = if fence_real {
                ("fence", format!("WindowServer render fence took {:.0} ms", worst_fence.unwrap_or(0.0)))
            } else {
                ("window_list", format!("WindowServer window list (main connection) took {:.0} ms; render fence at most {:.0} ms",
                    worst_list.unwrap_or(0.0), worst_fence.unwrap_or(0.0)))
            };
            let id = incident(home, &format!("windowserver_render_stall:{}", since as i64), json!({
                "trigger": "windowserver_render_stall", "episode_since_ms": since * 1000.0, "stalled_request": request,
                "worst_fence_ms": worst_fence, "inflight_ms": inflight_ms, "stalls": &stalls[..stalls.len().min(MAX_LISTED)],
                "window_list_max_ms": worst_list, "window_list_inflight_ms": list_inflight_ms,
                "threshold": {"incident_ms": INCIDENT_MS, "window_list_incident_ms": LIST_INCIDENT_MS, "stall_ms": STALL_MS,
                    "episode_gap_s": EPISODE_GAP_SECS},
            }))?;
            super::desktop::wake(home, now, crate::wake::Condition::RenderStall, &id, &detail);
        }
        let summary = json!({
            "at_ms": now * 1000.0, "samples": fences.len(), "error": error,
            "fence_ms": {"p50": percentile(&fence, 0.5), "p90": percentile(&fence, 0.9), "max": fence.last()},
            "window_list_ms": {"p50": percentile(&window, 0.5), "max": window.last()},
            "window_list_samples": lists.len(), "window_list_inflight_ms": list_inflight_ms,
            "inflight_ms": inflight_ms, "stall_count": stalls.len(),
            "stalled_ms": fences.iter().filter(|s| s.ms >= STALL_MS).map(|s| s.ms).sum::<f64>(),
            "stalls": &stalls[..stalls.len().min(MAX_LISTED)],
            "episode_since_ms": self.episode.map(|(since, _)| since * 1000.0),
            "display_wake": display_wake, "display_asleep_at_ms": asleep_at.map(|a| a * 1000.0),
            "threshold": {"incident_ms": INCIDENT_MS, "window_list_incident_ms": LIST_INCIDENT_MS, "stall_ms": STALL_MS,
                "period_ms": PERIOD.as_millis() as u64},
        });
        append(home, now, &summary)?;
        Ok(summary)
    }
}

fn percentile(sorted: &[f64], q: f64) -> Option<f64> {
    (!sorted.is_empty()).then(|| sorted[((sorted.len() - 1) as f64 * q).round() as usize])
}

fn append(home: &Path, now: f64, summary: &Value) -> Result<()> {
    let directory = root(home).join("render-probe");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let day = ((now * 1000.0) / DAY_MS).floor() * DAY_MS;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(directory.join(format!("{}.ndjson", day as u64)))?;
    file.write_all(format!("{summary}\n").as_bytes())?;
    Ok(())
}

fn interactive(name: &str) {
    // SAFETY: changes only the calling thread's QoS class; a refusal leaves it unchanged.
    let qos = unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0) };
    if qos != 0 {
        crate::log(&format!("windowserver probe ({name}): user-interactive QoS refused ({qos}); stalls include scheduling delay"));
    }
}

/// Times the render-server fence, the request frozen app main threads wait on.
fn fence_loop(shared: &Mutex<Shared>) {
    interactive("fence");
    loop {
        // Samples are stamped at request start, so a stall lines up with the app freeze it caused.
        let at = crate::unix_now();
        let asleep = native::display_asleep();
        {
            let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
            state.inflight_since = Some(at);
            if asleep { state.asleep_at = Some(at); }
        }
        let started = Instant::now();
        let fence = native::fence_round_trip();
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        {
            let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
            state.inflight_since = None;
            match fence {
                Ok(()) => {
                    state.error = None;
                    if state.fences.len() < MAX_SAMPLES { state.fences.push(Sample { at, ms }); }
                }
                Err(error) => state.error = Some(error.into()),
            }
        }
        std::thread::sleep(PERIOD);
    }
}

/// Times a window-list round-trip, the main-connection control, apart from the fence: a list that
/// waits must neither stop the fence requests nor be read as a slow fence.
fn list_loop(shared: &Mutex<Shared>) {
    interactive("window list");
    loop {
        let at = crate::unix_now();
        shared.lock().unwrap_or_else(|p| p.into_inner()).list_inflight_since = Some(at);
        let started = Instant::now();
        native::window_list_round_trip();
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        {
            let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
            state.list_inflight_since = None;
            if state.lists.len() < MAX_SAMPLES { state.lists.push(Sample { at, ms }); }
        }
        std::thread::sleep(PERIOD);
    }
}

#[path = "render_probe_native.rs"]
mod native;

#[cfg(test)]
#[path = "render_probe_tests.rs"]
mod tests;
