//! Same-hour follow-up of Trends words the hourly retro judged `机会` (founder 2026-11-07:
//! "异常词发现了之后赶紧跟进页面加入 wiki 提交"): instead of waiting for the next morning's keyword
//! room, the next watchdog run opens one `SEO异常词跟进` tab that merges each word into its page with
//! a Wiki screenshot, ships it to production and submits the page to the search engines.
//! `<state>/trend-followups.jsonl` records every handed word (`{"query","page","at","tab"}`), so a word
//! is handed once and the keyword room skips it.

use super::files::text;
use super::watch_publish::{launch, tab_label};
use super::watch_config::{TargetKeyword, WatchConfig};
use super::watch_judge_rules::CORE_MARKET;
use super::watch_trends::OPPORTUNITY;
use super::watch_trends_ledger::{Label, labels};
use chrono::{DateTime, FixedOffset};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

pub const KIND: &str = "异常词跟进";

fn ledger_path(state_dir: &str) -> String { format!("{state_dir}/trend-followups.jsonl") }

/// `query -> at` of every word already handed to a follow-up tab.
pub fn followed(state_dir: &str) -> BTreeMap<String, String> {
    std::fs::read_to_string(ledger_path(state_dir)).unwrap_or_default().lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| (text(&v["query"]).trim().to_lowercase(), text(&v["at"]).to_string()))
        .filter(|(q, _)| !q.is_empty()).collect()
}

const LOCALES: [&str; 5] = ["ja", "ko", "th", "zh-CN", "zh-TW"];

/// True when `page`, or its unprefixed path for a locale page, already has a `targets` row: a seed
/// page without one is not built yet, and the follow-up tab may not create pages.
fn page_shipped(page: &str, shipped: &BTreeSet<String>) -> bool {
    let unprefixed = page.trim_start_matches('/').split_once('/')
        .filter(|(locale, _)| LOCALES.contains(locale)).map(|(_, rest)| format!("/{rest}"));
    shipped.contains(page) || unprefixed.is_some_and(|p| shipped.contains(&p))
}

/// `机会` words on a built page that are neither shipped (`targets`) nor handed yet, oldest label
/// first. A word whose seed page is not built stays a keyword-room lead (`anomaly:<page>`).
pub fn pending(judged: &BTreeMap<String, Label>, targets: &BTreeSet<String>, shipped: &BTreeSet<String>, handed: &BTreeMap<String, String>) -> Vec<(String, Label)> {
    judged.iter().filter(|(q, l)| l.label == OPPORTUNITY && l.page.starts_with('/') && page_shipped(&l.page, shipped)
        && !targets.contains(*q) && !handed.contains_key(*q))
        .map(|(q, l)| (q.clone(), l.clone())).collect()
}

