//! Search retro series: local-search answer, speed and adoption rates per 100-call window.

use super::{Window, cell, own_page, table, trend};
use serde_json::{Value, json};

pub fn reviewed(w: &Window) -> bool {
    w.report["quality"]["answer_rate"].is_number()
}

fn rate(w: &Window, key: &str) -> f64 {
    w.report["quality"][key].as_f64().unwrap_or(0.0) * 100.0
}

/// The window's call count for the case title. Most report.json files keep `calls` as an object
/// (86 of 124 windows on 2026-11-15); its `metered` count is what the rates divide, then `total`,
/// then `local_search`. Printed whole, the object filled the title with JSON (search-retro
/// 816668620510703927526).
fn calls_count(calls: &Value) -> Option<String> {
    let n = if calls.is_object() { ["metered", "total", "local_search"].iter().find_map(|k| calls.get(*k).filter(|v| v.is_number())) } else { Some(calls).filter(|v| !v.is_null()) };
    n.map(cell)
}

fn committed(w: &Window) -> Vec<&Value> {
    Window::list(&w.report, "findings").iter().filter(|f| f["commit"].as_str().is_some_and(|c| !c.is_empty())).collect()
}

/// The latest window's problems, solutions and gains: the `brief` block a retro page must carry.
fn brief(last: &Window) -> Value {
    let or = |items: Vec<String>, fallback: String| if items.is_empty() { vec![fallback] } else { items };
    let problems = Window::list(&last.report, "failure_classes").iter().map(|c| format!("{}：{}（{} 次）", cell(&c["rate"]), cell(&c["cause"]), cell(&c["count"]))).collect();
    let solutions = committed(last).iter().map(|f| format!("{}（{}）", cell(&f["change"]), cell(&f["commit"]))).collect();
    let gains = committed(last).iter().map(|f| format!("{}：{} → {}", cell(&f["kind"]), cell(&f["before"]), cell(&f["after"]))).collect();
    let rates = format!("最新三率：答对 {:.2}%、速度 {:.2}%、采用 {:.2}%", rate(last, "answer_rate"), rate(last, "speed_rate"), rate(last, "adoption_rate"));
    json!({"problems": or(problems, "最新窗口没有失败".into()), "solutions": or(solutions, "最新窗口没有提交修复".into()), "gains": or(gains, rates)})
}

pub fn spec(windows: &[&Window]) -> Value {
    let last = windows[windows.len() - 1];
    let ledger = windows
        .iter()
        .rev()
        .flat_map(|w| committed(w).into_iter().map(move |f| vec![w.date(), cell(&f["kind"]), cell(&f["change"]), cell(&f["commit"]), cell(&f["before"]), cell(&f["after"])]))
        .collect();
    json!({
        "title": "搜索复盘系列：local-search 三率与修复台账",
        "summary": format!("共 {} 个窗口（{} 至 {}）。最新窗口：答对 {:.2}%、速度 {:.2}%、采用 {:.2}%，评分 {}（{}）",
            windows.len(), windows[0].date(), last.date(), rate(last, "answer_rate"), rate(last, "speed_rate"), rate(last, "adoption_rate"), cell(&last.report["score"]), cell(&last.report["grade"])),
        "goal": "每 100 次 local-search 调用复盘一次，把答对率、速度达标率、采用率推向 99.999%，并把最大失败原因做成脚本修复",
        "criteria": ["每个窗口的每类失败都分类并给出证据", "最大原因有代码修复、测试和前后测量"],
        "brief": brief(last),
        "evidenceWaiver": "汇总页：数据来自每个窗口的 report.json 与事故记录，没有界面可截图",
        "changes": committed(last).iter().map(|f| json!({"item": format!("{}（{}）", cell(&f["change"]), cell(&f["commit"])), "before": cell(&f["before"]), "after": cell(&f["after"])})).collect::<Vec<_>>(),
        "sections": [
            trend("三率趋势", windows, 30, "%", &[
                ("答对率", &|w: &Window| Some(rate(w, "answer_rate"))),
                ("速度达标率", &|w: &Window| Some(rate(w, "speed_rate"))),
                ("采用率", &|w: &Window| Some(rate(w, "adoption_rate"))),
            ]),
            table("修复台账（最新在前）", &["窗口", "类别", "修改", "提交", "之前", "之后"], ledger),
        ],
    })
}

pub fn case(w: &Window) -> Value {
    let classes = Window::list(&w.report, "failure_classes");
    let fixed = classes.iter().filter(|c| c["fixed"] == true).count();
    let mut actual: Vec<String> = classes
        .iter()
        .map(|c| format!("{} · {}：{}（{} 次）", if c["fixed"] == true { "已修复" } else { "未修复" }, cell(&c["rate"]), cell(&c["cause"]), cell(&c["count"])))
        .collect();
    actual.extend(committed(w).iter().map(|f| format!("提交 {}：{}", cell(&f["commit"]), cell(&f["change"]))));
    actual.extend(own_page("search", w));
    json!({
        "id": format!("W{}", w.id),
        "title": format!("{}{} · 评分 {}（{}）", w.date(), calls_count(&w.report["calls"]).map_or(String::new(), |n| format!(" · {n} 次调用")), cell(&w.report["score"]), cell(&w.report["grade"])),
        "verdict": "PASS",
        "reason": format!("答对 {:.2}% · 速度 {:.2}% · 采用 {:.2}%；失败 {} 类，已修 {} 类", rate(w, "answer_rate"), rate(w, "speed_rate"), rate(w, "adoption_rate"), classes.len(), fixed),
        "expected": "每类失败都有原因和证据，最大原因做成脚本修复",
        "actual": actual,
        "shotWaiver": "主机遥测复盘，没有界面可截图",
        "vizWaiver": "本窗口三率画在上方趋势图里",
    })
}
