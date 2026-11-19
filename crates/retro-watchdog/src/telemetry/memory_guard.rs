//! Memory guard (founder decision 2026-11-04, report watchdog-mem-lag-1793761332298, option B).
//! The 2026-11-04 host lag was memory exhaustion (swap 33.1 of 33.8 GiB, 245 MiB free, kernel_task
//! at 3.6 cores) that neither CPU gate saw: paging stalls kept whole-host CPU jumping between 68%
//! and 99%, and `kern.memorystatus_vm_pressure_level` stayed 1 because macOS counts the compressor
//! as available. This guard reads swap and free pages directly. When swap use is at least
//! SWAP_USED_BYTES and free memory is below FREE_BYTES for HOLD_SECS, it runs
//! `sim-lease trim --memory-relief` (every unleased, unheld, unused simulator shuts down now,
//! residents included), then the mobile-devices `lease.js trim --memory-relief` (every unleased
//! Android and HarmonyOS emulator stops now; 2026-11-18 four 7-8 GiB emulators ran beside four
//! simulators and relief covered only the simulators), and opens the `memory_exhausted` condition of the performance trigger
//! (`wake.rs`). One incident per episode; while the episode lasts, relief repeats every
//! RELIEF_SECS. State: `memory-guard.json`; one record per action in `memory-guard/<day>.ndjson`.
//! Each record names the largest footprints (`memory_holders.rs`): relief frees simulators only,
//! so a session must see at once when the memory sits elsewhere (2026-11-15: fseventsd, 38 GiB).
//! Each record also carries what was paged during the hold (`Paging`): the gate reads a level, and
//! on 2026-11-17 (perf retro 1794856006950) nothing told a host swapping out 5.6 GiB in three
//! minutes from one whose free pages dipped under a pinned swap. `memory_series.rs` keeps one
//! reading per minute with the two sysctls the admission gates read.
//! Paging thrash (`memory_thrash.rs`) opens an episode too: on 2026-11-18 23:10 SGT the desktop froze
//! at swap 14 GiB, under the swap rule, while decompression ran at 72-79 GiB per minute.
use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{io::Write, path::Path};

pub const SAMPLE_SECS: f64 = 5.0;
/// render-queue README: normal load keeps 10-12 GiB of swap and used/total sits near 90% even when
/// healthy (macOS adds 1 GiB swapfiles on demand), so a ratio cannot tell exhaustion from load;
/// 24 GiB is the same evidenced boundary as render-queue `swap_gib`. 2026-11-04 had 33.1 GiB.
pub const SWAP_USED_BYTES: f64 = 24.0 * 1024.0 * 1024.0 * 1024.0;
pub const FREE_BYTES: f64 = 1024.0 * 1024.0 * 1024.0;
pub const HOLD_SECS: f64 = 60.0;
const RELIEF_SECS: f64 = 600.0;
const SIM_LEASE: &str = "/home/user/.local/bin/sim-lease";
/// launchd's PATH has no node; this is the host's node shim.
const NODE: &str = "/home/user/.local/bin/node";
const EMULATOR_LEASE: &str = "/home/user/.codex/skills/mobile-devices/scripts/lease.js";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading { pub swap_total: f64, pub swap_used: f64, pub free: f64 }

impl Reading {
    pub fn exhausted(&self) -> bool {
        self.swap_used >= SWAP_USED_BYTES && self.free < FREE_BYTES
    }
    fn json(&self) -> Value {
        let gib = |b: f64| (b / 1_073_741_824.0 * 100.0).round() / 100.0;
        json!({"swap_total_gib": gib(self.swap_total), "swap_used_gib": gib(self.swap_used), "free_gib": gib(self.free),
            "swap_used_ratio": if self.swap_total > 0.0 { self.swap_used / self.swap_total } else { 0.0 }})
    }
}

/// `sysctl -n vm.swapusage`: "total = 23552.00M  used = 22291.31M  free = 1260.69M  (encrypted)".
pub fn parse_swap(text: &str) -> Option<(f64, f64)> {
    let field = |name: &str| -> Option<f64> {
        let rest = text.split(&format!("{name} = ")).nth(1)?;
        let token = rest.split_whitespace().next()?;
        let (number, unit) = token.split_at(token.len().checked_sub(1)?);
        let scale = match unit { "K" => 1024.0, "M" => 1_048_576.0, "G" => 1_073_741_824.0, _ => return None };
        Some(number.parse::<f64>().ok()? * scale)
    };
    Some((field("total")?, field("used")?))
}

/// `vm_stat`: page size from the header line, then "Pages free:  12345." times it.
pub fn parse_free(text: &str) -> Option<f64> {
    let page = text.lines().next()?.split("page size of ").nth(1)?.split_whitespace().next()?.parse::<f64>().ok()?;
    let free = text.lines().find(|l| l.starts_with("Pages free:"))?.split(':').nth(1)?.trim().trim_end_matches('.').parse::<f64>().ok()?;
    Some(free * page)
}

