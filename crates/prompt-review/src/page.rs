//! The run's Chinese report: `report.md` in the run directory, turned into an encrypted
//! reports.example.com page with the Mars_agent report skill (`spec.ts`, `build.ts`, `publish.ts`).
use crate::metrics::{self, BASELINE};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::path::Path;

pub struct Context<'a> {
    pub date: &'a str, pub time: &'a str, pub head: &'a str, pub base_sha: &'a str, pub base_source: &'a str,
    pub cur: &'a Value, pub prev: &'a Value, pub gate_failures: &'a [String], pub semantic: &'a Value,
    pub reasons: &'a [Value], pub history: &'a [Value], pub run_dir: &'a Path,
}

pub struct Published { pub url: Option<String>, pub slug: String }

pub use crate::publish::render;

const LIST_GROUPS: usize = 40;

/// The report guard refuses `users/<name>/…` paths (credential pattern), so home paths are written `~/…`.
pub fn home_relative(path: &Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let text = path.display().to_string();
    match text.strip_prefix(&home) { Some(rest) if !home.is_empty() => format!("~{rest}"), _ => text }
}

fn n(v: &Value, k: &str) -> i64 { v[k].as_i64().unwrap_or(0) }
fn short(sha: &str) -> &str { &sha[..sha.len().min(10)] }
fn signed(d: i64) -> String { if d > 0 { format!("+{d}") } else { d.to_string() } }
fn cell(text: &str, max: usize) -> String {
    let t: String = text.chars().take(max).collect::<String>().replace('|', "／").replace('\n', " ");
    if text.chars().count() > max { format!("{t}…") } else { t }
}

fn group_line(g: &Value) -> String {
    let members = g["members"].as_array().cloned().unwrap_or_default();
    let places: Vec<String> = members.iter().map(|m| {
        let src = m["src"].as_array().and_then(|s| s.first()).and_then(Value::as_str).map(|s| s.trim_start_matches("Mars_CRM/").to_string());
        format!("`{}`", src.unwrap_or_else(|| m["place"].as_str().unwrap_or("?").to_string()))
    }).collect();
    let quote = members.iter().map(|m| cell(m["text"].as_str().unwrap_or(""), 60)).next().unwrap_or_default();
    let new = if g["new_pairs"].as_u64().unwrap_or(0) > 0 { format!("新增 {}", g["new_standing_waste"]) } else { "旧".into() };
    format!("| {} | {} | {} | {} | {} | {} |", g["id"].as_str().unwrap_or(""), members.len(), g["standing_waste"], g["on_demand_waste"], new,
        format!("{}<br>{}", places.join(" "), quote))
}

