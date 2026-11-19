//! Deterministic attribution of one ControlCenter instance's port growth. Inputs are the 5 s
//! port series (`controlcenter-ports/`), the microphone ledger (`mic-sessions/`) and the unified
//! log lines where ControlCenter adds the AudioVideoModule menu-bar element. A window's growth is
//! the last count at its end minus the last count before its start; overlapping windows merge
//! first, so one jump is never counted twice. Incident 87046536238356200262 added the other menu-bar
//! rebuilds: any other module element (Display on a display hot-plug or iPad device, NowPlaying)
//! and a high-performance Screen Sharing session (audio in avconferenced, a virtual display that
//! rebuilds the menu bar on connect and disconnect; spans from screensharingd, `sharing.rs`, plus
//! avconferenced rows outside them); `cc_menubar_rebuild` is their union. Incident
//! 815083625503085831257 added ScreenCaptureKit stream starts (`capture.rs`): each can create the
//! AudioVideoModule, and replayd's lines outlive ControlCenter's in the unified log. Incident
//! 811358242637012976142 added Universal Control attaches (`universal.rs`) to the union and
//! dropped `mic-warm` rows from the microphone windows: mic-warm alone creates no element (N2.1),
//! and its re-warm row written after a restart spanned 9 h of the instance and hid +1,148 of
//! unexplained growth. Incident 810984561562712258015 added `cc_known_windows`, the union of every
//! window above including the microphone ledger: on an instance older than the ~9 h life of the
//! module lines (N2.8) the ledger is the only surviving sensor of the evening's AudioVideoModule
//! creations, so no single node reached its share although all windows held 96%.
use crate::monitor::root;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{path::Path, process::{Command, Stdio}};

const DAY_MS: f64 = 86_400_000.0;
/// 2026-10-24 (incident 82070399428663228139): the ports of one creation settle within 60 s.
const AV_BEFORE_MS: f64 = 5_000.0;
const AV_AFTER_MS: f64 = 60_000.0;
/// mic_sessions.rs SETTLE_SECS: ControlCenter updates its indicator after input stops.
const MIC_AFTER_MS: f64 = 10_000.0;
/// Module elements added this soon after launch are the initial menu-bar build, not a rebuild.
const LAUNCH_BUILD_MS: f64 = 30_000.0;
/// 2026-10-25 02:17 SGT: the virtual display appeared 6 s before avconferenced audio started; the
/// disconnect rebuild landed 5-12 s after audio stopped and settled within a minute.
const SHARING_BEFORE_MS: f64 = 20_000.0;
const SHARING_AFTER_MS: f64 = 60_000.0;

/// One ControlCenter count: (time ms, ports).
pub(crate) type Row = (f64, u64);

/// Growth inside the union of `windows`, measured on `rows` (sorted by time).
pub(crate) fn window_growth(rows: &[Row], windows: &[(f64, f64)]) -> i64 {
    let mut merged: Vec<(f64, f64)> = windows.to_vec();
    merged.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut union: Vec<(f64, f64)> = Vec::new();
    for (start, end) in merged {
        match union.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => union.push((start, end)),
        }
    }
    let at = |time: f64| rows.iter().rev().find(|r| r.0 <= time).or(rows.first()).map(|r| r.1 as i64);
    union.iter().filter_map(|(start, end)| Some(at(*end)? - at(*start)?)).sum()
}

