//! Production `$exception` events from posthog-js exception autocapture (project setting
//! `autocapture_exceptions_opt_in`, on since 2026-11-15 by founder decision, report
//! posthog-replay-fix-20261115): the 24-hour count against the prior days' mean and the most frequent
//! errors, so a script error on a visitor's first screen shows up here and on its replay timeline.

use super::config::Finding;
use super::posthog::{num, PostHog};
use chrono::{DateTime, FixedOffset, NaiveDateTime};
use serde::Serialize;
use serde_json::Value;

/// Singapore time exception autocapture was switched on; earlier windows hold no `$exception` at all.
const CAPTURE_SINCE: &str = "2026-11-15 13:40:00";
/// Full prior days needed before a spike is judged.
const BASELINE_DAYS: f64 = 3.0;
/// A spike: at least this many errors in 24 h and `SPIKE_RATIO` times the prior daily mean.
const SPIKE_MIN: f64 = 20.0;
const SPIKE_RATIO: f64 = 3.0;

const COUNTS: &str = "select uniqIf(uuid, timestamp > now() - interval 1 day) last24, uniqIf(uuid, timestamp <= now() - interval 1 day) prior, \
uniqIf(person_id, timestamp > now() - interval 1 day) people24 from events where event = '$exception' and properties.mars_env = 'production' \
and timestamp > now() - interval 8 day";
/// posthog-js 1.438 sends the error as `$exception_list` (no `$exception_types`/`$exception_values`); verified on beta
/// 2026-11-15 with a labelled probe. `issue` opens it in PostHog error tracking.
pub const TOP: &str = "select substring(JSONExtractString(toString(properties.$exception_list), 1, 'type'), 1, 60) kind, \
substring(JSONExtractString(toString(properties.$exception_list), 1, 'value'), 1, 120) message, uniq(uuid) n, uniq(person_id) people, \
any(cutQueryString(properties.$current_url)) page, any(properties.$exception_issue_id) issue from events where event = '$exception' \
and properties.mars_env = 'production' and timestamp > now() - interval 1 day group by kind, message order by n desc limit 8";

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionWatch {
    pub last24: f64,
    pub people24: f64,
    /// Mean per 24 h over the prior days that had capture on (at most 7); `None` before `BASELINE_DAYS`.
    pub prior_mean: Option<f64>,
    /// `[kind, message, count, people, page, issue id]`, most frequent first.
    pub top: Vec<Vec<Value>>,
}

/// Full days of capture before the last 24 h, capped at the 7 the count query reads.
fn prior_days(now: DateTime<FixedOffset>) -> f64 {
    let since = NaiveDateTime::parse_from_str(CAPTURE_SINCE, "%Y-%m-%d %H:%M:%S").expect("constant");
    let hours = (now.naive_local() - since).num_minutes() as f64 / 60.0;
    ((hours - 24.0) / 24.0).floor().clamp(0.0, 7.0)
}

pub fn collect(ph: &PostHog, now: DateTime<FixedOffset>, errors: &mut Vec<String>) -> Option<ExceptionWatch> {
    let counts = match ph.query(COUNTS) {
        Ok(rows) => rows.into_iter().next().unwrap_or_default(),
        Err(error) => { errors.push(format!("exceptions: {error}")); return None; }
    };
    let cell = |i: usize| num(&counts, i);
    let days = prior_days(now);
    let top = ph.query(TOP).unwrap_or_else(|error| { errors.push(format!("exceptions top: {error}")); Vec::new() });
    Some(ExceptionWatch { last24: cell(0), people24: cell(2), prior_mean: (days >= BASELINE_DAYS).then(|| cell(1) / days), top })
}

pub fn findings(watch: &ExceptionWatch) -> Vec<Finding> {
    let Some(mean) = watch.prior_mean else { return Vec::new() };
    if watch.last24 < SPIKE_MIN || watch.last24 < SPIKE_RATIO * mean {
        return Vec::new();
    }
    vec![Finding::new("health:exceptions-spike", "P1",
        format!("生产 24 小时脚本错误 {} 次（{} 人），是之前日均 {:.1} 次的 {:.1} 倍", watch.last24, watch.people24, mean, watch.last24 / mean.max(1.0)),
        "看看门狗页「脚本错误」表的首条错误和页面，在 PostHog 打开该错误所在会话的回放；多半是最近一次 example.com 发布引入的")]
}

pub fn section(watch: Option<&ExceptionWatch>) -> String {
    let Some(w) = watch else { return "## 脚本错误（$exception）\n\n未读到（PostHog 查询失败）。\n\n".into() };
    let mean = w.prior_mean.map_or("基线不足 3 天".to_string(), |m| format!("之前日均 {m:.1}"));
    let mut md = format!("## 脚本错误（$exception）\n\n生产 24 小时 {} 次，{} 人；{mean}。\n\n| 类型 | 信息 | 次数 | 人数 | 页面 | PostHog 错误 |\n|---|---|---|---|---|---|\n", w.last24, w.people24);
    if w.top.is_empty() { md.push_str("| 无 | | | | | |\n"); }
    for row in &w.top {
        let mut cells: Vec<String> = row.iter().map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string).replace('|', "/").replace('\n', " ")).collect();
        if let Some(issue) = cells.get_mut(5).filter(|id| !id.is_empty() && id.as_str() != "null") {
            *issue = format!("[打开](https://us.posthog.com/project/704719/error_tracking/{issue})");
        }
        md.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    md.push('\n');
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(text).unwrap()
    }

    #[test]
    fn no_spike_is_judged_before_three_prior_days() {
        assert_eq!(prior_days(at("2026-11-16T14:00:00+08:00")), 0.0);
        assert_eq!(prior_days(at("2026-11-19T14:00:00+08:00")), 3.0);
        assert_eq!(prior_days(at("2026-12-30T14:00:00+08:00")), 7.0);
        assert!(findings(&ExceptionWatch { last24: 500.0, prior_mean: None, ..Default::default() }).is_empty());
    }

    #[test]
    fn a_threefold_jump_over_twenty_is_p1() {
        let w = |last24, mean| ExceptionWatch { last24, prior_mean: Some(mean), ..Default::default() };
        assert_eq!(findings(&w(30.0, 5.0)).into_iter().map(|f| f.severity).collect::<Vec<_>>(), vec!["P1".to_string()]);
        assert!(findings(&w(19.0, 1.0)).is_empty());
        assert!(findings(&w(40.0, 20.0)).is_empty());
        assert!(section(Some(&w(0.0, 0.0))).contains("| 无 |"));
        let row = vec![Value::from("Error"), Value::from("boom"), Value::from(2), Value::from(1), Value::from("https://example.com/intro"), Value::from("01a1")];
        let page = section(Some(&ExceptionWatch { top: vec![row], ..Default::default() }));
        assert!(page.contains("| Error | boom | 2 | 1 | https://example.com/intro | [打开](https://us.posthog.com/project/704719/error_tracking/01a1) |"));
    }
}
