//! One run's rank convergence: today's SERP and volumes, merged with the production Search Console
//! rows into one `RankRow` per target, written to the history and annotated with its trend.

use super::watch_analyze::TargetRow;
use super::watch_config::WatchConfig;
use super::watch_rank::{RankRow, effective_rank};
use super::watch_rank_history::{self as history, Line};
use chrono::{DateTime, FixedOffset};
use std::collections::BTreeMap;

pub struct Ranks {
    pub rows: Vec<RankRow>,
    pub history: Vec<Line>,
}

fn host(site: &str) -> String {
    let rest = site.split_once("://").map_or(site, |(_, r)| r);
    rest.trim_end_matches('/').trim_start_matches("www.").to_string()
}

/// Builds today's rows; `None` when the config has no `rank` section.
pub fn collect(config: &WatchConfig, state_dir: &str, gsc: &[TargetRow], now: DateTime<FixedOffset>, errors: &mut Vec<String>) -> Option<Ranks> {
    let cfg = config.rank.as_ref()?;
    let today = now.date_naive();
    let day = today.format("%Y-%m-%d").to_string();
    let serps = super::watch_rank_serp::fetch(cfg, &config.targets, &host(&config.site), &format!("{state_dir}/rank/{day}"), errors);
    let ledger: BTreeMap<String, f64> = super::watch_keywords::read_ledger(state_dir).into_iter().map(|(k, e)| (k.to_lowercase(), e.volume)).collect();
    let volumes = super::watch_rank_serp::volumes(cfg, &config.targets, &ledger, state_dir, today, errors);
    let mut rows: Vec<RankRow> = config.targets.iter().zip(gsc).map(|(t, g)| {
        let serp = serps.get(&t.keyword);
        let position = serp.map(|s| s.position.unwrap_or(super::watch_rank::UNRANKED));
        RankRow {
            keyword: t.keyword.clone(), page: t.page.clone(), serp: position,
            serp_page: serp.map(|s| s.breadcrumb.clone()).unwrap_or_default(), leader: serp.map(|s| s.leader.clone()).unwrap_or_default(),
            gsc_position: g.position, impressions: g.impressions, volume: volumes.get(&t.keyword).copied(),
            rank: effective_rank(position, g.position), ..Default::default()
        }
    }).collect();
    let past = history::read(state_dir);
    history::annotate(&mut rows, &past, today);
    let history = history::upsert(state_dir, &past, &rows, &day).unwrap_or_else(|e| { errors.push(format!("rank history: {e}")); past });
    Some(Ranks { rows, history })
}
