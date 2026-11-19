//! Conversion alert thresholds (founder 2026-11-03: the proposal, on by itself once the production
//! baseline has `threshold_baseline_days` complete days). Before that every rule shows its value and
//! `未启用`; afterwards a breached rule is a P1 `alert:*` finding for the tracking-fix tab to triage.

use super::config::{Alerts, Finding};
use super::posthog::{num, text};
use chrono::{Duration, NaiveDate};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AlertRow {
    pub name: String,
    pub rule: String,
    pub day: String,
    pub value: String,
    pub baseline: String,
    pub status: String,
}

pub struct Window {
    pub today: NaiveDate,
    /// First complete production day.
    pub first_day: NaiveDate,
    /// Complete days the baseline needs before rules fire (also the baseline length).
    pub days: usize,
}

fn by_day(rows: &[Vec<Value>]) -> BTreeMap<String, Vec<f64>> {
    rows.iter().map(|r| (text(r, 0), (1..r.len()).map(|i| num(r, i)).collect())).collect()
}

fn pct(rate: f64) -> String {
    if rate.is_finite() { format!("{:.0}%", rate * 100.0) } else { "—".into() }
}

fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() { return f64::NAN; }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    if values.len() % 2 == 0 { (values[mid - 1] + values[mid]) / 2.0 } else { values[mid] }
}

