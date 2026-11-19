//! The hourly SEO retro (founder 2026-11-04: one Claude tab every watchdog hour that explains where
//! the traffic came from and what to optimize next). The tab writes a short summary to
//! `<state>/retro/<YYYY-MM-DDTHH>.md`; the next watchdog page shows the newest one.

use super::watch_advisor::ADVISOR_WAIT;
use super::watch_config::WatchState;
use super::watch_judge_rules::{AREAS, CORE_MARKET};
use super::watch_publish::{launch, tab_label};
use chrono::{DateTime, FixedOffset};

pub const KIND: &str = "每小时复盘";
pub const REPORT_SLUG: &str = "seo-hourly-retro";
/// Fixed so the report build's per-section waiver (`WATCH_TEXT`) finds it.
pub const SECTION: &str = "最近一次每小时复盘";

/// Founder 2026-11-12: the hourly retro leads with the latest hour; the 24-hour totals are context only.
const HOUR_FIRST: &str = "呈现原则：先讲本小时的最新状况（traffic.segments 里 window_hours 1、offset_hours 0 的窗口），对比前一小时（offset_hours 1）和昨天同一小时（window_hours 1、offset_hours 24）；近 24 小时的数字只在解释本小时的变化或判断趋势时提一句，不要在每一节和摘要里重复报 24 小时总数；本小时没有访客、订单或新数据就直接写「本小时无…」，不要用 24 小时数字填充。";

fn summary_path(state_dir: &str, now: DateTime<FixedOffset>) -> String {
    format!("{state_dir}/retro/{}.md", now.format("%Y-%m-%dT%H"))
}

