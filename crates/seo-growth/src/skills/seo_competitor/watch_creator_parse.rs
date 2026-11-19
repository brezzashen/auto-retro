//! Creator-account payloads (TikHub tools through AIsa) turned into one row per post. Each parser takes the
//! tool's `data` (the TikHub envelope). A counter the platform does not expose publicly stays `None`, never 0:
//! the Douyin post list and the WeChat Channels and Xiaohongshu lists all return 0 plays.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PostStat {
    /// `douyin`, `channels` (WeChat Channels) or `xiaohongshu`.
    pub platform: String,
    pub id: String,
    pub title: String,
    /// Publication time, epoch seconds.
    pub created: i64,
    pub plays: Option<i64>,
    pub likes: Option<i64>,
    pub comments: Option<i64>,
    pub collects: Option<i64>,
    pub shares: Option<i64>,
    /// WeChat Channels only: the heart counter (`like_count`, UI `喜欢`); its thumbs-up `点赞` is `fav_count`, kept in `likes` (founder checked the UI 2026-11-16).
    pub hearts: Option<i64>,
}

fn int(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str()?.parse().ok())
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().trim().to_owned()
}

fn items(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten()
}

/// Why the envelope is not a successful answer, if it is not.
pub fn envelope_error(data: &Value) -> Option<String> {
    let ok = matches!(data["code"].as_i64(), Some(200) | None) && !data["data"].is_null();
    (!ok).then(|| format!("TikHub code {}: {}", data["code"], data["message"].as_str().unwrap_or("no data")))
}

/// `get_tikhub_douyin_app_v3_fetch_user_post_videos`: the newest posts (a pinned post may come first).
pub fn douyin_posts(data: &Value) -> Vec<PostStat> {
    items(&data["data"]["aweme_list"]).map(|a| {
        let s = &a["statistics"];
        PostStat { platform: "douyin".into(), id: text(&a["aweme_id"]), title: text(&a["desc"]), created: int(&a["create_time"]).unwrap_or(0),
            plays: None, likes: int(&s["digg_count"]), comments: int(&s["comment_count"]), collects: int(&s["collect_count"]), shares: int(&s["share_count"]), hearts: None }
    }).collect()
}

/// `get_tikhub_douyin_app_v3_fetch_video_statistics`: aweme id → play count, the only public source of plays.
pub fn douyin_plays(data: &Value) -> BTreeMap<String, i64> {
    items(&data["data"]["statistics_list"]).filter_map(|s| Some((text(&s["aweme_id"]), int(&s["play_count"])?))).collect()
}

/// `post_tikhub_wechat_channels_v2_fetch_user_videos` with `raw: false`.
pub fn channels_videos(data: &Value) -> Vec<PostStat> {
    items(&data["data"]["videos"]).map(|v| {
        let title = [&v["title"], &v["description"]].into_iter().map(text).find(|t| !t.is_empty()).unwrap_or_default();
        PostStat { platform: "channels".into(), id: text(&v["id"]), title, created: int(&v["create_time"]).unwrap_or(0), plays: None,
            likes: int(&v["fav_count"]), comments: int(&v["comment_count"]), collects: None, shares: int(&v["forward_count"]), hearts: int(&v["like_count"]) }
    }).collect()
}

/// `get_tikhub_xiaohongshu_app_v2_get_user_posted_notes`: the newest notes (a pinned note may come first).
pub fn xiaohongshu_notes(data: &Value) -> Vec<PostStat> {
    items(&data["data"]["data"]["notes"]).map(|n| {
        let head = [&n["title"], &n["display_title"]].into_iter().map(text).find(|t| !t.is_empty()).unwrap_or_default();
        PostStat { platform: "xiaohongshu".into(), id: text(&n["id"]), title: format!("{head} {}", text(&n["desc"])).trim().to_owned(),
            created: int(&n["create_time"]).unwrap_or(0), plays: None, likes: int(&n["likes"]), comments: int(&n["comments_count"]),
            collects: int(&n["collected_count"]), shares: int(&n["share_count"]), hearts: None }
    }).collect()
}

/// The public address of a post, when the platform has one.
pub fn post_url(platform: &str, id: &str) -> Option<String> {
    match platform {
        "douyin" => Some(format!("https://www.douyin.com/video/{id}")),
        "xiaohongshu" => Some(format!("https://www.xiaohongshu.com/explore/{id}")),
        _ => None,
    }
}

pub fn platform_name(platform: &str) -> &'static str {
    match platform { "douyin" => "抖音", "channels" => "视频号", "xiaohongshu" => "小红书", _ => "其他" }
}
