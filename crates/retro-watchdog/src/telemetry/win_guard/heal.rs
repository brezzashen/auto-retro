//! Drives the GPU-lost self-heal (`gpu.rs` decides): counts lost rounds, defers, runs one planned
//! reboot on its own thread, records and notifies.
use super::{gpu, GpuHeal};
use serde_json::{json, Value};
use std::{path::Path, sync::mpsc::{self, TryRecvError}};

/// After any heal attempt (done, failed or held by the 24 h cap) no new one starts for this long.
const BLOCK_S: f64 = 3600.0;

impl GpuHeal {
    /// Collect a finished heal.
    pub(super) fn poll(&mut self, now: f64) {
        let Some(worker) = &self.worker else { return };
        match worker.try_recv() {
            Ok(record) => {
                let ready = record["ready"] == true;
                crate::dispatch::notify("看门狗：win 显卡自愈重启", &if ready { "win 重启完成，beta 引擎和 Kubernetes 已就绪".to_string() }
                    else { format!("win 自愈重启未完成：{}", record["error"].as_str().or(record["held"].as_str()).unwrap_or("见 win-guard 记录")) });
                self.last = json!({"at_ms": now * 1000.0, "result": record});
                self.worker = None;
                self.streak = 0;
                self.block_until = now + BLOCK_S;
            }
            Err(TryRecvError::Disconnected) => { self.worker = None; self.block_until = now + BLOCK_S; }
            Err(TryRecvError::Empty) => {}
        }
    }

    /// Fold one win_guard sample in and start the heal when `gpu::step` says so.
    pub(super) fn absorb(&mut self, home: &Path, now: f64, host: &str, sample: &Value) {
        if sample.is_null() || self.worker.is_some() { return; }
        let Some(reason) = gpu::lost(sample) else { self.streak = 0; self.defer_since = 0.0; return; };
        self.streak += 1;
        if now < self.block_until { return; }
        let paused = super::super::docker_watch::paused(home, now).is_some();
        let release = gpu::beta_release_running(home);
        if release && self.defer_since == 0.0 { self.defer_since = now; }
        let deferred = if self.defer_since > 0.0 { now - self.defer_since } else { 0.0 };
        match gpu::step(self.streak, paused, release, deferred) {
            gpu::Step::Wait => {}
            gpu::Step::Defer(why) => self.last = json!({"at_ms": now * 1000.0, "deferred": why, "streak": self.streak, "reason": reason}),
            gpu::Step::Heal => {
                let why = format!("GPU lost, self-heal: {reason}");
                crate::log(&format!("win guard: {why} ({} rounds)", self.streak));
                crate::dispatch::notify("看门狗：win 显卡丢失", &format!("连续 {} 轮 nvidia-smi 失败，开始自愈重启：{reason}", self.streak));
                let (sender, receiver) = mpsc::sync_channel(1);
                let (home, host) = (home.to_path_buf(), host.to_string());
                let spawned = std::thread::Builder::new().name("watchdog-win-gpu-heal".into()).spawn(move || {
                    let record = match super::super::docker_watch::self_heal_reboot(&home, &host, &why, "gpu_self_heal") {
                        None => json!({"held": "reboot_cap_24h", "reason": why}),
                        Some(Ok(record)) => record,
                        Some(Err(error)) => json!({"error": format!("{error:#}"), "reason": why}),
                    };
                    let _ = sender.send(record);
                });
                match spawned {
                    Ok(_) => { self.worker = Some(receiver); self.last = json!({"at_ms": now * 1000.0, "started": reason}); }
                    Err(error) => crate::log(&format!("win guard: gpu heal thread: {error}")),
                }
            }
        }
    }

    pub(super) fn state(&self) -> Value {
        json!({"streak": self.streak, "lost_rounds": gpu::LOST_ROUNDS, "heal_running": self.worker.is_some(),
            "block_until_ms": self.block_until * 1000.0, "last": self.last})
    }
}