/// The prompt of the hourly retro tab for the run at `now`.
pub fn prompt(page_url: &str, run_dir: &str, state_dir: &str, now: DateTime<FixedOffset>) -> String {
    let hour = now.format("%Y-%m-%d %H:00").to_string();
    let start = (now - chrono::Duration::hours(1)).format("%H:00").to_string();
    let summary = summary_path(state_dir, now);
    [
        format!("SEO Watchdog（{KIND}）：复盘 example.com 本小时（{start}–{hour}，新加坡时间）的流量与搜索表现。监控页：{page_url}；本次数据：{run_dir}/analysis.json；历史：{state_dir}/history.jsonl 与 {state_dir}/runs/。"),
        format!("授权：只读分析加发布报告；不改产品代码（页面与关键词改动归「找词」「关键词优化」「每日复盘」标签）。可以在 .claude/skills/seo-competitor/references/traffic-sources.md 追加新查明的来源并只提交这个文件；可以向 {state_dir}/trend-labels.jsonl 追加异常词判断。"),
        HOUR_FIRST.into(),
        format!("先读本小时摘要（一次 Bash 调用，不用 jq keys 摸索结构）：`mars-skill-tools seo-competitor hourDigest --run {run_dir}` 一次打印下面各步要读的数据：traffic 的本小时、前一小时、昨天同一小时与近 24 小时 segments、sources、batches、payments、hour.visitors、hour.db（含 followup）；上一轮以来新完成的 GSC 小时（firstIncompleteHour 没前进时直接写明）；anomalies、P0/P1 findings、geo、aisaSeo 的 aiModeRun 与 links、creatorPosts、googleAds 的系列表、errors；还没判断的异常上升词；上一轮复盘摘要和「## 访客类型分布」一节的 sed 行号。只有摘要里没有的块（aisaSeo.overview/aiMode/props 按词、googleAds 关键词行、geo/answers.jsonl 原文、更早的 runs）再用 jq 读。字段速查：analysis.json 的 traffic 下有 segments、sources、batches、payments 和 hour（visitors 是本小时访客明细，db 是本小时注册、订单与订阅变更，db.followup 是上一小时注册账号截至现在的对话与付费）；搜索看 gsc.hourly.metadata、ranks、findings、anomalies；AI 推荐率看 geo；AIsa 看 aisaSeo；上一轮摘要是 {state_dir}/retro/ 下最新的 .md；「## 访客类型分布」整节在 {run_dir}/report.md。"),
        "步骤：1. 加载 seo-competitor 与 conversion-analytics skill；".into(),
        "2. 流量从哪里来：先读 analysis.json，不要自己重算整点窗口。traffic.segments 已按会话入口来源（$session_entry_utm_source，否则 $session_entry_referring_domain，t.co 保留完整短链，自家跳转记为站内；两者都没有时按内置浏览器的 User-Agent 记为 doubao (app)、douyin (app)、xiaohongshu (app)、wechat (app)，否则 $direct）× 访客类型统计了截至本次整点的固定窗口真实访客（已扣除 traffic.own_ips、局域网、traffic.machine_ip_prefixes 与 watch_traffic.rs MACHINE_AGENTS 的 User-Agent 与 watch_fingerprints.rs 的设备指纹（Google 来路 Linux·纽约时区·1920 px 代理爬虫、Linux UTC 无头检查，只发 $pageview/$pageleave 的人），如微信链接预览爬虫）：window_hours 1 / 24 且 offset_hours 0 是近 1 小时 / 近 24 小时，offset_hours 1 是前一小时，offset_hours 24 是昨天同一小时与前 24 小时；traffic.batches 列出近 24 小时同一系统 + 浏览器 + 时区至少 3 人的成批访客（人数、只看页人数、首末时间、IP 段、落地页）。只发 $pageview 的访客占八成以上的来源（traffic.sources 的 pageview_only，页面标「疑似机器」）和 batches 里只看页为主、时间集中或系统反常（Linux + UTC 无头、iOS Firefox 等）的组先判断是不是机器，是就单独列出、从自然流量里扣掉；本小时每位真实访客的明细已在 traffic.hour.visitors（来源、系统、浏览器、时区、打码 IP、屏宽、首末时间、落地页与浏览页、pageviews、other、cta、signed_in、messages、User-Agent 前 180 字），判断机器和看访客行为先读它；只有它答不了（traffic 为 null 即本轮 PostHog 失败、原因在 errors，或 traffic.hour.visitors_error 非空、要看更早的小时或某个访客的完整事件序列）才用 mars-skill-tools conversion-analytics query 写 HogQL；traffic-sources.md 里没有的新来源按「流量溯源」方法查到帖子或页面；".into(),
        format!("3. 来了之后做了什么：每个主要来源的落地页、cta_clicked、auth_otp_verified、message_sent 人数；按 traffic.segments 分清新老用户和老用户是否付费；报告第一节是「本小时概况」：本小时访客人数与来源、新注册、新订单，对比前一小时和昨天同一小时；第二节再放「访客类型分布」作参考：把 {run_dir}/report.md 里「## 访客类型分布」整节（摘要行、对比行、### 标题行和 categories 代码块）原样复制过来，它已是截至本次整点的固定窗口，不用重算；只有扣掉你判定的机器访客时才改代码块 JSON 里的 values、cells 和 total，别改成 xychart 或 Markdown 表；再放来源明细表；摘要写出各类人数；接着放「上一小时注册回访」：读 traffic.hour.db.followup（看门狗本轮已用 `mars canary sql` 只读查好上一小时 from–to 注册的每个账号截至现在的实际使用：signups、chatted、paid 汇总，accounts 每行是 user8、created、threads、user_messages、first_message、last_message、checkouts、paid_orders、paid、sandbox_paid），写清上一小时注册几人、其中几人已对话（user_messages > 0）、几人发起过结账、几人已付费与套餐金额，没对话的人数单列；对照上一轮复盘（{state_dir}/retro/ 下最新的 .md 与它的报告）对这批注册来源的判断，说明哪个来源的注册真正用起来了、哪个来源注册后没有动作；sandbox_paid 只是 Apple 测试单；signups 为 0 就写「上一小时无新注册」；然后放「付费与订单」：读 traffic.payments（近 24 小时生产 order_paid 与 checkout_started，含 first_order、customer_type、注册日），先列本小时的订单，每一笔用 traffic.hour.db 核对（看门狗已用 `mars canary sql` 只读查好：counts 的 signups_h0/h1/h24 与 paid_h0/h1/h24 是本小时、前一小时、昨天同一小时的新注册与付费订单数；orders 是本小时创建或支付的 crm.payment_order 行，含 status、environment、gateway、order_type、plan、amount、打码 email、account_created、email_paid_accounts；subscription_changes 是本小时的 crm.subscription_change），traffic 为 null、traffic.hour.db_error 非空或要查更早的订单时才自己用 `mars canary sql` 只读查：environment = Sandbox 是 Apple 测试单，不算收入；测试账号单列；同一 checkout_email 多个账号重复付款、付费套餐覆盖了期限更长的赠送套餐都要标出；写清本小时新用户首单几人、老用户付费或续费几人与金额，查不到写原因；前几轮已核对过的更早订单不再逐笔重复，只在状态有变化（退款、取消、重复付款有新进展）时写；本小时没有新订单就写「本小时无新订单」；"),
        "4. 搜索与异常指标捕捉：Search Console 约比现在晚 14 小时（analysis.json gsc.hourly.metadata.firstIncompleteHour），只写上一轮复盘以来新完成的小时的展示与点击，firstIncompleteHour 没前进就写一句「GSC 本小时无新数据」，不要每小时重报最近 24 个完整小时的总数；再看目标词排名变化、收录进度、analysis.json 里的 findings 与 `anomalies`（监控页「异常指标捕捉」6 维度：搜索量价背离/首页低 CTR、关键词内耗、排名异动与 SERP/GSC 背离、Google 规范页偏离、新访客 CTA→注册漏斗断裂与机器批次、跨引擎断层），逐条验证常规假设 A 与反方假设 B；".into(),
        geo_step(run_dir, state_dir),
        aisa_step(run_dir, state_dir),
        creator_step(run_dir, state_dir),
        super::watch_ads_retro::step(run_dir),
        trends_step(state_dir),
        format!("6. 下一步怎么优化与双顾问对抗校验：先拟定不超过 3 条下一步草案写入临时文件 `<scratch>/retro-draft.md`；当 `analysis.json` 的 `anomalies` 非空或本轮有新判断的异常上升词时，运行 `mars-skill-tools seo-competitor advisor --run {run_dir} --mode hourly --draft <scratch>/retro-draft.md --out {run_dir}/retro/advisor-neon.md`{ADVISOR_WAIT}（调用 ag1 的 Gemini 4 Neon 站在额外视角对初判与下一步做红队反驳），对照原始指标采纳有效反驳后再定稿；每条下一步写清楚依据的数字、要改的页面或渠道、由哪个标签执行、看哪个指标在多久内变化，没有证据就写「本小时不建议改动」；"),
        format!("7. 发布报告（report skill，slug {REPORT_SLUG}，--encrypt，每小时原地覆盖），标题写复盘的小时；"),
        format!("8. 把不超过 8 行的中文摘要写到 {summary}（第一行写报告链接；第二行写本小时概况：近 1 小时访客人数与来源、对比前一小时和昨天同一小时、本小时新注册与新订单；第三行写上一小时注册回访：注册几人、已对话几人、已付费几人；第四行写谷歌广告：本小时与今天的花费、点击、广告带来的注册与每次注册成本，没有数据写原因；其余是本小时的付费、变化原因、AI 推荐率（GPT、Claude、Gemini 近 24 小时 k/n，豆包未测量时写明）、异常上升词、下一步；不写近 24 小时总数，除非它解释了本小时的变化），Watchdog 下一轮会把它放到监控页顶部；你的最后一条回复也给出同样的摘要。15 分钟内完成。"),
    ].join("\n")
}