/// Per-node attribution for ControlCenter `pid`, started at `start_s`, up to `end_ms`.
pub(crate) fn controlcenter(home: &Path, pid: i64, start_s: f64, end_ms: f64) -> Result<Value> {
    let rows: Vec<Row> = day_rows(home, "controlcenter-ports", start_s * 1000.0, end_ms)
        .filter(|r| r["pid"].as_i64() == Some(pid))
        .filter_map(|r| Some((r["at_ms"].as_f64()?, r["ports"].as_u64()?)))
        .collect();
    let rows = settled(rows, start_s * 1000.0);
    anyhow::ensure!(rows.len() >= 2, "fewer than two port counts for ControlCenter {pid}");
    let adds = if cfg!(test) { Vec::new() } else { module_creations(pid, start_s)? };
    let adds: Vec<&(f64, bool)> = adds.iter().filter(|a| a.0 > start_s * 1000.0 + LAUNCH_BUILD_MS).collect();
    let av: Vec<f64> = adds.iter().filter(|a| a.1).map(|a| a.0).collect();
    let modules: Vec<f64> = adds.iter().filter(|a| !a.1).map(|a| a.0).collect();
    let sessions: Vec<Value> = day_rows(home, "mic-sessions", start_s * 1000.0, end_ms)
        .filter(|s| s["cc_pid_before"].as_i64() == Some(pid)).collect();
    let span = |s: &Value| Some((s["start_ms"].as_f64()?, s["end_ms"].as_f64()?));
    let mic: Vec<(f64, f64)> = sessions.iter().filter(|s| s["app"] != "mic-warm").filter_map(span).map(|(a, b)| (a - 1_000.0, b + MIC_AFTER_MS)).collect();
    let viewers = if cfg!(test) { Vec::new() } else { super::sharing::sessions(start_s, end_ms)? };
    let audio = sessions.iter().filter(|s| s["app"] == "avconferenced").filter_map(span);
    // A viewer session and its avconferenced audio are one event.
    let mut sharing = viewers.clone();
    sharing.extend(audio.filter(|a| !viewers.iter().any(|v| a.0 <= v.1 && v.0 <= a.1)));
    // The CLI recomputes up to now; sessions after this instance's last count are not its events.
    let last_ms = rows.last().map_or(end_ms, |r| r.0);
    sharing.retain(|s| s.0 <= last_ms);
    let capture: Vec<f64> = if cfg!(test) { Vec::new() } else { super::capture::starts(start_s)? };
    let capture: Vec<f64> = capture.into_iter().filter(|t| *t > start_s * 1000.0 + LAUNCH_BUILD_MS && *t <= last_ms).collect();
    let universal: Vec<f64> = if cfg!(test) { Vec::new() } else { super::universal::attaches(start_s)? };
    let universal: Vec<f64> = universal.into_iter().filter(|t| *t > start_s * 1000.0 + LAUNCH_BUILD_MS && *t <= last_ms).collect();
    Ok(summarize(&rows, &av, &mic, &modules, &sharing, &[capture, universal]))
}

/// Drops counts taken during the initial menu-bar build (incident 86540618438375945180: 519 ports
/// 0 s after launch, 1,026 at 6 s), unless fewer than two counts would remain.
fn settled(rows: Vec<Row>, start_ms: f64) -> Vec<Row> {
    let after: Vec<Row> = rows.iter().copied().filter(|r| r.0 >= start_ms + LAUNCH_BUILD_MS).collect();
    if after.len() >= 2 { after } else { rows }
}

/// `modules`: other module element creations (ms); `sharing`: avconferenced session spans (ms);
/// `starts`: screen-capture stream starts and Universal Control attaches (ms).
pub(crate) fn summarize(rows: &[Row], av: &[f64], mic: &[(f64, f64)], modules: &[f64], sharing: &[(f64, f64)], starts: &[Vec<f64>; 2]) -> Value {
    let [capture, universal] = starts;
    let growth = rows.last().map_or(0, |r| r.1 as i64) - rows.first().map_or(0, |r| r.1 as i64);
    let av_windows: Vec<(f64, f64)> = av.iter().map(|t| (t - AV_BEFORE_MS, t + AV_AFTER_MS)).collect();
    let av_growth = window_growth(rows, &av_windows);
    let mic_growth = window_growth(rows, mic);
    let module_windows: Vec<(f64, f64)> = modules.iter().map(|t| (t - AV_BEFORE_MS, t + AV_AFTER_MS)).collect();
    let sharing_windows: Vec<(f64, f64)> = sharing.iter().map(|(a, b)| (a - SHARING_BEFORE_MS, b + SHARING_AFTER_MS)).collect();
    let capture_windows: Vec<(f64, f64)> = capture.iter().map(|t| (t - AV_BEFORE_MS, t + AV_AFTER_MS)).collect();
    let universal_windows: Vec<(f64, f64)> = universal.iter().map(|t| (t - AV_BEFORE_MS, t + AV_AFTER_MS)).collect();
    let rebuild: Vec<(f64, f64)> = av_windows.iter().chain(&module_windows).chain(&sharing_windows).chain(&capture_windows)
        .chain(&universal_windows).copied().collect();
    let rebuild_growth = window_growth(rows, &rebuild);
    let either: Vec<(f64, f64)> = rebuild.iter().chain(mic).copied().collect();
    let known = window_growth(rows, &either);
    let share = |part: i64| if growth > 0 { part as f64 / growth as f64 } else { 0.0 };
    let probe = |events: usize, windows: &[(f64, f64)]| json!({"events": events, "growth": window_growth(rows, windows), "share": share(window_growth(rows, windows))});
    json!({
        "first": {"at_ms": rows.first().map(|r| r.0), "ports": rows.first().map(|r| r.1)},
        "last": {"at_ms": rows.last().map(|r| r.0), "ports": rows.last().map(|r| r.1)},
        "growth": growth,
        "hours": rows.last().zip(rows.first()).map(|(l, f)| (l.0 - f.0) / 3_600_000.0),
        "probes": {
            "cc_av_module": {"events": av.len(), "growth": av_growth, "share": share(av_growth)},
            "cc_mic_sessions": {"sessions": mic.len(), "growth": mic_growth, "share": share(mic_growth)},
            "cc_other_modules": probe(modules.len(), &module_windows),
            "cc_screen_sharing": probe(sharing.len(), &sharing_windows),
            "cc_screen_capture": probe(capture.len(), &capture_windows),
            "cc_universal_control": probe(universal.len(), &universal_windows),
            "cc_menubar_rebuild": {"events": av.len() + modules.len() + sharing.len() + capture.len() + universal.len(), "growth": rebuild_growth, "share": share(rebuild_growth)},
            "cc_known_windows": {"growth": known, "share": share(known)},
            "cc_residual": {"growth": growth - known, "share": share(growth - known)},
        },
    })
}

