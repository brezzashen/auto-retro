//! `mars-skill-tools conversion-analytics watch [--config <file>] [--state <dir>] [--dry-run] [--no-publish] [--sync-dashboard]`:
//! one hourly run. It checks tracking health (24-hour and 2-hour windows), analyses the production
//! funnel, activation and retention, follows every handed-over finding (retro), locates the largest
//! drop, republishes the encrypted page and, for new tracking defects past their
//! cooldown, opens one `limit-resume launch` tab. Conversion findings stay suggestions on the page.

use super::collect::collect;
use super::config::{Finding, WatchState, read_config, read_state};
use super::publish::{dispatch, publish, threshold_item};
use super::{dashboard, funnel, health, hourly, report, retro, sources, visitors};
use crate::skills::seo_competitor::files::{fail, stringify, write_text};
use chrono::{DateTime, FixedOffset, Utc};

const DEFAULT_CONFIG: &str = ".claude/skills/conversion-analytics/references/watch.config.json";

fn arg(argv: &[String], name: &str) -> Option<String> {
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).cloned()
}

fn hours_since(at: &Option<String>, now: DateTime<FixedOffset>) -> f64 {
    at.as_deref().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map_or(f64::INFINITY, |t| (now - t).num_minutes() as f64 / 60.0)
}

/// The tracking defects to hand to one tab: every new P0/P1 once a new P0 is past the health
/// cooldown, or new P1s alone once past the longer fix cooldown.
fn decide(findings: &[Finding], state: &WatchState, cooldowns: (f64, f64), now: DateTime<FixedOffset>) -> Option<Vec<Finding>> {
    let fresh: Vec<Finding> = findings.iter().filter(|f| (f.severity == "P0" || f.severity == "P1") && !state.handled.contains(&f.key)).cloned().collect();
    let since = hours_since(&state.last_fix_at, now);
    let has_p0 = fresh.iter().any(|f| f.severity == "P0");
    ((has_p0 && since >= cooldowns.0) || (!fresh.is_empty() && since >= cooldowns.1)).then_some(fresh)
}

fn severity_rank(f: &Finding) -> u8 {
    f.severity.trim_start_matches('P').parse().unwrap_or(9)
}

/// `alias` of a `limit-resume launch --json` receipt; the page never shows the terminal handle or home path.
fn launched_on(receipt: &str) -> String {
    serde_json::from_str::<serde_json::Value>(receipt).ok().and_then(|v| v["alias"].as_str().map(String::from)).unwrap_or_else(|| "?".into())
}

pub fn main(argv: &[String]) -> i32 {

    match run(argv) { Ok(()) => 0, Err(error) => fail(&error) }
}