/// Cumulative `vm_stat` paging counters in bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paging { pub swapins: f64, pub swapouts: f64, pub compressions: f64, pub decompressions: f64 }

/// `vm_stat`: "Swapins:  44662547." and its neighbours, in pages of the size the header names.
pub fn parse_paging(text: &str) -> Option<Paging> {
    let page = text.lines().next()?.split("page size of ").nth(1)?.split_whitespace().next()?.parse::<f64>().ok()?;
    let field = |name: &str| -> Option<f64> {
        let line = text.lines().find(|l| l.starts_with(name))?;
        Some(line.split(':').nth(1)?.trim().trim_end_matches('.').parse::<f64>().ok()? * page)
    };
    Some(Paging { swapins: field("Swapins:")?, swapouts: field("Swapouts:")?, compressions: field("Compressions:")?, decompressions: field("Decompressions:")? })
}

/// What moved between the first exhausted sample of the hold and now; null without both readings.
pub fn paged(start: Option<(f64, Paging)>, now: f64, current: Option<Paging>) -> Value {
    let (Some((since, first)), Some(last)) = (start, current) else { return Value::Null };
    let gib = |bytes: f64| (bytes.max(0.0) / 1_073_741_824.0 * 100.0).round() / 100.0;
    json!({"window_s": (now - since).round(), "swapouts_gib": gib(last.swapouts - first.swapouts), "swapins_gib": gib(last.swapins - first.swapins),
        "compressions_gib": gib(last.compressions - first.compressions), "decompressions_gib": gib(last.decompressions - first.decompressions)})
}

#[derive(Default)]
pub struct MemoryGuard { last_sample: f64, since: Option<f64>, incident: Option<String>, last_relief: f64, hold_paging: Option<(f64, Paging)>,
    series: super::memory_series::Series, thrash: super::memory_thrash::Thrash }

/// What one sample decided; `observe` carries it out so tests can drive the state machine alone.
#[derive(Debug, PartialEq)]
pub enum Step { Idle, Holding, Open(String), Relieve(String) }

impl MemoryGuard {
    pub fn step(&mut self, now: f64, reading: &Reading) -> Step {
        self.step_with(now, reading.exhausted())
    }

    /// `exhausted`: the swap rule or sustained paging thrash.
    pub fn step_with(&mut self, now: f64, exhausted: bool) -> Step {
        if !exhausted {
            (self.since, self.incident) = (None, None);
            return Step::Idle;
        }
        let since = *self.since.get_or_insert(now);
        if now - since < HOLD_SECS { return Step::Holding; }
        if let Some(id) = &self.incident {
            if now - self.last_relief < RELIEF_SECS { return Step::Holding; }
            self.last_relief = now;
            return Step::Relieve(id.clone());
        }
        let id = format!("{:.0}", now * 1000.0);
        (self.incident, self.last_relief) = (Some(id.clone()), now);
        Step::Open(id)
    }

    pub fn observe(&mut self, home: &Path, now: f64) -> Result<()> {
        if now - self.last_sample < SAMPLE_SECS { return Ok(()); }
        self.last_sample = now;
        let (reading, paging) = read(home)?;
        // The per-minute series is evidence only; its failure never stops the guard.
        if let Err(error) = self.series.record(home, now, &reading, paging) {
            crate::log(&format!("memory series: {error:#}"));
        }
        let thrash = self.thrash.observe(now, paging.map(|p| p.decompressions), reading.free);
        let exhausted = reading.exhausted() || thrash.exhausted;
        let step = self.step_with(now, exhausted);
        // Counters of the first exhausted sample; the hold ends with the episode.
        self.hold_paging = match (self.since, self.hold_paging) {
            (None, _) => None,
            (Some(_), None) => paging.map(|counters| (now, counters)),
            (Some(_), held) => held,
        };
        let paged = paged(self.hold_paging, now, paging);
        write_json(&root(home).join("memory-guard.json"), &json!({"at_ms": now * 1000.0, "reading": reading.json(),
            "exhausted": exhausted, "since_ms": self.since.map(|s| s * 1000.0), "incident": self.incident, "thrash": thrash.json(now),
            "thresholds": {"swap_used_gib": SWAP_USED_BYTES / 1_073_741_824.0, "free_gib": FREE_BYTES / 1_073_741_824.0, "hold_s": HOLD_SECS, "relief_every_s": RELIEF_SECS,
                "thrash": super::memory_thrash::thresholds()}}))?;
        let paged = match (&paged, thrash.rate) {
            (Value::Object(map), Some(rate)) => { let mut map = map.clone(); map.insert("decompress_gib_per_min".into(), json!((rate * 10.0).round() / 10.0)); Value::Object(map) }
            _ => paged,
        };
        match step {
            Step::Open(id) => open(home, now, &id, &reading, self.since.unwrap_or(now), paged, (!reading.exhausted()).then_some(thrash.rate).flatten()),
            Step::Relieve(id) => relieve(home, now, &id, &reading, paged).map(|_| ()),
            Step::Idle | Step::Holding => Ok(()),
        }
    }
}

