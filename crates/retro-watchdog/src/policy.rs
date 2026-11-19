//! Decides which processes ran away: a listed tool process of this user whose CPU time over the
//! last `window` seconds averages at least `threshold` of one core, and that is not part of a build.

use crate::procs::Proc;
use std::collections::{HashMap, VecDeque};

pub const DEFAULT_NAMES: &[&str] = &[
    "node",
    "bun",
    "deno",
    "esbuild",
    "swift-frontend",
    "sourcekit-lsp",
    "SourceKitService",
    "Google Chrome Helper (Renderer)",
    "Google Chrome for Testing Helper (Renderer)",
    "Chromium Helper (Renderer)",
];

/// A process is exempt while it or any ancestor's command line contains one of these: builds and
/// test runs legitimately hold a core for minutes.
pub const DEFAULT_BUILD_MARKERS: &[&str] = &[
    "xcodebuild",
    "SWBBuildService",
    "XCBBuildService",
    "cargo ",
    "mars-cpu-build-gate",
    "sim-lease run",
    "docker build",
    "swift build",
    "release build",
    "scripts/release.ts",
    "scripts/deliver.ts",
    // Queued GPU renders (`render-queue run -- Blender -b …`) hold cores for whole shots.
    "render-queue run",
];

/// A process is exempt while it or any ancestor's command line contains one of these: shared
/// services (debug Chrome, MCP gateways) and their children are not a runaway's fault.
pub const DEFAULT_SHARED_SERVICE_MARKERS: &[&str] = &[
    "/Applications/Chrome Debug Ports/",
    "/.local/bin/browser-mcp-gateway",
    "/.local/bin/mcp-gateway ",
];

pub struct Policy {
    pub uid: u32,
    /// Fraction of one core, e.g. 0.9.
    pub threshold: f64,
    pub window_secs: f64,
    pub names: Vec<String>,
    pub build_markers: Vec<String>,
    pub shared_markers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Reading {
    pub pid: i32,
    pub start: i64,
    pub name: String,
    pub args: String,
    /// Average fraction of one core over the observed span.
    pub usage: f64,
    pub observed_secs: f64,
    pub runaway: bool,
}

struct Track {
    start: i64,
    samples: VecDeque<(f64, f64)>,
}

#[derive(Default)]
pub struct Tracker {
    tracks: HashMap<i32, Track>,
}

impl Tracker {
    /// Records one snapshot taken at `now` and returns every eligible process's current reading.
    pub fn observe(&mut self, now: f64, procs: &[Proc], policy: &Policy) -> Vec<Reading> {
        let by_pid: HashMap<i32, &Proc> = procs.iter().map(|proc| (proc.pid, proc)).collect();
        let eligible: Vec<&Proc> = procs.iter().filter(|proc| policy.eligible(proc, &by_pid)).collect();
        self.tracks.retain(|pid, _| eligible.iter().any(|proc| proc.pid == *pid));
        eligible
            .into_iter()
            .map(|proc| {
                let track = self.tracks.entry(proc.pid).or_insert(Track { start: proc.start, samples: VecDeque::new() });
                // A reused pid (start time moved) starts a fresh history; ps rounds elapsed time to 1 s.
                if (track.start - proc.start).abs() > 2 {
                    *track = Track { start: proc.start, samples: VecDeque::new() };
                }
                track.samples.push_back((now, proc.cpu_secs));
                // Keep the newest sample at or before the window start as the anchor.
                while track.samples.len() > 2 && track.samples[1].0 <= now - policy.window_secs {
                    track.samples.pop_front();
                }
                let (anchor_time, anchor_cpu) = track.samples[0];
                let observed_secs = now - anchor_time;
                let usage = if observed_secs > 0.0 { (proc.cpu_secs - anchor_cpu) / observed_secs } else { 0.0 };
                Reading {
                    pid: proc.pid,
                    start: proc.start,
                    name: proc.name.clone(),
                    args: proc.args.clone(),
                    usage,
                    observed_secs,
                    runaway: observed_secs >= policy.window_secs && usage >= policy.threshold,
                }
            })
            .collect()
    }

