//! Out-of-band power for the remote beta host (founder, 2026-11-02: a hung Windows PC must heal
//! without a person). Since 2026-11-06 the PC has no power-on card (it sat in PCIEX16_2); the only
//! out-of-band path is Wake-on-LAN on its Realtek NIC (BIOS ErP off, Power On By PCI-E on; Windows
//! S5WakeOnLan and WakeOnMagicPacket on, fast startup off). In-band restarts stay with `reboot`
//! (`shutdown /r` over SSH).
//!
//! While `remote::recover` waits for an unreachable host, each poll calls `step`:
//! 1. Before the grace (HANG_GRACE, or UPDATE_GRACE in the Sunday Windows Update window) it waits:
//!    an ordinary or update reboot comes back by itself.
//! 2. Windows RPC (TCP 135) still answering means the OS lives and only SSH is gone: held, because
//!    nothing here can restart a live OS without SSH.
//! 3. Otherwise the host is off or hung: send a Wake-on-LAN magic packet. It powers on an S5 host;
//!    a hung host ignores it and the next round, AFTER_ACTION_WAIT later, is held for a person.
//! 4. At most MAX_ACTIONS_24H wakes per 24 h (ledger `wol-actions.json`); a host whose MAC was never
//!    learned is held. A held or failed round enters recovery-retro.
use super::heal::run;
use crate::monitor::{root, write_json};
use serde_json::{json, Value};
use std::{
    net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket},
    path::Path,
    time::Duration,
};

/// Founder, 2026-11-05: 300 s (was 600). Three bugchecks on 2026-11-04 left the host hung, each
/// ~10 of its ~11 min beta outage spent in this wait; all three dumps were already written.
pub(super) const HANG_GRACE: Duration = Duration::from_secs(300);
/// Windows Update installs Sunday 04:00 UTC+8 and may sit at "configuring updates" for long.
pub(super) const UPDATE_GRACE: Duration = Duration::from_secs(2700);
/// Wait after a wake: POST, Windows boot, Docker Desktop and Kubernetes.
pub(super) const AFTER_ACTION_WAIT: Duration = Duration::from_secs(1200);
const MAX_ACTIONS_24H: usize = 3;
const LEDGER: &str = "wol-actions.json";
/// MACs learned from the ARP table while the host was up, keyed by IP.
const MAC_CACHE: &str = "wol-mac.json";
const RPC_PORT: u16 = 135;
const WOL_PORT: u16 = 9;

