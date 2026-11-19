//! The `达人投放跟进` section of the seo-watch page, drawn from one run's `creatorPosts` block.

use super::watch_creator::{CreatorRun, HistoryRow, TrackedPost};
use super::watch_creator_parse::{platform_name, PostStat};
use chrono::{DateTime, FixedOffset};

pub const SECTION: &str = "达人投放跟进";

fn sgt(epoch: i64) -> String {
    let offset = FixedOffset::east_opt(8 * 3600).expect("offset");
    DateTime::from_timestamp(epoch, 0).map_or("—".into(), |t| t.with_timezone(&offset).format("%m-%d %H:%M").to_string())
}

fn short(text: &str, n: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ").replace('|', "／");
    if flat.chars().count() > n { format!("{}…", flat.chars().take(n).collect::<String>()) } else { flat }
}

fn count(v: Option<i64>) -> String {
    v.map_or("不公开".into(), |n| n.to_string())
}

fn likes(s: &PostStat) -> String {
    match s.hearts { Some(h) => format!("{}（喜欢 {h}）", count(s.likes)), None => count(s.likes) }
}

/// Non-zero changes of each counter against an earlier snapshot, `—` without one.
pub fn delta(now: &PostStat, then: Option<&HistoryRow>) -> String {
    let Some(then) = then else { return "—".into() };
    let pairs = [("播放", now.plays, then.stat.plays), ("点赞", now.likes, then.stat.likes), ("喜欢", now.hearts, then.stat.hearts),
        ("评论", now.comments, then.stat.comments), ("收藏", now.collects, then.stat.collects), ("分享", now.shares, then.stat.shares)];
    let parts: Vec<String> = pairs.iter().filter_map(|(label, a, b)| {
        let d = (*a)? - (*b)?;
        (d != 0).then(|| format!("{label} {d:+}"))
    }).collect();
    if parts.is_empty() { "无变化".into() } else { parts.join("、") }
}

fn row(p: &TrackedPost) -> String {
    let s = &p.now.stat;
    let title = short(&s.title, 22);
    let post = p.url.as_ref().map_or(title.clone(), |u| format!("[{title}]({u})"));
    let checked = DateTime::parse_from_rfc3339(&p.now.at).map_or("—".into(), |t| t.format("%m-%d %H:%M").to_string());
    format!("| {} | {} | {} | {post} | {checked} | {} | {} | {} | {} | {} | {} | {} |\n", p.now.creator, platform_name(&s.platform), sgt(s.created),
        count(s.plays), likes(s), count(s.comments), count(s.collects), count(s.shares), delta(s, p.hour_ago.as_ref()), delta(s, p.day_ago.as_ref()))
}

pub fn section(run: Option<&CreatorRun>) -> String {
    let Some(run) = run else { return String::new() };
    let mut md = format!("## {SECTION}\n\n每小时经 AIsa（TikHub 接口）查达人账号最新帖子，只跟踪 `since` 之后发布、正文含关键词的投放帖子。抖音播放数来自视频统计接口；视频号与小红书不公开播放数，写「不公开」。\n\n");
    if run.posts.is_empty() {
        md.push_str("还没有找到投放帖子。\n\n");
    } else {
        md.push_str("| 达人 | 平台 | 发布（新加坡时间） | 帖子 | 查于 | 播放 | 点赞 | 评论 | 收藏 | 分享 | 比约 1 小时前 | 比约 24 小时前 |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n");
        for p in &run.posts { md.push_str(&row(p)); }
        md.push('\n');
        if run.posts.iter().any(|p| p.now.stat.hearts.is_some()) {
            md.push_str("视频号的点赞是 👍（接口 `fav_count`），括号里的喜欢是 ❤（接口 `like_count`）。\n\n");
        }
    }
    for a in &run.accounts {
        let name = platform_name(&a.platform);
        match (a.listed, &a.newest) {
            (None, _) => md.push_str(&format!("- {}（{name}）：本轮没有取到帖子列表，见下方未调用原因或 errors。\n", a.creator)),
            (Some(n), newest) if a.matched == 0 => md.push_str(&format!("- {}（{name}）：最近 {n} 条里还没有投放帖子（尚未发布）；最新一条 {}。\n", a.creator,
                newest.as_ref().map_or("—".into(), |(t, title)| format!("{}「{}」", sgt(*t), short(title, 20))))),
            (Some(n), _) => md.push_str(&format!("- {}（{name}）：最近 {n} 条里有 {} 条投放帖子。\n", a.creator, a.matched)),
        }
    }
    md.push_str(&format!("\n今日达人跟进花费 ${:.3} / 上限 ${:.2}。", run.spent_today_usd, run.daily_budget_usd));
    if !run.skipped.is_empty() { md.push_str(&format!("未调用：{}。", run.skipped.join("；"))); }
    md.push_str("\n\n");
    md
}
