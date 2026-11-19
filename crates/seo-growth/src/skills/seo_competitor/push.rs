//! `mars-skill-tools seo-competitor push [--config <file>] [--state <dir>] [--dry-run] [--no-publish]`:
//! one run of the search-engine push watchdog (launchd `local.autoretro.seo-push`). It pushes every sitemap
//! URL an engine has not accepted yet to Baidu (within its daily quota), IndexNow and Bing URL Submission
//! (within its daily quota), checks the files
//! that keep each site verified, republishes the report page, and opens a repair tab for a new failure
//! or a tab to submit the Baidu sitemap once Baidu accepts pushes.

use super::files::{fail, stringify, write_text};
use super::push_config::{PushState, read_config, read_state, read_token};
use super::push_engines::client;
use super::push_report::{markdown, repair_prompt, sitemap_prompt};
use super::push_site::{Keys, Run, SiteRound, round};
use super::watch_publish::{PageText, launch, publish};
use chrono::{DateTime, FixedOffset, Utc};

const DEFAULT_CONFIG: &str = ".claude/skills/seo-competitor/references/push.config.json";
const SITEMAP_MARKER: &str = "baidu-sitemap-submitted";

const PUSH_TEXT: PageText = PageText {
    goal: "把 example.cn 与 example.com 的 sitemap 地址主动推送给百度、IndexNow（Bing、Yandex 等）与 Bing URL 提交，并守住站点验证文件",
    case_title: "本次推送",
    shot_waiver: "证据是百度推送 API 与 IndexNow 的原始应答，保存在运行目录 analysis.json",
    evidence_waiver: "数据来自百度 data.zz.baidu.com 推送接口、api.indexnow.org、Bing Webmaster API 与对 sitemap 和验证文件的 HTTP 请求",
    limits: &["百度新站每日推送配额很小，剩余地址次日继续", "Bing URL 提交每站每天 100 个（与站长后台手动提交共用），剩余地址次日继续", "百度 sitemap 只能在网页后台提交，由看门狗派的标签完成"],
    section_waivers: &[("推送状态", "逐站状态清单：地址数与接收数已画在「推送进度」图，这里的数字是 sitemap 总数与验证文件的 HTTP 状态码，不是可比数值")],
};

fn arg(argv: &[String], name: &str) -> Option<String> {
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).cloned()
}

fn hours_since(at: &Option<String>, now: DateTime<FixedOffset>) -> f64 {
    at.as_deref().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map_or(f64::INFINITY, |t| (now - t).num_minutes() as f64 / 60.0)
}

/// Failure keys not yet handed to a repair tab; healed keys leave `handled` so a relapse alerts again.
fn new_failures(rounds: &[SiteRound], state: &mut PushState) -> Vec<(String, String)> {
    let current: Vec<(String, String)> = rounds.iter().flat_map(|r| r.failures.iter().cloned()).collect();
    state.handled.retain(|key| current.iter().any(|(k, _)| k == key));
    current.into_iter().filter(|(key, _)| !state.handled.contains(key)).collect()
}

/// One clause per site for the case line: healthy, or the engines and files that failed.
fn site_status(rounds: &[SiteRound]) -> String {
    rounds.iter().map(|r| {
        let host = r.site.trim_start_matches("https://");
        if r.failures.is_empty() { format!("{host} 正常") } else { format!("{host} 有问题：{}", r.failures.iter().map(|(key, _)| key.split(':').next().unwrap_or(key)).collect::<Vec<_>>().join("、")) }
    }).collect::<Vec<_>>().join("；")
}

pub fn main(argv: &[String]) -> i32 {
    match run(argv) { Ok(()) => 0, Err(error) => fail(&error) }
}