#[derive(Debug, PartialEq)]
pub(super) enum Decision { Wait, Wake, Held(&'static str) }

/// Sunday 03:55-05:30 in UTC+8, the PC's update-and-reboot window.
pub(super) fn in_update_window(unix_s: f64) -> bool {
    let local = unix_s as i64 + 8 * 3600;
    let weekday = (local.div_euclid(86_400) + 4).rem_euclid(7); // 1970-01-01 was a Thursday; 0 = Sunday
    let minute = local.rem_euclid(86_400) / 60;
    weekday == 0 && (235..330).contains(&minute)
}

pub(super) fn grace(unix_s: f64) -> Duration {
    if in_update_window(unix_s) { UPDATE_GRACE } else { HANG_GRACE }
}

/// `woke_before`: a wake already went out for this outage and the host still did not answer.
pub(super) fn decide(mac_known: bool, os_alive: bool, down_for: Duration, grace: Duration, recent: usize, woke_before: bool) -> Decision {
    if down_for < grace { return Decision::Wait; }
    if os_alive { return Decision::Held("os_alive_ssh_down"); }
    if woke_before { return Decision::Held("hung_no_power_control"); }
    if recent >= MAX_ACTIONS_24H { return Decision::Held("wol_cap_24h"); }
    if !mac_known { return Decision::Held("mac_unknown"); }
    Decision::Wake
}

/// The host's LAN address from `ssh -G <alias>`.
fn host_ip(host: &str) -> Option<String> {
    let out = run(Path::new("/usr/bin/ssh"), &["-G", host], Duration::from_secs(5));
    out.text.lines().find_map(|l| l.strip_prefix("hostname ")).map(|h| h.trim().to_string())
}

fn os_alive(ip: &str) -> bool {
    let Some(addr) = (ip, RPC_PORT).to_socket_addrs().ok().and_then(|mut a| a.next()) else { return false };
    (0..2).any(|_| TcpStream::connect_timeout(&SocketAddr::from(addr), Duration::from_secs(3)).is_ok())
}

/// Six MAC bytes from `arp -n` text (`? (192.0.2.8) at 02:00:00:00:00:01 on en0 …`; macOS drops
/// leading zeros) or from a cached `a0:ad:9f:06:1d:89`.
pub(super) fn parse_mac(text: &str) -> Option<[u8; 6]> {
    text.split_whitespace().find_map(|word| {
        let parts: Vec<&str> = word.split([':', '-']).collect();
        if parts.len() != 6 { return None; }
        let mut mac = [0u8; 6];
        for (slot, part) in mac.iter_mut().zip(&parts) { *slot = u8::from_str_radix(part, 16).ok()?; }
        Some(mac)
    })
}

/// The host's MAC: the ARP table first (cached for later), else the cache.
fn mac_for(home: &Path, ip: &str) -> Option<[u8; 6]> {
    let path = root(home).join(MAC_CACHE);
    let mut cache = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or_else(|| json!({}));
    let arp = run(Path::new("/usr/sbin/arp"), &["-n", ip], Duration::from_secs(5));
    if let Some(mac) = parse_mac(&arp.text) {
        cache[ip] = json!(mac.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":"));
        let _ = write_json(&path, &cache);
        return Some(mac);
    }
    cache[ip].as_str().and_then(parse_mac)
}

/// Magic packet: six 0xFF bytes then the MAC sixteen times.
pub(super) fn magic_packet(mac: &[u8; 6]) -> Vec<u8> {
    let mut packet = vec![0xFF; 6];
    for _ in 0..16 { packet.extend_from_slice(mac); }
    packet
}

/// Send the packet to the /24 broadcast of `ip` (the studio LAN) and to the limited broadcast.
fn wake(ip: &str, mac: &[u8; 6]) -> Value {
    let subnet = ip.rsplit_once('.').map(|(net, _)| format!("{net}.255")).unwrap_or_else(|| "255.255.255.255".into());
    let packet = magic_packet(mac);
    let sent: Vec<Value> = [subnet.as_str(), "255.255.255.255"].iter().map(|target| {
        let result = UdpSocket::bind("0.0.0.0:0").and_then(|s| { s.set_broadcast(true)?; s.send_to(&packet, (*target, WOL_PORT)) });
        json!({"to": target, "ok": result.is_ok(), "error": result.err().map(|e| e.to_string())})
    }).collect();
    json!({"ok": sent.iter().any(|s| s["ok"] == true), "sent": sent})
}

fn ledger(home: &Path, now_s: f64) -> (Vec<f64>, Option<f64>) {
    let ledger = std::fs::read_to_string(root(home).join(LEDGER)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let recent = ledger.as_ref().and_then(|l| l["actions_ms"].as_array().cloned()).unwrap_or_default().iter()
        .filter_map(Value::as_f64).filter(|ms| now_s * 1000.0 - ms < 86_400_000.0).collect();
    (recent, ledger.and_then(|l| l["last"]["at_ms"].as_f64()))
}

/// One poll while the host is unreachable. `None` = keep waiting; `Some` = this round's decision
/// (a wake sent, or held), recorded as a recovery step.
pub(super) fn step(home: &Path, host: &str, down_for: Duration) -> Option<Value> {
    step_with(home, host, down_for, grace(crate::unix_now()), true)
}

/// `step` with an explicit grace. A reboot the watchdog itself issued (`reboot`) that has not
/// finished within its grace passes `check_os = false`: a shutdown that powered the host off but
/// never started it again comes back with a wake; one hung mid-shutdown is held for a person.
pub(super) fn step_with(home: &Path, host: &str, down_for: Duration, grace: Duration, check_os: bool) -> Option<Value> {
    let now = crate::unix_now();
    if down_for < grace { return None; }
    let (mut recent, last_ms) = ledger(home, now);
    let woke_before = last_ms.is_some_and(|ms| now * 1000.0 - ms < down_for.as_millis() as f64);
    let ip = host_ip(host).unwrap_or_default();
    let alive = check_os && !ip.is_empty() && os_alive(&ip);
    let mac = if ip.is_empty() { None } else { mac_for(home, &ip) };
    let decision = decide(mac.is_some(), alive, down_for, grace, recent.len(), woke_before);
    let base = json!({"action": "wake_on_lan", "host_ip": ip, "os_alive": alive, "mac_known": mac.is_some(),
        "down_s": down_for.as_secs(), "grace_s": grace.as_secs(), "recent_24h": recent.len(), "woke_before": woke_before});
    let mac = match (decision, mac) {
        (Decision::Wait, _) => return None,
        (Decision::Held(why), _) => { let mut r = base; r["decision"] = json!(format!("held: {why}")); return Some(r); }
        (Decision::Wake, Some(mac)) => mac,
        (Decision::Wake, None) => return None,
    };
    let result = wake(&ip, &mac);
    if result["ok"] == true {
        recent.push(now * 1000.0);
        let _ = write_json(&root(home).join(LEDGER), &json!({"actions_ms": recent, "last": {"op": "wake", "at_ms": now * 1000.0}}));
    }
    let mut r = base;
    r["decision"] = json!("wake");
    r["acted"] = json!(result["ok"] == true);
    r["result"] = result;
    crate::log(&format!("docker watch: wake-on-lan {host}: {r}"));
    Some(r)
}

#[cfg(test)]
#[path = "power_tests.rs"]
mod tests;