fn run(argv: &[String]) -> Result<(), String> {
    let config = read_config(&arg(argv, "--config").unwrap_or_else(|| DEFAULT_CONFIG.into()))?;
    if argv.iter().any(|a| a == "--sync-dashboard") {
        for line in dashboard::sync(&config)? { println!("{line}"); }
        return Ok(());
    }
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let state_dir = arg(argv, "--state").unwrap_or_else(|| format!("{home}/.local/state/conversion-watch"));
    let dry_run = argv.iter().any(|a| a == "--dry-run");
    let now = Utc::now().with_timezone(&FixedOffset::east_opt(8 * 3600).expect("offset"));
    let run_dir = format!("{state_dir}/runs/{}", now.format("%Y-%m-%dT%H%M"));
    std::fs::create_dir_all(&run_dir).map_err(|e| e.to_string())?;
    let state_path = format!("{state_dir}/state.json");
    let mut state = read_state(&state_path);
    let root = std::env::current_dir().map_err(|e| e.to_string())?.to_string_lossy().into_owned();

    let analysis = collect(&config, &root, now);
    let mut findings = health::findings(&analysis.health, config.zero_min_daily);
    findings.extend(hourly::silence_finding(&analysis.hourly, now, config.silence_hours, config.silence_min_hourly)
        .map(|silent| visitors::judge(silent, analysis.visitors.as_ref())));
    findings.extend(analysis.alert_findings.iter().cloned());
    findings.extend(sources::unread_finding(&analysis.errors, analysis.health.relay.as_ref().map(|r| r.events_last_hour)));
    findings.extend(analysis.leak.iter().map(funnel::leak_finding));
    findings.extend(analysis.recovery.iter().flat_map(super::recovery::findings));
    findings.extend(analysis.exceptions.iter().flat_map(super::exceptions::findings));
    let (retro_rows, stale) = retro::reconcile(&mut state, &findings, now, analysis.errors.is_empty(), config.stale_hours);
    findings.extend(stale);
    findings.sort_by_key(severity_rank);

    let page_url = format!("https://reports.example.com/{}/", config.report.slug);
    let decision = decide(&findings, &state, (config.dispatch.health_cooldown_hours, config.dispatch.fix_cooldown_hours), now);
    let dispatch_note = match (&decision, dry_run) {
        (None, _) => format!("没有新的埋点缺陷，或仍在冷却期，不派工；转化发现只作建议{}", match (&state.last_fix_at, &state.last_session) {
            (Some(at), Some(receipt)) => format!("（上次派工：{} 于 {}）", launched_on(receipt), at.get(..16).unwrap_or(at).replace('T', " ")),
            _ => String::new(),
        }),
        (Some(items), true) => format!("（演练）将派「转化追踪修复」标签处理 {} 项", items.len()),
        (Some(items), false) => match dispatch(&page_url, &run_dir, items) {
            Ok(session) => {
                state.handled.extend(items.iter().map(|f| f.key.clone()));
                state.handed_at.extend(items.iter().map(|f| (f.key.clone(), now.to_rfc3339())));
                state.last_fix_at = Some(now.to_rfc3339());
                state.last_session = Some(session.clone());
                format!("已派「转化追踪修复」标签处理 {} 项（账号 {}）", items.len(), launched_on(&session))
            }
            Err(error) => format!("派「转化追踪修复」失败：{error}"),
        },
    };

    let at_sgt = now.format("%Y-%m-%d %H:%M 新加坡时间").to_string();
    write_text(&format!("{run_dir}/report.md"), &report::markdown(&analysis, &findings, &retro_rows, &at_sgt, &dispatch_note, now.date_naive()))?;
    let record = serde_json::json!({"at": now.to_rfc3339(), "dryRun": dry_run, "findings": findings, "dispatch": dispatch_note, "analysis": analysis});
    write_text(&format!("{run_dir}/analysis.json"), &stringify(&record)?)?;
    let count = |s: &str| findings.iter().filter(|f| f.severity == s).count();
    let production = analysis.health.envs.iter().find(|e| e.name == "production");
    let production24h = if analysis.health.volume_error.is_some() { None } else { Some(production.map_or(0.0, |p| p.last24)) };
    let row = serde_json::json!({"at": now.to_rfc3339(), "dryRun": dry_run, "production24h": production24h,
        "p0": count("P0"), "p1": count("P1"), "p2": count("P2"), "visitors": analysis.funnel.iter().map(|d| d.counts[0]).sum::<f64>(),
        "errors": analysis.errors.len(), "dispatch": dispatch_note});
    let history = format!("{state_dir}/history.jsonl");
    let prior = std::fs::read_to_string(&history).unwrap_or_default();
    write_text(&history, &format!("{prior}{}\n", stringify(&row)?))?;
    if !dry_run { write_text(&state_path, &stringify(&state)?)?; }

    if !argv.iter().any(|a| a == "--no-publish") {
        let status = match (count("P0"), count("P1")) {
            (0, 0) if !analysis.errors.is_empty() => format!("本轮 {} 个数据源没有应答，对应部分未知（见发现）；其余部分没有埋点缺陷", analysis.errors.len()),
            (0, 0) => format!("埋点健康正常；漏斗、激活与留存见各节（生产基线 {} 天，只报趋势）", analysis.baseline_days.max(0)),
            (0, n) => format!("{n} 项埋点缺陷（P1），按冷却规则派工修复"),
            (n, _) => format!("{n} 项埋点健康故障（P0），按冷却规则派工修复"),
        };
        publish(&config.report, &state_dir, &run_dir, count("P0") > 0, &at_sgt, &status, threshold_item(&analysis, config.threshold_baseline_days))?;
    }
    println!("{}", stringify(&row)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t: &str) -> DateTime<FixedOffset> { DateTime::parse_from_rfc3339(t).unwrap() }

    #[test]
    fn a_new_p0_dispatches_after_the_short_cooldown_and_p2_never_does() {
        let now = at("2026-11-04T09:23:00+08:00");
        let state = WatchState { last_fix_at: Some("2026-11-04T01:00:00+08:00".into()), ..Default::default() };
        let picked = decide(&[Finding::new("health:relay", "P0", "", ""), Finding::new("health:dup", "P1", "", ""), Finding::new("funnel:leak:visit", "P2", "", "")], &state, (6.0, 24.0), now).unwrap();
        assert_eq!(picked.iter().map(|f| f.key.as_str()).collect::<Vec<_>>(), vec!["health:relay", "health:dup"]);
        assert!(decide(&[Finding::new("health:dup", "P1", "", "")], &state, (6.0, 24.0), now).is_none());
        assert!(decide(&[Finding::new("funnel:leak:visit", "P2", "", "")], &WatchState::default(), (6.0, 24.0), now).is_none());
    }

    #[test]
    fn the_page_names_the_account_but_never_the_terminal_handle() {
        assert_eq!(launched_on(r#"{"alias":"cc5","handle":"term_40da","home":"/Users/x"}"#), "cc5");
        assert_eq!(launched_on("not json"), "?");
    }

    #[test]
    fn a_handled_key_is_never_handed_over_twice() {
        let state = WatchState { handled: vec!["health:relay".into()], ..Default::default() };
        assert!(decide(&[Finding::new("health:relay", "P0", "", "")], &state, (6.0, 24.0), at("2026-11-05T09:23:00+08:00")).is_none());
    }
}