/// The AI recommendation step (founder 2026-11-12): read the watchdog's `geo` block, never ask the models again.
fn geo_step(run_dir: &str, state_dir: &str) -> String {
    format!("4.5 AI 推荐率（GPT、Claude、Gemini、豆包）：读 {run_dir}/analysis.json 的 geo（本小时每题每引擎的 mentioned、rank、products、marsCited、error）、{run_dir}/geo/answers.jsonl（答案原文）和 {state_dir}/geo/records.jsonl（历史），不要自己再调用模型；报告加一节「AI 推荐率」：每个引擎本小时、近 24 小时、近 7 天的被推荐次数 / 完整答案数与首推次数，失败或被截断的答案单列不计入分母，geo.unmeasured 里的引擎（如豆包）写「未测量」和原因，不要拿别的模型冒充；有推荐就引用答案里那一句并核对描述是否对（Mars 是关系与沟通 AI 助手，不是 BI 工具）；没推荐就列出答案推荐了哪些产品、引用了哪些站点，说明 example.com 缺在哪里；样本少于 30 个时 10 个百分点以内的变化写成噪声；")
}

/// AIsa search signals (step 4.6): read the watchdog's `aisaSeo` block, never call AIsa again.
fn aisa_step(run_dir: &str, state_dir: &str) -> String {
    format!("4.6 AIsa 搜索信号：读 {run_dir}/analysis.json 的 aisaSeo（overview 是今天每个目标词 Google 自然结果页的 AI 概览，aiModeRun 是本轮查的 Google AI 模式答案，aiMode 是每个词最近一次 AI 模式，props 是关键词意图与难度，links 是每天新增与丢失的外链，skipped 是本轮没调用的原因）和 {state_dir}/aisa/ 下的缓存，不要自己再调用 AIsa；报告加一节「AIsa 搜索信号」：只写本轮新查的 AI 模式词（有没有 AI 答案、是否引用 example.com、引用了哪些站点）、example.com 在 AI 概览与 AI 模式里的引用次数比昨天同一小时（{state_dir}/runs/ 里昨天同一小时的 analysis.json）的变化、近 7 天外链与来源域的新增和丢失数；没有变化就写一句「AIsa 信号本小时无变化」；Mars 没被引用的词，对照被引用的站点说明我们的页面缺什么，按 props 的意图与难度判断值不值得追，难度 ≤ 20 的商业或交易意图词优先；error 或 deferred 的行不算未引用；")
}

