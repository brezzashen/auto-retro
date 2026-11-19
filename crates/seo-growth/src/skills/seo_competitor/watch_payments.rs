//! Production `order_paid` and `checkout_started` over the last 24 whole hours, with the buyer's account
//! creation date and `customer_type`. `order_paid` carries no store environment, so an Apple Sandbox order
//! reads as production here; only `crm.payment_order.environment` tells them apart.

use super::watch_segments::END;
use crate::skills::conversion_analytics::posthog::{num, text, PostHog};
use serde::Serialize;

pub const SECTION: &str = "付费与订单（近 24 小时）";

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct PaymentRow {
    /// Singapore time, `MM-DD HH:MM`.
    pub at: String,
    /// `order_paid` (server truth) or `checkout_started` (web or iOS).
    pub event: String,
    /// `gateway` on `order_paid`, `method` on `checkout_started`.
    pub gateway: String,
    pub plan: String,
    pub order_type: String,
    pub amount: f64,
    pub currency: String,
    /// `order_paid.first_order`: the account's first paid order.
    pub first_order: String,
    /// Person property set by `analytics-sync` after the order (`paying`, `granted`, `lapsed`, `free`).
    pub customer_type: String,
    /// Account creation date (Singapore); empty for a visitor who never signed in.
    pub account_created: String,
    /// First 8 characters of the PostHog person id, to join rows of one buyer.
    pub person: String,
}

pub fn sql(end: &str) -> String {
    format!(
        "select formatDateTime(toTimeZone(e.timestamp, 'Asia/Singapore'), '%m-%d %H:%i'), e.event, \
         coalesce(toString(e.properties.gateway), toString(e.properties.method), ''), coalesce(toString(e.properties.plan), ''), \
         coalesce(toString(e.properties.order_type), ''), toFloat(coalesce(e.properties.amount, e.properties.value, 0)), \
         coalesce(toString(e.properties.currency), ''), coalesce(toString(e.properties.first_order), ''), coalesce(pp.ct, ''), \
         coalesce(pp.created, ''), substring(toString(e.person_id), 1, 8) \
         from events e left join (select id, toString(properties.customer_type) ct, \
          formatDateTime(toTimeZone(properties.account_created_at, 'Asia/Singapore'), '%Y-%m-%d') created from persons) pp on pp.id = e.person_id \
         where e.event in ('order_paid', 'checkout_started') and e.properties.mars_env = 'production' \
         and e.timestamp >= {end} - interval 24 hour and e.timestamp < {end} order by e.timestamp limit 100"
    )
}

pub fn collect(ph: &PostHog) -> Result<Vec<PaymentRow>, String> {
    Ok(ph.query(&sql(END))?.iter().map(|r| PaymentRow {
        at: text(r, 0), event: text(r, 1), gateway: text(r, 2), plan: text(r, 3), order_type: text(r, 4),
        amount: num(r, 5), currency: text(r, 6), first_order: text(r, 7), customer_type: text(r, 8),
        account_created: text(r, 9), person: text(r, 10),
    }).collect())
}

/// The page section: one line of totals, then every row.
pub fn section(rows: &[PaymentRow], error: Option<&str>) -> String {
    let mut md = format!("## {SECTION}\n\n");
    if let Some(error) = error { return md + &format!("PostHog 本次没有返回付费数据：{error}\n\n"); }
    let paid: Vec<&PaymentRow> = rows.iter().filter(|r| r.event == "order_paid").collect();
    let buyers = |pred: &dyn Fn(&PaymentRow) -> bool| { let mut p: Vec<&str> = paid.iter().filter(|r| pred(r)).map(|r| r.person.as_str()).collect(); p.sort(); p.dedup(); p.len() };
    md.push_str(&format!("付费订单 {} 笔，首单 {} 人、再次付费 {} 人；发起结账 {} 次。PostHog 的 order_paid 不区分 Apple Sandbox 测试单，复盘按 crm.payment_order 核对。\n\n",
        paid.len(), buyers(&|r| r.first_order == "true"), buyers(&|r| r.first_order != "true"), rows.len() - paid.len()));
    if rows.is_empty() { return md; }
    md.push_str("| 时间 | 事件 | 渠道 | 套餐 | 类型 | 金额 | 首单 | 用户类型 | 注册日 | 用户 |\n|---|---|---|---|---|---|---|---|---|---|\n");
    for r in rows {
        md.push_str(&format!("| {} | {} | {} | {} | {} | {} {} | {} | {} | {} | {} |\n", r.at, r.event, r.gateway, r.plan, r.order_type,
            r.amount, r.currency, r.first_order, r.customer_type, r.account_created, r.person));
    }
    md + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paid(person: &str, first: &str) -> PaymentRow {
        PaymentRow { event: "order_paid".into(), first_order: first.into(), person: person.into(), amount: 139.0, currency: "CNY".into(), ..Default::default() }
    }

    #[test]
    fn the_query_reads_production_orders_and_checkouts_in_the_fixed_window() {
        let sql = sql("E");
        assert!(sql.contains("e.event in ('order_paid', 'checkout_started')") && sql.contains("mars_env = 'production'"));
        assert!(sql.contains("E - interval 24 hour") && sql.contains("< E") && sql.contains("from persons"));
    }

    #[test]
    fn the_section_counts_first_and_returning_buyers_once_each() {
        let checkout = PaymentRow { event: "checkout_started".into(), person: "a".into(), ..Default::default() };
        let md = section(&[checkout, paid("a", "true"), paid("b", "false"), paid("b", "false")], None);
        assert!(md.starts_with("## 付费与订单") && md.contains("付费订单 3 笔，首单 1 人、再次付费 1 人；发起结账 1 次"));
        assert!(md.contains("| 139 CNY |") && md.contains("Sandbox"));
        assert!(section(&[], Some("HTTP 504")).contains("HTTP 504"));
    }
}
