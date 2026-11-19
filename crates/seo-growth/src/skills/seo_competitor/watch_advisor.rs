//! `mars-skill-tools seo-competitor advisor`: invokes `ag1` on Gemini 4 Neon (`gemini-4-neon-medium`,
//! automatic fallback to `gemini-4-neon-low`) as an independent adversarial red-team advisor
//! against the session that leads the SEO tab: Fable 5.2 for `SEO每日复盘`, the account default
//! Opus 5.5 for `SEO每小时复盘` and `SEO找词` (`watch_publish::lead_model`; founder 2026-11-17).

use super::files::{fail, stringify, write_text};
use super::watch_advisor_pack::build_pack;
use super::watch_judge_rules::CORE_MARKET;
use chrono::{FixedOffset, Utc};
use serde::Serialize;
use std::process::Command;
use std::time::Instant;

pub const DEFAULT_MODEL: &str = "gemini-4-neon-medium";
pub const FALLBACK_MODEL: &str = "gemini-4-neon-low";
const DEFAULT_TIMEOUT_SECS: u64 = 240;

/// Appended to every retro prompt that runs this command: it blocks 2-4 min and prints the whole
/// opinion on exit, so the tab waits with one background Bash call.
pub const ADVISOR_WAIT: &str = "（这条命令约 2–4 分钟：原样作为一次 Bash 调用并设 run_in_background: true，结束时自动通知，通知里的输出就是完整顾问意见，同时写入 --out 与 <out>.json 回执；不要另加 `&`、`> log`、退出标记、until/sleep 轮询、tail -f、pgrep 或 Monitor，等待期间先写报告其余部分）";

const USAGE: &str = "Usage: mars-skill-tools seo-competitor advisor --run <run_dir> [--state <state_dir>] [--draft <lead_draft.md>] [--mode daily|hourly|keywords] [--model <id>] [--ag1 <bin>] [--timeout <secs>] [--out <out.md>] [--dry-run]";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvisorReceipt {
    pub at: String,
    pub mode: String,
    pub model: String,
    pub fallback_used: bool,
    pub elapsed_ms: u128,
    pub anomaly_count: usize,
    pub ok: bool,
    pub output_path: String,
    pub error: Option<String>,
}

fn arg(argv: &[String], name: &str) -> Option<String> {
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).cloned()
}

/// The model that leads the tab running `mode`, named in the prompt so Neon argues against the
/// session that actually wrote the draft (`watch_publish::lead_model` owns the tab's lead).
pub fn lead_name(mode: &str) -> &'static str {
    let kind = match mode { "hourly" => "每小时复盘", "keywords" => "找词", _ => "每日复盘" };
    match super::watch_publish::lead_model(kind) { Some("fable") => "Fable 5.2", _ => "Opus 5.5" }
}

pub fn build_prompt(mode: &str, pack: &str, has_draft: bool) -> String {
    let lead = lead_name(mode);
    let focus = match mode {
        "hourly" => "每小时复盘：聚焦近 1h/24h 流量真伪（机器批次/达人/私域脉冲 vs 自然搜索）、CTA→注册漏斗断裂、GSC 新变化与异常上升词判断",
        "keywords" => "找词唱反调：聚焦候选词的 SERP 真实意图（工具页 vs 资讯/论坛贴/AI Overview 零点击）、巨头首页霸屏、与现有落地页的内耗风险",
        _ => "每日复盘：全面审视搜索量价背离、首页低 CTR、多页抢词、排名骤降、规范页冲突、Bing/Google 断层与拟改文案",
    };
    let target = if has_draft { format!("逐条挑战下方第 6 节 {lead} 的初判与拟执行方案") } else { "独立给出反惯性诊断与风险预警".into() };
    [
        format!("你是独立红队顾问（Gemini 4 Neon via ag1），站在 Google 搜索生态、真实 SERP 意图与增长漏斗的外部视角，与主导会话 {lead} 进行对抗性分析。"),
        "【硬性执行规则】不要调用任何工具、不要读文件、不要跑命令；下方已完整打包原始 JSON 指标与异常捕捉清单。请直接用精炼中文输出以下 5 节（每节不超过 4 条要点，单刀直入摆数字与结论，不要统计字数）：".into(),
        format!("当前场景：{focus}（{target}）。产品背景：example.com（Mars CRM，新加坡与美国全球化个人关系与沟通 AI 助手，英文默认 + /zh-CN、/zh-TW、/ja、/ko、/th、/fr、/it、/es）。"),
        format!("{CORE_MARKET}红队反驳时不要把真人或本地咨询意图当成否决理由。"),
        format!("### 1. 对 {lead} 方案的反驳与盲点\n指出最薄弱的 2–4 个假设（如：瓶颈实为外链/权重或规范页归并却盲目堆砌 H2/FAQ；标题上线不足 7 天就频繁重写打断测试；把私域脉冲回落或脚本点 CTA 误判为 SEO 波动；忽视 AI Overview / Reddit 霸屏导致有排名无点击）。"),
        "### 2. 异常指标深层归因\n针对下方「2. 异常指标捕捉」中的关键项，在「假设 A vs 假设 B」之间基于交叉数据做出裁定（或给出第 3 种根因），必须引用具体数字。".into(),
        "### 3. 明确否决项（Do-Not-Do）\n列出本轮绝不应改动的页面、标题或不应采纳的候选词/异常词，说明做错的代价（内耗、稀释权重、重复建设）。若无否决项请写明原因。".into(),
        format!("### 4. 高杠杆替代建议（最多 3 条）\n给出 {lead} 遗漏或排序偏低的高回报动作：写明目标路径/渠道、具体动作（标题钩子 / 内链合并 / 规范页差异化 / 机器过滤规则 / 外链缺口 / 注册漏斗修复）、预期指标变化与 7/14 天验收阈值。"),
        format!("### 5. 双顾问分歧裁决表\n输出一张 Markdown 表：`| 议题 | {lead} 倾向 | Gemini 4 Neon 对抗意见 | 决定胜负的核查证据 |`，便于主会话逐项核实源码/数据后定案。"),
        "--- 原始客观数据包 ---".into(),
        pack.into(),
    ].join("\n\n")
}