/// Step 5 of the hourly retro: judge every unknown Google Trends riser (founder 2026-11-07: an
/// automated filter dropped what it could not read; "nano banana" was such a word under AI).
/// Sponsored creator posts (step 4.7, founder 2026-11-16): the watchdog queried them through AIsa minutes ago.
fn creator_step(run_dir: &str, state_dir: &str) -> String {
    format!("4.7 达人投放跟进：读 {run_dir}/analysis.json 的 creatorPosts（posts 每行是一条投放帖子本轮经 AIsa 查到的 plays、likes（视频号是 👍 点赞）、comments、collects、shares、hearts（视频号 ❤ 喜欢）与 at，hourAgo、dayAgo 是约 1 小时前与 24 小时前的快照；accounts 是每个达人账号本轮列到几条、匹配几条、最新一条；skipped 是没调用的原因）和 {state_dir}/creator/history.jsonl（每小时一行的历史），看门狗本轮刚用 AIsa 查过，不要自己再调用 AIsa；creatorPosts 为 null 或 posts 为空就跳过本节；报告加一节「达人投放跟进」：每条帖子发布至今的播放（只有抖音公开播放数，其他平台写「不公开」，不要写 0）、点赞、评论、收藏、分享，本小时较上一小时的增量，用 history.jsonl 画一张按小时的累计播放与点赞折线图；把增量和本小时 traffic.hour.visitors 里来自抖音、微信（MicroMessenger、视频号）、小红书的访客以及 db 里 onboarding 来源答 douyin、wechat、xiaohongshu 的注册对照，说明这条素材带来了多少访客与注册，查不到就写「本小时未见可归因访客」；accounts 里还没有匹配帖子的平台写「尚未发布」；at 早于本轮整点的行写明数据停在哪个时间；")
}

