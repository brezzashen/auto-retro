//! Paging thrash: the memory state the swap rule and every admission gate missed. On 2026-11-18
//! 23:10-23:16 SGT the desktop froze (kernel_task 350-480 % compressing and paging, sys CPU 60-75 %)
//! while `memory-guard/series` read swap 14 GiB of 16, 44-48 % `kern.memorystatus_level` and
//! `kern.memorystatus_vm_pressure_level` 1: `Reading::exhausted` (swap >= 24 GiB) stayed false and
//! sim-lease, lease.js and render-queue all admitted. What did move was the compressor: 0.05 GiB
//! free and 72-79 GiB of decompressions per minute (normal load 0-15, busy 30-60).
//!
//! The guard keeps the decompression rate over the trailing `WINDOW_SECS` of its 5 s samples and
//! publishes it in `memory-guard.json` (`thrash`), which every boot and build gate reads, so the
//! thresholds live here once:
//! - `BOOT_HOLD_GIB_PER_MIN` (40): a new simulator, emulator or Gradle/hvigor build waits. Over the
//!   850 minutes of 2026-11-18 it held 23 minutes the pressure rule admitted, 13 of them the freeze.
//! - `RELIEF_GIB_PER_MIN` (60) with free memory under `FREE_BYTES` for `RELIEF_SUSTAIN_SECS`: the
//!   episode counts as exhausted (relief and one `memory_exhausted` incident). Backtest on the same
//!   day: two episodes (18:12, swap 21 GiB, and the 23:07 freeze); one minute of sustain would
//!   have opened 19, each waking a perf session.
use super::memory_guard::FREE_BYTES;
use serde_json::{json, Value};
use std::collections::VecDeque;

pub const WINDOW_SECS: f64 = 60.0;
pub const BOOT_HOLD_GIB_PER_MIN: f64 = 40.0;
pub const RELIEF_GIB_PER_MIN: f64 = 60.0;
pub const RELIEF_SUSTAIN_SECS: f64 = 180.0;
const GIB: f64 = 1_073_741_824.0;

#[derive(Default)]
pub struct Thrash { samples: VecDeque<(f64, f64)>, since: Option<f64> }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Verdict {
    /// Decompressed GiB per minute over the trailing window; `None` until half a window is sampled.
    pub rate: Option<f64>,
    pub boot_hold: bool,
    /// The relief rate with low free memory has lasted `RELIEF_SUSTAIN_SECS`.
    pub exhausted: bool,
    pub since: Option<f64>,
}

impl Thrash {
    /// One guard sample: `decompressions` is the cumulative `vm_stat` counter in bytes.
    pub fn observe(&mut self, now: f64, decompressions: Option<f64>, free: f64) -> Verdict {
        match decompressions {
            Some(total) => self.samples.push_back((now, total)),
            None => self.samples.clear(),
        }
        while self.samples.front().is_some_and(|(t, _)| now - t > WINDOW_SECS) {
            self.samples.pop_front();
        }
        let rate = match (self.samples.front(), self.samples.back()) {
            (Some(&(t0, d0)), Some(&(t1, d1))) if t1 - t0 >= WINDOW_SECS / 2.0 => Some((d1 - d0).max(0.0) / GIB / (t1 - t0) * 60.0),
            _ => None,
        };
        let hot = rate.is_some_and(|r| r >= RELIEF_GIB_PER_MIN) && free < FREE_BYTES;
        self.since = if hot { Some(self.since.unwrap_or(now)) } else { None };
        Verdict { rate, boot_hold: rate.is_some_and(|r| r >= BOOT_HOLD_GIB_PER_MIN),
            exhausted: self.since.is_some_and(|s| now - s >= RELIEF_SUSTAIN_SECS), since: self.since }
    }
}

impl Verdict {
    pub fn json(&self, now: f64) -> Value {
        json!({"decompress_gib_per_min": self.rate.map(|r| (r * 10.0).round() / 10.0), "boot_hold": self.boot_hold,
            "exhausted": self.exhausted, "hot_for_s": self.since.map(|s| (now - s).round())})
    }
}

pub fn thresholds() -> Value {
    json!({"window_s": WINDOW_SECS, "boot_hold_gib_per_min": BOOT_HOLD_GIB_PER_MIN,
        "relief_gib_per_min": RELIEF_GIB_PER_MIN, "relief_sustain_s": RELIEF_SUSTAIN_SECS})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds 5 s samples decompressing `per_min` GiB per minute from `start` for `secs`.
    fn run(t: &mut Thrash, start: f64, secs: f64, per_min: f64, free_gib: f64, base: &mut f64) -> Verdict {
        let mut last = None;
        let mut now = start;
        while now <= start + secs {
            *base += per_min / 12.0 * GIB;
            last = Some(t.observe(now, Some(*base), free_gib * GIB));
            now += 5.0;
        }
        last.unwrap()
    }

    #[test]
    fn the_23_10_freeze_holds_boots_and_becomes_exhaustion_after_three_minutes() {
        let (mut t, mut d) = (Thrash::default(), 0.0);
        let early = run(&mut t, 0.0, 60.0, 76.0, 0.05, &mut d);
        assert!(early.boot_hold && !early.exhausted, "{early:?}");
        assert!((early.rate.unwrap() - 76.0).abs() < 0.5, "{early:?}");
        let later = run(&mut t, 65.0, 180.0, 76.0, 0.05, &mut d);
        assert!(later.exhausted, "{later:?}");
    }

    #[test]
    fn busy_paging_or_free_memory_is_not_exhaustion() {
        let (mut t, mut d) = (Thrash::default(), 0.0);
        let busy = run(&mut t, 0.0, 600.0, 45.0, 0.1, &mut d);
        assert!(busy.boot_hold && !busy.exhausted, "45 GiB/min holds boots only: {busy:?}");
        let (mut t, mut d) = (Thrash::default(), 0.0);
        let roomy = run(&mut t, 0.0, 600.0, 80.0, 4.0, &mut d);
        assert!(!roomy.exhausted, "4 GiB free is not exhaustion: {roomy:?}");
        let (mut t, mut d) = (Thrash::default(), 0.0);
        let calm = run(&mut t, 0.0, 120.0, 10.0, 0.1, &mut d);
        assert!(!calm.boot_hold && !calm.exhausted);
    }

    #[test]
    fn a_dip_restarts_the_sustain_and_a_short_window_gives_no_rate() {
        let mut t = Thrash::default();
        assert_eq!(t.observe(0.0, Some(0.0), 0.0).rate, None);
        assert_eq!(t.observe(20.0, Some(30.0 * GIB), 0.0).rate, None, "20 s is under half a window");
        let (mut t, mut d) = (Thrash::default(), 0.0);
        run(&mut t, 0.0, 150.0, 80.0, 0.1, &mut d);
        let dip = run(&mut t, 155.0, 60.0, 5.0, 0.1, &mut d);
        assert!(dip.since.is_none());
        let again = run(&mut t, 220.0, 120.0, 80.0, 0.1, &mut d);
        assert!(!again.exhausted, "the sustain starts over after a dip: {again:?}");
        assert_eq!(t.observe(400.0, None, 0.0).rate, None, "an unreadable vm_stat clears the window");
    }
}