fn invoke_once(ag1_bin: &str, model: &str, timeout_secs: u64, prompt: &str) -> Result<String, String> {
    let out = Command::new(ag1_bin)
        .args(["--model", model, "--disable-slash-commands", "--print-timeout", &format!("{timeout_secs}s"), "-p", prompt])
        .output()
        .map_err(|e| format!("spawn {ag1_bin}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !stdout.is_empty() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let last = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("empty response");
    Err(format!("{model} exited {}: {last}", out.status))
}

fn invoke_with_fallback(ag1_bin: &str, primary: &str, timeout_secs: u64, prompt: &str) -> (String, bool, Result<String, String>) {
    match invoke_once(ag1_bin, primary, timeout_secs, prompt) {
        Ok(text) => (primary.to_string(), false, Ok(text)),
        Err(first_err) if primary != FALLBACK_MODEL => {
            match invoke_once(ag1_bin, FALLBACK_MODEL, timeout_secs.min(240), prompt) {
                Ok(text) => (FALLBACK_MODEL.to_string(), true, Ok(text)),
                Err(second_err) => (FALLBACK_MODEL.to_string(), true, Err(format!("{first_err}; fallback {second_err}"))),
            }
        }
        Err(err) => (primary.to_string(), false, Err(err)),
    }
}

pub fn main(argv: &[String]) -> i32 {
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return 0;
    }
    match run(argv) {
        Ok(()) => 0,
        Err(error) => fail(&error),
    }
}

fn run(argv: &[String]) -> Result<(), String> {
    let run_dir = arg(argv, "--run").ok_or_else(|| format!("missing --run\n{USAGE}"))?;
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let default_state = format!("{home}/.local/state/seo-watch");
    let state_dir = arg(argv, "--state").or_else(|| std::path::Path::new(&default_state).exists().then_some(default_state));
    let draft = arg(argv, "--draft");
    let mode = arg(argv, "--mode").unwrap_or_else(|| "daily".into());
    let model = arg(argv, "--model").unwrap_or_else(|| DEFAULT_MODEL.into());
    let ag1_bin = arg(argv, "--ag1").unwrap_or_else(|| format!("{home}/.local/bin/ag1"));
    let timeout = arg(argv, "--timeout").and_then(|s| s.parse::<u64>().ok()).unwrap_or(DEFAULT_TIMEOUT_SECS);
    let out_path = arg(argv, "--out").unwrap_or_else(|| format!("{run_dir}/advisor-neon.md"));
    let dry_run = argv.iter().any(|a| a == "--dry-run");

    let (pack, anomalies) = build_pack(&run_dir, state_dir.as_deref(), draft.as_deref(), lead_name(&mode))?;
    let prompt = build_prompt(&mode, &pack, draft.is_some());
    write_text(&format!("{out_path}.prompt.txt"), &prompt)?;
    if dry_run {
        println!("{prompt}");
        return Ok(());
    }

    let sgt = FixedOffset::east_opt(8 * 3600).expect("offset");
    let at = Utc::now().with_timezone(&sgt).to_rfc3339();
    let started = Instant::now();
    let (used_model, fallback_used, result) = invoke_with_fallback(&ag1_bin, &model, timeout, &prompt);
    let elapsed_ms = started.elapsed().as_millis();

    let (ok, body, error) = match result {
        Ok(text) => (true, format!("# Gemini 4 Neon 对抗性顾问意见（{used_model}）\n\n> 生成时间：{at} · 耗时：{:.1}s · 捕捉异常：{} 项\n\n{text}\n", elapsed_ms as f64 / 1000.0, anomalies.len()), None),
        Err(err) => (false, format!("# Gemini 4 Neon 对抗性顾问未返回（NOT-VERIFIED）\n\n- 原因：{err}\n- 主会话须在复盘报告中注明外部对抗顾问本次未验证，并自行按数据包中的「假设 B（反方视角）」完成双向证伪。\n"), Some(err)),
    };
    write_text(&out_path, &body)?;
    let receipt = AdvisorReceipt { at, mode, model: used_model, fallback_used, elapsed_ms, anomaly_count: anomalies.len(), ok, output_path: out_path, error };
    write_text(&format!("{}.json", receipt.output_path), &stringify(&receipt)?)?;
    println!("{body}");
    if let Some(err) = receipt.error { return Err(err); }
    Ok(())
}

#[cfg(test)]
#[path = "watch_advisor_tests.rs"]
mod tests;