/// Rows of the `<utc-day-ms>.ndjson` files covering `from_ms..=to_ms`.
fn day_rows(home: &Path, directory: &str, from_ms: f64, to_ms: f64) -> impl Iterator<Item = Value> {
    let first = (from_ms / DAY_MS).floor() as u64;
    let last = (to_ms / DAY_MS).floor() as u64;
    let directory = root(home).join(directory);
    (first..=last).flat_map(move |day| {
        std::fs::read_to_string(directory.join(format!("{}.ndjson", day * DAY_MS as u64))).unwrap_or_default()
            .lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect::<Vec<_>>()
    })
}

/// Times (ms) at which ControlCenter `pid` added menu-bar module elements, each flagged when it is
/// the AudioVideoModule.
fn module_creations(pid: i64, start_s: f64) -> Result<Vec<(f64, bool)>> {
    let minutes = ((crate::unix_now() - start_s) / 60.0).ceil().max(1.0) as u64 + 1;
    let predicate = format!("processID == {pid} AND eventMessage BEGINSWITH \"Adding elements for modules\"");
    let out = Command::new("/usr/bin/log")
        .args(["show", "--style", "ndjson", "--last", &format!("{minutes}m"), "--predicate", &predicate])
        .stdin(Stdio::null()).stderr(Stdio::null()).output().context("run log show")?;
    anyhow::ensure!(out.status.success(), "log show exited {}", out.status);
    Ok(String::from_utf8_lossy(&out.stdout).lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|e| Some((parse_time(e["timestamp"].as_str()?)?, e["eventMessage"].as_str()?.contains("AudioVideoModule"))))
        .collect())
}

