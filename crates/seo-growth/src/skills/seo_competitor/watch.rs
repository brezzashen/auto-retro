//! `mars-skill-tools seo-competitor watch [--config <file>] [--state <dir>] [--dry-run] [--no-publish] [--discover]`:
//! one hourly SEO watchdog run (launchd `local.autoretro.seo-watch`). It probes every sitemap URL, reads
//! Search Console, turns both into findings, republishes the live report page and, when a finding is
//! new and its cooldown has passed, opens one `limit-resume launch` tab to fix, optimize or review.

use super::files::{fail, stringify, write_text};
use super::watch_analyze::{self as analyze, Finding};
use super::watch_config::{WatchState, read_config, read_state};
use super::watch_gsc;
use super::{watch_copy_cap, watch_decisions, watch_link_gap, watch_new_page_grace};
use super::watch_page_cliff;
use super::watch_probe::{SiteProbe, probe_site};
use super::watch_publish::{WATCH_TEXT, dispatch, publish};
use super::watch_report::{Page, markdown};
use super::{watch_geo, watch_geo_view, watch_retro, watch_traffic, watch_trends_followup, watch_trends_ledger, watch_trends_view};
use super::{watch_anomalies, watch_bing, watch_origin, watch_board, watch_keywords, watch_page_gate, watch_rank, watch_rank_run, watch_rank_view, watch_reindex, watch_reindex_resend};
use chrono::{DateTime, Duration, FixedOffset, Utc};

const DEFAULT_CONFIG: &str = ".claude/skills/seo-competitor/references/watch.config.json";
/// Reindex follow-ups inspected per run on top of the sitemap rotation (`watch_reindex.rs`).
const REINDEX_PER_RUN: usize = 12;

fn arg(argv: &[String], name: &str) -> Option<String> {
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).cloned()
}

fn hours_since(at: &Option<String>, now: DateTime<FixedOffset>) -> f64 {
    at.as_deref().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map_or(f64::INFINITY, |t| (now - t).num_minutes() as f64 / 60.0)
}

/// Which tab to open, if any: a new P0 first, then a new traffic spike, then new P1 work, then the daily review.
fn decide(findings: &[Finding], state: &WatchState, cooldowns: (f64, f64), retro_hour: u32, max_work: usize, now: DateTime<FixedOffset>) -> Option<(&'static str, Vec<Finding>)> {
    let fresh = |severity: &str| findings.iter().filter(|f| f.severity == severity && !state.handled.contains(&f.key)).cloned().collect::<Vec<_>>();
    let health = fresh("P0");
    if !health.is_empty() && hours_since(&state.last_health_at, now) >= cooldowns.1 { return Some(("健康修复", health)); }
    let (traffic, work): (Vec<Finding>, Vec<Finding>) = fresh("P1").into_iter().partition(|f| f.key.starts_with("traffic:"));
    if !traffic.is_empty() { return Some(("流量溯源", traffic)); }
    if !work.is_empty() && hours_since(&state.last_optimize_at, now) >= cooldowns.0 { return Some(("关键词优化", work.into_iter().take(max_work).collect())); }
    let today = now.format("%Y-%m-%d").to_string();
    if now.format("%H").to_string().parse::<u32>().ok() == Some(retro_hour) && state.last_retro_day.as_deref() != Some(&today) {
        return Some(("每日复盘", findings.to_vec()));
    }
    None
}

pub fn main(argv: &[String]) -> i32 {
    match run(argv) { Ok(()) => 0, Err(error) => fail(&error) }
}