    pub fn forget(&mut self, pid: i32) {
        self.tracks.remove(&pid);
    }
}

impl Policy {
    /// The resident policy as the LaunchAgent environment configures it (same variables as `main.rs`).
    pub fn from_env() -> Policy {
        let number = |name: &str, default: f64| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
        let list = |name: &str, default: &[&str]| -> Vec<String> {
            std::env::var(name).map_or_else(
                |_| default.iter().map(|item| item.to_string()).collect(),
                |value| value.split(',').map(str::trim).filter(|item| !item.is_empty()).map(String::from).collect(),
            )
        };
        Policy {
            // SAFETY: getuid has no preconditions and cannot fail.
            uid: unsafe { libc::getuid() },
            threshold: number("THRESHOLD_PCT", 90.0) / 100.0,
            window_secs: number("WINDOW_SECS", 300.0),
            names: list("NAMES", DEFAULT_NAMES),
            build_markers: list("BUILD_MARKERS", DEFAULT_BUILD_MARKERS),
            shared_markers: list("SHARED_MARKERS", DEFAULT_SHARED_SERVICE_MARKERS),
        }
    }

    pub(crate) fn eligible(&self, proc: &Proc, by_pid: &HashMap<i32, &Proc>) -> bool {
        proc.uid == self.uid
            && self.names.contains(&proc.name)
            && !ancestry_matches(proc, by_pid, &self.build_markers)
            && !ancestry_matches(proc, by_pid, &self.shared_markers)
    }
}

/// True when the process or one of its ancestors has a command line containing one of `markers`.
fn ancestry_matches(proc: &Proc, by_pid: &HashMap<i32, &Proc>, markers: &[String]) -> bool {
    let mut current = Some(proc);
    // The depth bound guards against a ppid cycle in a torn snapshot.
    for _ in 0..64 {
        let Some(node) = current else { return false };
        if markers.iter().any(|marker| node.args.contains(marker.as_str())) {
            return true;
        }
        if node.ppid <= 1 || node.ppid == node.pid {
            return false;
        }
        current = by_pid.get(&node.ppid).copied();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        Policy {
            uid: 501,
            threshold: 0.9,
            window_secs: 300.0,
            names: DEFAULT_NAMES.iter().map(|name| name.to_string()).collect(),
            build_markers: DEFAULT_BUILD_MARKERS.iter().map(|marker| marker.to_string()).collect(),
            shared_markers: DEFAULT_SHARED_SERVICE_MARKERS.iter().map(|marker| marker.to_string()).collect(),
        }
    }

    fn proc(pid: i32, ppid: i32, name: &str, args: &str, cpu_secs: f64) -> Proc {
        Proc { pid, ppid, uid: 501, start: 1_000, cpu_secs, rss_kib: 0, name: name.into(), args: args.into() }
    }

    fn run(tracker: &mut Tracker, usage_per_sec: f64, seconds: u32) -> Reading {
        let mut last = None;
        for step in 0..=seconds / 30 {
            let elapsed = f64::from(step * 30);
            let procs = [proc(42, 7, "node", "node spin.js", elapsed * usage_per_sec)];
            last = tracker.observe(2_000.0 + elapsed, &procs, &policy()).into_iter().next();
        }
        last.expect("node stays eligible")
    }

    #[test]
    fn a_core_held_for_the_whole_window_is_a_runaway() {
        let reading = run(&mut Tracker::default(), 1.0, 300);
        assert!(reading.runaway);
        assert!((reading.usage - 1.0).abs() < 1e-9);
    }

    #[test]
    fn short_or_light_load_is_not_a_runaway() {
        assert!(!run(&mut Tracker::default(), 1.0, 270).runaway, "under five minutes");
        assert!(!run(&mut Tracker::default(), 0.8, 600).runaway, "below 90%");
    }

    #[test]
    fn the_window_slides_so_old_idle_time_stops_counting() {
        let mut tracker = Tracker::default();
        let policy = policy();
        let mut cpu = 0.0;
        for step in 0..=40 {
            let now = f64::from(step * 30);
            if step > 20 {
                cpu += 30.0;
            }
            let reading = &tracker.observe(now, &[proc(42, 7, "node", "node", cpu)], &policy)[0];
            if step == 40 {
                assert!(reading.runaway, "busy for the last ten minutes: {reading:?}");
                assert!(reading.observed_secs < 330.0);
            }
        }
    }

    #[test]
    fn builds_unlisted_names_and_other_users_are_exempt() {
        let policy = policy();
        let mut tracker = Tracker::default();
        let gate = proc(9, 1, "bun", "bun /Users/x/.local/state/mars-cpu-build-gate/cli.ts run --slot", 0.0);
        let compiler = proc(42, 10, "swift-frontend", "swift-frontend -frontend -c", 900.0);
        let driver = proc(10, 9, "swift-driver", "swift-driver", 0.0);
        assert!(tracker.observe(0.0, &[gate, driver, compiler], &policy).is_empty(), "inside a build");
        let xcode_build = proc(11, 1, "SWBBuildService", "/Applications/Xcode.app/.../SWBBuildService.bundle/Contents/MacOS/SWBBuildService", 0.0);
        let ide_compiler = proc(43, 11, "swift-frontend", "swift-frontend -frontend -c", 900.0);
        assert!(tracker.observe(0.0, &[xcode_build, ide_compiler], &policy).is_empty(), "inside an Xcode IDE build");
        let ship = proc(12, 1, "bun", "bun scripts/release.ts ship-web web/src/a.ts", 0.0);
        let renderer = proc(44, 12, "Google Chrome Helper (Renderer)", "Google Chrome Helper (Renderer) --type=renderer", 900.0);
        assert!(tracker.observe(0.0, &[ship, renderer], &policy).is_empty(), "inside a release run");
        let queue = proc(13, 1, "render-queue", "/Users/x/.local/bin/render-queue run --project Feature90 --shot S03 --frames 170 -- Blender -b s.blend", 0.0);
        let render = proc(45, 13, "node", "node encode-frames.js", 900.0);
        assert!(tracker.observe(0.0, &[queue, render], &policy).is_empty(), "inside a queued GPU render");

        let mut stranger = proc(5, 1, "node", "node", 0.0);
        stranger.uid = 0;
        let unlisted = proc(6, 1, "Xcode", "Xcode", 0.0);
        assert!(tracker.observe(0.0, &[stranger, unlisted], &policy).is_empty());
    }

    #[test]
    fn shared_services_and_their_children_are_exempt() {
        let policy = policy();
        let mut tracker = Tracker::default();
        let debug_chrome = proc(20, 1, "Google Chrome", "/home/user/Applications/Chrome Debug Ports/Chrome Debug 9223.app/Contents/MacOS/Google Chrome --remote-debugging-port=9223", 0.0);
        let renderer = proc(21, 20, "Google Chrome Helper (Renderer)", "Google Chrome Helper (Renderer) --type=renderer", 900.0);
        assert!(tracker.observe(0.0, &[debug_chrome, renderer], &policy).is_empty(), "debug Chrome renderer");
        let gateway = proc(30, 1, "node", "/home/user/.local/bin/browser-mcp-gateway --config x", 0.0);
        let npx = proc(31, 30, "npx", "npx chrome-devtools-mcp", 0.0);
        let child = proc(32, 31, "node", "node chrome-devtools-mcp.js", 900.0);
        assert!(tracker.observe(0.0, &[gateway, npx, child], &policy).is_empty(), "grandchild of a gateway");
    }

    #[test]
    fn a_reused_pid_restarts_its_history() {
        let mut tracker = Tracker::default();
        let policy = policy();
        tracker.observe(0.0, &[proc(42, 7, "node", "node", 0.0)], &policy);
        let mut reborn = proc(42, 7, "node", "node", 400.0);
        reborn.start = 1_300;
        let reading = &tracker.observe(400.0, &[reborn], &policy)[0];
        assert_eq!(reading.observed_secs, 0.0);
        assert!(!reading.runaway);
    }
}
