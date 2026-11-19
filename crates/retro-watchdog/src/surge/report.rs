//! Writes `analysis/<id>/report.json` and a Chinese `report.md` for a surge, then posts one macOS
//! notification summarizing what was ended and how ControlCenter looked.
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::Value;
use std::{fmt::Write as _, os::unix::fs::OpenOptionsExt, path::Path};

pub fn write(home: &Path, id: &str, report: &Value) -> Result<()> {
    let directory = root(home).join("analysis").join(id);
    write_json(&directory.join("report.json"), report)?;
    let markdown = markdown(report);
    let temporary = directory.join(format!("report.md.{}.tmp", std::process::id()));
    std::fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&temporary)
        .and_then(|mut file| std::io::Write::write_all(&mut file, markdown.as_bytes()))?;
    std::fs::rename(temporary, directory.join("report.md"))?;
    notify(report);
    Ok(())
}

fn markdown(report: &Value) -> String {
    let mut text = String::new();
    let seconds = (report["detected_at_ms"].as_f64().unwrap_or(0.0) - report["hot_since_ms"].as_f64().unwrap_or(0.0)) / 1000.0;
    let _ = writeln!(text, "# CPU 突增 {}\n\n## 触发\n\n整机 CPU 连续 {seconds:.0} 秒 > 95%。\n", report["id"].as_str().unwrap_or(""));
    let _ = writeln!(text, "## 处置\n");
    let actions = report["actions"].as_array().cloned().unwrap_or_default();
    if actions.is_empty() {
        let _ = writeln!(text, "- 无符合清理规则的进程或定时任务，未终止任何进程。");
    }
    for action in &actions {
        let decision = &action["decision"];
        let usage = decision["usage_one_core"].as_f64().map_or("未知".into(), |u| format!("{:.0}%", u * 100.0));
        let _ = writeln!(text, "- `{}` {}：单核 {usage}（{} 秒窗口），RSS {} MiB，{} → {}", decision["rule"].as_str().unwrap_or(""),
            decision["target"].as_str().unwrap_or(""), decision["evidence_window_s"], decision["rss_kib"].as_u64().unwrap_or(0) / 1024,
            decision["action"]["kind"], action["outcome"]);
    }
    let _ = writeln!(text, "\n## 异常定时任务与报错\n");
    for job in report["failing_launchd_jobs"].as_array().into_iter().flatten() {
        let _ = writeln!(text, "- `{}` 上次退出 {}，pid {}", job["label"].as_str().unwrap_or(""), job["last_exit"], job["pid"]);
        let tail = job["stderr_tail"].as_array().map_or(&[][..], Vec::as_slice);
        for line in &tail[tail.len().saturating_sub(3)..] {
            let _ = writeln!(text, "  - `{}`", line.as_str().unwrap_or("").chars().take(200).collect::<String>());
        }
    }
    for crash in report["recent_crash_reports"].as_array().into_iter().flatten() {
        let _ = writeln!(text, "- 崩溃报告 `{}`（{} 秒前）", crash["file"].as_str().unwrap_or(""), crash["age_s"]);
    }
    let cc = &report["control_center"];
    let _ = writeln!(text, "\n## 控制中心\n\n- pid {}，运行 {} 秒，RSS {} MiB，60 秒单核 CPU {}，Mach 端口 {} / 上限 {}",
        cc["pid"], cc["uptime_s"], cc["rss_mib"], cc["cpu_one_core_60s"], cc["ports"], cc["port_limit"]);
    let _ = writeln!(text, "- 近 10 分钟渲染卡顿：{}\n- 异常：{}\n- 重启：{}", cc["render_10min"], cc["abnormal"], cc["restart"]);
    let _ = writeln!(text, "\n## 处置后\n\n- 整机 CPU 采样：{}", report["host_cpu_after"]);
    text
}

fn notify(report: &Value) {
    let actions = report["actions"].as_array().map_or(0, Vec::len);
    let failing = report["failing_launchd_jobs"].as_array().map_or(0, Vec::len);
    let cc = report["control_center"]["abnormal"].as_array().map_or(0, Vec::len);
    let restarted = report["control_center"]["restart"]["outcome"] == "restarted";
    let message = format!("CPU 连续 >95%：处置 {actions} 项，异常定时任务 {failing} 个，控制中心{}{}",
        if cc == 0 { "正常" } else { "异常" }, if restarted { "（已重启）" } else { "" });
    let script = format!("display notification \"{}\" with title \"CPU 看门狗\"", message.replace('"', "\\\""));
    let _ = std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).stdin(std::process::Stdio::null()).output();
}