fn trends_step(state_dir: &str) -> String {
    format!("5. 异常上升词（Google Trends，监控页「{}」一节；数据在 {state_dir}/trends/latest.json 的 rising 与 heat，历史在 {state_dir}/trends/anomalies.json）：待判断的词是 kind = unknown 的词和 source 含 topic: 的 kind = focus 词（领域拉取来的关注词，relationship 下多是名人八卦），只看 {state_dir}/trend-labels.jsonl 里还没有判断的；market 非空的词来自 trends.markets 各地区的本地语言拉取（JP 日本、KR 韩国、TH 泰国、SG 新加坡简体中文、SG-TW 新加坡繁体中文、SG-EN 新加坡英语、HK 香港、MO 澳门、TW 台湾；source 形如 topic:jp:恋愛、topic:sg-en:dating），外语词先译出中文意思再查，note 写明地区和译义，机会的 page 用该语言已有的落地页（日语 /ja/… 只有 web/src/seo/seoLocales.ts 的 JA_PAGE_PATHS 列出的页面，目前是 /ja/ai-relationship-coach 恋爱咨询、/ja/personal-ai-assistant AI 助手与 AI 秘书、/ja/text-message-analyzer LINE 聊天分析与既読スルー、/ja/relationship-red-flags 恋爱危险信号；对口的种子页没有日语版就判观察并在 note 写建议补日语页，不要填不存在的 /ja 路径，它会 302 到英文页，SG 简体中文 /zh-CN/…，SG-TW、HK、MO、TW 繁体中文 /zh-TW/…，SG-EN 英语用不带语言前缀的英文页；韩语 /ko/… 只有 KO_PAGE_PATHS 列出的页面，目前是 /ko/ai-relationship-coach 연애 상담与권태기、/ko/personal-ai-assistant AI 비서与일정 관리、/ko/text-message-analyzer 카톡 분석与호감 신호、/ko/relationship-red-flags 레드플래그与나쁜 남자 특징；泰语 /th/… 只有 TH_PAGE_PATHS 列出的页面，目前是 /th/ai-relationship-coach、/th/personal-ai-assistant、/th/relationship-red-flags），KR、TH 的词对口的种子页没有该语言版本时同样判观察并在 note 写建议补的页面，不要填不存在的 /ko、/th 路径；先判断 flag 为「新出现」「快速上升」的词（体量对比：volume 当月量 ÷ base 前 11 月均量 = surge，accel 是周加速，「季节回潮」「已回落」排在后面），这些数字只用来排序，不能代替判断；逐个查清它是什么（WebSearch；像模型、AI 工具或应用名的词用 x-dev skill 搜 X），不认识不等于噪声，先查再判；按与 Mars 关注领域（{AREAS}）的关系判为 机会 / 观察 / 噪声 / 无关（{CORE_MARKET}），每个词向 trend-labels.jsonl 追加一行 {{\"query\":…,\"label\":…,\"page\":…,\"note\":\"它是什么、为什么这样判\",\"at\":…}}；机会必须填 page（watch.config.json discovery.seeds 里最接近的落地页路径），找词下一轮自动把它当候选，没有合适页面就判观察并在 note 写建议的新页；只有判为机会的词进找词；heat 里升温的领域说明原因；没有新词就写「本小时无新异常词」；报告加一节「异常上升词」，列出本小时判断的词、它是什么和判断理由；",
        super::watch_trends_view::SECTION)
}

/// Opens the hourly retro tab once per Singapore hour (founder 2026-11-04), whatever else this run
/// dispatched; returns the page note, empty when this hour's retro already opened.
pub fn open(state: &mut WatchState, page_url: &str, run_dir: &str, state_dir: &str, now: DateTime<FixedOffset>, dry_run: bool, errors: &mut Vec<String>) -> String {
    let hour = now.format("%Y-%m-%dT%H").to_string();
    if state.last_hourly_retro.as_deref() == Some(hour.as_str()) { return String::new(); }
    if dry_run { return format!("（演练）将派「{KIND}」标签"); }
    let dir = format!("{run_dir}/retro");
    let launched = std::fs::create_dir_all(&dir).map_err(|e| e.to_string())
        .and_then(|_| launch(&format!("SEO{KIND}"), &prompt(page_url, run_dir, state_dir, now), &dir));
    match launched {
        Ok(session) => {
            state.last_hourly_retro = Some(hour);
            format!("已派「{KIND}」标签（{}）", tab_label(&session))
        }
        Err(error) => { errors.push(format!("retro: {error}")); format!("派「{KIND}」失败") }
    }
}

