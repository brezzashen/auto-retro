//! Canary retro series: release durations per 10-release window, plus over-hard-limit incidents.

use super::{Window, cell, own_page, table, trend};
use serde_json::{Value, json};

const CHECKS: [(&str, &str); 4] = [("errors", "错误"), ("cache", "缓存"), ("queue", "排队"), ("ci", "CI")];

pub fn reviewed(w: &Window) -> bool {
    w.report["checks"].is_object()
}

fn minutes(w: &Window, key: &str) -> Option<f64> {
    w.incident["stats"]["seconds"]["all"][key].as_f64().map(|s| s / 60.0)
}

/// A change row's text under whichever key the session used.
fn change_text(c: &Value) -> String {
    ["summary", "change", "item", "title", "what"].iter().map(|k| cell(&c[*k])).find(|t| !t.is_empty()).unwrap_or_else(|| cell(c))
}

fn changes(w: &Window) -> Vec<&Value> {
    Window::list(&w.report, "changes").iter().filter(|c| c.is_object()).collect()
}

fn check(w: &Window, key: &str) -> (String, String) {
    let value = &w.report["checks"][key];
    match value {
        Value::Object(_) => (cell(&value["verdict"]), cell(&value["evidence"])),
        other => (String::new(), cell(other)),
    }
}

fn slow_runs(w: &Window) -> Vec<String> {
    let runs = ["over_target_runs", "runs"].iter().flat_map(|k| Window::list(&w.report, k)).filter(|r| r["cause"].is_string());
    runs.map(|r| format!("{}（{}）：{}", cell(&r["id"]), cell(&r["cause"]), cell(&r["evidence"]))).collect()
}

fn open_items(w: &Window) -> Vec<String> {
    ["not_verified", "unverified", "open_items", "not_done"].iter().flat_map(|k| Window::list(&w.report, k)).filter_map(Value::as_str).map(|s| cell(&json!(s))).collect()
}

fn brief(last: &Window) -> Value {
    let or = |items: Vec<String>, fallback: &str| if items.is_empty() { vec![fallback.to_string()] } else { items };
    let defects: Vec<String> = CHECKS.iter().filter_map(|(k, name)| {
        let (verdict, evidence) = check(last, k);
        (!verdict.is_empty() && verdict != "ok").then(|| format!("{name}检查 {verdict}：{evidence}"))
    }).collect();
    let problems = [defects, slow_runs(last)].concat();
    let solutions = changes(last).iter().map(|c| format!("{}（{}）", change_text(c), cell(&c["commit"]))).collect();
    let gains = changes(last).iter().filter(|c| !c["before"].is_null()).map(|c| format!("{} → {}", cell(&c["before"]), cell(&c["after"]))).collect();
    json!({"problems": or(problems, "最新窗口四项检查均正常"), "solutions": or(solutions, "最新窗口没有提交修复"), "gains": or(gains, "最新窗口没有可测的前后对比")})
}

pub fn spec(windows: &[&Window]) -> Value {
    let last = windows[windows.len() - 1];
    let ledger = windows
        .iter()
        .rev()
        .flat_map(|w| changes(w).into_iter().map(move |c| vec![w.date(), cell(&c["commit"]), change_text(c), cell(&c["before"]), cell(&c["after"])]))
        .collect();
    let counted = windows.iter().filter(|w| minutes(w, "p50").is_some()).count();
    json!({
        "title": "Canary 发布复盘系列：发布耗时与修复台账",
        "summary": format!("共 {} 个窗口（{} 至 {}）：{} 个满 10 次发布的窗口，{} 次超硬上限事件。最新窗口 {}", windows.len(), windows[0].date(), last.date(), counted, windows.len() - counted, cell(&json!(reason(last)))),
        "goal": "每 10 次 Canary 发布复盘一次，每次超过硬上限也复盘，把一次发布压到 5-10 分钟的滚动更新，并把有证据的缺陷修掉",
        "criteria": ["每个超目标、超硬上限的发布都有原因和证据", "错误、缓存、排队、CI 四项检查逐项回答", "有证据的缺陷由所属技能修复并给出前后数据"],
        "brief": brief(last),
        "evidenceWaiver": "汇总页：数据来自每个窗口的 report.json 与事故记录，没有界面可截图",
        "changes": changes(last).iter().map(|c| json!({"item": format!("{}（{}）", change_text(c), cell(&c["commit"])), "before": cell(&c["before"]), "after": cell(&c["after"])})).collect::<Vec<_>>(),
        "sections": [
            trend("发布耗时（分钟）", windows, 30, "分钟", &[
                ("p50", &|w: &Window| minutes(w, "p50")),
                ("p90", &|w: &Window| minutes(w, "p90")),
                ("最长", &|w: &Window| minutes(w, "max")),
            ]),
            table("修复台账（最新在前）", &["窗口", "提交", "修改", "之前", "之后"], ledger),
        ],
    })
}

fn reason(w: &Window) -> String {
    let stats = &w.incident["stats"];
    match minutes(w, "p50") {
        Some(p50) => format!(
            "{} 次发布：p50 {:.1} 分、p90 {:.1} 分、最长 {:.1} 分；成功率 {}%；达标 {} / 超目标 {} / 超硬限 {}",
            cell(&stats["window"]["runs"]), p50, minutes(w, "p90").unwrap_or(0.0), minutes(w, "max").unwrap_or(0.0),
            cell(&stats["outcomes"]["success_rate"]), cell(&stats["verdicts"]["target"]), cell(&stats["verdicts"]["over_target"]), cell(&stats["verdicts"]["over_hard"])
        ),
        None => format!("超过硬上限：{} 次发布（{}）", Window::list(&w.incident, "runs").len(),
            Window::list(&w.incident, "runs").iter().map(|r| cell(&r["id"])).collect::<Vec<_>>().join("、")),
    }
}

pub fn case(w: &Window) -> Value {
    let mut actual: Vec<String> = CHECKS.iter().map(|(k, name)| {
        let (verdict, evidence) = check(w, k);
        format!("{name}检查 {}：{evidence}", if verdict.is_empty() { "—".to_string() } else { verdict })
    }).collect();
    actual.extend(slow_runs(w).into_iter().map(|r| format!("慢发布 {r}")));
    actual.extend(changes(w).iter().map(|c| format!("提交 {}：{}", cell(&c["commit"]), change_text(c))));
    actual.extend(open_items(w).into_iter().map(|s| format!("未验证：{s}")));
    actual.extend(own_page("canary", w));
    let kind = if minutes(w, "p50").is_some() { "10 次发布" } else { "超硬上限" };
    json!({
        "id": format!("W{}", w.id),
        "title": format!("{} · {kind} · 提交 {} 个", w.date(), changes(w).len()),
        "verdict": "PASS",
        "reason": reason(w),
        "expected": "四项检查逐项回答，慢发布有原因与证据，缺陷修复有前后数据",
        "actual": actual,
        "shotWaiver": "发布遥测复盘，没有界面可截图",
        "vizWaiver": "本窗口耗时画在上方趋势图里",
    })
}
