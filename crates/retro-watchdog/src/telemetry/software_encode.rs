//! Software video encode guard (founder request 2026-11-02: video cutting and editing on this Mac
//! uses the GPU, never the CPU).
//!
//! Perf session 81170095979879900502: a report build fell back from VideoToolbox to libx265 for
//! about 100 clips at once (71 ffmpeg, 18-22 cores) and WindowServer stalled for 70.8 s. Every
//! CHECK_SECS this guard finds the user's ffmpeg processes that encode video in software — an
//! explicit software encoder (`-c:v libx264`, `libx265`, `libvpx`, …) or a video file output with
//! no video codec, which ffmpeg fills with libx264 — and at once demotes each (nice 20 plus the
//! Darwin background policy, so it runs on efficiency cores) without stopping it. Each demotion is
//! appended to `software-encodes/<utc-day-ms>.ndjson`. An encode still running at the next check
//! (a sustained load, not a one-second test fixture) sends its caller (the parent's command) to
//! `recovery_retro` as kind `software_encode`, at most once per caller in CALLER_WINDOW_MS, so the
//! performance session moves that caller onto VideoToolbox.
use crate::monitor::{root, write_json};
use crate::procs::Proc;
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::{collections::HashMap, io::Write, os::unix::fs::{DirBuilderExt, OpenOptionsExt}, path::Path};

pub(super) const CHECK_SECS: f64 = 15.0;
const CALLER_WINDOW_MS: f64 = 86_400_000.0;
const DAY_MS: f64 = 86_400_000.0;
const SOFTWARE: [&str; 17] = ["libx264", "libx264rgb", "libx265", "libvpx", "libvpx-vp9", "libaom-av1", "libsvtav1",
    "librav1e", "libkvazaar", "libxvid", "libopenh264", "mpeg4", "mpeg2video", "prores", "prores_ks", "prores_aw", "dnxhd"];
const VIDEO_CODEC_FLAGS: [&str; 6] = ["-c:v", "-vcodec", "-codec:v", "-c:v:0", "-c", "-codec"];
/// Output extensions whose ffmpeg default video encoder is a software one.
const VIDEO_OUTPUTS: [&str; 6] = [".mp4", ".mov", ".m4v", ".mkv", ".webm", ".avi"];

/// A demoted encode: its start time, the record logged, and whether its caller was sent for review.
struct Demoted { start: i64, record: Value, reviewed: bool }

#[derive(Default)]
pub(super) struct SoftwareEncode {
    last_check: f64,
    demoted: HashMap<i32, Demoted>,
}

/// `ps` start times of one process can differ by a second between snapshots.
fn same_start(a: i64, b: i64) -> bool { (a - b).abs() <= 2 }

impl SoftwareEncode {
    pub(super) fn observe(&mut self, home: &Path, now: f64, processes: &[Proc]) -> Result<()> {
        if self.last_check > 0.0 && now >= self.last_check && now - self.last_check < CHECK_SECS {
            return Ok(());
        }
        self.last_check = now;
        let uid = unsafe { libc::getuid() };
        let found: Vec<(&Proc, String)> = processes.iter()
            .filter(|p| p.uid == uid && p.name == "ffmpeg")
            .filter_map(|p| software_encoder(&p.args).map(|encoder| (p, encoder)))
            .collect();
        self.demoted.retain(|pid, d| found.iter().any(|(p, _)| p.pid == *pid && same_start(p.start, d.start)));
        let state_path = root(home).join("software-encode-watch.json");
        let mut callers: Map<String, Value> = super::read(&state_path)
            .and_then(|s| s["callers"].as_object().cloned()).unwrap_or_default();
        callers.retain(|_, at| at.as_f64().is_some_and(|at| now * 1000.0 - at < CALLER_WINDOW_MS));
        for (proc, encoder) in &found {
            if let Some(known) = self.demoted.get_mut(&proc.pid) {
                // Still encoding one check later: a sustained load, so its caller is reviewed once a day.
                let caller = known.record["caller"].as_str().unwrap_or_default().to_string();
                if !known.reviewed && !callers.contains_key(&caller) {
                    callers.insert(caller, json!(now * 1000.0));
                    if let Err(error) = crate::recovery_retro::record(home, "software_encode", &known.record) {
                        crate::log(&format!("software encode retro: {error:#}"));
                    }
                }
                known.reviewed = true;
                continue;
            }
            let demoted = demote(proc.pid);
            let caller = processes.iter().find(|p| p.pid == proc.ppid).map(|p| caller_of(&p.args)).unwrap_or_default();
            let record = json!({"at_ms": now * 1000.0, "pid": proc.pid, "start": proc.start, "encoder": encoder,
                "caller": caller, "outcome": if demoted { "demoted" } else { "failed: setpriority" },
                "args": proc.args.chars().take(240).collect::<String>()});
            crate::log(&format!("software encode: {record}"));
            append(home, now, &record)?;
            self.demoted.insert(proc.pid, Demoted { start: proc.start, record, reviewed: false });
        }
        write_json(&state_path, &json!({"at_ms": now * 1000.0, "active": found.len(), "callers": callers,
            "threshold": {"check_s": CHECK_SECS, "caller_window_s": CALLER_WINDOW_MS / 1000.0}}))
    }
}

