//! The run's unanswered sources as one P2: a source that did not answer leaves its part of the
//! analysis unknown, which the page must say instead of reading it as zero (2026-11-13 13:27: a
//! spent PostHog query budget emptied the volume query and raised a false "production silent" P0).

use super::config::Finding;

/// Source label of an `errors` line (`"<label>: <error>"`).
fn label(error: &str) -> &str {
    error.split_once(':').map_or(error, |(l, _)| l.trim())
}

/// `None` when every source answered. `events_last_hour` is the hourly query's production events in
/// the current and previous Singapore hour, quoted only when that query itself answered.
pub fn unread_finding(errors: &[String], events_last_hour: Option<f64>) -> Option<Finding> {
    if errors.is_empty() { return None; }
    let mut labels: Vec<&str> = Vec::new();
    for e in errors { if !labels.contains(&label(e)) { labels.push(label(e)); } }
    let hourly_ok = !labels.contains(&"hourly");
    let mut title = format!("本轮 {} 个数据源没有应答（{}），对应部分本轮未知，不按 0 判断", labels.len(), labels.join("、"));
    if labels.contains(&"volume") {
        title.push_str("；生产是否静默、各事件是否归零本轮无法判断");
        if let (true, Some(n)) = (hourly_ok, events_last_hour) { title.push_str(&format!("（小时查询仍看到最近两小时 {n} 个生产事件）")); }
    }
    let budget = errors.iter().any(|e| e.contains("HTTP 429"));
    let action = if budget {
        "PostHog 项目每小时的查询读取配额被用完（所有会话共用一个配额），下一轮自动重读；连续出现时，减少各会话对长时间范围的查询"
    } else {
        "看错误原文：凭证或网络问题先恢复（GKE 用 gcp-reauth），下一轮自动重读；连续出现再修看门狗"
    };
    Some(Finding::new("watch:sources-unread", "P2", title, action))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_answering_is_no_finding() {
        assert_eq!(unread_finding(&[], Some(9.0)), None);
    }

    #[test]
    fn a_spent_budget_names_the_unread_volume_and_the_hourly_count() {
        let errors = ["volume: PostHog query/: HTTP 429 budget", "funnel: PostHog query/: HTTP 429 budget", "CronJob: kubectl: Unable to connect"].map(String::from);
        let f = unread_finding(&errors, Some(157.0)).expect("finding");
        assert_eq!((f.key.as_str(), f.severity.as_str()), ("watch:sources-unread", "P2"));
        assert!(f.title.contains("3 个数据源") && f.title.contains("volume、funnel、CronJob"));
        assert!(f.title.contains("157 个生产事件"));
        assert!(f.action.contains("配额"));
    }

    #[test]
    fn the_hourly_count_is_not_quoted_when_that_query_failed_too() {
        let errors = ["volume: x", "hourly: y"].map(String::from);
        let f = unread_finding(&errors, Some(0.0)).expect("finding");
        assert!(!f.title.contains("小时查询仍看到"));
        assert!(f.action.contains("gcp-reauth"));
    }
}
