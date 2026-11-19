//! The watchdog's daily page (reports.example.com `conversion-watch`, encrypted: our own product
//! analytics) as Markdown sections for the report skill's `spec.ts sections`.

use super::collect::Analysis;
use super::config::Finding;
use super::posthog::{num, text};
use super::queries::STEPS;
use chrono::{Duration, NaiveDate};

fn cell(text: &str) -> String {
    text.replace('|', "/").replace('\n', " ")
}

fn pct(part: f64, whole: f64) -> String {
    if whole > 0.0 { format!("{:.0}%", part / whole * 100.0) } else { "—".into() }
}

fn table(head: &[&str], rows: impl Iterator<Item = Vec<String>>) -> String {
    let mut md = format!("| {} |\n|{}|\n", head.join(" | "), head.iter().map(|_| "---").collect::<Vec<_>>().join("|"));
    let mut any = false;
    for row in rows { any = true; md.push_str(&format!("| {} |\n", row.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | "))); }
    if !any { md.push_str(&format!("| 无数据 |{}\n", " |".repeat(head.len().saturating_sub(1)))); }
    md.push('\n');
    md
}

fn health_section(a: &Analysis) -> String {
    let h = &a.health;
    let mut md = String::from("## 追踪健康\n\n各环境近 24 小时事件数（按 uuid 去重）与此前每 24 小时均值：\n\n");
    if let Some(error) = &h.volume_error {
        md.push_str(&format!("> 本轮体量查询失败（{}），下面两张表本轮未知，不代表 0。\n\n", cell(error)));
    }
    md.push_str(&table(&["mars_env", "近 24 h", "此前日均"], h.envs.iter().map(|e| vec![e.name.clone(), format!("{}", e.last24), format!("{:.1}", e.prior_mean)])));
    md.push_str("生产环境逐个事件（`AnalyticsEvent` 联合类型 + SDK + 服务端事件）：\n\n");
    md.push_str(&table(&["事件", "来源", "近 24 h", "此前日均", "每 uuid 份数", "状态"], h.events.iter().map(|e| {
        let status = match (e.last24 > 0.0, e.prior_mean > 0.0) { (true, _) => "到达", (false, true) => "今日为 0", (false, false) => "从未到达" };
        vec![format!("`{}`", e.name), e.source.clone(), format!("{}", e.last24), format!("{:.1}", e.prior_mean), format!("{:.2}", e.duplicates), status.into()]
    })));
    let arrived: Vec<_> = h.events.iter().filter(|e| e.last24 > 0.0).collect();
    if arrived.len() >= 2 {
        md.push_str(&format!("```mermaid\nxychart-beta horizontal\ntitle \"Production events, last 24 h (unique uuid)\"\nx-axis [{}]\ny-axis \"Events\" 0 --> {}\nbar [{}]\n```\n\n",
            arrived.iter().map(|e| format!("\"{}\"", e.name)).collect::<Vec<_>>().join(", "),
            arrived.iter().map(|e| e.last24).fold(1.0_f64, f64::max).ceil(),
            arrived.iter().map(|e| format!("{}", e.last24)).collect::<Vec<_>>().join(", ")));
    }
    match &h.cron {

        Some(c) => md.push_str(&format!("- `analytics-sync` CronJob：上次成功 {}（{:.1} 小时前），24 小时内失败 {} 次{}\n",
            c.last_success.as_deref().unwrap_or("从未"), c.age_hours.unwrap_or(f64::NAN), c.failed_24h, if c.suspended { "，**已暂停**" } else { "" })),
        None => md.push_str("- `analytics-sync` CronJob：本次未能读取（见数据源错误）\n"),
    }
    if let Some(r) = &h.relay {
        md.push_str(&format!("- `/ingest` 中转：SDK 资源 HTTP {}，采集端 HTTP {}（伪 token 得到 PostHog 自己的 401 即为通）→ {}\n", r.asset_status, r.capture_status, if r.ok { "正常" } else { "**异常**" }));
        if r.attempts > 1 || r.error.is_some() {
            md.push_str(&format!("  - 探测 {} 次；最后一次传输错误：{}；生产最近一小时经中转到达 {} 个事件\n", r.attempts, r.error.as_deref().unwrap_or("无"), r.events_last_hour));
        }
    }
    let (signups, web, auth) = h.stitching;
    md.push_str(&format!("- 身份串联：追踪开始后新注册 {signups}，其中有网页事件 {web}、有登录事件 {auth}\n"));
    for (host, path, event, count) in &h.unlabelled { md.push_str(&format!("- 缺 `mars_env`：{host}{path} `{event}` × {count}\n")); }
    md.push('\n');
    md
}

fn funnel_section(a: &Analysis) -> String {
    let mut md = String::from("## 每日漏斗\n\n每行是当天**首次访问**的生产访客（新加坡日期），各列是其中 24 小时内依次走到该步的人数；未满 24 小时的队列还会增长。\n\n");
    let mut head = vec!["首访日期"];
    head.extend(STEPS.iter().map(|s| s.label));
    head.push("状态");
    md.push_str(&table(&head, a.funnel.iter().map(|d| {
        let mut row = vec![d.day.clone()];
        row.extend(d.counts.iter().enumerate().map(|(k, c)| if k == 0 { format!("{c}") } else { format!("{c}（{}）", pct(*c, d.counts[k - 1])) }));
        row.push(if d.matured { "已满 24 h".into() } else { "进行中".into() });
        row
    })));
    let totals: Vec<f64> = (0..STEPS.len()).map(|k| a.funnel.iter().map(|d| d.counts[k]).sum()).collect();
    if totals[0] > 0.0 {
        md.push_str(&format!("```mermaid\nxychart-beta\ntitle \"People per step (all cohorts shown)\"\nx-axis [{}]\ny-axis \"People\" 0 --> {}\nbar [{}]\n```\n\n",
            STEPS.iter().map(|s| format!("\"{}\"", s.key)).collect::<Vec<_>>().join(", "), totals[0].ceil(),
            totals.iter().map(|t| format!("{t}")).collect::<Vec<_>>().join(", ")));
    }
    md
}

fn cohort_section(a: &Analysis, today: NaiveDate) -> String {
    let mut md = String::from("## 激活与留存\n\n激活（founder 定义）：注册后 24 小时内既发出首条消息、又添加首位联系人（来源 chat 或 manual）。\n\n");
    md.push_str(&table(&["注册日期", "注册", "24 h 内发消息", "24 h 内加联系人", "激活", "激活率", "状态"], a.activation.iter().map(|r| vec![
        text(r, 0), format!("{}", num(r, 1)), format!("{}", num(r, 2)), format!("{}", num(r, 3)), format!("{}", num(r, 4)), pct(num(r, 4), num(r, 1)),
        if num(r, 5) > 0.0 { "已满 24 h".into() } else { "进行中".into() },
    ])));
    md.push_str("留存：注册队列在第 1、7、30 天当天（新加坡日期）有 `$pageview` 或 `message_sent` 的比例；未到的天显示 —。\n\n");
    let cell = |d0: &str, n: i64, part: f64, size: f64| match NaiveDate::parse_from_str(d0, "%Y-%m-%d") {
        Ok(d) if d + Duration::days(n) < today => format!("{part}（{}）", pct(part, size)),
        Ok(d) if d + Duration::days(n) == today => format!("{part}（进行中）"),
        _ => "—".into(),
    };
    md.push_str(&table(&["注册日期", "人数", "D1", "D7", "D30"], a.retention.iter().map(|r| {
        let (d0, size) = (text(r, 0), num(r, 1));
        vec![d0.clone(), format!("{size}"), cell(&d0, 1, num(r, 2), size), cell(&d0, 7, num(r, 3), size), cell(&d0, 30, num(r, 4), size)]
    })));
    md
}

fn signin_section(a: &Analysis) -> String {
    let mut md = String::from("## 登录与来源\n\n登录方式（`auth_*` 事件，窗口同漏斗）：\n\n");
    md.push_str(&table(&["method", "发码", "验证通过", "验证码成功率", "密码登录", "OAuth 发起", "失败"], a.methods.iter().map(|r| vec![
        text(r, 0), format!("{}", num(r, 1)), format!("{}", num(r, 2)), pct(num(r, 2), num(r, 1)), format!("{}", num(r, 3)), format!("{}", num(r, 4)), format!("{}", num(r, 5)),
    ])));
    md.push_str("登录失败原因：\n\n");
    md.push_str(&table(&["step", "error_code", "次数"], a.failures.iter().map(|r| vec![text(r, 0), text(r, 1), format!("{}", num(r, 2))])));
    md.push_str("追踪以来新注册的首次来源（PostHog 人物属性 `$initial_*`）：\n\n");
    md.push_str(&table(&["utm_source", "referring_domain", "人数"], a.sources.iter().map(|r| vec![text(r, 0), text(r, 1), format!("{}", num(r, 2))])));
    md
}

/// The suggestion line under a breakdown: the group (at least `min` people) that loses the most people
/// beyond the pooled rate, or nothing when no group is large enough or none trails.
fn weakest(b: &super::collect::Breakdown, min: f64) -> String {
    let (reached, next) = b.rows.iter().fold((0.0, 0.0), |(r, n), row| (r + num(row, 1), n + num(row, 2)));
    let pooled = if reached > 0.0 { next / reached } else { return String::new() };
    let worst = b.rows.iter().filter(|r| num(r, 1) >= min && text(r, 0) != "(none)")
        .map(|r| (text(r, 0), num(r, 1), num(r, 2) / num(r, 1))).filter(|(_, _, rate)| *rate < pooled)
        .max_by(|x, y| ((pooled - x.2) * x.1).total_cmp(&((pooled - y.2) * y.1)));
    match worst {
        Some((value, people, rate)) => format!("建议：先看「{}」这一组（{people} 人，转化 {:.0}%，整体 {:.0}%，按整体转化率算少了 {:.0} 人）；产品改动只作建议，需 founder 授权。\n\n", cell(&value), rate * 100.0, pooled * 100.0, (pooled - rate) * people),
        None => String::new(),
    }
}

fn alerts_section(a: &Analysis) -> String {
    let mut md = format!("## 告警阈值\n\n阈值来自 {}。基线满了自动开始判定；判定的是前一个完整日（激活看最近一个满 24 小时的注册日）。\n\n", a.alerts_decided);
    md.push_str(&table(&["规则", "判定日", "当日", "基线", "状态"], a.alerts.iter().map(|r| vec![r.rule.clone(), r.day.clone(), r.value.clone(), r.baseline.clone(), r.status.clone()])));
    md
}

fn locate_section(a: &Analysis) -> String {

    let mut md = String::from("## 定位\n\n");
    let Some(leak) = &a.leak else { return md + "样本不足（每一步都少于最小样本），今天不定位。\n\n" };
    md.push_str(&format!("{}\n\n", super::funnel::leak_finding(leak).title));
    let totals: Vec<f64> = (0..STEPS.len()).map(|k| a.funnel.iter().map(|d| d.counts[k]).sum()).collect();
    let mut chart = String::from("```mermaid\nflowchart LR\n");
    for (k, step) in STEPS.iter().enumerate() {
        chart.push_str(&format!("  s{k}[\"{} {}\"]\n", step.label, totals[k]));
        if k > 0 { chart.push_str(&format!("  s{} -->|{}| s{k}\n", k - 1, pct(totals[k], totals[k - 1]))); }
    }
    chart.push_str(&format!("  classDef leak stroke-width:3px\n  class s{},s{} leak\n```\n\n", leak.step, leak.step + 1));
    md.push_str(&chart);
    for b in &a.breakdowns {
        md.push_str(&format!("**{}**\n\n", b.label));
        md.push_str(&table(&["取值", "到达", "进入下一步", "转化"], b.rows.iter().map(|r| vec![text(r, 0), format!("{}", num(r, 1)), format!("{}", num(r, 2)), pct(num(r, 2), num(r, 1))])));
        md.push_str(&weakest(b, a.min_sample));
        let shown: Vec<&Vec<serde_json::Value>> = b.rows.iter().filter(|r| num(r, 1) > 0.0).take(8).collect();
        if shown.len() >= 2 {
            md.push_str(&format!("```mermaid\nxychart-beta horizontal\ntitle \"Conversion % by group\"\nx-axis [{}]\ny-axis \"%\" 0 --> 100\nbar [{}]\n```\n\n",
                shown.iter().map(|r| format!("\"{}\"", cell(&text(r, 0)).replace('"', "'").chars().take(24).collect::<String>())).collect::<Vec<_>>().join(", "),
                shown.iter().map(|r| format!("{:.1}", num(r, 2) / num(r, 1) * 100.0)).collect::<Vec<_>>().join(", ")));
        }
    }
    md.push_str(&format!("证明用的 HogQL（项目 704719，`POST /api/projects/704719/query/`）：\n\n```sql\n{}\n```\n\n", a.funnel_hogql));
    if let Some(b) = a.breakdowns.first() { md.push_str(&format!("```sql\n{}\n```\n\n", b.hogql)); }
    md
}

pub fn markdown(a: &Analysis, findings: &[Finding], retro: &[super::retro::RetroRow], at_sgt: &str, dispatch: &str, today: NaiveDate) -> String {
    let prod = a.health.envs.iter().find(|e| e.name == "production");
    let count = |s: &str| findings.iter().filter(|f| f.severity == s).count();
    let mut md = format!("{at_sgt} 更新。生产近 24 小时 {} 个事件（此前日均 {:.0}）；追踪健康 P0 {} 项、P1 {} 项；生产基线 {} 天。\n\n## 本小时结论\n\n",
        prod.map_or(0.0, |p| p.last24), prod.map_or(0.0, |p| p.prior_mean), count("P0"), count("P1"), a.baseline_days.max(0));
    if findings.is_empty() { md.push_str("没有需要处理的事项。\n"); }
    for f in findings { md.push_str(&format!("- **{}** {} → {}\n", f.severity, f.title, f.action)); }
    md.push_str(&format!("\n派工：{dispatch}\n\n"));
    md.push_str(&super::retro::section(retro));
    md.push_str(&super::hourly::section(&a.hourly));
    md.push_str(&health_section(a));
    md.push_str(&funnel_section(a));
    md.push_str(&cohort_section(a, today));
    md.push_str(&signin_section(a));
    md.push_str(&alerts_section(a));
    md.push_str(&super::recovery::section(a.recovery.as_ref()));
    md.push_str(&super::exceptions::section(a.exceptions.as_ref()));
    md.push_str(&locate_section(a));
    if !a.errors.is_empty() { md.push_str(&format!("## 数据源错误\n\n{}\n", a.errors.iter().map(|e| format!("- {}", cell(e))).collect::<Vec<_>>().join("\n"))); }
    md
}

/// The prompt of a tracking-fix tab; `findings` are the new P0/P1 tracking defects.
pub fn prompt(page_url: &str, run_dir: &str, findings: &[Finding]) -> String {
    let list: Vec<String> = findings.iter().map(|f| format!("- [{}] {} → {}", f.severity, f.title, f.action)).collect();
    [
        format!("转化看门狗（追踪修复）：Mars CRM 每小时 PostHog 巡检发现埋点或数据管道缺陷。监控页：{page_url}；本次数据：{run_dir}/analysis.json。"),
        "授权（founder 2026-11-03）：可修改埋点代码（Mars_CRM/web/src/lib/analytics.ts 及 track 调用点、mars-crm-server routes/analytics_sync.rs、analytics-sync CronJob 清单），跑测试，commit/push，按 mars-crm 流程交付 beta，验收后交付 Canary，并用 mars_env='beta'/'canary' 的实时事件查询证明到达；绝不 promote 到生产（example.com 每次需 founder 单独要求），在报告里列出等待 promote 的修复。".into(),
        "`alert:` 开头的是转化告警（founder 2026-11-03 定的阈值）：先排查是否登录、支付或埋点故障；埋点缺陷按上面的授权修复，其余只写原因与建议。".into(),
        "不改产品转化逻辑（文案、流程、定价）：那类发现只写建议。步骤：1. 加载 mars-crm 与 conversion-analytics skill；2. 逐条复现（HogQL 原始结果），区分『没人走到』与『埋点坏了』；3. 修复真缺陷并补测试；4. 发布复盘报告（report skill）：改了什么、哪条查询证明、promote 后哪项指标会恢复；若发现新规律，更新 conversion-analytics skill 的 Watchdog 一节。".into(),
        "事项：".into(),
        list.join("\n"),
    ].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_day_says_so_and_the_prompt_never_promotes() {
        let md = markdown(&Analysis::default(), &[], &[], "2026-11-03 09:23", "无", NaiveDate::from_ymd_opt(2026, 11, 3).unwrap());
        assert!(md.contains("没有需要处理的事项"));
        assert!(md.contains("| 无数据 |"));
        let f = Finding::new("health:x", "P0", "t", "a");
        let text = prompt("https://report", "/tmp/run", &[f]);
        assert!(text.contains("绝不 promote 到生产"));
        assert!(text.contains("- [P0] t → a"));
    }
}
