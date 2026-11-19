//! Side effects of a run: the encrypted report page (through the report skill's scripts) with the
//! alert-threshold proposal in its `confirm` list, and the tracking-fix tab (`limit-resume launch`).

use super::collect::Analysis;
use super::config::{Finding, ReportTarget};
use super::posthog::num;
use super::report::prompt;
use crate::skills::seo_competitor::watch_publish::bun;
use serde_json::{Value, json};
use std::process::Command;

const REPORT_SCRIPTS: &str = ".claude/skills/report/scripts";
const THRESHOLD_MARK: &str = "告警阈值";

/// The founder decision the skill requires before any conversion alarm, with today's baseline.
pub fn threshold_item(a: &Analysis, target_days: usize) -> Value {
    let signups: f64 = a.activation.iter().map(|r| num(r, 1)).sum();
    let days = a.activation.len().max(1) as f64;
    let (sent, verified) = a.methods.iter().fold((0.0, 0.0), |(s, v), r| (s + num(r, 1), v + num(r, 2)));
    json!({
        "question": format!("{THRESHOLD_MARK}（生产基线 {}/{target_days} 天；当前日均注册 {:.1}，验证码 {verified}/{sent}）：满 {target_days} 天后按哪种方式告警？\
            提案：①每日注册（user_signed_up 按人去重）低于前 14 天中位数的 50%；②验证码成功率比 14 天基线低 15 个百分点（当日发码 ≥ 20 才判）；\
            ③结账成功率（order_paid / checkout_started）低于基线一半（当日结账 ≥ 5 才判）；④24 小时激活率低于基线一半（当日注册 ≥ 10 才判）。\
            追踪归零（此前日均 ≥ 3 的事件 24 小时为 0 → P0）属于追踪健康，现在已生效。", a.baseline_days.max(0), signups / days),
        "options": ["按提案设置，满 14 天自动启用", "满 14 天后先给我看基线再定", "继续只报趋势，不告警"],
        "recommended": 1,
    })
}

fn decided(spec: &str) -> bool {
    std::fs::read_to_string(spec).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|s| s["decisions"].as_array().cloned()).into_iter().flatten()
        .any(|d| d.to_string().contains(THRESHOLD_MARK))
}

/// Sections whose content is a findings list or day-by-day tables; the charts sit in the funnel and locate sections.
const TABLE_SECTIONS: [&str; 8] = ["本小时结论", "复盘", "追踪健康", "漏单补单", "激活与留存", "登录与来源", "告警阈值", "数据源错误"];

/// `sections[<title>].vizWaiver` for every table section this run's Markdown has.
fn waivers(markdown: &str) -> serde_json::Map<String, Value> {
    markdown.lines().filter_map(|l| l.strip_prefix("## ")).filter(|t| TABLE_SECTIONS.contains(t))
        .map(|t| (format!("sections[{t}].vizWaiver"), json!("这一节是清单或逐日表格，每一行就是原始查询结果；趋势图在「每日漏斗」与「定位」两节")))
        .collect()
}

/// The page's one case. The report build refuses a FAIL without `expected` and `actual`, so a P0 run
/// never republished the page (2026-11-18 16:43).
fn hour_case(p0: bool, at_sgt: &str, status: &str) -> Value {
    json!({
        "title": "本小时巡检",
        "verdict": if p0 { "FAIL" } else { "PASS" },
        "reason": format!("{at_sgt}：{status}"),
        "expected": ["生产 PostHog 事件持续到达，没有 P0 埋点健康故障"],
        "actual": status,
        "shotWaiver": "证据是 PostHog HogQL 查询、kubectl 与 /ingest 探测的原始 JSON，保存在运行目录 analysis.json",
    })
}

/// Rebuilds `<state>/report` from this run's `report.md` and publishes it encrypted.
pub fn publish(target: &ReportTarget, state_dir: &str, run_dir: &str, p0: bool, at_sgt: &str, status: &str, threshold: Value) -> Result<(), String> {
    let dir = format!("{state_dir}/report");
    let spec = format!("{dir}/spec.json");
    let script = |name: &str| format!("{REPORT_SCRIPTS}/{name}");
    if !std::path::Path::new(&spec).exists() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        bun(&[&script("spec.ts"), "init", &dir, "--slug", &target.slug, "--title", &target.title, "--group", "research", "--env", "prod",
            "--goal", "每小时检查 Mars CRM 的 PostHog 埋点是否健康，分析生产漏斗、激活与留存，定位掉点，并派工修复埋点缺陷"], None)?;
    }
    bun(&[&script("spec.ts"), "sections", &spec, &format!("{run_dir}/report.md")], None)?;
    let values = json!({
        "cases.C1": hour_case(p0, at_sgt, status),
        "goal": "每小时检查 Mars CRM 的 PostHog 埋点是否健康，分析生产漏斗、激活与留存，复盘已派工事项，定位掉点，并派工修复埋点缺陷",
        "evidenceWaiver": "数据来自 PostHog Cloud US 项目 704719 的 HogQL 查询（个人 API key，只读查询）、生产 GKE 的 kubectl get 与 example.com/ingest 探测",
        "limits": [
            "只统计 mars_env = production；beta、canary 事件只用于验证埋点",
            "iOS SDK、App Store 续订与退款尚未接入，iOS 注册在网页漏斗里看不到前几步",
            "追踪从 2026-11-02 22:38（新加坡时间）开始：早期队列混有老用户，基线不足 14 天前只报趋势",
        ],
        "confirm": if decided(&spec) { json!([]) } else { json!([threshold]) },
    });
    let mut values = values;
    let markdown = std::fs::read_to_string(format!("{run_dir}/report.md")).unwrap_or_default();
    if let Some(map) = values.as_object_mut() {
        map.extend(waivers(&markdown));
        map.insert("summary".into(), json!(markdown.lines().next().unwrap_or_default()));
    }
    bun(&[&script("spec.ts"), "set", &spec, "--file", "-"], Some(&values.to_string()))?;
    bun(&[&script("build.ts"), &spec], None)?;
    bun(&[&script("publish.ts"), &format!("{dir}/report"), "--encrypt"], None)?;
    Ok(())
}

/// Opens one tracking-fix tab for `findings` and returns the launcher's JSON receipt.
pub fn dispatch(page_url: &str, run_dir: &str, findings: &[Finding]) -> Result<String, String> {
    let file = format!("{run_dir}/prompt.txt");
    std::fs::write(&file, prompt(page_url, run_dir, findings)).map_err(|e| format!("{file}: {e}"))?;
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let out = Command::new(format!("{home}/.local/bin/limit-resume"))
        .args(["launch", "--prompt-file", &file, "--title", "转化追踪修复", "--cwd", &cwd.to_string_lossy(), "--no-wait", "--json"])
        .output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("limit-resume launch failed: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or_default()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn recovery_section_carries_a_viz_waiver() {
        // The report build refuses a section of bare counts without a chart or waiver; the page stopped publishing on 2026-11-15.
        let waived = super::waivers(&super::super::recovery::section(None));
        assert!(waived.contains_key("sections[漏单补单].vizWaiver"));
    }

    #[test]
    fn a_failing_hour_carries_expected_and_actual() {
        let case = super::hour_case(true, "2026-11-18 16:43", "1 项埋点健康故障（P0）");
        assert_eq!(case["verdict"], "FAIL");
        assert!(case["expected"].as_array().is_some_and(|e| !e.is_empty()) && case["actual"].as_str().is_some_and(|a| !a.is_empty()));
    }
}
