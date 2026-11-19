//! `<state>/rank-history.jsonl`: one line per target keyword per Singapore day, rewritten for the
//! current day on every run, so each word's rank curve, 7-day change and stall length are measurable.

use super::files::{stringify, write_text};
use super::watch_rank::{RankRow, weight_of, weighted_gap};
use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub day: String,
    pub keyword: String,
    pub page: String,
    pub rank: f64,
    pub serp: Option<f64>,
    pub gsc_position: f64,
    pub impressions: f64,
    pub volume: Option<f64>,
    /// False when neither the SERP nor Search Console measured the word that day.
    pub measured: bool,
}

pub fn path(state_dir: &str) -> String { format!("{state_dir}/rank-history.jsonl") }

pub fn read(state_dir: &str) -> Vec<Line> {
    std::fs::read_to_string(path(state_dir)).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Replaces `day`'s lines with `rows` and keeps every other day.
pub fn upsert(state_dir: &str, history: &[Line], rows: &[RankRow], day: &str) -> Result<Vec<Line>, String> {
    let mut out: Vec<Line> = history.iter().filter(|l| l.day != day).cloned().collect();
    out.extend(rows.iter().map(|r| Line { day: day.into(), keyword: r.keyword.clone(), page: r.page.clone(), rank: r.rank, serp: r.serp,
        gsc_position: r.gsc_position, impressions: r.impressions, volume: r.volume, measured: r.measured() }));
    let text: Vec<String> = out.iter().map(stringify).collect::<Result<_, _>>()?;
    write_text(&path(state_dir), &(text.join("\n") + "\n"))?;
    Ok(out)
}

fn measured_days<'a>(history: &'a [Line], keyword: &str, before: &str) -> Vec<&'a Line> {
    let mut lines: Vec<&Line> = history.iter().filter(|l| l.keyword == keyword && l.measured && l.day.as_str() < before).collect();
    lines.sort_by(|a, b| a.day.cmp(&b.day));
    lines
}

/// The keyword's last measured line before `day`: the day `RankRow::previous` was read from.
pub fn previous_line<'a>(history: &'a [Line], keyword: &str, day: &str) -> Option<&'a Line> {
    measured_days(history, keyword, day).last().copied()
}

/// Fills `previous`, `delta7` and `stalled_days` of today's rows from the earlier days.
pub fn annotate(rows: &mut [RankRow], history: &[Line], today: NaiveDate) {
    let day = today.format("%Y-%m-%d").to_string();
    let week_ago = (today - Duration::days(7)).format("%Y-%m-%d").to_string();
    for r in rows.iter_mut().filter(|r| r.measured()) {
        let earlier = measured_days(history, &r.keyword, &day);
        r.previous = earlier.last().map(|l| l.rank);
        r.delta7 = earlier.iter().rev().find(|l| l.day <= week_ago).map(|l| l.rank - r.rank);
        r.stalled_days = stalled_days(&earlier, r.rank, today);
    }
}

/// Days since the last day that set a new best rank by at least one position, today included.
pub fn stalled_days(earlier: &[&Line], today_rank: f64, today: NaiveDate) -> i64 {
    let mut best = f64::INFINITY;
    let mut since: Option<&str> = None;
    for l in earlier {
        if l.rank <= best - 1.0 { best = l.rank; since = Some(&l.day); }
    }
    if today_rank <= best - 1.0 { return 0; }
    since.and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).map_or(0, |d| (today - d).num_days())
}

/// The weighted gap of every day in the last `days`, oldest first, over the words measured that day.
pub fn gap_by_day(history: &[Line], today: NaiveDate, days: i64) -> Vec<(String, f64)> {
    let from = (today - Duration::days(days - 1)).format("%Y-%m-%d").to_string();
    let mut by_day: BTreeMap<&str, Vec<&Line>> = BTreeMap::new();
    for l in history.iter().filter(|l| l.measured && l.day >= from) { by_day.entry(&l.day).or_default().push(l); }
    by_day.into_iter().map(|(d, lines)| (d.to_string(), weighted_gap(lines.iter().map(|l| (l.rank, weight_of(l.volume)))))).collect()
}
