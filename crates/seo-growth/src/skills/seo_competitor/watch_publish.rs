//! Side effects of a watchdog run: republishing the live report page through the report skill's
//! scripts, and opening a Claude tab through `limit-resume launch`.

use super::files::write_text;
use super::watch_analyze::Finding;
use super::watch_report::prompt;
use std::io::Write;
use std::process::{Command, Stdio};

const REPORT_SCRIPTS: &str = ".claude/skills/report/scripts";

/// The fixed wording of one watchdog's report page.
pub struct PageText {
    pub goal: &'static str,
    pub case_title: &'static str,
    pub shot_waiver: &'static str,
    pub evidence_waiver: &'static str,
    pub limits: &'static [&'static str],
    /// `(section title, vizWaiver)` for page sections that list independent items.
    pub section_waivers: &'static [(&'static str, &'static str)],
}

pub const WATCH_TEXT: PageText = PageText {
    goal: "每小时监控 example.com 的搜索指标与页面健康，给出关键词建议并派工优化与复盘",
    case_title: "本小时巡检",
    shot_waiver: "证据是 Search Console API 与页面 HTML 探测的原始 JSON，保存在运行目录 analysis.json",
    evidence_waiver: "数据来自 Google Search Console API（只读服务账号）与对 sitemap.xml 中每个地址的 HTTP 探测",
    limits: &["Search Console 小时数据约有数小时延迟，28 天数据约 2 天延迟", "Bing 一节：Webmaster API 的抓取统计约晚 6 天，名次每次只查几个词的 Bing 第一页；百度与 Bing 的主动推送见 seo-push 页"],
    section_waivers: &[
        ("本小时建议", "逐条列出本小时的待办事项，每条是独立的检查结论，不是一条推理链"),
        ("异常指标捕捉", "按类别柱状图展示异常数，表中逐条列出独立指标异常与双向归因假设，不是单条因果链"),
        ("页面健康", "逐个列出有问题的地址及其问题，是清单，不是统计"),
        ("找词复盘（已派工的词）", "逐个列出已派工的词及其上线后的 Search Console 数据，是清单"),
        ("付费与订单（近 24 小时）", "逐笔列出订单与结账事件，是清单；人数在首行"),
        ("最近一次每小时复盘", "复盘标签写的几行文字摘要，图表在它链接的每小时复盘报告里"),
        ("CPC 关键词池", "首行是词池各类的计数，表中逐个列出带标记的词及其广告数据，每行是独立的词；同系列 CPC 对比图在每小时复盘的 CPC 关键词池复盘一节"),
        ("异常上升词（Google Trends）","逐个列出各市场的上升词与判断（机会、观察、噪声、无关）及对口页面，每行是独立的词，不是推理链"),
        ("Bing 排名与收录","逐个列出目标词的 Bing 名次与 Bing 查询，是清单；收录趋势见 seo-push 页"),
        ("AI 推荐率", "按引擎逐行列出被推荐次数与完整答案数，每行是独立引擎的计数，样本少时不画趋势图"),
        ("AIsa 搜索信号", "逐项列出 AI 概览与 AI 模式的引用计数、关键词意图与外链增减，每行是独立来源的计数，不是对比或推理链"),
        ("达人投放跟进", "逐条列出每条达人投放帖子本小时的播放、点赞等计数与较 1 小时前和 24 小时前的变化，是清单；按小时的趋势在每小时复盘报告里画"),
    ],
};

pub(crate) fn bun(args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    let mut child = Command::new("bun").args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("bun {}: {e}", args.join(" ")))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("bun {} failed: {}", args.first().copied().unwrap_or_default(), stderr.lines().last().unwrap_or_default()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Rebuilds `<state>/report` from this run's `report.md` and publishes it encrypted.
/// `status` is the one-line state of this run shown as the case reason.
#[allow(clippy::too_many_arguments)]
pub fn publish(slug: &str, title: &str, text: &PageText, state_dir: &str, run_dir: &str, failed: bool, at_sgt: &str, status: &str) -> Result<(), String> {
    let dir = format!("{state_dir}/report");
    let spec = format!("{dir}/spec.json");
    let script = |name: &str| format!("{REPORT_SCRIPTS}/{name}");
    if !std::path::Path::new(&spec).exists() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        bun(&[&script("spec.ts"), "init", &dir, "--slug", slug, "--title", title, "--group", "research", "--env", "prod", "--goal", text.goal], None)?;
    }
    bun(&[&script("spec.ts"), "set", &spec, "sections=[]"], None)?;
    bun(&[&script("spec.ts"), "sections", &spec, &format!("{run_dir}/report.md")], None)?;
    let mut values = serde_json::json!({
        "cases.C1": {
            "title": text.case_title,
            "verdict": if failed { "FAIL" } else { "PASS" },
            "reason": format!("{at_sgt}：{status}"),
            "expected": [text.goal],
            "actual": status,
            "shotWaiver": text.shot_waiver,
        },
        "evidenceWaiver": text.evidence_waiver,
        "limits": text.limits,
    });
    let markdown = std::fs::read_to_string(format!("{run_dir}/report.md")).unwrap_or_default();
    for (section, waiver) in present_waivers(text.section_waivers, &markdown) {
        values[format!("sections[{section}].vizWaiver")] = serde_json::json!(waiver);
    }
    bun(&[&script("spec.ts"), "set", &spec, "--file", "-"], Some(&values.to_string()))?;
    bun(&[&script("build.ts"), &spec], None)?;
    bun(&[&script("publish.ts"), &format!("{dir}/report"), "--encrypt"], None)?;
    Ok(())
}

