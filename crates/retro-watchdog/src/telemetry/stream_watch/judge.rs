//! Per-sample breach rules and the per-session accumulator of the stream watch. Pure functions,
//! so the thresholds are unit-tested without a Windows host or a router.
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Average and worst round trip from the Windows host to the client (LAN Wi-Fi is single digits).
pub(super) const RTT_AVG_MS: f64 = 20.0;
pub(super) const RTT_MAX_MS: f64 = 60.0;
/// Lost pings out of the probe's six.
pub(super) const LOST_PINGS: usize = 2;
pub(super) const RSSI_MIN: i64 = -67;
/// Encoded frames below this share of the requested rate while the GPU is busy: the game, not the
/// encoder, is short of frames (a static menu also encodes fewer frames, but at low GPU load).
pub(super) const FPS_RATIO: f64 = 0.75;
pub(super) const GPU_BUSY: i64 = 90;
pub(super) const GPU_SATURATED: i64 = 97;
pub(super) const STEAM_MBPS: f64 = 20.0;
pub(super) const NIC_RX_MBPS: f64 = 100.0;
/// A kind must hold this many samples in a row before it notifies.
pub(super) const SUSTAIN: u32 = 3;
const HEAVY_PIXELS: i64 = 2560 * 1440;

/// Chinese label and whether a sustained breach notifies (info kinds only go to the summary).
pub(super) fn label(kind: &str) -> (&'static str, bool) {
    match kind {
        "network_rtt" => ("网络延迟高", true),
        "network_loss" => ("网络丢包", true),
        "wifi_signal" => ("Wi-Fi 信号弱", true),
        "wifi_band_2g" => ("客户端连在 2.4 GHz", true),
        "competing_download" => ("Steam 正在下载", true),
        "competing_traffic" => ("主机入站流量大", true),
        "gpu_contention" => ("offload 渲染占用显卡", true),
        "other_encoder" => ("其他程序占用编码器", true),
        "reconnects" => ("串流反复重连", true),
        "render_fps_low" => ("游戏帧率不足（显卡满载）", true),
        "gpu_saturated" => ("显卡满载", false),
        "client_yuv444" => ("客户端开了 YUV 4:4:4（手机解码会拖出数百毫秒延迟，请在串流设置里关闭）", true),
        "heavy_decode" => ("客户端解码负担重（HDR 10-bit 高分辨率）", false),
        _ => ("未知", false),
    }
}

fn num(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

/// The pings that came back, and how many were lost (-1 marks a lost ping in the probe).
pub(super) fn pings(sample: &Value) -> (Vec<f64>, usize) {
    let list: Vec<f64> = match &sample["ping_ms"] { Value::Array(a) => a.iter().filter_map(num).collect(), v => num(v).into_iter().collect() };
    let lost = list.iter().filter(|v| **v < 0.0).count();
    (list.into_iter().filter(|v| *v >= 0.0).collect(), lost)
}

/// NVENC average fps, or `None` when nvidia-smi had no window: it reports fps 0 and latency 0 in
/// about a third of samples of a running stream (2026-11-14: 0 fps at 10% encoder load and 50 Mbps).
pub(super) fn encoded_fps(sample: &Value) -> Option<f64> {
    num(&sample["enc_fps"]).filter(|fps| *fps > 0.0)
}

fn pixels(viewport: &str) -> i64 {
    let mut it = viewport.split('x').filter_map(|p| p.parse::<i64>().ok());
    it.next().unwrap_or(0) * it.next().unwrap_or(0)
}

/// Breaches of one connected sample: (kind, detail).
pub(super) fn breaches(sample: &Value, router: &Value) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let (rtt, lost) = pings(sample);
    if !rtt.is_empty() {
        let avg = rtt.iter().sum::<f64>() / rtt.len() as f64;
        let max = rtt.iter().cloned().fold(0.0, f64::max);
        if avg > RTT_AVG_MS || max > RTT_MAX_MS { out.push(("network_rtt", format!("平均 {avg:.0} ms，最高 {max:.0} ms"))); }
    }
    if lost >= LOST_PINGS { out.push(("network_loss", format!("{lost}/{} 个 ping 丢失", lost + rtt.len()))); }
    if let Some(rssi) = num(&router["rssi"]) { if (rssi as i64) < RSSI_MIN { out.push(("wifi_signal", format!("RSSI {rssi:.0}"))); } }
    if router["band"].as_str().is_some_and(|b| b.starts_with("2G")) { out.push(("wifi_band_2g", router["band"].as_str().unwrap_or("").into())); }
    if let Some(rate) = num(&sample["steam_download_mbps"]) { if rate > STEAM_MBPS { out.push(("competing_download", format!("{rate:.0} Mbps"))); } }
    if let Some(rx) = num(&sample["nic_rx_mbps"]) { if rx > NIC_RX_MBPS { out.push(("competing_traffic", format!("入站 {rx:.0} Mbps"))); } }
    if sample["offload_jobs"].as_i64().unwrap_or(0) > 0 { out.push(("gpu_contention", format!("{} 个渲染任务", sample["offload_jobs"]))); }
    if sample["enc_sessions"].as_i64().unwrap_or(0) > 1 {
        out.push(("other_encoder", format!("{} 个编码会话 {}", sample["enc_sessions"], sample["other_streamers"])));
    }
    if sample["connects_10m"].as_i64().unwrap_or(0) >= 3 { out.push(("reconnects", format!("10 分钟内连接 {} 次", sample["connects_10m"]))); }
    let gpu = sample["gpu_util"].as_i64().unwrap_or(0);
    if let (Some(fps), Some(want)) = (encoded_fps(sample), num(&sample["requested_fps"])) {
        if want > 0.0 && fps < want * FPS_RATIO && gpu >= GPU_BUSY {
            out.push(("render_fps_low", format!("编码 {fps:.0}/{want:.0} fps，GPU {gpu}%（{}）", sample["top_gpu_process"].as_str().unwrap_or("?"))));
        }
    }
    if gpu >= GPU_SATURATED { out.push(("gpu_saturated", format!("GPU {gpu}%"))); }
    let flags = sample["encoder_flags"].as_str().unwrap_or("");
    let hdr = sample["color"].as_str() == Some("HDR") || flags.contains("10-bit");
    // Founder A/B 2026-11-14: 4:4:4 to the iPhone was the 400-500 ms lag; 4:2:0 at the same
    // resolution, HDR and Wi-Fi felt smooth. Phone decoders lack HEVC 4:4:4, so it always notifies.
    if flags.contains("yuv444") {
        out.push(("client_yuv444", format!("{} {} {}", sample["viewport"].as_str().unwrap_or(""), sample["encoder"].as_str().unwrap_or(""), flags)));
    } else if hdr && pixels(sample["viewport"].as_str().unwrap_or("")) > HEAVY_PIXELS {
        out.push(("heavy_decode", format!("{} {} {}", sample["viewport"].as_str().unwrap_or(""), sample["encoder"].as_str().unwrap_or(""), flags)));
    }
    out
}

