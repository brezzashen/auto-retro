//! Remote beta runtime (2026-10-28): Mars CRM beta's Docker engine and Kubernetes run on the
//! studio Windows PC, reached through launchd `local.autoretro.offload-tunnel` (`offload tunnel --alias
//! <host>`: API on 127.0.0.1:16443 plus 8035-8038). This Mac's Docker Desktop is retired there.
//!
//! On 2026-10-28 07:53 the tunnel lost Local Network access, the API probe failed ten times, and
//! the local recovery ran `docker desktop restart` on this Mac: Desktop rewrote the `desktop-linux`
//! context to its own socket and its old Compose Postgres and forwarders took 8035-8038 from the
//! tunnel. With the tunnel agent installed, recovery therefore never starts this Mac's Docker:
//!
//! 0. Local Docker Desktop running: `docker desktop stop`, then point `desktop-linux` back at the
//!    host. On 2026-10-28 11:33 it was opened from the Dock while the host rebooted, took
//!    8035-8038, and the tunnel bind-looped until a person stopped it; this step runs before any
//!    of the actions below (founder, 2026-10-28: a host reboot must heal without a person).
//! 1. Tunnel not listening: `launchctl kickstart -k` the tunnel agent.
//! 2. Engine down on the host: `ssh <host> schtasks /run /tn MarsDockerDesktop` (Desktop is a GUI
//!    app there; only the logon task starts it in the console session).
//! 3. Engine and tunnel up, API down: held for analysis; restarting the host's Desktop is a
//!    beta-wide outage that a person decides. Exception (2026-11-17, cause-tree N4.12.1): when the
//!    API server answers inside the kind node, only the host's published ports are stale (its VM
//!    restarted under a surviving Desktop backend), beta is already down, and restarting the node
//!    and forwarder containers publishes them again in under a minute.
//! 4. Host unreachable over SSH (`remote_host_down`, cause-tree N4.11): the round waits for the
//!    host to come back by itself (its logon task starts Desktop); past the hang grace the PC's
//!    power-on card powers it on or force-restarts it (`power`, since 2026-11-02), and the round
//!    then records its boot evidence. On 2026-11-02 a 0x7F double fault kept it dark 05:33-05:49
//!    SGT and ENGINE_WAIT expired first. A Kernel-Power 41 since boot keeps N4.11 (crash); a clean
//!    OS shutdown of this outage without one is N4.13 (restart hung after shutdown, 2026-11-03).
//! 5. Host reachable, engine still down after step 2 and ENGINE_WAIT: the host's VM is wedged
//!    (2026-11-03), so `reboot::escalate` reboots it, with Wake-on-LAN behind a shutdown that stays off.
use super::heal::{run, PROBE_TIMEOUT};
use super::Probe;
use serde_json::{json, Value};
use std::{
    net::{SocketAddr, TcpStream},
    path::Path,
    time::{Duration, Instant},
};

