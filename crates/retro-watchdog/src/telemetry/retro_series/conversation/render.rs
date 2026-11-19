//! Page blocks of the conversation thread view: one case per open thread with its fix versions and
//! rate trend, folded tables for resolved, quiet and necessary threads, and the batch index.

use super::super::{Window, cell, table};
use super::clip;
use super::threads::{State, Thread, Version};
use serde_json::{Value, json};

pub const OWNER: &str = "conversation-retro 复盘路由（每满 100 个新对话自动派出）";

pub fn state_label(s: State) -> &'static str {
    match s {
        State::Escalate => "需升级",
        State::Failing => "跟进失败",
        State::NoFix => "无修复版本",
        State::Active => "跟进中",
        State::Redefine => "口径待改",
        State::Resolved => "已解决",
        State::Quiet => "已沉寂",
        State::Necessary => "必要操作",
    }
}

fn verdict_label(v: &str) -> &str {
    match v {
        "effective" => "有效",
        "ineffective" => "无效",
        "regressed" => "退化",
        "insufficient_data" => "数据不足",
        "scripted" => "已脚本化",
        "existing_entry" => "已有入口",
        "skill" => "写入技能",
        "deferred" => "延后",
        "necessary" => "必要操作",
        other => other,
    }
}

/// The thread's Chinese name: its own `zh`, else its newest version's, else the raw identifier.
pub fn name(t: &Thread) -> String {
    let newest = t.versions.last();
    match (&t.signature, &t.zh) {
        (Some(sig), Some(zh)) => format!("{}（{}）", clip(&json!(zh), 40), clip(&json!(sig), 30)),
        (Some(sig), None) => clip(&json!(sig), 40),
        (None, _) => {
            let v = newest.expect("a probe thread has its version");
            format!("{}：{}", v.probe, v.zh.clone().map_or_else(|| clip(&json!(v.change), 40), |z| clip(&json!(z), 40)))
        }
    }
}

fn version_line(k: usize, v: &Version) -> String {
    let what = v.zh.clone().map_or_else(|| clip(&json!(v.change), 50), |z| clip(&json!(z), 50));
    // Runs of the same verdict collapse into `label×n`.
    let mut runs: Vec<(&str, usize)> = Vec::new();
    for s in &v.verdicts {
        match runs.last_mut() {
            Some((label, n)) if *label == verdict_label(s) => *n += 1,
            _ => runs.push((verdict_label(s), 1)),
        }
    }
    let cut = runs.len().saturating_sub(5);
    let more = if cut > 0 { "… → " } else { "" };
    let follow = if runs.is_empty() { "尚无跟进".to_string() } else { format!("跟进 {more}{}", runs[cut..].iter().map(|(l, n)| if *n > 1 { format!("{l}×{n}") } else { l.to_string() }).collect::<Vec<_>>().join(" → ")) };
    let rate = v.rate.as_ref().map_or(String::new(), |(d, r)| format!("｜{} 每百会话 {r:.1}", d.get(5..).unwrap_or(d)));
    format!("第 {k} 版 {} {}：{what}（{}）｜{follow}{rate}｜{}", v.date.get(5..).unwrap_or(&v.date), v.probe, v.sha, v.status_short())
}

/// Failing versions named with their newest rate: `R42 连续退化（11-15 每百会话 4.9）`.
fn failing(t: &Thread, status: &str) -> String {
    let named: Vec<String> = t
        .versions
        .iter()
        .filter(|v| if status == "escalate" { v.status == "escalate" } else { v.status == "active" && (v.last == "regressed" || v.last == "ineffective") })
        .map(|v| {
            let rate = v.rate.as_ref().map_or(String::new(), |(d, r)| format!("（{} 每百会话 {r:.1}）", d.get(5..).unwrap_or(d)));
            format!("{} {}{}{rate}", v.probe, if status == "escalate" { "连续" } else { "最近" }, verdict_label(&v.last))
        })
        .collect();
    named.join("、")
}

pub fn next_step(t: &Thread) -> String {
    match t.state {
        State::Escalate => format!("{}：已连续 3 轮无效或退化，下一轮复盘先换方案", failing(t, "escalate")),
        State::Failing => format!("{}：下一轮复盘复核，仍不行就出新版本", failing(t, "active")),
        State::NoFix => format!("还没有修复版本，最近一次判定为{}", verdict_label(&t.last_verdict)),
        State::Active => match t.versions.iter().rev().find(|v| v.status == "active") {
            Some(v) if v.last == "effective" => format!("{} 已连续有效 {} 轮，满 3 轮即算解决", v.probe, v.streak),
            Some(v) if v.last.is_empty() => format!("{} 刚上线，等下一轮复盘第一次跟进", v.probe),
            Some(v) => format!("{} 最近判定{}，等更多批次数据", v.probe, verdict_label(&v.last)),
            None => "等下一轮复盘跟进".into(),
        },
        State::Redefine => "连续 3 轮数据不足：要换匹配规则才能判断效果".into(),
        _ => String::new(),
    }
}

fn axis(windows: &[&Window], i: usize) -> String {
    format!("\"{} {}\"", windows[i].date().get(5..).unwrap_or(""), cell(&windows[i].report["scope"]))
}

/// A line of calls per 100 sessions over the windows the thread was measured in.
fn trend(t: &Thread, windows: &[&Window]) -> Option<Value> {
    let points = &t.rate[t.rate.len().saturating_sub(20)..];
    if points.len() < 2 {
        return None;
    }
    let high = points.iter().map(|p| p.1).fold(0.0_f64, f64::max).ceil() as i64 + 1;
    let x = points.iter().map(|p| axis(windows, p.0)).collect::<Vec<_>>().join(", ");
    let y = points.iter().map(|p| format!("{:.1}", p.1)).collect::<Vec<_>>().join(", ");
    let mermaid = format!("xychart-beta\n  title \"每 100 个会话的调用次数\"\n  x-axis [{x}]\n  y-axis \"次\" 0 --> {high}\n  line [{y}]");
    Some(json!({"mermaid": mermaid, "caption": "横轴为复盘批次（日期 范围）；线越低说明重复越少"}))
}