/// The newest retro summary as a page section, or an empty string before the first retro.
pub fn latest_section(state_dir: &str) -> String {
    let dir = format!("{state_dir}/retro");
    let newest = std::fs::read_dir(&dir).ok().into_iter().flatten().filter_map(Result::ok)
        .map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|e| e == "md")).max();
    let Some(path) = newest else { return String::new() };
    let Ok(text) = std::fs::read_to_string(&path) else { return String::new() };
    let hour = path.file_stem().map(|s| s.to_string_lossy().replace('T', " ")).unwrap_or_default();
    format!("## {SECTION}\n\n复盘时间：{hour}:00（新加坡时间）。\n\n{}\n\n", text.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t: &str) -> DateTime<FixedOffset> { DateTime::parse_from_rfc3339(t).unwrap() }

    #[test]
    fn the_prompt_names_the_summary_file_and_stays_read_only() {
        let text = prompt("https://report", "/s/runs/x", "/s", at("2026-11-04T22:17:00+08:00"));
        assert!(text.contains("/s/retro/2026-11-04T22.md"));
        assert!(text.contains("不改产品代码") && text.contains(REPORT_SLUG));
        assert!(text.contains("/s/trend-labels.jsonl") && text.contains("不认识不等于噪声") && text.contains("8. 把不超过 8 行"));
        assert!(text.contains("不要自己重算整点窗口") && text.contains("offset_hours 24") && text.contains("traffic.batches"), "the retro reads the fixed windows instead of querying them");
        assert!(text.contains("traffic.payments") && text.contains("crm.payment_order") && text.contains("Sandbox"), "the retro checks every order in the database");
        assert!(text.contains("traffic.hour.visitors") && text.contains("traffic.hour.db") && text.contains("不用 jq keys 摸索结构"),
            "conversation retro 85832046819103478997: the retro reads the collected hour rows instead of writing HogQL and SQL each hour");
        assert!(text.contains("seo-competitor hourDigest --run /s/runs/x"),
            "conversation retro 8718675065504968149: one digest call replaces about 13 jq reads of analysis.json per hourly tab");
        assert!(text.contains("「上一小时注册回访」") && text.contains("traffic.hour.db.followup") && text.contains("第三行写上一小时注册回访"),
            "founder 2026-11-15: the retro follows up the previous hour's sign-ups with their chats and payments");
        assert!(text.contains("先判断 flag 为「新出现」「快速上升」的词") && text.contains("source 含 topic: 的 kind = focus"));
        assert!(text.contains("seo-competitor advisor --run /s/runs/x --mode hourly") && text.contains("Gemini 4 Neon") && text.contains("anomalies"));
        assert!(text.contains("advisor-neon.md`（这条命令约 2–4 分钟") && text.contains("run_in_background: true"),
            "conversation retro 813130305414909650913: the tab waits for the advisor with one background call, no wait loop");
        assert!(text.contains("关注领域（关系与约会沟通、读懂短信、情感咨询与挽回、识别高价值男性") && text.contains(CORE_MARKET),
            "founder 2026-11-11: counselling, win-back and high-value-man words are judged under the core-market rule");
        assert!(!text.contains("关注领域（关系与约会沟通、读懂短信、人脉 CRM"), "the old domain list without the core themes is gone");
        assert!(text.contains("本小时（21:00–2026-11-04 22:00") && text.contains("呈现原则：先讲本小时") && text.contains("第一节是「本小时概况」"),
            "founder 2026-11-12: the hourly retro leads with the latest hour");
        assert!(text.contains("不要在每一节和摘要里重复报 24 小时总数") && text.contains("GSC 本小时无新数据") && text.contains("第二行写本小时概况"));
        assert!(!text.contains("读 Search Console 最近 24 个完整小时"), "the summary no longer restates the 24-hour GSC totals every hour");
        assert!(text.contains("4.5 AI 推荐率") && text.contains("/s/runs/x/geo/answers.jsonl") && text.contains("不要自己再调用模型") && text.contains("不要拿别的模型冒充"),
            "founder 2026-11-12: the retro reads the hourly AI recommendation watch and never relabels another model as Doubao");
        assert!(text.contains("4.6 AIsa 搜索信号") && text.contains("/s/runs/x/analysis.json 的 aisaSeo") && text.contains("不要自己再调用 AIsa"),
            "the retro reads the AIsa search signals the watchdog collected and never buys them again");
        assert!(text.contains("4.7 达人投放跟进") && text.contains("/s/runs/x/analysis.json 的 creatorPosts") && text.contains("/s/creator/history.jsonl")
            && text.contains("不要写 0") && text.contains("尚未发布"),
            "founder 2026-11-16: the retro follows the sponsored creator posts' plays and likes every hour from the watchdog's AIsa read");
        assert!(text.contains("4.8 谷歌广告") && text.contains("第四行写谷歌广告"), "founder 2026-11-18: the hourly retro reviews Google Ads bids and results");
    }

    #[test]
    fn the_page_shows_the_newest_summary() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().to_string_lossy().to_string();
        assert!(latest_section(&state).is_empty());
        std::fs::create_dir_all(format!("{state}/retro")).unwrap();
        std::fs::write(format!("{state}/retro/2026-11-04T21.md"), "old").unwrap();
        std::fs::write(format!("{state}/retro/2026-11-04T22.md"), "来源：t.co\n").unwrap();
        let section = latest_section(&state);
        assert!(section.contains("2026-11-04 22:00") && section.contains("来源：t.co") && !section.contains("old"));
    }
}