pub(super) const TUNNEL_LABEL: &str = "local.autoretro.offload-tunnel";
const TUNNEL_PLIST: &str = "Library/LaunchAgents/local.autoretro.offload-tunnel.plist";
const TUNNEL_LOG: &str = ".local/state/offload/tunnel.log";
const API_PORT: u16 = 16443;
const START_TASK: &str = "MarsDockerDesktop";
const TUNNEL_WAIT: Duration = Duration::from_secs(60);
const STOP_WAIT: Duration = Duration::from_secs(120);
/// Desktop plus Kubernetes on the Windows host after a logon-task start.
const ENGINE_WAIT: Duration = Duration::from_secs(420);
/// A bugcheck reboot of the host plus Desktop and Kubernetes (2026-11-02: about 19 min dark).
const HOST_WAIT: Duration = Duration::from_secs(1800);
const HOST_DOWN_NODE: &str = "N4.11";
/// The host shut its OS down cleanly (Kernel-General 13, no Kernel-Power 41 this boot) and then
/// stayed dark: 2026-11-03 both soft reboots (16:42 and 19:05 SGT) hung after shutdown until the
/// power card reset them, while the record still read N4.11 with the 2026-11-02 bugcheck.
const HOST_RESTART_NODE: &str = "N4.13";
/// How long before the recovery a clean shutdown still belongs to this outage (a planned reboot's
/// `docker-pause` held the 2026-11-03 19:05 one for 15 min first).
const CLEAN_SHUTDOWN_LOOKBACK_MS: f64 = 3_600_000.0;
const ENGINE_WEDGED_NODE: &str = "N4.12";
const PORTS_STALE_NODE: &str = "N4.12.1";
/// Docker Desktop's kind node and the three beta forwarders, whose published ports go stale together.
const PUBLISHED: [&str; 4] = ["mars-crm-port-8035", "mars-crm-port-8036", "mars-crm-port-8037", "desktop-control-plane"];
/// Node restart to a serving API (2026-11-17: 15 s to the API, 38 s to beta health).
const REPUBLISH_WAIT: Duration = Duration::from_secs(180);
/// One cmd.exe line (Windows OpenSSH's default shell): boot time, whether this boot followed an
/// unexpected shutdown (Kernel-Power 41 logged since boot, with its bugcheck), and the last clean
/// OS shutdown, as JSON. `kernel_power_41_utc` is the newest 41 of any boot.
const BOOT_EVIDENCE: &str = "powershell -NoProfile -Command \"$ProgressPreference='SilentlyContinue'; \
$bt=(Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToUniversalTime(); \
$e=Get-WinEvent -FilterHashtable @{LogName='System';Id=41} -MaxEvents 1 -ErrorAction SilentlyContinue; \
$u=[bool]($e -and $e.TimeCreated.ToUniversalTime() -ge $bt); $k=@{}; \
if($u){([xml]$e.ToXml()).Event.EventData.Data | ForEach-Object { $k[$_.Name]=$_.'#text' }}; \
$s=Get-WinEvent -FilterHashtable @{LogName='System';ProviderName='Microsoft-Windows-Kernel-General';Id=13} -MaxEvents 1 -ErrorAction SilentlyContinue; \
[ordered]@{boot_utc=$bt.ToString('o'); boot_ms=([DateTimeOffset]$bt).ToUnixTimeMilliseconds(); unexpected_this_boot=$u; \
kernel_power_41_utc=$(if($e){$e.TimeCreated.ToUniversalTime().ToString('o')}); \
last_clean_shutdown_ms=$(if($s){([DateTimeOffset]$s.TimeCreated).ToUnixTimeMilliseconds()}); \
bugcheck_code=$k['BugcheckCode']; bugcheck_p1=$k['BugcheckParameter1']} | ConvertTo-Json -Compress\"";

/// The SSH alias of the remote beta host when the tunnel agent is installed, else `None`.
pub(super) fn host(home: &Path) -> Option<String> {
    alias_from_plist(&std::fs::read_to_string(home.join(TUNNEL_PLIST)).ok()?)
}

/// The `<string>` after `<string>--alias</string>` in the tunnel's ProgramArguments.
pub(super) fn alias_from_plist(plist: &str) -> Option<String> {
    let strings: Vec<&str> = plist.split("<string>").skip(1).filter_map(|s| s.split_once("</string>").map(|(v, _)| v.trim())).collect();
    let at = strings.iter().position(|s| *s == "--alias")?;
    strings.get(at + 1).filter(|s| !s.is_empty()).map(|s| s.to_string())
}

/// The engine on the remote host, addressed by host rather than by the `desktop-linux` context,
/// which a local Desktop start rewrites.
pub(super) fn probe_engine(home: &Path, host: &str) -> Probe {
    let started = Instant::now();
    let target = format!("ssh://{host}");
    let out = run(&home.join(".docker/bin/docker"), &["-H", &target, "version", "--format", "{{.Server.Version}}"], PROBE_TIMEOUT);
    let ok = out.code == Some(0);
    Probe { ok, ms: started.elapsed().as_millis() as u64, error: (!ok).then(|| out.text.trim().chars().take(300).collect()) }
}

/// Whether the API server answers inside the kind node, whatever its published port does.
fn cluster_ready_inside(home: &Path, host: &str) -> bool {
    let target = format!("ssh://{host}");
    let out = run(&home.join(".docker/bin/docker"), &["-H", &target, "exec", PUBLISHED[3], "kubectl", "--kubeconfig",
        "/etc/kubernetes/admin.conf", "get", "--raw=/readyz"], PROBE_TIMEOUT);
    out.code == Some(0) && out.text.trim() == "ok"
}

fn tunnel_listening() -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], API_PORT)), Duration::from_secs(2)).is_ok()
}

pub(super) fn ssh(host: &str, command: &str, timeout: Duration) -> super::heal::Out {
    run(Path::new("/usr/bin/ssh"), &["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", host, command], timeout)
}