fn read(home: &Path) -> Result<(Reading, Option<Paging>)> {
    let swap = crate::telemetry::command::capture(home, "/usr/sbin/sysctl", &["-n", "vm.swapusage"], 3)?;
    let (swap_total, swap_used) = parse_swap(&swap).context("unparsable vm.swapusage")?;
    let stat = crate::telemetry::command::capture(home, "/usr/bin/vm_stat", &[], 3)?;
    Ok((Reading { swap_total, swap_used, free: parse_free(&stat).context("unparsable vm_stat")? }, parse_paging(&stat)))
}

/// `cpu-watchdog xcode-canvas`: runs `sim-lease xcode-canvas` (a read-only list of the Swift files
/// open in Xcode) as this program's child and keeps the answer. Relief's canvas park sends Apple
/// events to Xcode, and macOS files the Automation grant for them under this program's identity;
/// a launchd one-shot of this command raises that prompt once, outside an incident.
pub fn xcode_canvas_check(home: &Path) -> Result<()> {
    let output = std::process::Command::new(SIM_LEASE).arg("xcode-canvas").output().context("spawn sim-lease xcode-canvas")?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let record = json!({"at_ms": crate::unix_now() * 1000.0, "exit": output.status.code(),
        "answer": serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text))});
    write_json(&root(home).join("xcode-canvas-check.json"), &record)?;
    println!("{record}");
    Ok(())
}

/// Runs one relief command and keeps its exit code and the stderr lines naming what it shut down.
fn relief_run(program: &str, args: &[&str]) -> (Option<i32>, Vec<String>) {
    match std::process::Command::new(program).args(args).output() {
        Ok(out) => (out.status.code(), String::from_utf8_lossy(&out.stderr).lines().filter(|l| l.contains("memory relief")).map(str::to_string).collect()),
        Err(error) => (None, vec![format!("spawn {program}: {error}")]),
    }
}

/// Shuts down every simulator and emulator nobody uses and records what the lease tools reported
/// and who holds memory.
fn relieve(home: &Path, now: f64, id: &str, reading: &Reading, paged: Value) -> Result<Value> {
    let (exit, mut lines) = relief_run(SIM_LEASE, &["trim", "--memory-relief"]);
    let (emulator_exit, emulator_lines) = relief_run(NODE, &[EMULATOR_LEASE, "trim", "--memory-relief"]);
    lines.extend(emulator_lines);
    let record = json!({"at_ms": now * 1000.0, "incident": id, "action": "sim-lease trim --memory-relief; lease.js trim --memory-relief",
        "exit": exit, "emulator_exit": emulator_exit,
        "shut_down": lines, "reading": reading.json(), "paging": paged, "holders": super::memory_holders::capture(home)});
    let directory = root(home).join("memory-guard");
    std::fs::create_dir_all(&directory)?;
    let day = (now / 86_400.0).floor() as u64;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(directory.join(format!("{day}.ndjson")))?;
    writeln!(file, "{record}")?;
    crate::log(&format!("memory guard {id}: {}", record));
    Ok(record)
}

/// `thrash_rate`: set when paging thrash, not the swap rule, opened the episode.
fn open(home: &Path, now: f64, id: &str, reading: &Reading, since: f64, paged: Value, thrash_rate: Option<f64>) -> Result<()> {
    let relief = relieve(home, now, id, reading, paged)?;
    let event = json!({"id": id, "trigger": "memory_exhausted", "hot_since_ms": since * 1000.0, "detected_at_ms": now * 1000.0,
        "cause": if thrash_rate.is_some() { "paging_thrash" } else { "swap" },
        "reading": reading.json(), "relief": relief, "telemetry": crate::telemetry::evidence(home)});
    write_json(&root(home).join("incidents").join(format!("{id}.json")), &event)?;
    let lead = match thrash_rate {
        Some(rate) => format!("paging thrash: {rate:.0} GiB/min decompressed with "),
        None => String::new(),
    };
    let detail = format!("{lead}swap {:.1}/{:.1} GiB used and {:.2} GiB free for {:.0} s; unused simulators and emulators shut down",
        reading.swap_used / 1_073_741_824.0, reading.swap_total / 1_073_741_824.0, reading.free / 1_073_741_824.0, now - since);
    let detail = match relief["paging"]["swapouts_gib"].as_f64() {
        Some(out) => format!("{detail}; {out:.1} GiB swapped out in the hold"),
        None => detail,
    };
    let detail = match super::memory_holders::largest(&relief["holders"]) {
        Some(holder) => format!("{detail}; largest holder {holder}"),
        None => detail,
    };
    crate::wake::trigger(home, now * 1000.0, crate::wake::Condition::MemoryExhausted, id, &detail, true, json!({})).map(|_| ())
}

#[cfg(test)]
#[path = "memory_guard_tests.rs"]
mod tests;
