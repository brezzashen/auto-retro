//! Sponsored creator posts of every watchdog run (block `creatorPosts` of `analysis.json`). Each run lists every
//! configured creator account through AIsa (TikHub tools), keeps the posts published since the creator's `since`
//! day whose text names a keyword (or whose id is pinned), adds Douyin plays and appends one row per tracked
//! post to `<state>/creator/history.jsonl`. Spend stops at `dailyBudgetUsd` per Singapore day; this budget is
//! separate from `aisaSeo`.

use super::aisa::run_tool_charged;
use super::files::{read_json, stringify};
use super::watch_creator_parse::{self as parse, PostStat};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Write;

const DOUYIN_LIST: &str = "get_tikhub_douyin_app_v3_fetch_user_post_videos";
const DOUYIN_STATS: &str = "get_tikhub_douyin_app_v3_fetch_video_statistics";
const CHANNELS_LIST: &str = "post_tikhub_wechat_channels_v2_fetch_user_videos";
const XIAOHONGSHU_LIST: &str = "get_tikhub_xiaohongshu_app_v2_get_user_posted_notes";
/// The Douyin statistics tool accepts at most two aweme ids per call.
const DOUYIN_STATS_BATCH: usize = 2;
/// A snapshot at least this old counts as "an hour ago" (runs fire at minute 17, a few minutes apart in length).
const HOUR_AGO_MINUTES: i64 = 50;
const DAY_AGO_HOURS: i64 = 23;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CreatorConfig {
    pub daily_budget_usd: f64,
    pub creators: Vec<Creator>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Creator {
    pub name: String,
    /// First Singapore day whose posts can be ours, `YYYY-MM-DD`.
    pub since: String,
    /// A post counts when its text contains one of these, case-insensitive.
    pub keywords: Vec<String>,
    /// Douyin `sec_user_id`.
    #[serde(default)]
    pub douyin: Option<String>,
    /// WeChat Channels finder username (`v2_…@finder`).
    #[serde(default)]
    pub channels: Option<String>,
    /// Xiaohongshu user id.
    #[serde(default)]
    pub xiaohongshu: Option<String>,
    /// Post ids tracked whatever their text says.
    #[serde(default)]
    pub posts: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRow {
    /// Check time, RFC 3339 in Singapore time.
    pub at: String,
    pub creator: String,
    #[serde(flatten)]
    pub stat: PostStat,
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccountCheck {
    pub creator: String,
    pub platform: String,
    /// Posts the list returned; `None` when this run did not get the list.
    pub listed: Option<usize>,
    pub matched: usize,
    /// The newest post on the account: publication epoch seconds and title.
    pub newest: Option<(i64, String)>,
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TrackedPost {
    #[serde(flatten)]
    pub now: HistoryRow,
    pub url: Option<String>,
    pub hour_ago: Option<HistoryRow>,
    pub day_ago: Option<HistoryRow>,
}

/// The `creatorPosts` block of `analysis.json`.
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CreatorRun {
    pub posts: Vec<TrackedPost>,
    pub accounts: Vec<AccountCheck>,
    pub spent_today_usd: f64,
    pub daily_budget_usd: f64,
    pub skipped: Vec<String>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SpendState { day: String, spent_usd: f64 }

struct Spend { state: SpendState, cap: f64, dry_run: bool, skipped: Vec<String> }

impl Spend {
    fn call(&mut self, what: &str, tool: &str, args: Value, errors: &mut Vec<String>) -> Option<Value> {
        if self.dry_run { self.skipped.push(format!("{what}：演练不调用")); return None; }
        if self.state.spent_usd >= self.cap {
            self.skipped.push(format!("{what}：今日已花 ${:.3}，达到上限 ${:.2}", self.state.spent_usd, self.cap));
            return None;
        }
        match run_tool_charged(tool, args) {
            None => { self.skipped.push(format!("{what}：没有 AIsa 密钥")); None }
            Some(Err(e)) => { errors.push(format!("creator {what}: {e}")); None }
            Some(Ok((data, charged))) => {
                self.state.spent_usd += charged;
                match parse::envelope_error(&data) { Some(e) => { errors.push(format!("creator {what}: {e}")); None } None => Some(data) }
            }
        }
    }
}

fn list(spend: &mut Spend, creator: &str, platform: &str, account: &str, errors: &mut Vec<String>) -> Option<Vec<PostStat>> {
    let what = format!("{creator} {}", parse::platform_name(platform));
    match platform {
        "douyin" => spend.call(&what, DOUYIN_LIST, json!({"sec_user_id": account, "count": 20}), errors).map(|d| parse::douyin_posts(&d)),
        "channels" => spend.call(&what, CHANNELS_LIST, json!({"body": {"username": account, "raw": false}}), errors).map(|d| parse::channels_videos(&d)),
        _ => spend.call(&what, XIAOHONGSHU_LIST, json!({"user_id": account}), errors).map(|d| parse::xiaohongshu_notes(&d)),
    }
}

/// Whether a listed post is one of ours: pinned by id, or published since `since` (Singapore) and naming a keyword.
pub fn is_ours(creator: &Creator, post: &PostStat, offset: &FixedOffset) -> bool {
    if creator.posts.contains(&post.id) { return true; }
    let since = NaiveDate::parse_from_str(&creator.since, "%Y-%m-%d").ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0)?.and_local_timezone(*offset).single()).map_or(i64::MAX, |t| t.timestamp());
    let title = post.title.to_lowercase();
    post.created >= since && creator.keywords.iter().any(|k| title.contains(&k.to_lowercase()))
}

fn add_douyin_plays(spend: &mut Spend, rows: &mut [HistoryRow], errors: &mut Vec<String>) {
    let ids: Vec<String> = rows.iter().filter(|r| r.stat.platform == "douyin").map(|r| r.stat.id.clone()).collect();
    let mut plays = BTreeMap::new();
    for chunk in ids.chunks(DOUYIN_STATS_BATCH) {
        if let Some(data) = spend.call("抖音播放数", DOUYIN_STATS, json!({"aweme_ids": chunk.join(",")}), errors) { plays.extend(parse::douyin_plays(&data)); }
    }
    for row in rows.iter_mut() { if let Some(p) = plays.get(&row.stat.id) { row.stat.plays = Some(*p); } }
}

fn read_history(path: &str) -> Vec<HistoryRow> {
    std::fs::read_to_string(path).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn append_history(path: &str, rows: &[HistoryRow], errors: &mut Vec<String>) {
    let text: String = rows.iter().filter_map(|r| stringify(r).ok()).map(|l| format!("{}\n", l.replace('\n', " "))).collect();
    let written = std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|mut f| f.write_all(text.as_bytes()));
    if let Err(e) = written { errors.push(format!("creator history (creator/history.jsonl): {}", e.kind())); }
}

/// The newest row of the post at or before `before`.
fn snapshot(history: &[HistoryRow], row: &HistoryRow, before: DateTime<FixedOffset>) -> Option<HistoryRow> {
    history.iter().filter(|h| h.stat.platform == row.stat.platform && h.stat.id == row.stat.id)
        .filter(|h| DateTime::parse_from_rfc3339(&h.at).is_ok_and(|t| t <= before)).max_by(|a, b| a.at.cmp(&b.at)).cloned()
}

/// The latest row of every post this run or an earlier run tracked, newest publication first.
fn current(fresh: Vec<HistoryRow>, history: &[HistoryRow]) -> Vec<HistoryRow> {
    let mut latest: BTreeMap<(String, String), HistoryRow> = BTreeMap::new();
    for row in history.iter().cloned().chain(fresh) {
        let key = (row.stat.platform.clone(), row.stat.id.clone());
        if latest.get(&key).is_none_or(|old| old.at <= row.at) { latest.insert(key, row); }
    }
    let mut rows: Vec<HistoryRow> = latest.into_values().collect();
    rows.sort_by(|a, b| b.stat.created.cmp(&a.stat.created).then(a.stat.platform.cmp(&b.stat.platform)));
    rows
}

/// One run; `None` when the config has no `creatorPosts` block. A dry run calls nothing and shows the latest history.
pub fn collect(cfg: Option<&CreatorConfig>, state_dir: &str, now: DateTime<FixedOffset>, dry_run: bool, errors: &mut Vec<String>) -> Option<CreatorRun> {
    let cfg = cfg?;
    let dir = format!("{state_dir}/creator");
    if !dry_run && let Err(e) = std::fs::create_dir_all(&dir) { errors.push(format!("creator dir: {}", e.kind())); }
    let today = now.format("%Y-%m-%d").to_string();
    let state_path = format!("{dir}/state.json");
    let mut state: SpendState = read_json(&state_path).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    if state.day != today { state = SpendState { day: today, spent_usd: 0.0 }; }
    let mut spend = Spend { state, cap: cfg.daily_budget_usd, dry_run, skipped: Vec::new() };
    let at = now.to_rfc3339();
    let (mut fresh, mut accounts) = (Vec::new(), Vec::new());
    for c in &cfg.creators {
        for (platform, account) in [("douyin", &c.douyin), ("channels", &c.channels), ("xiaohongshu", &c.xiaohongshu)] {
            let Some(account) = account else { continue };
            let mut check = AccountCheck { creator: c.name.clone(), platform: platform.into(), ..Default::default() };
            if let Some(posts) = list(&mut spend, &c.name, platform, account, errors) {
                check.listed = Some(posts.len());
                check.newest = posts.iter().max_by_key(|p| p.created).map(|p| (p.created, p.title.chars().take(40).collect()));
                let ours: Vec<HistoryRow> = posts.into_iter().filter(|p| is_ours(c, p, now.offset()))
                    .map(|stat| HistoryRow { at: at.clone(), creator: c.name.clone(), stat }).collect();
                check.matched = ours.len();
                fresh.extend(ours);
            }
            accounts.push(check);
        }
    }
    add_douyin_plays(&mut spend, &mut fresh, errors);
    let history_path = format!("{dir}/history.jsonl");
    let history = read_history(&history_path);
    if !dry_run && !fresh.is_empty() { append_history(&history_path, &fresh, errors); }
    let posts = current(fresh, &history).into_iter().map(|row| {
        let checked = DateTime::parse_from_rfc3339(&row.at).unwrap_or(now);
        TrackedPost {
            url: parse::post_url(&row.stat.platform, &row.stat.id),
            hour_ago: snapshot(&history, &row, checked - Duration::minutes(HOUR_AGO_MINUTES)),
            day_ago: snapshot(&history, &row, checked - Duration::hours(DAY_AGO_HOURS)),
            now: row,
        }
    }).collect();
    if !dry_run && let Err(e) = stringify(&spend.state).and_then(|t| std::fs::write(&state_path, t).map_err(|e| e.kind().to_string())) {
        errors.push(format!("creator state (creator/state.json): {e}"));
    }
    Some(CreatorRun { posts, accounts, spent_today_usd: spend.state.spent_usd, daily_budget_usd: cfg.daily_budget_usd, skipped: spend.skipped })
}

#[cfg(test)]
#[path = "watch_creator_tests.rs"]
mod tests;