fn pct(values: &[f64], p: f64) -> Value {
    if values.is_empty() { return Value::Null; }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let i = ((v.len() - 1) as f64 * p).round() as usize;
    json!((v[i] * 10.0).round() / 10.0)
}

/// One Sunshine client session, from its CLIENT CONNECTED line to the first sample without it.
#[derive(Default)]
pub(super) struct Session {
    pub key: String,
    pub started_ms: f64,
    pub last_ms: f64,
    pub samples: u32,
    pub errors: u32,
    streak: BTreeMap<&'static str, u32>,
    notified: BTreeSet<&'static str>,
    counts: BTreeMap<&'static str, u32>,
    rtt: Vec<f64>,
    sent: usize,
    lost: usize,
    fps: Vec<f64>,
    tx: Vec<f64>,
    gpu: Vec<f64>,
    rssi: Vec<f64>,
    first: Value,
    last: Value,
}

impl Session {
    pub(super) fn new(key: &str, now_ms: f64) -> Self {
        Session { key: key.into(), started_ms: now_ms, last_ms: now_ms, ..Default::default() }
    }

    /// Add a connected sample; returns the kinds that just became sustained and should notify.
    pub(super) fn add(&mut self, now_ms: f64, sample: &Value, router: &Value) -> Vec<(&'static str, String)> {
        self.samples += 1;
        self.last_ms = now_ms;
        let (rtt, lost) = pings(sample);
        self.sent += rtt.len() + lost;
        self.lost += lost;
        self.rtt.extend(&rtt);
        encoded_fps(sample).into_iter().for_each(|v| self.fps.push(v));
        num(&sample["nic_tx_mbps"]).into_iter().for_each(|v| self.tx.push(v));
        num(&sample["gpu_util"]).into_iter().for_each(|v| self.gpu.push(v));
        num(&router["rssi"]).into_iter().for_each(|v| self.rssi.push(v));
        let format = json!({"viewport": sample["viewport"], "requested_fps": sample["requested_fps"], "encoder": sample["encoder"],
            "color": sample["color"], "encoder_flags": sample["encoder_flags"], "client_ip": sample["client_ip"],
            "node": router["node_ip"], "band": router["band"]});
        if self.first.is_null() { self.first = format.clone(); }
        self.last = format;
        let found = breaches(sample, router);
        let kinds: BTreeSet<&str> = found.iter().map(|(k, _)| *k).collect();
        self.streak.retain(|k, _| kinds.contains(k));
        let mut fire = Vec::new();
        for (kind, detail) in found {
            *self.counts.entry(kind).or_default() += 1;
            let streak = self.streak.entry(kind).or_default();
            *streak += 1;
            if *streak >= SUSTAIN && label(kind).1 && self.notified.insert(kind) { fire.push((kind, detail)); }
        }
        fire
    }

    /// The closing record: duration, percentiles and each breach's share of the samples, largest first.
    pub(super) fn summary(&self, ended_by: &str) -> Value {
        let mut kinds: Vec<(&&str, &u32)> = self.counts.iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(a.1));
        let share = |n: u32| (n as f64 / self.samples.max(1) as f64 * 100.0).round();
        json!({"kind": "stream_session", "key": self.key, "started_ms": self.started_ms, "ended_ms": self.last_ms,
            "minutes": ((self.last_ms - self.started_ms) / 60_000.0 * 10.0).round() / 10.0, "ended_by": ended_by,
            "samples": self.samples, "probe_errors": self.errors, "format_first": self.first, "format_last": self.last,
            "rtt_ms": {"p50": pct(&self.rtt, 0.5), "p95": pct(&self.rtt, 0.95), "max": pct(&self.rtt, 1.0)},
            "ping_loss": {"sent": self.sent, "lost": self.lost},
            "enc_fps": {"p5": pct(&self.fps, 0.05), "p50": pct(&self.fps, 0.5)},
            "stream_mbps": {"p50": pct(&self.tx, 0.5), "p95": pct(&self.tx, 0.95)},
            "gpu_util": {"p50": pct(&self.gpu, 0.5)}, "rssi": {"p50": pct(&self.rssi, 0.5), "min": pct(&self.rssi, 0.0)},
            "breaches": kinds.iter().map(|(k, n)| json!({"kind": k, "label": label(k).0, "samples": n, "share_pct": share(**n)})).collect::<Vec<_>>(),
            "client_side": "decode time, render queue and client frame drops are only in the client's statistics overlay"})
    }
}