/// The prompt of one follow-up tab for `words` (`(query, label)`).
pub fn prompt(words: &[(String, Label)], page_url: &str, state_dir: &str) -> String {
    let list: Vec<String> = words.iter().map(|(q, l)| format!("- 「{q}」→ {}；复盘判断：{}", l.page, l.note)).collect();
    [
        format!("SEO Watchdog（{KIND}）：每小时复盘刚把下面的 Google Trends 异常上升词判为「机会」。创始人 2026-11-07：「异常词发现了之后赶紧跟进页面加入 wiki 提交」，所以不等第二天的找词轮次，本标签马上把它们做上线并提交收录。监控页：{page_url}；判断记录：{state_dir}/trend-labels.jsonl；体量数据：{state_dir}/trends/latest.json（volume、surge、accel、flag）。"),
        "授权（与「找词」相同，founder 2026-11-04「找词的 skill 可以不用我授权，直接更新生产」）：直接修改关键词落地页与 SEO 元数据（Mars_CRM/web/src/screens/marketing/keywords/**、Mars_CRM/web/src/seo/**、Mars Wiki 截图与 web/src/data/wiki.json、.claude/skills/seo-competitor/references/watch.config.json 的 targets），跑测试，commit/push，交付 beta → Canary → 生产 example.com，无需再问；不新建落地页（新页面仍归每天的找词轮次，最多 1 个）。".into(),
        format!("词：\n{}", list.join("\n")),
        format!("判词规则：{CORE_MARKET}第 2 步核实时，搜索者想找真人咨询师或本地服务不算与 Mars 不对口，不能据此改判观察。"),
        "步骤：1. 加载 mars-crm、seo-competitor 与 changelog skill（Mars Wiki 一节）；2. 先核实：WebSearch 或 x-dev 确认这个词现在指什么，用 monid 跑一次 DataForSEO SERP 看前 10；意图与 Mars 不对口、或已过热度（latest.json 里标「已回落」）时不做，向 trend-labels.jsonl 追加一行改判为「观察」并写原因；3. 每个要做的词并入它的页面：en、zh-CN、zh-TW 三份文案各加一个含该词的 H2、一段具体用例、三条要点和一条 FAQ（FAQ 与 FAQPage JSON-LD 一致），并在该页 KEYWORD_PAGE_MEDIA 加一张对应的 Mars Wiki 截图（每段一张，seo-locales.test.ts 校验数量；截图按 changelog skill 的 shots.js 拍）；这个词对应的 Mars 用法在 Mars Wiki（web/src/data/wiki.json 与 mars-guide.md）里还没有时，按 changelog skill「Mars Wiki guide」补一条 Wiki 条目并链接到该页；4. 在 watch.config.json 的 targets 加 {keyword, page}；5. beta 1440 与 390 截图验收，按 mars-crm 交付第 3b 步派 feature-verifier，交付 Canary 并 `mars deliver accept`；6. 上生产：同车产品提交都要有负责会话的 Canary passed 结论（缺的等该会话或 release-queue，不问创始人），已有会话在主导 promote 时只用 session-send 告诉它，否则 `mars canary promote --dry-run` 再 `mars canary promote`，然后 curl example.com 改动页的非 JS HTML 与 FAQPage JSON-LD；7. 提交收录：运行 `mars-skill-tools seo-competitor push`（改了标题或文案的页面会重新提交给 IndexNow，新 URL 还会提交 Bing 与百度），并在 9223 浏览器的 Google Search Console（sc-domain:example.com，user@example.com）对每个改动页点「请求编入索引」（seo skill 的 engines 参考），记下每个引擎的回执；8. 发布报告（report skill，--encrypt）：每个词它是什么、体量和倍数、改了哪页哪段和哪张 Wiki 截图、生产验收截图、各引擎提交回执、7/14/28 天看哪个指标。60 分钟内完成。".into(),
    ].join("\n\n")
}

/// The pending `机会` words still inside today's cap of `per_day` handed words (Singapore day `today`).
pub fn due(per_day: usize, targets: &[TargetKeyword], state_dir: &str, today: &str) -> Vec<(String, Label)> {
    let handed = followed(state_dir);
    let room = per_day.saturating_sub(handed.values().filter(|at| at.starts_with(today)).count());
    let shipped: BTreeSet<String> = targets.iter().map(|t| t.page.clone()).collect();
    let targets: BTreeSet<String> = targets.iter().map(|t| t.keyword.to_lowercase()).collect();
    pending(&labels(state_dir), &targets, &shipped, &handed).into_iter().take(room).collect()
}