/// A PowerShell script as one `powershell -EncodedCommand` argument, so cmd.exe (Windows OpenSSH's
/// default shell) applies no quoting to it.
pub(super) fn powershell(script: &str) -> String {
    use base64::Engine as _;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    format!("powershell.exe -NoProfile -NonInteractive -EncodedCommand {}", base64::engine::general_purpose::STANDARD.encode(utf16))
}

pub(super) fn host_reachable(host: &str) -> bool {
    ssh(host, "exit", Duration::from_secs(15)).code == Some(0)
}

/// The host's boot time and last Kernel-Power 41 bugcheck, or the raw output when it is not JSON.
pub(super) fn host_boot(host: &str) -> Value {
    let out = ssh(host, BOOT_EVIDENCE, Duration::from_secs(60));
    serde_json::from_str(out.text.trim()).unwrap_or_else(|_| json!({"exit": out.code, "output": out.text.chars().take(300).collect::<String>()}))
}

fn local_desktop_running() -> bool {
    run(Path::new("/usr/bin/pgrep"), &["-x", "com.docker.backend"], PROBE_TIMEOUT).code == Some(0)
}

fn tail(path: &Path, lines: usize) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].iter().map(|l| l.chars().take(300).collect()).collect()
}

/// The cause the remote evidence supports.
pub(super) fn classify(evidence: &Value) -> &'static str {
    if evidence["host_reachable"] == false { return "remote_host_down"; }
    if evidence["tunnel_listening"] == false { return "tunnel_down"; }
    if evidence["engine_ok"] == false { return "remote_engine_down"; }
    "remote_kubernetes_down"
}

/// The actions, in order, that the remote evidence calls for. A host reboot can leave the engine
/// and the tunnel down together, so both are started in one round; empty means held.
pub(super) fn plan(evidence: &Value) -> Vec<&'static str> {
    let mut actions = Vec::new();
    if evidence["local_desktop_running"] == true { actions.extend(["stop_local_desktop", "restore_desktop_linux"]); }
    // An unreachable host cannot run its start task; its own logon task starts Desktop at boot.
    if evidence["engine_ok"] == false && evidence["host_reachable"] != false { actions.push("start_remote_desktop"); }
    if evidence["tunnel_listening"] == false { actions.push("kickstart_tunnel"); }
    if actions.is_empty() && evidence["engine_ok"] == true && evidence["cluster_ready_inside"] == true { actions.push("republish_ports"); }
    actions
}

fn act(home: &Path, host: &str, action: &str) -> Value {
    let docker = home.join(".docker/bin/docker");
    let target = format!("ssh://{host}");
    let out = match action {
        "stop_local_desktop" => run(&docker, &["desktop", "stop"], STOP_WAIT),
        "restore_desktop_linux" => run(&docker, &["context", "update", "desktop-linux", "--docker", &format!("host=ssh://{host}")], PROBE_TIMEOUT),
        "start_remote_desktop" => ssh(host, &format!("schtasks /run /tn {START_TASK}"), Duration::from_secs(30)),
        "republish_ports" => run(&docker, &[&["-H", target.as_str(), "restart"][..], &PUBLISHED[..]].concat(), STOP_WAIT),
        _ => run(Path::new("/bin/launchctl"), &["kickstart", "-k", &format!("gui/{}/{TUNNEL_LABEL}", uid())], PROBE_TIMEOUT),
    };
    json!({"action": action, "exit": out.code, "output": out.text.chars().take(600).collect::<String>()})
}

