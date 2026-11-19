//! Google Trends curve reading for the keyword room (Gefei's method, report
//! `gefei-keyword-method-20261110`): the birth date of a word from its 5-year weekly and 30-day daily
//! curves (sources 0vz7te71m0, conkcr1mid: under 30 days is best, under a year still workable), a
//! yearly peak that marks a seasonal word (60wmnukjzg trap 1), and monthly searches of a word the
//! keyword tools still show as 0, scaled from a reference word of known volume (dva3ltp2ex).

use super::files::{at, items, text};
use chrono::{Datelike, NaiveDate};
use serde_json::Value;
use std::collections::BTreeMap;

/// A point under this share of the curve's own maximum counts as no searches.
const QUIET: f64 = 0.05;
/// Quiet points in a row that mark a time before the word existed.
const QUIET_RUN: usize = 3;
/// Share of points after a birth that must carry searches, so scattered noise is no birth.
const ALIVE: f64 = 0.7;
/// Share of points before a birth that must be quiet.
const UNBORN: f64 = 0.8;
/// A seasonal peak month holds at least this many times its year's mean.
const PEAK: f64 = 2.0;
/// Complete years whose peaks must agree.
const SEASON_YEARS: usize = 3;
/// Days of the 30-day curve the reference scaling averages.
const SCALE_DAYS: usize = 7;

/// When a word first carried searches inside one curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Born {
    /// Searches already at the window start.
    Before,
    At(NaiveDate),
    /// No searches anywhere.
    Never,
}

/// `(week or day start, value of keyword #idx)` for every complete point of a `google_trends_graph`.
/// DataForSEO sends `null` both for too few searches and for a gap in its pull: a null counts as 0,
/// but a row of a multi-keyword curve where every keyword is null is a gap and is skipped (verified
/// 2026-11-10: the first 17 days of a 30-day curve were null for the 5,400-a-month reference too).
pub fn curve(json: &Value, idx: usize) -> Vec<(NaiveDate, f64)> {
    let first = super::monid::first(json);
    let gap = |p: &Value| { let vs = items(&p["values"]); vs.len() > 1 && vs.iter().all(Value::is_null) };
    items(at(first, &["items"])).iter().filter(|i| text(at(i, &["type"])) == "google_trends_graph")
        .flat_map(|i| items(at(i, &["data"])).iter())
        .filter(|p| !p["missing_data"].as_bool().unwrap_or(false) && !gap(p))
        .filter_map(|p| Some((NaiveDate::parse_from_str(p["date_from"].as_str()?.get(..10)?, "%Y-%m-%d").ok()?, p["values"][idx].as_f64().unwrap_or(0.0))))
        .collect()
}

/// The first point after the last run of `QUIET_RUN` quiet points, when the time before it was
/// mostly quiet (scattered nulls inside an old word's curve are no birth) and the word stays alive after it.
pub fn birth(points: &[(NaiveDate, f64)]) -> Born {
    let max = points.iter().map(|p| p.1).fold(0.0, f64::max);
    if max <= 0.0 { return Born::Never; }
    let quiet = |v: f64| v < max * QUIET;
    let mut run = 0;
    let mut start = None;
    for (i, p) in points.iter().enumerate() {
        if quiet(p.1) { run += 1; continue; }
        if run >= QUIET_RUN { start = Some(i); }
        run = 0;
    }
    let Some(i) = start else { return if quiet(points[0].1) { Born::Never } else { Born::Before } };
    let (before, after) = points.split_at(i);
    let share = |ps: &[(NaiveDate, f64)], alive: bool| ps.iter().filter(|p| quiet(p.1) != alive).count() as f64 / ps.len().max(1) as f64;
    let alive = after.iter().filter(|p| !quiet(p.1)).count();
    let born = alive >= 2.min(after.len()) && share(after, true) >= ALIVE && share(before, false) >= UNBORN;
    if born { Born::At(points[i].0) } else { Born::Before }
}

/// Days since the word appeared: the 30-day daily curve refines a weekly birth inside its window;
/// `None` when it was there before the 5-year window or never carried searches.
pub fn age_days(weekly: Born, daily: Born, today: NaiveDate) -> Option<i64> {
    let day = match (weekly, daily) {
        (Born::At(w), Born::At(d)) if (today - w).num_days() <= 42 => d,
        (Born::At(w), _) => w,
        _ => return None,
    };
    Some((today - day).num_days().max(0))
}

/// The month (1-12) a word peaks in every complete year of a 5-year weekly curve, when it does.
pub fn seasonal_month(points: &[(NaiveDate, f64)]) -> Option<u32> {
    let mut months: BTreeMap<(i32, u32), Vec<f64>> = BTreeMap::new();
    for (d, v) in points { months.entry((d.year(), d.month())).or_default().push(*v); }
    let mut years: BTreeMap<i32, Vec<(u32, f64)>> = BTreeMap::new();
    for ((y, m), vs) in months { years.entry(y).or_default().push((m, vs.iter().sum::<f64>() / vs.len() as f64)); }
    let peaks: Vec<u32> = years.values().filter(|ms| ms.len() == 12).filter_map(|ms| {
        let mean = ms.iter().map(|m| m.1).sum::<f64>() / 12.0;
        let top = ms.iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
        (mean > 0.0 && top.1 >= PEAK * mean).then_some(top.0)
    }).collect();
    let full = years.values().filter(|ms| ms.len() == 12).count();
    if peaks.len() < SEASON_YEARS || peaks.len() < full { return None; }
    let near = |a: u32, b: u32| { let d = a.abs_diff(b); d <= 1 || d == 11 };
    let around = |m: u32| peaks.iter().filter(|p| near(**p, m)).count();
    let exact = |m: u32| peaks.iter().filter(|p| **p == m).count();
    let mode = (1..=12).max_by_key(|m| (around(*m), exact(*m)))?;
    peaks.iter().all(|p| near(*p, mode)).then_some(mode)
}

/// Monthly searches of keyword #0 scaled from keyword #1 (`reference` searches a month) over the last
/// `SCALE_DAYS` complete days of one shared 30-day curve; `None` when the reference shows nothing.
pub fn scaled_volume(json: &Value, reference: f64) -> Option<f64> {
    let (word, refs) = (curve(json, 0), curve(json, 1));
    let tail = |c: &[(NaiveDate, f64)]| c.iter().rev().take(SCALE_DAYS).map(|p| p.1).sum::<f64>();
    let base = tail(&refs);
    (base > 0.0).then(|| (reference * tail(&word) / base).round())
}

#[cfg(test)]
#[path = "watch_birth_tests.rs"]
mod tests;
