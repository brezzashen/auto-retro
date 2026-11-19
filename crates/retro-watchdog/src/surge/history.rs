//! Per-process CPU-time samples for the surge rules: at most one every STEP_SECS, kept for
//! MAX_AGE_SECS so the one-hour cleanup row can be proven. A pid whose start time moves by more
//! than two seconds (ps rounds elapsed time) is a reused pid and starts a fresh track.
use crate::procs::Proc;
use std::collections::{HashMap, VecDeque};

const STEP_SECS: f64 = 30.0;
const MAX_AGE_SECS: f64 = 3_700.0;

#[derive(Clone, Default)]
struct Track {
    start: i64,
    samples: VecDeque<(f64, f64)>,
}

#[derive(Clone, Default)]
pub struct History {
    last: f64,
    tracks: HashMap<i32, Track>,
}

impl History {
    pub fn record(&mut self, now: f64, processes: &[Proc]) {
        if self.last > 0.0 && now >= self.last && now - self.last < STEP_SECS {
            return;
        }
        self.last = now;
        let live: HashMap<i32, i64> = processes.iter().map(|p| (p.pid, p.start)).collect();
        self.tracks.retain(|pid, _| live.contains_key(pid));
        for p in processes {
            let track = self.tracks.entry(p.pid).or_insert_with(|| Track { start: p.start, ..Track::default() });
            if (track.start - p.start).abs() > 2 {
                *track = Track { start: p.start, ..Track::default() };
            }
            track.samples.push_back((now, p.cpu_secs));
            while track.samples.front().is_some_and(|(at, _)| now - at > MAX_AGE_SECS) {
                track.samples.pop_front();
            }
        }
    }

    /// Average fraction of one core from the newest sample at least `window` seconds old to `p`'s
    /// current counter at `now`; None while the process has not been observed that long.
    pub fn usage(&self, p: &Proc, now: f64, window: f64) -> Option<f64> {
        let track = self.tracks.get(&p.pid).filter(|track| (track.start - p.start).abs() <= 2)?;
        let (at, cpu) = track.samples.iter().rev().find(|(at, _)| now - at >= window)?;
        Some((p.cpu_secs - cpu).max(0.0) / (now - at))
    }
}