fn tone(status: &str, last: &str) -> &'static str {
    match status {
        "graduated" => "toneGreen",
        "escalate" => "toneRed",
        _ if last == "regressed" || last == "ineffective" => "toneRed",
        _ => "toneOrange",
    }
}

/// The fix iteration as a flowchart: first seen, each version coloured by its outcome, the state now.
fn flow(t: &Thread, windows: &[&Window]) -> Value {
    let safe = |s: &str| s.replace(['"', '[', ']', '(', ')', '|', '<', '>'], " ");
    let dir = if t.versions.len() > 3 { "TB" } else { "LR" };
    let mut lines = vec![format!("flowchart {dir}"), format!("  F[\"{} 首次发现\"]", windows[t.first].date().get(5..).unwrap_or(""))];
    let mut prev = "F".to_string();
    for (k, v) in t.versions.iter().enumerate() {
        let id = format!("V{}", k + 1);
        let short = v.status_short();
        lines.push(format!("  {prev} --> {id}[\"第 {} 版 {} {}<br/>{}\"]", k + 1, v.probe, v.date.get(5..).unwrap_or(""), safe(short)));
        lines.push(format!("  class {id} {}", tone(&v.status, &v.last)));
        prev = id;
    }
    let now_tone = match t.state {
        State::Escalate | State::Failing => "toneRed",
        _ => "toneOrange",
    };
    lines.push(format!("  {prev} --> N[\"现状：{}\"]", state_label(t.state)));
    lines.push(format!("  class N {now_tone}"));
    json!({"mermaid": lines.join("\n"), "caption": "修复迭代：绿色已解决，红色无效或退化，橙色仍在跟进或口径待改"})
}

pub fn case(t: &Thread, windows: &[&Window]) -> Value {
    let w = windows[t.first];
    let mut actual = vec![match &t.signature {
        Some(_) => format!("首次发现 {}（{}）；出现在 {} 个批次，最近 {}", w.date(), cell(&w.report["scope"]), t.seen.len(), windows[*t.seen.last().unwrap_or(&t.first)].date()),
        None => format!("首次发现 {}（{}）", w.date(), cell(&w.report["scope"])),
    }];
    actual.extend(t.versions.iter().enumerate().map(|(k, v)| version_line(k + 1, v)));
    if let Some(&(i, r)) = t.rate.last() {
        actual.push(format!("最近一次测量 {}：每 100 个会话 {r:.1} 次", windows[i].date()));
    }
    actual.push(format!("负责：{OWNER}"));
    let verdict = if matches!(t.state, State::Escalate | State::Failing) { "FAIL" } else { "NOT-VERIFIED" };
    let mut case = json!({
        "id": t.key,
        "title": name(t),
        "verdict": verdict,
        "reason": format!("{}：{}", state_label(t.state), next_step(t)),
        "expected": "改动后这个操作的重复次数持续下降，连续 3 轮跟进有效",
        "actual": actual,
        "shotWaiver": "对话记录复盘，没有界面可截图",
    });
    case["diagrams"] = json!([flow(t, windows)]);
    match trend(t, windows) {
        Some(d) => case["diagrams"].as_array_mut().expect("diagrams list").push(d),
        None => case["vizWaiver"] = json!("调用次数只在一个批次有记录，画不出走势；各版本的最新次数写在版本行里"),
    }
    case
}

/// A folded table of closed threads: resolved, quiet or necessary.
pub fn closed(title: &str, threads: &[&Thread], windows: &[&Window]) -> Value {
    if threads.is_empty() {
        return Value::Null;
    }
    let rows = threads
        .iter()
        .map(|t| {
            let versions = t.versions.iter().map(|v| format!("{} {}", v.probe, v.date)).collect::<Vec<_>>().join("、");
            let last = windows[*t.seen.last().unwrap_or(&t.first)].date();
            vec![name(t), if versions.is_empty() { "—".into() } else { versions }, t.seen.len().max(1).to_string(), last]
        })
        .collect();
    let mut section = table(&format!("{title}（{}）", threads.len()), &["问题", "修复版本", "出现批次", "最近"], rows);
    section["fold"] = json!(true);
    section
}

/// The top brief: the worst open threads as aligned problem / done so far / next step rows.
pub fn brief(open: &[&Thread]) -> Value {
    let top = &open[..open.len().min(3)];
    if top.is_empty() {
        return json!({"problems": ["没有未解决的问题"], "solutions": ["所有修复版本都已连续 3 轮有效"], "gains": ["下一轮复盘继续观察"]});
    }
    let problems: Vec<String> = top.iter().map(|t| format!("{}：{}，出现在 {} 个批次", state_label(t.state), name(t), t.seen.len().max(1))).collect();
    let solutions: Vec<String> = top
        .iter()
        .map(|t| match t.versions.last() {
            Some(v) => format!("已出 {} 版，最新 {} {}：{}", t.versions.len(), v.probe, v.date.get(5..).unwrap_or(&v.date), v.zh.clone().unwrap_or_else(|| clip(&json!(v.change), 30))),
            None => "还没有修复版本".to_string(),
        })
        .collect();
    let gains: Vec<String> = top.iter().map(|t| next_step(t)).collect();
    json!({"problems": problems, "solutions": solutions, "gains": gains})
}
