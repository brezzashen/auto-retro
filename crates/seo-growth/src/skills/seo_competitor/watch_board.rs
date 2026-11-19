//! The keyword-room sections of the watchdog page: today's round (funnel, verdicts, competitor
//! pages) and the retro of every handed-over keyword against its 28-day Search Console row.

use super::files::read_json;
use super::watch_analyze::TargetRow;
use super::watch_keywords::{Ledger, Round};
use chrono::NaiveDate;

/// Days after shipping from which a keyword without impressions counts as a miss.
const HIT_AFTER_DAYS: i64 = 14;

fn cell(text: &str) -> String { text.replace('|', "/").replace('\n', " ") }

fn num(v: &serde_json::Value, key: &str) -> String {
    v[key].as_f64().map_or("—".into(), |n| if n.fract() == 0.0 { format!("{n}") } else { format!("{n:.2}") })
}

pub fn round_markdown(round: Option<&Round>) -> String {
    let mut md = String::from("## 找词看板（每天一轮）\n\n");
    let Some(r) = round else { return md + "还没有跑过找词轮次。\n\n" };
    let total: usize = r.pool.values().sum();
    let label = |k: &str| match k { "related" => "相关词", "trends" => "上升词", "competitor-rank" => "竞品排名词", "competitor-page" => "竞品新页",
        "anomaly" => "异常词（已判机会）", _ => "自家 GSC" };
    let sources: Vec<String> = r.pool.iter().map(|(k, n)| format!("{} {n}", label(k))).collect();
    let count = |v: &str| r.judged.iter().filter(|j| j["verdict"] == v).count();
    md.push_str(&format!("{} 一轮：候选 {total} 个（{}），初判做 {}，补数与唱反调后做 {}，派工 {} 个：{}。\n\n",
        r.day, sources.join("、"), r.prejudged_do, count("做"), r.picks.len(), if r.picks.is_empty() { "无".into() } else { r.picks.join("、") }));
    md.push_str(&format!("```mermaid\nxychart-beta\ntitle \"Keyword funnel {}\"\nx-axis [\"候选\", \"初判做\", \"最终做\", \"派工\"]\ny-axis \"Keywords\" 0 --> {}\nbar [{total}, {}, {}, {}]\n```\n\n",
        r.day, total.max(1), r.prejudged_do, count("做"), r.picks.len()));
    if !r.gate.note.is_empty() {
        let pages: Vec<String> = r.gate.pages.iter().map(|p| format!("{}（{} 首见，{}，点击 {}）", p.path, p.first_seen, p.status, p.clicks)).collect();
        md.push_str(&format!("{}。{}\n\n", r.gate.note, if pages.is_empty() { String::new() } else { format!("新页：{}。", pages.join("、")) }));
    }
    md.push_str("| 词 | 判词 | 来源 | 页面 | 月搜 | KD | 季 / 年趋势 | 胜率 | 理由 / 唱反调 |\n|---|---|---|---|---|---|---|---|---|\n");
    for j in r.judged.iter().filter(|j| j["verdict"] != "放弃").take(15) {
        md.push_str(&format!("| {} | {} | {} | {} | {} | {} | {}% / {}% | {} | {} |\n", cell(j["keyword"].as_str().unwrap_or_default()),
            j["verdict"].as_str().unwrap_or_default(), cell(j["source"].as_str().unwrap_or_default()), j["page"].as_str().unwrap_or_default(),
            num(j, "volume"), num(j, "kd"), num(j, "quarterly"), num(j, "yearly"), num(j, "winRate"),
            cell(&[j["reason"].as_str().unwrap_or_default(), j["objection"].as_str().unwrap_or_default()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("；"))));
    }
    let mut dropped = std::collections::BTreeMap::<String, usize>::new();
    for j in r.judged.iter().filter(|j| j["verdict"] == "放弃") { *dropped.entry(j["reason"].as_str().unwrap_or_default().split(' ').next().unwrap_or_default().to_string()).or_default() += 1; }
    let reasons: Vec<String> = dropped.iter().map(|(k, n)| format!("{k} {n}")).collect();
    md.push_str(&format!("\n放弃（前 40 个）：{}。\n", if reasons.is_empty() { "无".into() } else { reasons.join("、") }));
    let pages: Vec<String> = r.new_pages.iter().map(|(d, urls)| format!("{d} {} 个（{}）", urls.len(), urls.iter().take(3).cloned().collect::<Vec<_>>().join("、"))).collect();
    md.push_str(&format!("\n竞品新页面：{}。\n", if pages.is_empty() { "无（首轮只记录基线）".into() } else { pages.join("；") }));
    if !r.errors.is_empty() { md.push_str(&format!("\n找词数据源错误：{}\n", r.errors.join("；"))); }
    md + "\n"
}

/// The handed-over keywords with their Search Console data since shipping.
pub fn retro_markdown(ledger: &Ledger, targets: &[TargetRow], today: NaiveDate) -> String {
    let mut md = String::from("## 找词复盘（已派工的词）\n\n");
    let handed: Vec<(&String, &super::watch_keywords::Entry)> = ledger.iter().filter(|(_, e)| e.dispatched.is_some()).collect();
    if handed.is_empty() { return md + "还没有派工的词。\n\n"; }
    let days = |d: &Option<String>| d.as_deref().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).map(|d| (today - d).num_days());
    let row = |k: &str| targets.iter().find(|t| t.keyword == k);
    let mature: Vec<_> = handed.iter().filter(|(_, e)| days(&e.shipped).is_some_and(|n| n >= HIT_AFTER_DAYS)).collect();
    let hits = mature.iter().filter(|(k, _)| row(k).is_some_and(|t| t.impressions > 0.0)).count();
    md.push_str(&format!("派工 {} 个，已上线 {} 个；上线满 {HIT_AFTER_DAYS} 天的 {} 个中 {hits} 个已有展示。\n\n",
        handed.len(), handed.iter().filter(|(_, e)| e.shipped.is_some()).count(), mature.len()));
    md.push_str("| 词 | 页面 | 派工 | 上线 | 上线天数 | 28 天展示 | 点击 | 平均排名 |\n|---|---|---|---|---|---|---|---|\n");
    for (k, e) in handed {
        let t = row(k);
        md.push_str(&format!("| {} | {} | {} | {} | {} | {} | {} | {} |\n", cell(k), e.page, e.dispatched.as_deref().unwrap_or("—"), e.shipped.as_deref().unwrap_or("未上线"),
            days(&e.shipped).map_or("—".into(), |n| n.to_string()), t.map_or("—".into(), |t| t.impressions.to_string()),
            t.map_or("—".into(), |t| t.clicks.to_string()), t.filter(|t| t.position > 0.0).map_or("—".into(), |t| format!("{:.1}", t.position))));
    }
    md + "\n"
}

