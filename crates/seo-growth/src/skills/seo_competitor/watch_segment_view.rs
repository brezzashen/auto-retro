//! The visitor-type distribution block that opens the seo-watch traffic section (founder 2026-11-05: a clear
//! chart before the visitor tables showing how many of each user type came; then "加入 icon 和筛选 以及不同颜色，
//! 图例对应颜色区分"). It sums `watch_segments` rows over sources for the last day and the last hour, in a fixed
//! type order, and emits one report `categories` block: per-type colour and icon on the legend chips, the bars
//! and the table rows, with group and per-type filters.

use super::watch_segments::SegmentRow;
use serde_json::{json, Value};

/// Every segment label `watch_segments::SEGMENT` can produce, in reading order: anonymous first, then accounts.
pub const TYPES: [&str; 8] = ["新访客", "回访访客", "新注册", "老用户·付费", "老用户·赠送", "老用户·曾付费", "老用户·免费", "已登录·未同步"];

/// `(icon, semantic tone, filter group)` of each entry of `TYPES`; red and green stay free for verdicts.
const LOOKS: [(&str, &str, &str); 8] = [
    ("user-plus", "blue", "anon"), ("repeat", "cyan", "anon"), ("user-check", "teal", "signup"), ("crown", "yellow", "existing"),
    ("gift", "pink", "existing"), ("history", "bronze", "existing"), ("user", "indigo", "existing"), ("unlink", "gray", "existing"),
];

/// Visitors, CTA clickers, sign-ups and senders of one type in one window.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Totals {
    pub visitors: f64,
    pub cta: f64,
    pub signups: f64,
    pub senders: f64,
}

/// Per-type totals of one current window (offset 0), in `TYPES` order.
pub fn totals(rows: &[SegmentRow], hours: u32) -> Vec<Totals> {
    TYPES.iter().map(|ty| {
        rows.iter().filter(|r| r.window_hours == hours && r.offset_hours == 0 && r.segment == *ty).fold(Totals::default(), |a, r| Totals {
            visitors: a.visitors + r.visitors, cta: a.cta + r.cta, signups: a.signups + r.signups, senders: a.senders + r.senders,
        })
    }).collect()
}

fn share(part: f64, whole: f64) -> String {
    if whole > 0.0 { format!("{:.0}%", part / whole * 100.0) } else { "—".into() }
}

/// The `categories` JSON of both windows: series last day and last hour, then share, CTA, sign-ups and senders.
pub fn categories(day: &[Totals], hour: &[Totals]) -> Value {
    let day_sum = day.iter().map(|t| t.visitors).sum::<f64>();
    let items: Vec<Value> = TYPES.iter().zip(LOOKS).zip(day.iter().zip(hour)).map(|((ty, (icon, tone, group)), (d, h))| json!({
        "label": ty, "icon": icon, "tone": tone, "group": group,
        "values": [d.visitors, h.visitors], "cells": [share(d.visitors, day_sum), d.cta, d.signups, d.senders],
    })).collect();
    let sum = |f: fn(&Totals) -> f64, w: &[Totals]| w.iter().map(f).sum::<f64>();
    json!({
        "label": "访客类型", "series": ["近 24 小时", "近 1 小时"], "columns": ["占比", "点 CTA", "注册", "发消息"],
        "groups": [{"id": "anon", "label": "匿名访客"}, {"id": "signup", "label": "新注册"}, {"id": "existing", "label": "老用户"}],
        "items": items,
        "total": {"label": "合计", "values": [day_sum, sum(|t| t.visitors, hour)], "cells": ["100%", sum(|t| t.cta, day), sum(|t| t.signups, day), sum(|t| t.senders, day)]},
    })
}

/// `+ 0.0` turns the `-0.0` that an empty float sum yields into `0`, so an empty window prints "0 人", not "-0 人".
fn visitors_in(rows: &[SegmentRow], hours: u32, offset: u32) -> f64 {
    rows.iter().filter(|r| r.window_hours == hours && r.offset_hours == offset).map(|r| r.visitors).sum::<f64>() + 0.0
}

/// The comparison line: whole-hour windows ending at this run's hour against the previous hour, the same
/// hour yesterday and the previous day.
fn comparison(rows: &[SegmentRow]) -> String {
    format!("对比（整点窗口，截至本次整点）：近 1 小时 {} 人，前一小时 {} 人，昨天同一小时 {} 人；近 24 小时 {} 人，前 24 小时 {} 人。\n\n",
        visitors_in(rows, 1, 0), visitors_in(rows, 1, 1), visitors_in(rows, 1, 24), visitors_in(rows, 24, 0), visitors_in(rows, 24, 24))
}