pub(super) fn recover(home: &Path, host: &str) -> Value {
    let started_at_ms = crate::unix_now() * 1000.0;
    let engine = probe_engine(home, host);
    let reachable = engine.ok || host_reachable(host);
    let evidence = json!({"remote_host": host, "host_reachable": reachable, "tunnel_listening": tunnel_listening(), "engine_ok": engine.ok,
        "engine_error": engine.error, "local_desktop_running": local_desktop_running(),
        "cluster_ready_inside": engine.ok.then(|| cluster_ready_inside(home, host)),
        "tunnel_log": tail(&home.join(TUNNEL_LOG), 8)});
    let cause = classify(&evidence);
    let base = |outcome: &str, steps: Vec<Value>| json!({"at_ms": crate::unix_now() * 1000.0,
        "started_at_ms": started_at_ms, "kind": "docker_recovery", "mode": "remote", "cause": cause,
        "outcome": outcome, "steps": steps, "evidence": evidence});
    let actions = plan(&evidence);
    let mut steps: Vec<Value> = actions.iter().map(|action| act(home, host, action)).collect();
    // Engine and tunnel up with the API down: held for a person (step 3).
    let republish = actions.contains(&"republish_ports");
    if !actions.iter().any(|a| matches!(*a, "start_remote_desktop" | "kickstart_tunnel" | "republish_ports")) {
        return base("held: remote_kubernetes_down", steps);
    }
    let mut wait = if republish { REPUBLISH_WAIT } else if engine.ok { TUNNEL_WAIT } else if reachable { ENGINE_WAIT } else { HOST_WAIT };
    let waited = Instant::now();
    // The engine probe failed ENGINE_FAILS rounds (~2 min) before this recovery started.
    let down_before = Duration::from_secs((super::ENGINE_FAILS as f64 * super::SAMPLE_SECS) as u64);
    let mut power: Option<Value> = None;
    let mut ready = loop {
        if probe_engine(home, host).ok && super::heal::probe_kube(home).ok { break true; }
        if !reachable && power.is_none() && !host_reachable(host) {
            if let Some(step) = super::power::step(home, host, down_before + waited.elapsed()) {
                if step["acted"] == true { wait = waited.elapsed() + super::power::AFTER_ACTION_WAIT; }
                power = Some(step);
            }
        }
        if waited.elapsed() >= wait { break false; }
        std::thread::sleep(Duration::from_secs(5));
    };
    steps.extend(power);
    steps.push(json!({"action": "wait_ready", "ready": ready, "seconds": waited.elapsed().as_secs()}));
    // Step 5: the host answers but its engine stayed dead through the start task: wedged, reboot it.
    let paused = super::paused(home, crate::unix_now());
    if let Some(pause) = paused.as_ref().filter(|_| !ready && reachable && !engine.ok) {
        steps.push(json!({"action": "reboot_host", "decision": "held: paused", "pause": pause}));
    }
    let mut wedged = false;
    if !ready && reachable && !engine.ok && paused.is_none() && !probe_engine(home, host).ok && host_reachable(host) {
        let (more, rebooted) = super::reboot::escalate(home, host);
        steps.extend(more);
        ready = rebooted;
        wedged = true;
    }
    let mut record = base(if ready { "recovered" } else { "failed" }, steps);
    if wedged { record["attribution"] = json!({"node": ENGINE_WEDGED_NODE, "stopped_by": "remote_engine_wedged"}); }
    if republish { record["attribution"] = json!({"node": PORTS_STALE_NODE, "stopped_by": "remote_ports_stale"}); }
    if !reachable {
        let boot = host_reachable(host).then(|| host_boot(host));
        record["attribution"] = host_down_attribution(boot.as_ref(), started_at_ms);
        if let Some(boot) = boot { record["host_boot"] = boot; }
    } else if !wedged && host_reachable(host) {
        // 2026-11-04 20:58 SGT: reachable at the start (tunnel_down), the host then crashed twice
        // (0x9F) and booted 21:01:37 inside the wait; the record carried no boot evidence or node.
        let boot = host_boot(host);
        if booted_during(&boot, started_at_ms - down_before.as_secs_f64() * 1000.0) {
            record["attribution"] = host_down_attribution(Some(&boot), started_at_ms);
            record["host_boot"] = boot;
        }
    }
    record
}

/// Whether the host booted at or after `since_ms` (the outage this recovery answers).
pub(super) fn booted_during(boot: &Value, since_ms: f64) -> bool {
    boot["boot_ms"].as_f64().is_some_and(|ms| ms >= since_ms)
}

/// N4.13 when the host came back from a clean OS shutdown of this outage (no Kernel-Power 41 since
/// boot), else N4.11: an unexpected shutdown, or no boot evidence to tell them apart.
pub(super) fn host_down_attribution(boot: Option<&Value>, started_at_ms: f64) -> Value {
    let clean = boot.is_some_and(|b| b["unexpected_this_boot"] == false
        && b["last_clean_shutdown_ms"].as_f64().is_some_and(|ms| ms >= started_at_ms - CLEAN_SHUTDOWN_LOOKBACK_MS));
    if clean {
        json!({"node": HOST_RESTART_NODE, "stopped_by": "remote_host_restart"})
    } else {
        json!({"node": HOST_DOWN_NODE, "stopped_by": "remote_host_down"})
    }
}

fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}