/// Report text: `##` sections the report skill turns into page sections, Mermaid fences into charts.
pub fn markdown(c: &Context) -> String {
    let (cur, prev) = (c.cur, c.prev);
    let mut md = String::new();
    let points: Vec<(String, i64)> = std::iter::once((format!("{} 基线", &BASELINE.0[5..]), BASELINE.1))
        .chain((c.base_source == "backfill").then(|| ("上周（回填）".to_string(), n(prev, "prefix"))))
        .chain(c.history.iter().filter(|r| r["ok"] == true).map(|r| (r["date"].as_str().unwrap_or("")[5..].to_string(), r["metrics"]["prefix"].as_i64().unwrap_or(0))))
        .chain(std::iter::once((c.date[5..].to_string(), n(cur, "prefix")))).collect();
    let labels = points.iter().map(|(l, _)| format!("\"{l}\"")).collect::<Vec<_>>().join(", ");
    let values = points.iter().map(|(_, v)| v.to_string()).collect::<Vec<_>>().join(", ");
    let top = points.iter().map(|(_, v)| *v).max().unwrap_or(0) * 11 / 10;
    let _ = writeln!(md, "## 趋势\n\n### 请求前缀字符数（每周一次）\n```mermaid\nxychart-beta\n    title \"请求前缀字符数\"\n    x-axis [{labels}]\n    y-axis \"字符\" 0 --> {top}\n    line [{values}]\n```\n");
    let sem: Vec<(String, i64)> = (c.base_source == "backfill").then(|| ("上周（回填）".to_string(), n(prev, "semantic_standing_waste"))).into_iter().chain(c.history.iter().filter(|r| r["ok"] == true).map(|r| (r["date"].as_str().unwrap_or("")[5..].to_string(), r["metrics"]["semantic_standing_waste"].as_i64().unwrap_or(0))))
        .chain(std::iter::once((c.date[5..].to_string(), n(cur, "semantic_standing_waste")))).collect();
    let _ = writeln!(md, "### 常驻语义重复字符数\n```mermaid\nxychart-beta\n    title \"常驻语义重复\"\n    x-axis [{}]\n    y-axis \"字符\" 0 --> {}\n    bar [{}]\n```\n",
        sem.iter().map(|(l, _)| format!("\"{l}\"")).collect::<Vec<_>>().join(", "), sem.iter().map(|(_, v)| *v).max().unwrap_or(0) * 12 / 10 + 1,
        sem.iter().map(|(_, v)| v.to_string()).collect::<Vec<_>>().join(", "));
    let _ = writeln!(md, "## 本周与上周\n\n### 各部分字符数\n```mermaid\nxychart-beta\n    title \"上周与本周（字符）\"\n    %% series: 上周, 本周\n    x-axis [\"系统提示词\", \"工具声明\", \"create\", \"结构重复\", \"常驻语义重复\"]\n    bar [{}, {}, {}, {}, {}]\n    bar [{}, {}, {}, {}, {}]\n```\n",
        n(prev, "system"), n(prev, "tools"), n(prev, "create"), n(prev, "structural_waste"), n(prev, "semantic_standing_waste"),
        n(cur, "system"), n(cur, "tools"), n(cur, "create"), n(cur, "structural_waste"), n(cur, "semantic_standing_waste"));
    let _ = writeln!(md, "## 语义重复清单\n\n同一条规则换了说法写在多处（系统提示词、工具描述、skill 目录与正文、资源发现文本）。“常驻多付”是每次调用都要多发的字符；“按需”是 skill 加载时重复的字符。位置取第一份源码拷贝的行号。\n\n| 组 | 份数 | 常驻多付 | 按需 | 本周新增 | 位置与摘录 |\n|---|---|---|---|---|---|");
    for g in c.semantic["groups"].as_array().into_iter().flatten().take(LIST_GROUPS) {
        let _ = writeln!(md, "{}", group_line(g));
    }
    let total = c.semantic["groups"].as_array().map(Vec::len).unwrap_or(0);
    if total > LIST_GROUPS {
        let _ = writeln!(md, "\n其余 {} 组见运行目录 semantic.json。", total - LIST_GROUPS);
    }
    let _ = writeln!(md, "\n## 结构重复（分析器）\n\n| 份数 | 多付 | 位置 |\n|---|---|---|");
    let analysis: Value = std::fs::read(c.run_dir.join("analysis.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    for s in analysis["report"]["subtrees"].as_array().into_iter().flatten().take(12) {
        let wh = s["where"].as_array().map(|w| w.iter().filter_map(Value::as_str).take(3).collect::<Vec<_>>().join(" ; ")).unwrap_or_default();
        let _ = writeln!(md, "| {} | {} | `{}` |", s["count"], s["waste"], cell(&wh, 140));
    }
    if !c.gate_failures.is_empty() {
        let _ = writeln!(md, "\n对上周采集的门禁失败：\n");
        c.gate_failures.iter().for_each(|f| { let _ = writeln!(md, "- {}", cell(f, 200)); });
    }
    let _ = writeln!(md, "\n## 开会话规则\n\n- 前缀一周增长 ≥ {} 字符（约 1,000 token，超过三次单次改动门禁 300 token 的量）\n- prompt-dedup 门禁对上周采集失败（新的结构重复 ≥ 200、重复描述 ≥ 100 字符或重复系统句子）\n- 新增语义重复：单组常驻多付 ≥ {} 字符（相当于门禁 100 字符重复描述的换说法版本）或合计 ≥ {}\n- 常驻语义重复一周增长 ≥ {} 字符\n- 任一条触发即开一个会话；上一次会话未确认（monitor-ack）时不再开新会话",
        metrics::GROWTH_CHARS, metrics::NEW_GROUP_WASTE, metrics::NEW_WASTE_TOTAL, metrics::SEMANTIC_GROWTH);
    let _ = writeln!(md, "\n## 运行信息\n\n- 本周提交 `{}`（origin/mastra，独立工作树）；对比 `{}`（{}）\n- 新加坡时间 {} {}\n- 单元 {} 个，候选对 {}，判定重复 {} 对（向量 {} ≥ {}，重排 {} ≥ {}）\n- 运行目录 `{}`",
        short(c.head), short(c.base_sha), if c.base_source == "backfill" { "首周回填：7 天前的 mastra" } else { "上周运行的采集" }, c.date, c.time,
        c.semantic["units"], c.semantic["candidates"], c.semantic["pairs"].as_array().map(Vec::len).unwrap_or(0),
        c.semantic["embedder"].as_str().unwrap_or(""), c.semantic["threshold"], c.semantic["reranker"].as_str().unwrap_or(""), c.semantic["same"], home_relative(c.run_dir));
    md
}

/// The spec values `spec.ts set --file -` applies after the sections are read from report.md.
pub fn values(c: &Context) -> Value {
    let (cur, prev) = (c.cur, c.prev);
    let growth = n(cur, "prefix") - n(prev, "prefix");
    let since_base = (n(cur, "prefix") - BASELINE.1) * 100 / BASELINE.1;
    let has = |code: &str| c.reasons.iter().any(|r| r["code"] == code);
    let row = |item: &str, key: &str| json!({"item": item, "expected": format!("上周 {}", n(prev, key)), "actual": format!("本周 {}（{}）", n(cur, key), signed(n(cur, key) - n(prev, key))), "verdict": "PASS"});
    let waiver = "只读分析：证据是离线采集的请求体、分析器 JSON 与源码行号，没有界面可截图";
    let groups: Vec<Value> = c.semantic["groups"].as_array().cloned().unwrap_or_default();
    let mut top: Vec<&Value> = groups.iter().filter(|g| g["standing_waste"].as_u64().unwrap_or(0) > 0).collect();
    top.sort_by_key(|g| std::cmp::Reverse(g["standing_waste"].as_u64().unwrap_or(0)));
    top.truncate(3);
    let (problems, solutions, gains): (Vec<String>, Vec<String>, Vec<String>) = if top.is_empty() {
        (vec!["本周没有常驻语义重复".into()], vec!["保持现状".into()], vec!["无".into()])
    } else {
        top.iter().map(|g| {
            let places = g["places"].as_array().map(|p| p.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("、")).unwrap_or_default();
            let w = g["standing_waste"].as_i64().unwrap_or(0);
            (format!("{}：同一内容写在 {} 份（{}）", g["id"].as_str().unwrap_or(""), g["members"].as_array().map(Vec::len).unwrap_or(0), cell(&places, 80)),
             format!("{}：只留一处权威说明，其余改为指向它或删去", g["id"].as_str().unwrap_or("")),
             format!("每次调用少约 {w} 字符（≈ {} token）", w / 3))
        }).fold((vec![], vec![], vec![]), |mut acc, (a, b, d)| { acc.0.push(a); acc.1.push(b); acc.2.push(d); acc })
    };
    let decision = if c.reasons.is_empty() { "未超过阈值，不开会话".to_string() } else {
        format!("开一个去重会话：{}", c.reasons.iter().filter_map(|r| r["text"].as_str()).collect::<Vec<_>>().join("；"))
    };
    json!({
        "summary": format!("前缀 {} 字符（较上周 {}，较 10-08 基线 +{since_base}%）；常驻语义重复 {} 字符 {} 组；{decision}",
            n(cur, "prefix"), signed(growth), n(cur, "semantic_standing_waste"), groups.len()),
        "brief": {"problems": problems, "solutions": solutions, "gains": gains},
        "evidenceWaiver": waiver,
        // spec.ts init fills context from the shared checkout and its simulators; the run measured the capture tree.
        "context": [{"label": "采集", "value": format!("origin/mastra {}（独立工作树）", short(c.head))},
            {"label": "对比", "value": format!("{}（{}）", short(c.base_sha), if c.base_source == "backfill" { "7 天前的 mastra 回填" } else { "上次运行" })}],
        "limits": ["离线采集：一句 hi 的首轮，未提到飞书，没有个人 skill；按需注入的工具与 skill 正文只按源码计入", "token 按字符除以 3 估算",
            "语义重复由本机向量与重排模型判定，可能把同一资源的不同说明归为一组；会话需逐组判断后再改"],
        "cases.C1": {"title": "前缀体积", "verdict": if has("growth") { "FAIL" } else { "PASS" },
            "reason": format!("前缀 {} 字符，一周 {}（阈值 +{}）", n(cur, "prefix"), signed(growth), metrics::GROWTH_CHARS),
            "checks": [row("请求前缀", "prefix"), row("系统提示词", "system"), row("工具声明", "tools"), row("create 工具", "create")], "shotWaiver": waiver},
        "cases.C2": {"title": "结构重复与门禁", "verdict": if has("gate") { "FAIL" } else { "PASS" },
            "reason": format!("结构重复 {} 字符，重复描述 {}，门禁失败 {} 项", n(cur, "structural_waste"), n(cur, "description_waste"), c.gate_failures.len()),
            "checks": [row("结构重复（≥ 60）", "structural_waste"), row("重复描述", "description_waste"), row("约束文本", "constraint_chars"), row("系统重复片段 token", "system_internal_tokens")], "shotWaiver": waiver},
        "cases.C3": {"title": "语义重复", "verdict": if has("semantic_new") || has("semantic_growth") { "FAIL" } else { "PASS" },
            "reason": format!("{} 组，常驻多付 {} 字符，新增 {} 组（其中常驻 {} 组）", groups.len(), n(cur, "semantic_standing_waste"), n(cur, "semantic_new_groups"),
                groups.iter().filter(|g| g["new_standing_waste"].as_u64().unwrap_or(0) > 0).count()),
            "checks": [row("常驻语义重复", "semantic_standing_waste"), row("语义重复组", "semantic_groups")], "shotWaiver": waiver},
        "cases.C4": {"title": "是否开会话", "verdict": "PASS", "reason": decision, "expected": "超过任一阈值才开一个会话", "actual": decision, "shotWaiver": waiver},
        "sections[语义重复清单].vizWaiver": "逐组清单，每行是一组独立的重复，数字在“本周与上周”图里汇总",
        "sections[结构重复（分析器）].vizWaiver": "分析器原样输出的重复子树清单",
        "sections[开会话规则].vizWaiver": "规则条文，不是测量值",
        "sections[运行信息].vizWaiver": "运行参数清单",
    })
}