/// Both sections for the latest round on disk.
pub fn markdown(state_dir: &str, last_day: Option<&str>, ledger: &Ledger, targets: &[TargetRow], today: NaiveDate) -> String {
    let round: Option<Round> = last_day.and_then(|d| read_json(&format!("{state_dir}/discover/{d}/round.json")).ok()).and_then(|v| serde_json::from_value(v).ok());
    round_markdown(round.as_ref()) + &retro_markdown(ledger, targets, today)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::watch_keywords::Entry;

    #[test]
    fn the_retro_counts_hits_only_after_two_weeks() {
        let mut ledger = Ledger::new();
        ledger.insert("best ai for relationship advice".into(), Entry { page: "/ai-relationship-coach".into(), dispatched: Some("2026-11-04".into()), shipped: Some("2026-11-05".into()), ..Default::default() });
        let t = TargetRow { keyword: "best ai for relationship advice".into(), page: "/ai-relationship-coach".into(), impressions: 12.0, clicks: 1.0, position: 18.4 };
        let md = retro_markdown(&ledger, &[t], NaiveDate::from_ymd_opt(2026, 11, 25).unwrap());
        assert!(md.contains("上线满 14 天的 1 个中 1 个已有展示"));
        assert!(md.contains("| 20 | 12 | 1 | 18.4 |"));
    }

    #[test]
    fn no_round_yet_says_so() {
        assert!(round_markdown(None).contains("还没有跑过"));
    }
}