/// One `##` section: the summary line, the comparison line, then the `categories` block (legend chips, bars and the type table).
pub fn section(rows: &[SegmentRow], error: Option<&str>) -> String {
    let mut md = String::from("## 访客类型分布（近 24 小时 / 近 1 小时）\n\n");
    if let Some(e) = error { return md + &format!("访客分类查询失败：{e}\n\n"); }
    let (day, hour) = (totals(rows, 24), totals(rows, 1));
    let (day_sum, hour_sum) = (day.iter().map(|t| t.visitors).sum::<f64>(), hour.iter().map(|t| t.visitors).sum::<f64>());
    if day_sum == 0.0 { return md + "近 24 小时没有可分类的真实访客。\n\n"; }
    let parts: Vec<String> = TYPES.iter().zip(&day).filter(|(_, t)| t.visitors > 0.0).map(|(ty, t)| format!("{ty} {}", t.visitors)).collect();
    md.push_str(&format!("近 24 小时真实访客 {day_sum} 人：{}；近 1 小时 {hour_sum} 人。老用户按 analytics-sync 写入的 customer_type 分付费、赠送、曾付费、免费。\n\n", parts.join("、")));
    md.push_str(&comparison(rows));
    let block = serde_json::to_string_pretty(&categories(&day, &hour)).unwrap_or_default();
    md + &format!("### 各类访客人数（近 24 小时 / 近 1 小时）\n```categories\n{block}\n```\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(hours: u32, source: &str, segment: &str, visitors: f64) -> SegmentRow {
        SegmentRow { window_hours: hours, source: source.into(), segment: segment.into(), visitors, cta: 1.0, ..Default::default() }
    }

    #[test]
    fn sums_sources_per_type_and_emits_one_coloured_block_in_fixed_order() {
        let rows = [row(24, "t.co", "新访客", 17.0), row(24, "$direct", "新访客", 19.0), row(24, "$direct", "老用户·付费", 3.0), row(1, "$direct", "新访客", 1.0)];
        let day = totals(&rows, 24);
        assert_eq!(day[0].visitors, 36.0);
        assert_eq!(day[0].cta, 2.0);
        assert_eq!(day[3].visitors, 3.0);
        let md = section(&rows, None);
        assert!(md.starts_with("## 访客类型分布") && md.matches("\n## ").count() == 0, "one section holds chart and table");
        assert!(md.contains("近 24 小时真实访客 39 人：新访客 36、老用户·付费 3；近 1 小时 1 人"));
        let fence = md.find("```categories\n").unwrap();
        assert!(md[..fence].trim_end().ends_with("）"), "a ### line right above the fence titles the block");
        let json: Value = serde_json::from_str(md[fence + 14..].split("\n```").next().unwrap()).unwrap();
        let labels: Vec<&str> = json["items"].as_array().unwrap().iter().map(|i| i["label"].as_str().unwrap()).collect();
        assert_eq!(labels, TYPES);
        assert_eq!(json["items"][3], json!({"label": "老用户·付费", "icon": "crown", "tone": "yellow", "group": "existing", "values": [3.0, 0.0], "cells": ["8%", 1.0, 0.0, 0.0]}));
        assert_eq!(json["total"]["values"], json!([39.0, 1.0]));
        let report = json!({"title": "访客", "categories": json});
        assert!(report.is_object(), "the report build produces a JSON object");
    }

    #[test]
    fn earlier_windows_feed_the_comparison_line_not_the_chart() {
        let earlier = |hours, offset, visitors| SegmentRow { offset_hours: offset, ..row(hours, "$direct", "新访客", visitors) };
        let rows = [row(24, "t.co", "新访客", 5.0), row(1, "t.co", "新访客", 1.0), earlier(1, 1, 2.0), earlier(1, 24, 3.0), earlier(24, 24, 40.0)];
        assert_eq!(totals(&rows, 24)[0].visitors, 5.0);
        assert_eq!(totals(&rows, 1)[0].visitors, 1.0);
        assert!(section(&rows, None).contains("近 1 小时 1 人，前一小时 2 人，昨天同一小时 3 人；近 24 小时 5 人，前 24 小时 40 人。"));
        let no_yesterday = [row(24, "t.co", "新访客", 5.0), row(1, "t.co", "新访客", 1.0)];
        assert!(section(&no_yesterday, None).contains("昨天同一小时 0 人"), "an empty window prints 0, not -0");
    }

    #[test]
    fn reports_a_failed_or_empty_query_instead_of_an_empty_chart() {
        assert!(section(&[], Some("timeout")).contains("访客分类查询失败：timeout"));
        assert!(section(&[], None).contains("没有可分类的真实访客"));
    }
}