/// Every rule's row for the page and the findings of the breached ones.
pub fn evaluate(rates: &[Vec<Value>], activation: &[Vec<Value>], cfg: &Alerts, w: &Window) -> (Vec<AlertRow>, Vec<Finding>) {
    let yesterday = w.today - Duration::days(1);
    let complete = (yesterday - w.first_day).num_days().max(0) as usize;
    let enabled = complete >= w.days;
    let prior: Vec<String> = (1..=w.days as i64).map(|n| yesterday - Duration::days(n)).filter(|d| *d >= w.first_day).map(|d| d.format("%Y-%m-%d").to_string()).collect();
    let (rates, signups) = (by_day(rates), by_day(activation));
    let day = yesterday.format("%Y-%m-%d").to_string();
    let get = |map: &BTreeMap<String, Vec<f64>>, d: &str, i: usize| map.get(d).and_then(|v| v.get(i).copied()).unwrap_or(0.0);
    let sum = |map: &BTreeMap<String, Vec<f64>>, days: &[String], i: usize| days.iter().map(|d| get(map, d, i)).sum::<f64>();
    let off = format!("未启用（完整基线 {complete}/{} 天，{} 起判定）", w.days, (w.first_day + Duration::days(w.days as i64 + 1)).format("%Y-%m-%d"));
    let mut rows = Vec::new();
    let mut findings = Vec::new();
    let mut judge = |name: &str, rule: String, day: &str, value: String, baseline: String, sample_ok: bool, breached: bool, title: String| {
        let status = if !enabled { off.clone() } else if !sample_ok { "样本不足".into() } else if breached { "告警".into() } else { "正常".into() };
        if enabled && sample_ok && breached {
            findings.push(Finding::new(format!("alert:{name}"), "P1", title, "先排查登录、支付或埋点是否故障（HogQL 原始结果 + 生产日志）；埋点缺陷按授权修复，产品转化原因只写建议"));
        }
        rows.push(AlertRow { name: name.into(), rule, day: day.into(), value, baseline, status });
    };

    let today_signups = get(&signups, &day, 0);
    let base_signups = median(prior.iter().map(|d| get(&signups, d, 0)).collect());
    judge("signups", format!("每日注册低于前 {} 天中位数的 {}", w.days, pct(cfg.signup_median_ratio)), &day, format!("{today_signups}"), format!("中位数 {base_signups}"),
        !prior.is_empty(), today_signups < base_signups * cfg.signup_median_ratio, format!("{day} 注册 {today_signups}，低于前 {} 天中位数 {base_signups} 的一半", prior.len()));

    let (sent, verified) = (get(&rates, &day, 0), get(&rates, &day, 1));
    let base_otp = sum(&rates, &prior, 1) / sum(&rates, &prior, 0);
    judge("otp", format!("验证码成功率比基线低 {:.0} 个百分点（发码 ≥ {}）", cfg.otp_drop_points * 100.0, cfg.otp_min_sent), &day, format!("{verified}/{sent} = {}", pct(verified / sent)), pct(base_otp),
        sent >= cfg.otp_min_sent && base_otp.is_finite(), verified / sent < base_otp - cfg.otp_drop_points, format!("{day} 验证码成功率 {}，基线 {}", pct(verified / sent), pct(base_otp)));

    let (started, paid) = (get(&rates, &day, 2), get(&rates, &day, 3));
    let base_checkout = sum(&rates, &prior, 3) / sum(&rates, &prior, 2);
    judge("checkout", format!("结账成功率低于基线的 {}（结账 ≥ {}）", pct(cfg.checkout_ratio), cfg.checkout_min_started), &day, format!("{paid}/{started} = {}", pct(paid / started)), pct(base_checkout),
        started >= cfg.checkout_min_started && base_checkout.is_finite(), paid / started < base_checkout * cfg.checkout_ratio, format!("{day} 结账成功率 {}，基线 {}", pct(paid / started), pct(base_checkout)));

    let matured: Vec<&Vec<Value>> = activation.iter().filter(|r| num(r, 5) > 0.0).collect();
    let (last, earlier) = matured.split_last().map_or((None, &[][..]), |(l, e)| (Some(*l), e));
    let base: Vec<&&Vec<Value>> = earlier.iter().rev().take(w.days).collect();
    let base_rate = base.iter().map(|r| num(r, 4)).sum::<f64>() / base.iter().map(|r| num(r, 1)).sum::<f64>();
    let (cohort, size, active) = last.map_or((String::from("—"), 0.0, 0.0), |r| (text(r, 0), num(r, 1), num(r, 4)));
    judge("activation", format!("24 小时激活率低于基线的 {}（当日注册 ≥ {}）", pct(cfg.activation_ratio), cfg.activation_min_signups), &cohort, format!("{active}/{size} = {}", pct(active / size)), pct(base_rate),
        size >= cfg.activation_min_signups && base_rate.is_finite(), active / size < base_rate * cfg.activation_ratio, format!("{cohort} 注册队列 24 小时激活率 {}，基线 {}", pct(active / size), pct(base_rate)));
    (rows, findings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg() -> Alerts {
        Alerts { decided: String::new(), signup_median_ratio: 0.5, otp_drop_points: 0.15, otp_min_sent: 20.0, checkout_ratio: 0.5, checkout_min_started: 5.0, activation_ratio: 0.5, activation_min_signups: 10.0 }
    }
    fn day(n: i64) -> String { (NaiveDate::from_ymd_opt(2026, 11, 3).unwrap() + Duration::days(n)).format("%Y-%m-%d").to_string() }
    fn signups(last: f64) -> Vec<Vec<Value>> {
        (0..15).map(|n| vec![json!(day(n)), json!(if n == 14 { last } else { 10.0 }), json!(0), json!(0), json!(0), json!(true)]).collect()
    }
    fn window(today: NaiveDate) -> Window { Window { today, first_day: NaiveDate::from_ymd_opt(2026, 11, 3).unwrap(), days: 14 } }

    #[test]
    fn rules_stay_off_until_the_baseline_is_complete() {
        let (rows, findings) = evaluate(&[], &signups(2.0), &cfg(), &window(NaiveDate::from_ymd_opt(2026, 11, 10).unwrap()));
        assert!(findings.is_empty());
        assert!(rows.iter().all(|r| r.status.starts_with("未启用") && r.status.contains("2026-11-18")));
    }

    #[test]
    fn a_halved_signup_day_alerts_once_enabled_and_thin_samples_do_not() {
        let (rows, findings) = evaluate(&[], &signups(2.0), &cfg(), &window(NaiveDate::from_ymd_opt(2026, 11, 18).unwrap()));
        assert_eq!(findings.iter().map(|f| f.key.as_str()).collect::<Vec<_>>(), vec!["alert:signups"]);
        assert_eq!(rows.iter().find(|r| r.name == "otp").unwrap().status, "样本不足");
        assert_eq!(rows.iter().find(|r| r.name == "activation").unwrap().status, "样本不足");
    }
}
