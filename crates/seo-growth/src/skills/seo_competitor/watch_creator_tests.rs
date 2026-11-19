use super::*;
use super::super::watch_creator_view::{delta, section};
use chrono::TimeZone;

fn sgt() -> FixedOffset { FixedOffset::east_opt(8 * 3600).unwrap() }

fn creator() -> Creator {
    Creator { name: "小苋AI".into(), since: "2026-11-16".into(), keywords: vec!["exampleapp".into()], douyin: Some("sec".into()),
        channels: Some("v2_ab@finder".into()), xiaohongshu: Some("6000".into()), posts: vec![] }
}

fn douyin_list() -> Value {
    json!({"code": 200, "data": {"aweme_list": [
        {"aweme_id": "7623", "create_time": 1774883510, "desc": "pinned old post about ExampleApp", "statistics": {"digg_count": 1734, "play_count": 0, "comment_count": 102, "collect_count": 9, "share_count": 3}},
        {"aweme_id": "7708", "create_time": 1794817853, "desc": "一个人创业 这次用 ExampleApp AI 试了聊天截图建档", "statistics": {"digg_count": 1, "play_count": 0, "comment_count": 0, "collect_count": 0, "share_count": 1}}
    ]}})
}

#[test]
fn parses_every_platform_and_keeps_hidden_plays_unknown() {
    let posts = parse::douyin_posts(&douyin_list());
    assert_eq!((posts[1].id.as_str(), posts[1].likes, posts[1].plays), ("7708", Some(1), None), "the Douyin list's play_count 0 is not a real count");
    let plays = parse::douyin_plays(&json!({"code": 200, "data": {"statistics_list": [{"aweme_id": "7708", "digg_count": 1, "play_count": 43, "share_count": 1}]}}));
    assert_eq!(plays.get("7708"), Some(&43));
    let channels = parse::channels_videos(&json!({"code": 200, "data": {"videos": [{"id": "1505", "title": "一个人创业 人脉也交给AI管", "create_time": 1794814373,
        "read_count": 0, "like_count": 1, "fav_count": 4, "forward_count": 5, "comment_count": 1}]}}));
    assert_eq!((channels[0].plays, channels[0].likes, channels[0].hearts, channels[0].shares), (None, Some(4), Some(1), Some(5)), "fav_count is the thumbs-up 点赞");
    let notes = parse::xiaohongshu_notes(&json!({"code": 200, "data": {"data": {"notes": [{"id": "6af9", "title": "怀客松", "desc": "开幕", "create_time": 1794759580,
        "likes": 11, "collected_count": 0, "comments_count": 0, "share_count": 3, "view_count": 0}]}}}));
    assert_eq!((notes[0].title.as_str(), notes[0].likes, notes[0].plays), ("怀客松 开幕", Some(11), None));
    assert!(parse::envelope_error(&json!({"code": 400, "message": "bad", "data": null})).is_some());
    assert!(parse::envelope_error(&douyin_list()).is_none());
}

#[test]
fn only_posts_since_the_start_day_that_name_a_keyword_are_ours() {
    let posts = parse::douyin_posts(&douyin_list());
    assert!(!is_ours(&creator(), &posts[0], &sgt()), "a pinned post from March names the keyword but predates since");
    assert!(is_ours(&creator(), &posts[1], &sgt()));
    let pinned = Creator { posts: vec!["7623".into()], ..creator() };
    assert!(is_ours(&pinned, &posts[0], &sgt()), "a pinned id is always tracked");
    let other = PostStat { title: "裸辞一个月".into(), created: 1794817853, ..Default::default() };
    assert!(!is_ours(&creator(), &other, &sgt()));
}

fn history_row(at: &str, plays: i64, likes: i64) -> HistoryRow {
    HistoryRow { at: at.into(), creator: "小苋AI".into(), stat: PostStat { platform: "douyin".into(), id: "7708".into(), title: "一个人创业".into(),
        created: 1794817853, plays: Some(plays), likes: Some(likes), comments: Some(0), collects: Some(0), shares: Some(1), hearts: None } }
}

#[test]
fn a_dry_run_calls_nothing_and_shows_the_latest_history_with_changes() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(format!("{state}/creator")).unwrap();
    let rows = [history_row("2026-11-16T16:17:00+08:00", 43, 1), history_row("2026-11-16T17:17:00+08:00", 120, 6)];
    let text: String = rows.iter().map(|r| format!("{}\n", serde_json::to_string(r).unwrap())).collect();
    std::fs::write(format!("{state}/creator/history.jsonl"), text).unwrap();
    let cfg = CreatorConfig { daily_budget_usd: 1.0, creators: vec![creator()] };
    let now = sgt().with_ymd_and_hms(2026, 11, 16, 18, 10, 0).unwrap();
    let mut errors = Vec::new();
    let run = collect(Some(&cfg), &state, now, true, &mut errors).unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(run.skipped.len(), 3, "each account is skipped in a dry run: {:?}", run.skipped);
    assert_eq!(run.posts.len(), 1);
    assert_eq!(run.posts[0].now.stat.plays, Some(120));
    assert_eq!(delta(&run.posts[0].now.stat, run.posts[0].hour_ago.as_ref()), "播放 +77、点赞 +5", "the hour-ago snapshot is anchored on the row's own check");
    assert!(run.posts[0].day_ago.is_none());
    assert_eq!(delta(&rows[0].stat, Some(&rows[0])), "无变化");
    assert!(std::fs::read_to_string(format!("{state}/creator/history.jsonl")).unwrap().lines().count() == 2, "a dry run appends nothing");
    let md = section(Some(&run));
    assert!(md.contains("## 达人投放跟进") && md.contains("https://www.douyin.com/video/7708") && md.contains("| 120 |"), "{md}");
    assert!(md.contains("本轮没有取到帖子列表"), "accounts not listed this run say so: {md}");
}

#[test]
fn the_page_says_not_public_and_not_yet_posted_instead_of_zero() {
    let channels = TrackedPost { now: HistoryRow { at: "2026-11-16T18:17:00+08:00".into(), creator: "小苋AI".into(), stat: PostStat { platform: "channels".into(),
        id: "1505".into(), title: "人脉也交给AI管".into(), created: 1794814373, likes: Some(4), hearts: Some(1), ..Default::default() } }, ..Default::default() };
    let xhs = AccountCheck { creator: "小苋AI".into(), platform: "xiaohongshu".into(), listed: Some(21), matched: 0, newest: Some((1794759580, "我主办的怀客松开幕啦！".into())) };
    let run = CreatorRun { posts: vec![channels], accounts: vec![xhs], daily_budget_usd: 1.0, ..Default::default() };
    let md = section(Some(&run));
    assert!(md.contains("| 不公开 | 4（喜欢 1） |") && md.contains("11-16 15:32"), "{md}");
    assert!(md.contains("小红书）：最近 21 条里还没有投放帖子（尚未发布）；最新一条 11-16 00:19「我主办的怀客松开幕啦！」"), "{md}");
}