/// Opens one follow-up tab for this hour's due `机会` words (`trends.followUpPerDay`; absent is off).
pub fn open(config: &WatchConfig, page_url: &str, run_dir: &str, state_dir: &str, now: DateTime<FixedOffset>, dry_run: bool, errors: &mut Vec<String>) -> String {
    let Some(per_day) = config.trends.as_ref().and_then(|t| t.follow_up_per_day) else { return String::new() };
    let words = due(per_day, &config.targets, state_dir, &now.format("%Y-%m-%d").to_string());
    if words.is_empty() { return String::new(); }
    let names: Vec<&str> = words.iter().map(|(q, _)| q.as_str()).collect();
    if dry_run { return format!("（演练）将派「{KIND}」标签处理 {}", names.join("、")); }
    let dir = format!("{run_dir}/followup");
    let launched = std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|_| launch(&format!("SEO{KIND}"), &prompt(&words, page_url, state_dir), &dir));
    match launched {
        Ok(session) => {
            let tab = tab_label(&session);
            let lines: String = words.iter().map(|(q, l)| json!({"query": q, "page": l.page, "at": now.to_rfc3339(), "tab": tab}).to_string() + "\n").collect();
            let saved = std::fs::OpenOptions::new().create(true).append(true).open(ledger_path(state_dir)).and_then(|mut f| f.write_all(lines.as_bytes()));
            if let Err(e) = saved { errors.push(format!("followup: {e}")); }
            format!("已派「{KIND}」标签处理 {}（{tab}）", names.join("、"))
        }
        Err(error) => { errors.push(format!("followup: {error}")); format!("派「{KIND}」失败") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(l: &str, page: &str) -> Label { Label { label: l.into(), page: page.into(), note: "n".into() } }

    #[test]
    fn only_unshipped_unhanded_opportunities_with_a_page_are_followed_up() {
        let judged: BTreeMap<String, Label> = [("instinct ai", label("机会", "/personal-ai-assistant")), ("runable", label("观察", "")),
            ("pocket", label("机会", "")), ("shipped", label("机会", "/personal-crm")), ("handed", label("机会", "/personal-crm"))]
            .into_iter().map(|(q, l)| (q.to_string(), l)).collect();
        let targets = BTreeSet::from(["shipped".to_string()]);
        let pages = BTreeSet::from(["/personal-ai-assistant".to_string(), "/personal-crm".to_string()]);
        let handed = BTreeMap::from([("handed".to_string(), "2026-11-07T14:17:00+08:00".to_string())]);
        let got: Vec<String> = pending(&judged, &targets, &pages, &handed).into_iter().map(|(q, _)| q).collect();
        assert_eq!(got, vec!["instinct ai".to_string()]);
        let text = prompt(&pending(&judged, &targets, &pages, &handed), "https://report", "/s");
        assert!(text.contains("「instinct ai」→ /personal-ai-assistant") && text.contains("KEYWORD_PAGE_MEDIA") && text.contains("请求编入索引"));
        assert!(text.contains(CORE_MARKET) && text.contains("不能据此改判观察"), "the follow-up keeps counselling words");
    }

    #[test]
    fn a_word_on_an_unbuilt_seed_page_waits_for_the_keyword_room() {
        // 2026-11-11: "signs you are wasting your time trying to get your ex back" → /win-him-back, a seed with no page yet.
        let judged: BTreeMap<String, Label> = [("get ex back", label("机会", "/win-him-back")), ("恋愛相談 ai", label("机会", "/ja/ai-relationship-coach")),
            ("연애 상담 앱", label("机会", "/ko/ai-relationship-coach"))].into_iter().map(|(q, l)| (q.to_string(), l)).collect();
        let pages = BTreeSet::from(["/ai-relationship-coach".to_string(), "/ja/ai-relationship-coach".to_string()]);
        let got: Vec<String> = pending(&judged, &BTreeSet::new(), &pages, &BTreeMap::new()).into_iter().map(|(q, _)| q).collect();
        assert_eq!(got, vec!["恋愛相談 ai".to_string(), "연애 상담 앱".to_string()], "a locale page counts as built through its unprefixed page");
    }

    #[test]
    fn the_ledger_caps_words_per_day_and_lists_handed_words() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().to_string_lossy().to_string();
        std::fs::write(format!("{state}/trend-labels.jsonl"), r#"{"query":"instinct ai","label":"机会","page":"/personal-ai-assistant","note":"n"}"#).unwrap();
        std::fs::write(ledger_path(&state), r#"{"query":"other","page":"/x","at":"2026-11-07T09:17:00+08:00","tab":"t"}"#.to_string() + "\n").unwrap();
        let target = |keyword: &str| TargetKeyword { keyword: keyword.into(), page: "/personal-ai-assistant".into() };
        let built = [target("personal ai assistant")];
        assert!(due(1, &built, &state, "2026-11-07").is_empty(), "today's one slot is used");
        assert_eq!(due(2, &built, &state, "2026-11-07").len(), 1);
        assert!(due(2, &[target("Instinct AI")], &state, "2026-11-07").is_empty(), "a word in targets has shipped");
        assert_eq!(due(1, &built, &state, "2026-11-08").len(), 1, "a new day has a new cap");
        assert!(due(2, &[], &state, "2026-11-07").is_empty(), "no targets row: the page is not built");
    }
}
