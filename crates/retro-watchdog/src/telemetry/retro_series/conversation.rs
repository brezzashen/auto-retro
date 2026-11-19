//! Conversation retro series, organised by problem thread (founder 2026-11-15): each open problem is
//! one case holding its fix versions, their follow-up verdicts and its rate trend; closed problems
//! and the batch index fold away. The brief at the top is the newest batch, so a batch link still
//! lands on what that batch changed. Lines come from `zh` fields or report.md headings; English
//! `why`/`before`/`after` text stays in report.json.

mod render;
mod threads;

use super::Window;
use crate::monitor::root;
use regex::Regex;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::LazyLock;
use threads::{Sources, State};

static SHA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[0-9a-f]{7,40}\b").unwrap());

/// The route prompt's request for the page's Chinese lines.
pub const ZH_FIELDS: &str = "Give each report.json finding zh {problem, solution, gain}, each repeats row zh (what the repeated operation is, in Chinese) and each new retro-probes.json entry zh (its fix, in Chinese): one short sentence each, at most 40 characters; the series page shows only these.";

pub fn reviewed(w: &Window) -> bool {
    w.report["repeats"].is_array() || w.report["findings"].is_array()
}

/// Table-safe text cut to `n` characters.
pub(super) fn clip(value: &Value, n: usize) -> String {
    let text = super::cell(value).trim_end_matches('…').replace('`', "");
    if text.chars().count() > n { format!("{}…", text.chars().take(n).collect::<String>()) } else { text }
}

/// The short commit id in a `commit` or `change` text such as "mastra fe7968b8ff (0.1.1.12501)".
pub(super) fn sha(commit: &Value) -> String {
    SHA.find(commit.as_str().unwrap_or("")).map_or_else(|| "无提交号".to_string(), |m| m.as_str().chars().take(10).collect())
}

fn after_colon(line: &str) -> Option<String> {
    line.split_once('：').map(|(_, rest)| rest.trim().replace('`', "")).filter(|t| !t.is_empty())
}

/// report.md H1 after its colon, unless it only names the window id or a conversation count.
fn headline(w: &Window) -> Option<String> {
    static GENERIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+ 个新?(会话|对话)").unwrap());
    w.md.lines().find_map(|l| l.strip_prefix("# ")).and_then(after_colon).filter(|h| !h.contains(&w.id) && !GENERIC.is_match(h))
}

fn batches(windows: &[&Window]) -> Value {
    let rows = windows
        .iter()
        .rev()
        .map(|w| {
            let probes: Vec<String> = Window::list(&w.report, "findings").iter().filter_map(|f| f["probe"].as_str().map(str::to_string)).collect();
            vec![w.date(), super::cell(&w.report["scope"]), super::cell(&w.incident["count"]), headline(w).map_or("—".into(), |h| clip(&json!(h), 30)), if probes.is_empty() { "—".into() } else { probes.join("、") }]
        })
        .collect();
    let mut section = super::table(&format!("复盘批次（{}）", windows.len()), &["日期", "范围", "对话数", "本批主题", "新版本"], rows);
    section["fold"] = json!(true);
    section
}

fn read(path: &Path) -> Value {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null)
}

pub fn spec(windows: &[&Window], home: &Path) -> Value {
    let registry = read(&home.join(".codex/skills/watchdog/references/retro-probes.json"));
    let lifecycle = read(&root(home).join("probe-lifecycle.json"));
    let all = threads::build(windows, &Sources { registry: &registry, lifecycle: &lifecycle });
    let open: Vec<&threads::Thread> = all.iter().filter(|t| t.state < State::Resolved).collect();
    let of = |s: State| all.iter().filter(|t| t.state == s).collect::<Vec<_>>();
    let order = [State::Escalate, State::Failing, State::NoFix, State::Active, State::Redefine, State::Resolved, State::Quiet, State::Necessary];
    let counts: Vec<(State, usize)> = order.iter().map(|s| (*s, of(*s).len())).filter(|(_, n)| *n > 0).collect();
    let pie = counts.iter().map(|(s, n)| format!("  \"{}\" : {n}", render::state_label(*s))).collect::<Vec<_>>().join("\n");
    let last = windows[windows.len() - 1];
    let tally = counts.iter().map(|(s, n)| format!("{} {n}", render::state_label(*s))).collect::<Vec<_>>().join("、");
    let latest = headline(last).unwrap_or_else(|| "见下方复盘批次".into());
    let sections: Vec<Value> = [
        json!({"title": "问题状态", "diagrams": [{"mermaid": format!("pie title 问题状态（共 {} 个）\n{pie}", all.len()), "caption": "未解决的问题逐个列在下方用例里"}]}),
        render::closed("已解决的问题", &of(State::Resolved), windows),
        render::closed("已沉寂的问题（近 3 批未再出现）", &of(State::Quiet), windows),
        render::closed("判定为必要操作", &of(State::Necessary), windows),
        batches(windows),
    ]
    .into_iter()
    .filter(|s| !s.is_null())
    .collect();
    json!({
        "title": "对话复盘：问题跟进与修复版本",
        "summary": format!("共跟踪 {} 个问题（{tally}）。下面每个未解决的问题列出首次发现、每一版修复和逐批跟进结果。最新批次 {} {}：{latest}。", all.len(), last.date(), super::cell(&last.report["scope"])),
        "goal": "每个重复操作问题持续跟进：出修复版本、逐批验证效果，无效就迭代下一版",
        "brief": render::brief(&open),
        "evidenceWaiver": "汇总页：数据来自每个批次的 report.json、repeats.json、followups.json 和探针登记表，没有界面可截图",
        "limits": [format!("未解决的问题由{}负责，下一轮复盘复核", render::OWNER), "每 100 个会话的调用次数只在该操作进入当批前 46 名时才有记录".to_string()],
        "sections": sections,
        "cases": open.iter().map(|t| render::case(t, windows)).collect::<Vec<_>>(),
    })
}