/// The software video encoder an ffmpeg command line uses, or None when it encodes on the GPU,
/// copies, or writes no video file.
pub(super) fn software_encoder(args: &str) -> Option<String> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    let mut chosen = None;
    for pair in tokens.windows(2) {
        if VIDEO_CODEC_FLAGS.contains(&pair[0]) { chosen = Some(pair[1]); }
    }
    match chosen {
        Some(codec) => SOFTWARE.contains(&codec).then(|| codec.to_string()),
        None if tokens.contains(&"-vn") => None,
        None => tokens.last().filter(|out| VIDEO_OUTPUTS.iter().any(|ext| out.to_ascii_lowercase().ends_with(ext)))
            .map(|out| if out.to_ascii_lowercase().ends_with(".webm") { "libvpx-vp9 (default)" } else { "libx264 (default)" }.to_string()),
    }
}

/// The parent command's first two words with directories dropped, e.g. `bun buildMedia.js`.
pub(super) fn caller_of(args: &str) -> String {
    args.split_whitespace().take(2).map(|w| w.rsplit('/').next().unwrap_or(w)).collect::<Vec<_>>().join(" ")
}

fn demote(pid: i32) -> bool {
    let id = pid as libc::id_t;
    let nice = unsafe { libc::setpriority(libc::PRIO_PROCESS, id, 20) } == 0;
    let background = unsafe { libc::setpriority(libc::PRIO_DARWIN_PROCESS, id, libc::PRIO_DARWIN_BG) } == 0;
    nice && background
}

fn append(home: &Path, now: f64, record: &Value) -> Result<()> {
    let directory = root(home).join("software-encodes");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let day = ((now * 1000.0) / DAY_MS).floor() * DAY_MS;
    std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
        .open(directory.join(format!("{}.ndjson", day as u64)))?
        .write_all(format!("{record}\n").as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_explicit_and_default_software_encoders_only() {
        let x265 = "ffmpeg -y -i a.mp4 -c:v libx265 -preset fast -crf 20 -tag:v hvc1 out.mp4";
        assert_eq!(software_encoder(x265).as_deref(), Some("libx265"));
        assert_eq!(software_encoder("ffmpeg -i a.mov out.mp4").as_deref(), Some("libx264 (default)"));
        assert_eq!(software_encoder("ffmpeg -i a.mp4 -vcodec libvpx-vp9 b.webm").as_deref(), Some("libvpx-vp9"));
        assert_eq!(software_encoder("ffmpeg -hwaccel videotoolbox -i a.mp4 -c:v hevc_videotoolbox -allow_sw 0 b.mp4"), None);
        assert_eq!(software_encoder("ffmpeg -i a.mp4 -c copy b.mp4"), None);
        assert_eq!(software_encoder("ffmpeg -i a.mp4 -vn -c:a aac b.m4a"), None);
        assert_eq!(software_encoder("ffmpeg -ss 1 -i a.mp4 -frames:v 1 poster.jpg"), None);
        assert_eq!(software_encoder("ffmpeg -i a.mp4 -f null -"), None);
    }

    #[test]
    fn the_last_video_codec_flag_wins() {
        assert_eq!(software_encoder("ffmpeg -i a.mp4 -c copy -c:v libx264 b.mp4").as_deref(), Some("libx264"));
        assert_eq!(software_encoder("ffmpeg -i a.mp4 -c:v libx264 -c:v h264_videotoolbox b.mp4"), None);
    }

    #[test]
    fn start_times_one_second_apart_are_the_same_process() {
        assert!(same_start(1_793_600_471, 1_793_600_472));
        assert!(!same_start(1_793_600_471, 1_793_600_480));
    }

    #[test]
    fn caller_drops_directories() {
        assert_eq!(caller_of("bun /Volumes/x/.claude/skills/report/scripts/buildMedia.js --flag"), "bun buildMedia.js");
    }
}