/// The waivers whose section this run's page has: a section left out (no PostHog data, no Bing run) would make
/// `spec.ts set` refuse the whole page, so the page would not republish.
fn present_waivers<'a>(waivers: &'a [(&'a str, &'a str)], markdown: &str) -> Vec<&'a (&'a str, &'a str)> {
    waivers.iter().filter(|(section, _)| markdown.lines().any(|line| line.trim_end() == format!("## {section}"))).collect()
}

/// The model family that leads the `kind` tab instead of the account's default: Fable leads the
/// daily retro (founder 2026-11-10); every other tab keeps the default.
pub fn lead_model(kind: &str) -> Option<&'static str> {
    (kind == "每日复盘").then_some("fable")
}

/// The `limit-resume launch` arguments for a prompt file; `lead` adds `--lead <family>`, which
/// starts the tab on that model and falls back to the next one only when no account has room.
fn launch_args<'a>(file: &'a str, title: &'a str, cwd: &'a str, lead: Option<&'a str>) -> Vec<&'a str> {
    let mut args = vec!["launch", "--prompt-file", file, "--title", title, "--cwd", cwd, "--no-wait", "--json"];
    if let Some(lead) = lead { args.extend(["--lead", lead]); }
    args
}

/// Writes `prompt` to `<run_dir>/prompt.txt`, opens one tab titled `title` and returns the launcher's JSON receipt.
pub fn launch(title: &str, prompt: &str, run_dir: &str) -> Result<String, String> {
    launch_led(title, prompt, run_dir, None)
}

/// `launch` for a tab the model family `lead` must lead.
pub fn launch_led(title: &str, prompt: &str, run_dir: &str, lead: Option<&str>) -> Result<String, String> {
    let file = format!("{run_dir}/prompt.txt");
    write_text(&file, prompt)?;
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let out = Command::new(format!("{home}/.local/bin/limit-resume"))
        .args(launch_args(&file, title, &cwd.to_string_lossy(), lead))
        .output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("limit-resume launch failed: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or_default()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Opens one SEO watch tab for `findings`.
pub fn dispatch(kind: &str, page_url: &str, run_dir: &str, findings: &[Finding]) -> Result<String, String> {
    launch_led(&format!("SEO{kind}"), &prompt(kind, page_url, run_dir, findings), run_dir, lead_model(kind))
}

/// `alias · handle` of a `limit-resume launch --json` receipt for the page; the raw receipt carries
/// profile paths and fields the publication guard refuses, so it stays in `state.json` only.
/// A led tab (`lead` `{wanted, runs}`) also names its model, and says when another one leads.
pub fn tab_label(receipt: &str) -> String {
    serde_json::from_str::<serde_json::Value>(receipt).ok()
        .map(|v| {
            let lead = match (v["lead"]["wanted"].as_str(), v["lead"]["runs"].as_str()) {
                (Some(wanted), Some(runs)) if wanted == runs => format!(" · {wanted} 主导"),
                (Some(wanted), runs) => format!(" · {wanted} 额度不足，改由 {} 主导", runs.or(v["agent"].as_str()).unwrap_or("?")),
                (None, _) => String::new(),
            };
            format!("{} · {}{lead}", v["alias"].as_str().unwrap_or("?"), v["handle"].as_str().unwrap_or("?"))
        })
        .unwrap_or_else(|| "标签已打开".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waiver_is_set_only_for_a_section_the_page_has() {
        let waivers = [("付费与订单（近 24 小时）", "a"), ("AI 推荐率", "b")];
        let kept = present_waivers(&waivers, "# t\n\n## AI 推荐率\n\ntext\n### 付费与订单（近 24 小时）\n");
        assert_eq!(kept, vec![&("AI 推荐率", "b")], "a missing PostHog section no longer blocks the page");
    }

    #[test]
    fn only_the_daily_retro_is_led_by_fable() {
        assert_eq!(lead_model("每日复盘"), Some("fable"));
        for kind in ["健康修复", "流量溯源", "关键词优化", "找词", "每小时复盘"] { assert_eq!(lead_model(kind), None, "{kind}"); }
        let led = launch_args("/r/prompt.txt", "SEO每日复盘", "/w", lead_model("每日复盘"));
        assert_eq!(led[led.len() - 2..], ["--lead", "fable"]);
        assert!(!launch_args("/r/prompt.txt", "SEO找词", "/w", None).contains(&"--lead"));
    }

    #[test]
    fn the_tab_label_names_the_lead_model_and_a_fallback() {
        assert_eq!(tab_label(r#"{"alias":"cc4","handle":"term_a"}"#), "cc4 · term_a");
        assert_eq!(tab_label(r#"{"alias":"cc4","handle":"term_a","lead":{"wanted":"fable","runs":"fable"}}"#), "cc4 · term_a · fable 主导");
        assert_eq!(tab_label(r#"{"alias":"cc4","handle":"term_a","lead":{"wanted":"fable","runs":"opus"}}"#), "cc4 · term_a · fable 额度不足，改由 opus 主导");
        assert_eq!(tab_label(r#"{"alias":"co2","agent":"codex","handle":"term_a","lead":{"wanted":"fable","runs":null}}"#), "co2 · term_a · fable 额度不足，改由 codex 主导");
    }
}