fn run(argv: &[String]) -> Result<(), String> {
    let config = read_config(&arg(argv, "--config").unwrap_or_else(|| DEFAULT_CONFIG.into()))?;
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let state_dir = arg(argv, "--state").unwrap_or_else(|| format!("{home}/.local/state/seo-watch"));
    let dry_run = argv.iter().any(|a| a == "--dry-run");
    let sgt = FixedOffset::east_opt(8 * 3600).expect("offset");
    let now = Utc::now().with_timezone(&sgt);
    let run_dir = format!("{state_dir}/runs/{}", now.format("%Y-%m-%dT%H"));
    std::fs::create_dir_all(&run_dir).map_err(|e| e.to_string())?;
    let state_path = format!("{state_dir}/state.json");
    let mut state = read_state(&state_path);
    let mut errors = Vec::new();

    let site = probe_site(&config.site, config.canary.as_deref()).unwrap_or_else(|error| {
        errors.push(format!("probe: {error}"));
        SiteProbe { urls: Vec::new(), released: Vec::new(), probes: Vec::new(), mirror: Vec::new() }
    });
    let (urls, mut probes) = (site.urls, site.probes);
    if let Some(origin) = &config.origin {
        let since = now.with_timezone(&Utc).format("%Y-%m-%dT%H:%M:%SZ").to_string();
        if let Err(error) = watch_origin::clear_probe_path_slowness(&mut probes, origin, &since) { errors.push(format!("origin: {error}")); }
    }
    for url in &urls { state.first_seen.entry(url.clone()).or_insert_with(|| now.to_rfc3339()); }
    // Only pages live on production go to URL Inspection; a page still waiting for promotion would spend quota.
    let live: Vec<String> = probes.iter().filter(|p| p.status == 200 && p.canonical == p.url).map(|p| p.url.clone()).collect();
    let take = config.inspect_per_run.min(live.len());
    watch_reindex_resend::run(&watch_reindex::read(&state_dir), &state_dir, &mut state, now, dry_run, &mut errors);
    let reindex = watch_reindex::read(&state_dir);
    let mut inspect = watch_reindex::due_for_inspection(&reindex, &state, now, REINDEX_PER_RUN);
    let rotation: Vec<String> = (0..take).map(|i| live[(state.inspect_cursor + i) % live.len()].clone()).collect();
    inspect.extend(rotation.into_iter().filter(|url| !inspect.contains(url)).collect::<Vec<_>>());
    if !live.is_empty() { state.inspect_cursor = (state.inspect_cursor + take) % live.len(); }

    let pacific = Utc::now().with_timezone(&FixedOffset::west_opt(7 * 3600).expect("offset")).date_naive();
    let day = |days: i64| (pacific - Duration::days(days)).format("%Y-%m-%d").to_string();
    let gsc = watch_gsc::collect(&config.gsc, &day(0), &day(3), &day(28), &inspect, &now.to_rfc3339());
    errors.extend(gsc.errors.iter().cloned());
    for (url, inspection) in &gsc.inspections { state.inspections.insert(url.clone(), inspection.clone()); }

    let rows = analyze::query_rows(&gsc.query_page);
    let targets = watch_rank::gsc_targets(&rows, &config.targets, &config.site);
    let ranks = watch_rank_run::collect(&config, &state_dir, &targets, now, &mut errors);
    let (last, previous) = (analyze::window(&gsc.hourly, now, 0), analyze::window(&gsc.hourly, now, 24));
    let mut findings = analyze::health_findings(&probes, &site.mirror, &site.released);
    let waiting = watch_reindex::waiting(&reindex, &state, now);
    let unasked: Vec<String> = urls.iter().filter(|url| !waiting.contains(*url)).cloned().collect();
    findings.extend(analyze::index_findings(&unasked, &state, now));
    findings.extend(watch_reindex::findings(&reindex, &state, now));
    findings.extend(analyze::keyword_findings(&rows, &config.targets));
    findings.extend(super::watch_locale_words::findings(super::watch_locale_words::LEDGER, super::watch_locale_words::COPY_DIR));
    if let Some(r) = &ranks { findings.extend(watch_rank::findings(&r.rows, &now.format("%Y-%m-%d").to_string())); }
    findings.extend(analyze::trend_findings(&last, &previous));
    let bing = watch_bing::collect(&config.bing, &config.site, &config.targets, &state_dir, &now.to_rfc3339(), dry_run, &mut errors);
    if let Some(b) = &bing { findings.extend(watch_bing::findings(b)); }
    let baidu = super::watch_baidu::collect(&config.baidu, &state_dir, now, dry_run, &mut errors);
    let traffic = watch_traffic::collect(&config.traffic).map_err(|error| errors.push(format!("posthog: {error}"))).ok();
    if let Some(t) = &traffic { findings.extend(watch_traffic::spike_findings(t, &config.traffic)); }
    let trends = watch_trends_ledger::hourly(&config, &state_dir, &mut state, now, dry_run, &mut errors);
    let (trend_section, trend_findings) = watch_trends_view::apply(trends.as_ref(), config.trends.as_ref(), &state_dir);
    findings.extend(trend_findings);
    let geo = watch_geo::collect(config.geo.as_ref(), &state_dir, &run_dir, now, dry_run, &mut errors);
    let aisa = super::watch_aisa::collect(&config, &state_dir, now, dry_run, &mut errors);
    let creator = super::watch_creator::collect(config.creator_posts.as_ref(), &state_dir, now, dry_run, &mut errors);
    let ads = super::watch_ads::collect(config.google_ads.as_ref(), &run_dir, super::watch_ads::terms_due(&state_dir, now), dry_run, &mut errors);
    let mut cpc_pool = super::cpc_pool_view::run(config.google_ads.as_ref().and_then(|a| a.pool.as_ref()), ads.as_ref(), &state_dir, now, dry_run, &mut errors);
    if let Some(pool) = cpc_pool.as_mut().filter(|_| ads.as_ref().is_some_and(|a| a["error"].is_null())) { super::cpc_pool_preview::run(pool, &state_dir, &run_dir, &now.to_rfc3339(), super::cpc_pool_preview::Reader::Retro, dry_run, &mut errors); }
    let mut anomalies = watch_page_cliff::anomalies(&gsc.day_page, &config.site);
    anomalies.extend(watch_anomalies::detect(&watch_anomalies::AnomalyInput {
        urls: &unasked, state: &state, gsc_pages: &gsc.pages, query_rows: &rows, machine_queries: &config.machine_queries, ranks: ranks.as_ref(),
        traffic: traffic.as_ref(), bing: bing.as_ref(), last: &last, previous: &previous, site: &config.site, today: now.date_naive(),
    }));
    findings.extend(watch_anomalies::findings(&anomalies, &findings));
    findings.extend(watch_decisions::findings(&state_dir, &now.format("%Y-%m-%d").to_string()));
    watch_new_page_grace::apply(&mut findings, &config.targets, &config.site, &mut state, now);
    if let Some(cap) = &config.dispatch.copy_cap { watch_copy_cap::apply(&mut findings, &config.targets, &config.site, cap, &|slug| watch_copy_cap::commits(cap, slug)); }
    if let Some(gap) = &config.dispatch.link_gap { watch_link_gap::apply(&mut findings, &config.targets, &config.site, &mut state, gap, &watch_link_gap::fetch, &|url| watch_link_gap::ledger_links(&gap.ledger, url)); }
    let mut top = rows.clone();
    top.sort_by(|a, b| b.impressions.total_cmp(&a.impressions));
    top.truncate(15);
    let hourly: Vec<(String, f64)> = gsc.hourly["rows"].as_array().into_iter().flatten().filter_map(|row| {
        let at = DateTime::parse_from_rfc3339(row["keys"][0].as_str()?).ok()?.with_timezone(&sgt);
        (now - at <= Duration::hours(48)).then(|| (at.format("%m-%d %H").to_string(), row["impressions"].as_f64().unwrap_or(0.0)))
    }).collect();
    let checked = urls.iter().filter_map(|u| state.inspections.get(u)).collect::<Vec<_>>();
    let indexed = (checked.iter().filter(|i| i.coverage == "Submitted and indexed").count(), checked.len());

    let max_work = config.rank.as_ref().map_or(usize::MAX, |r| r.max_work_per_tab);
    let decision = decide(&findings, &state, (config.dispatch.optimize_cooldown_hours, config.dispatch.health_cooldown_hours), config.dispatch.retro_hour_sgt, max_work, now);
    let page_url = format!("https://reports.example.com/{}/", config.report.slug);
    let dispatch_note = match (&decision, dry_run) {
        (None, _) => "本小时无新事项或仍在冷却期，不派工".to_string(),
        (Some((kind, items)), true) => format!("（演练）将派「{kind}」标签处理 {} 项", items.len()),
        (Some((kind, items)), false) => match dispatch(kind, &page_url, &run_dir, items) {
            Ok(session) => {
                state.handled.extend(items.iter().map(|f| f.key.clone()));
                match *kind { "健康修复" => state.last_health_at = Some(now.to_rfc3339()), "关键词优化" => state.last_optimize_at = Some(now.to_rfc3339()), "流量溯源" => {}
                    _ => state.last_retro_day = Some(now.format("%Y-%m-%d").to_string()) }
                state.last_session = Some(session.clone());
                format!("已派「{kind}」标签处理 {} 项（{}）", items.len(), super::watch_publish::tab_label(&session))
            }
            Err(error) => { errors.push(format!("dispatch: {error}")); format!("派「{kind}」失败") }
        },
    };

    let mut ledger = watch_keywords::read_ledger(&state_dir);
    let force = argv.iter().any(|a| a == "--discover");
    let gate = watch_page_gate::gate(&config, &state, &state_dir, &gsc.pages, now);
    let found = watch_keywords::hourly(&config, &state_dir, &mut state, &mut ledger, &rows, &gate, now, force, dry_run, &page_url, &run_dir, &mut errors);
    let dispatch_note = if found.is_empty() { dispatch_note } else { format!("{dispatch_note}；{found}") };
    let retro = watch_retro::open(&mut state, &page_url, &run_dir, &state_dir, now, dry_run, &mut errors);
    let dispatch_note = if retro.is_empty() { dispatch_note } else { format!("{dispatch_note}；{retro}") };
    let follow = watch_trends_followup::open(&config, &page_url, &run_dir, &state_dir, now, dry_run, &mut errors);
    let dispatch_note = if follow.is_empty() { dispatch_note } else { format!("{dispatch_note}；{follow}") };
    let board = trend_section + &watch_board::markdown(&state_dir, state.last_discover_day.as_deref(), &ledger, &targets, now.date_naive());
    let at_sgt = now.format("%Y-%m-%d %H:%M 新加坡时间").to_string();
    let md = markdown(&Page { at_sgt: &at_sgt, probes: &probes, indexed, last: &last, previous: &previous, hourly: &hourly,
        targets: &targets, top: &top, traffic: traffic.as_ref(), findings: &findings, errors: &errors, dispatch: &dispatch_note, keywords: &board });
    let md = md.replacen("## 本小时建议", &format!("{}## 本小时建议", watch_retro::latest_section(&state_dir)), 1);
    let md = md.replacen("## 本小时建议", &format!("{}## 本小时建议", super::watch_aisa_view::section(aisa.as_ref())), 1);
    let md = md.replacen("## 本小时建议", &format!("{}## 本小时建议", super::watch_creator_view::section(creator.as_ref())), 1);
    let md = md.replacen("## 本小时建议", &format!("{}## 本小时建议", super::cpc_pool_view::section(cpc_pool.as_ref())), 1);
    let md = md.replacen("## 本小时建议", &format!("{}## 本小时建议", watch_geo_view::section(config.geo.as_ref(), geo.as_ref(), &watch_geo::read_records(&state_dir), now)), 1);
    let md = md.replacen("## 每小时搜索展示（Google，新加坡时间）", &format!("{}## 每小时搜索展示（Google，新加坡时间）", watch_anomalies::section(&anomalies)), 1);
    let md = md.replacen("## 目标关键词（近 28 天）", &format!("{}## 目标关键词（近 28 天）", watch_rank_view::section(ranks.as_ref(), now.date_naive())), 1);
    let md = md.replacen("## 目标关键词（近 28 天）", &format!("{}## 目标关键词（近 28 天）", watch_bing::section(bing.as_ref())), 1);
    let md = md.replacen("## 目标关键词（近 28 天）", &format!("{}## 目标关键词（近 28 天）", super::watch_baidu::section(baidu.as_ref())), 1);
    write_text(&format!("{run_dir}/report.md"), &md)?;
    let analysis = serde_json::json!({"at": now.to_rfc3339(), "probes": probes, "targets": targets, "ranks": ranks.as_ref().map(|r| &r.rows), "top": top, "findings": findings, "anomalies": anomalies,
        "last24h": last, "previous24h": previous, "traffic": traffic, "trends": trends, "bing": bing, "baidu": baidu, "geo": geo, "aisaSeo": aisa, "creatorPosts": creator, "googleAds": ads, "cpcPool": cpc_pool, "indexed": indexed, "errors": errors, "dispatch": dispatch_note, "newPageGate": gate, "gsc": {"hourly": gsc.hourly, "queryPage": gsc.query_page, "pages": gsc.pages, "dayPage": gsc.day_page, "sitemaps": gsc.sitemaps}});
    write_text(&format!("{run_dir}/analysis.json"), &stringify(&analysis)?)?;
    let row = serde_json::json!({"at": now.to_rfc3339(), "pages": probes.len(), "failing": probes.iter().filter(|p| !p.problems.is_empty()).count(),
        "indexed": indexed.0, "inspected": indexed.1, "impressions24h": last.impressions, "clicks24h": last.clicks, "findings": findings.len(), "anomalies": anomalies.len(), "dispatch": dispatch_note});
    let history = format!("{state_dir}/history.jsonl");
    let prior = std::fs::read_to_string(&history).unwrap_or_default();
    write_text(&history, &format!("{prior}{}\n", stringify(&row)?))?;
    if !dry_run { write_text(&state_path, &stringify(&state)?)?; }
    let p0 = findings.iter().any(|f| f.severity == "P0");
    if !argv.iter().any(|a| a == "--no-publish") {
        let failing = probes.iter().filter(|p| !p.problems.is_empty()).count();
        let waiting = findings.iter().any(|f| f.key == "promote:pending");
        let status = match (p0, failing, waiting) {
            (true, _, _) => "有页面健康问题（P0），按冷却规则派工".to_string(),
            (false, 0, _) => "页面健康正常；指标与建议见各节".to_string(),
            (false, n, true) => format!("生产 {n} 个页面仍是旧版本，Canary 已修好，等待 promote"),
            (false, n, false) => format!("{n} 个页面有轻微问题，见页面健康一节"),
        };
        publish(&config.report.slug, &config.report.title, &WATCH_TEXT, &state_dir, &run_dir, p0, &at_sgt, &status)?;
    }
    println!("{}", stringify(&row)?);
    Ok(())
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;