fn run(argv: &[String]) -> Result<(), String> {
    let config = read_config(&arg(argv, "--config").unwrap_or_else(|| DEFAULT_CONFIG.into()))?;
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let state_dir = arg(argv, "--state").unwrap_or_else(|| format!("{home}/.local/state/seo-push"));
    let dry_run = argv.iter().any(|a| a == "--dry-run");
    let beijing = FixedOffset::east_opt(8 * 3600).expect("offset");
    let now = Utc::now().with_timezone(&beijing);
    let run_dir = format!("{state_dir}/runs/{}", now.format("%Y-%m-%dT%H%M"));
    std::fs::create_dir_all(&run_dir).map_err(|e| e.to_string())?;
    let state_path = format!("{state_dir}/state.json");
    let mut state = read_state(&state_path);
    let (day, stamp) = (now.format("%Y-%m-%d").to_string(), now.to_rfc3339());

    let bing_key = config.bing_api_key_file.as_deref().and_then(|f| read_token(f, &home));
    let http = client()?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    let rounds: Vec<SiteRound> = runtime.block_on(async {
        let run = Run { client: &http, day: &day, now: &stamp, dry_run };
        let mut rounds = Vec::new();
        for site in &config.sites {
            let token = site.baidu_token_file.as_deref().and_then(|f| read_token(f, &home));
            let entry = state.sites.entry(site.site.trim_end_matches('/').to_string()).or_default();
            let keys = Keys { baidu: token.as_deref(), indexnow: &config.indexnow_key, bing: bing_key.as_deref() };
            rounds.push(round(&run, site, &keys, entry).await);
        }
        rounds
    });

    let page_url = format!("https://reports.example.com/{}/", config.report.slug);
    let mut notes = Vec::new();
    let fresh = new_failures(&rounds, &mut state);
    if !fresh.is_empty() && hours_since(&state.last_repair_at, now) >= config.repair_cooldown_hours {
        if dry_run { notes.push(format!("（演练）将派「SEO推送修复」处理 {} 项", fresh.len())); }
        else {
            match launch("SEO推送修复", &repair_prompt(&page_url, &run_dir, &fresh), &run_dir) {
                Ok(session) => { state.handled.extend(fresh.iter().map(|(k, _)| k.clone())); state.last_repair_at = Some(stamp.clone()); state.last_session = Some(session.clone()); notes.push(format!("已派「SEO推送修复」（{}）", super::watch_publish::tab_label(&session))); }
                Err(error) => notes.push(format!("派「SEO推送修复」失败：{error}")),
            }
        }
    }
    let marker = format!("{state_dir}/{SITEMAP_MARKER}");
    let baidu_open: Vec<String> = state.sites.iter().filter(|(_, s)| !s.baidu.pushed.is_empty()).map(|(site, _)| site.clone()).collect();
    if !baidu_open.is_empty() && !std::path::Path::new(&marker).exists() && hours_since(&state.last_sitemap_tab_at, now) >= config.baidu_sitemap_retry_hours {
        if dry_run { notes.push("（演练）将派「百度sitemap」标签".into()); }
        else {
            match launch("百度sitemap", &sitemap_prompt(&page_url, &marker, &baidu_open), &run_dir) {
                Ok(session) => { state.last_sitemap_tab_at = Some(stamp.clone()); state.last_session = Some(session.clone()); notes.push(format!("已派「百度sitemap」提交标签（{}）", super::watch_publish::tab_label(&session))); }
                Err(error) => notes.push(format!("派「百度sitemap」失败：{error}")),
            }
        }
    }

    let at_sgt = now.format("%Y-%m-%d %H:%M 新加坡时间").to_string();
    write_text(&format!("{run_dir}/report.md"), &markdown(&at_sgt, &rounds, &notes))?;
    let failed = rounds.iter().any(|r| !r.failures.is_empty());
    let row = serde_json::json!({"at": stamp, "rounds": rounds, "dispatch": notes});
    write_text(&format!("{run_dir}/analysis.json"), &stringify(&row)?)?;
    let history = format!("{state_dir}/history.jsonl");
    let prior = std::fs::read_to_string(&history).unwrap_or_default();
    write_text(&history, &format!("{prior}{}\n", serde_json::to_string(&row).map_err(|e| e.to_string())?))?;
    if !dry_run { write_text(&state_path, &stringify(&state)?)?; }
    if !argv.iter().any(|a| a == "--no-publish") {
        let status = format!("{}。各引擎的推送数与配额见「推送进度」与「推送状态」", site_status(&rounds));
        publish(&config.report.slug, &config.report.title, &PUSH_TEXT, &state_dir, &run_dir, failed, &at_sgt, &status)?;
    }
    println!("{}", serde_json::to_string(&row).map_err(|e| e.to_string())?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_alerts_once_and_again_after_it_heals_and_relapses() {
        let broken = |key: &str| SiteRound { failures: vec![(key.into(), "x".into())], ..Default::default() };
        let mut state = PushState::default();
        assert_eq!(new_failures(&[broken("file:a")], &mut state).len(), 1);
        state.handled.push("file:a".into());
        assert!(new_failures(&[broken("file:a")], &mut state).is_empty());
        assert!(new_failures(&[SiteRound::default()], &mut state).is_empty());
        assert!(state.handled.is_empty());
        assert_eq!(new_failures(&[broken("file:a")], &mut state).len(), 1);
    }

    #[test]
    fn the_case_line_names_failing_engines_without_counts() {
        let ok = SiteRound { site: "https://example.com".into(), ..Default::default() };
        let bad = SiteRound { site: "https://example.cn".into(), failures: vec![("bing:https://example.cn".into(), "x".into())], ..Default::default() };
        assert_eq!(site_status(&[bad, ok]), "example.cn 有问题：bing；example.com 正常");
    }
}