/// Epoch ms of `2026-10-24 22:09:16.137407+0800` (log show) or `2026-10-24T08:00:01.5Z` (transcripts).
pub(crate) fn parse_time(text: &str) -> Option<f64> {
    let (date, rest) = text.split_once([' ', 'T'])?;
    let mut ymd = date.split('-').map(|n| n.parse::<i64>().ok());
    let (y, m, d) = (ymd.next()??, ymd.next()??, ymd.next()??);
    let zone_at = rest.find(['+', '-', 'Z'])?;
    let (clock, zone) = rest.split_at(zone_at);
    let mut hms = clock.split(':');
    let (h, mi) = (hms.next()?.parse::<f64>().ok()?, hms.next()?.parse::<f64>().ok()?);
    let s = hms.next()?.parse::<f64>().ok()?;
    let offset_min = match zone {
        "Z" | "" => 0.0,
        z => {
            let digits = z[1..].replace(':', "");
            let sign = if z.starts_with('-') { -1.0 } else { 1.0 };
            sign * (digits.get(..2)?.parse::<f64>().ok()? * 60.0 + digits.get(2..4).unwrap_or("0").parse::<f64>().ok()?)
        }
    };
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some(((days as f64 * 86_400.0) + h * 3600.0 + mi * 60.0 + s - offset_min * 60.0) * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_windows_count_one_jump_once() {
        let rows = [(0.0, 1000), (10.0, 1065), (20.0, 1065), (30.0, 1130), (40.0, 1140)];
        assert_eq!(window_growth(&rows, &[(5.0, 12.0), (8.0, 15.0)]), 65);
        assert_eq!(window_growth(&rows, &[(5.0, 12.0), (25.0, 31.0)]), 130);
        let summary = summarize(&rows, &[], &[(5.0, 12.0)], &[], &[], &[vec![], vec![]]);
        assert_eq!(summary["growth"], 140);
        assert_eq!(summary["probes"]["cc_residual"]["growth"], 75);
        assert_eq!(summary["probes"]["cc_known_windows"]["growth"], 65);
    }

    #[test]
    fn module_windows_explain_their_share() {
        let rows = [(0.0, 1000), (10_000.0, 1065), (100_000.0, 1065), (110_000.0, 1100)];
        let summary = summarize(&rows, &[8_000.0], &[], &[], &[], &[vec![], vec![]]);
        assert_eq!(summary["probes"]["cc_av_module"]["growth"], 65);
        assert!((summary["probes"]["cc_av_module"]["share"].as_f64().unwrap() - 0.65).abs() < 1e-9);
    }

    #[test]
    fn rebuild_unions_display_modules_and_screen_sharing() {
        // AudioVideoModule +65 at 10 s, Display +100 at 200 s, a sharing session 400-440 s adding +600.
        let rows = [(0.0, 1000), (12_000.0, 1065), (205_000.0, 1165), (420_000.0, 1465), (450_000.0, 1765), (900_000.0, 1770)];
        let summary = summarize(&rows, &[10_000.0], &[(400_000.0, 450_000.0)], &[200_000.0], &[(400_000.0, 440_000.0)], &[vec![], vec![]]);
        let probes = &summary["probes"];
        assert_eq!(probes["cc_other_modules"]["growth"], 100);
        assert_eq!(probes["cc_screen_sharing"]["growth"], 600);
        assert_eq!(probes["cc_menubar_rebuild"]["growth"], 765);
        assert_eq!(probes["cc_menubar_rebuild"]["events"], 3);
        assert_eq!(probes["cc_residual"]["growth"], 5);
    }

    #[test]
    fn screen_capture_starts_join_the_rebuild() {
        // No ControlCenter module line survived; a capture stream started at 100 s and +100 followed.
        let rows = [(0.0, 1000), (95_000.0, 1000), (110_000.0, 1100), (500_000.0, 1100)];
        let probes = summarize(&rows, &[], &[], &[], &[], &[vec![100_000.0], vec![]])["probes"].clone();
        assert_eq!(probes["cc_screen_capture"]["growth"], 100);
        assert_eq!(probes["cc_menubar_rebuild"]["growth"], 100);
        assert_eq!(probes["cc_residual"]["growth"], 0);
    }

    #[test]
    fn universal_control_attaches_join_the_rebuild() {
        // Incident 811358242637012976142: a pointer attach at 100 s, +97 then settled at +65.
        let rows = [(0.0, 2898), (95_000.0, 2898), (102_000.0, 2995), (110_000.0, 2963), (500_000.0, 2963)];
        let probes = summarize(&rows, &[], &[], &[], &[], &[vec![], vec![100_000.0]])["probes"].clone();
        assert_eq!(probes["cc_universal_control"]["growth"], 65);
        assert_eq!(probes["cc_menubar_rebuild"]["growth"], 65);
        assert_eq!(probes["cc_menubar_rebuild"]["events"], 1);
        assert_eq!(probes["cc_residual"]["growth"], 0);
    }

    #[test]
    fn launch_build_counts_are_not_the_baseline() {
        let rows = vec![(1_000.0, 519), (7_000.0, 1026), (40_000.0, 1019), (90_000.0, 1100)];
        assert_eq!(settled(rows.clone(), 0.0), vec![(40_000.0, 1019), (90_000.0, 1100)]);
        assert_eq!(settled(rows[..3].to_vec(), 0.0).len(), 3);
    }

    #[test]
    fn parses_log_and_transcript_times() {
        assert_eq!(parse_time("1970-01-01T00:00:01Z"), Some(1000.0));
        assert_eq!(parse_time("2026-10-24 22:09:16.5+0800"), parse_time("2026-10-24T14:09:16.5Z"));
        assert_eq!(parse_time("2026-10-24T14:09:16.000Z").map(|ms| ms as i64), Some(1_792_850_956_000));
    }
}
