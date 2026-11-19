//! `--sync-dashboard`: keeps the watchdog's lasting HogQL tables on the production dashboard
//! (2292253), one insight per name, updated in place instead of duplicated.

use super::config::WatchConfig;
use super::onboarding_queries;
use super::posthog::PostHog;
use super::queries;
use reqwest::Method;
use serde_json::{Value, json};

/// The onboarding exit events' first day (`onboardingExit.ts`).
const ONBOARDING_EXIT_SINCE: &str = "2026-11-07";

/// `(name, description, HogQL)` of every watchdog insight.
pub fn insights(since: &str) -> Vec<(&'static str, &'static str, String)> {
    let month = "today() - 30";
    vec![
        ("看门狗 · 每日漏斗（首访队列，24 小时内依次到达）", "每行一天首次访问的生产访客，列为 24 小时内依次走到该步的人数（conversion-watch）", queries::funnel(since, month)),
        ("看门狗 · 注册后 24 小时激活（按注册日）", "founder 定义：24 小时内首条消息 + 首位联系人（chat/manual），服务端 user_signed_up 按人去重", queries::activation(since)),
        ("看门狗 · D1/D7/D30 留存（注册队列）", "第 1/7/30 天当天有 $pageview 或 message_sent 的人数", queries::retention(since)),
        ("看门狗 · 追踪健康：生产每日事件数（uuid 去重）", "每个事件每天的到达量；某行突然消失即埋点归零", queries::daily_events()),
        ("看门狗 · 登录失败原因（30 天）", "auth_failed 按 step 与 error_code", queries::signin_failures(month)),
        ("看门狗 · 新注册首次来源", "追踪开始后的 user_signed_up 按 $initial_utm_source 与 $initial_referring_domain", queries::signup_sources(since)),
        ("引导 · 每日完成、跳过与中途离开", "手机引导线（Web 与 iOS）：看过任一步的人、走完的人、通过跳过问卷退出的人、中途离开的人，以及走完用时中位数（秒）", onboarding_queries::daily(ONBOARDING_EXIT_SINCE)),
        ("引导 · 跳过原因（30 天）", "onboarding_skipped 的 reason 与打开跳过问卷时所在的步骤，含跳过前用时中位数（秒）", onboarding_queries::skip_reasons(month)),
        ("引导 · 中途离开且未完成的步骤（30 天）", "onboarding_abandoned 中 30 天内没有 onboarding_completed 的人：离开时的步骤、方式（page_hide/background/left_screen）与用时中位数（秒）", onboarding_queries::drop_steps(month)),
    ]
}

/// Creates or updates each insight on the dashboard; returns one line per insight.
pub fn sync(config: &WatchConfig) -> Result<Vec<String>, String> {
    let ph = PostHog::from_keychain(&config.host, config.project, &config.keychain_service)?;
    let board = ph.api(Method::GET, &format!("dashboards/{}/", config.dashboard), None)?;
    let existing: Vec<(String, u64)> = board["tiles"].as_array().into_iter().flatten()
        .filter_map(|t| Some((t["insight"]["name"].as_str()?.to_string(), t["insight"]["id"].as_u64()?))).collect();
    let mut lines = Vec::new();
    for (name, description, hogql) in insights(&config.tracking_start) {
        let query = json!({ "kind": "DataVisualizationNode", "source": { "kind": "HogQLQuery", "query": hogql }, "display": "ActionsTable" });
        let body = json!({ "name": name, "description": description, "query": query, "dashboards": [config.dashboard] });
        let answer: Value = match existing.iter().find(|(n, _)| n == name) {
            Some((_, id)) => ph.api(Method::PATCH, &format!("insights/{id}/"), Some(&body))?,
            None => ph.api(Method::POST, "insights/", Some(&body))?,
        };
        lines.push(format!("{} {} ({})", if existing.iter().any(|(n, _)| n == name) { "updated" } else { "created" }, name, answer["short_id"].as_str().unwrap_or("?")));
    }
    Ok(lines)
}
