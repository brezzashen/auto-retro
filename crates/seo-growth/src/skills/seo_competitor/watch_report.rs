//! The watchdog's live report page (reports.example.com `<slug>`, encrypted: it carries our own
//! Search Console data) and the prompt of the tab it opens to optimize or review.

use super::watch_advisor::ADVISOR_WAIT;
use super::watch_analyze::{Finding, QueryRow, TargetRow, Window};
use super::watch_judge_rules::CORE_MARKET;
use super::watch_probe::PageProbe;
use super::watch_traffic::Traffic;

/// Everything one hourly page shows.
pub struct Page<'a> {
    pub at_sgt: &'a str,
    pub probes: &'a [PageProbe],
    pub indexed: (usize, usize),
    pub last: &'a Window,
    pub previous: &'a Window,
    /// `(Singapore "MM-DD HH", impressions)` per hour, oldest first.
    pub hourly: &'a [(String, f64)],
    pub targets: &'a [TargetRow],
    pub top: &'a [QueryRow],
    /// PostHog production traffic; `None` when PostHog did not answer.
    pub traffic: Option<&'a Traffic>,
    pub findings: &'a [Finding],
    pub errors: &'a [String],
    pub dispatch: &'a str,
    /// The keyword-room sections (`watch_board`), shown right after the hour's findings.
    pub keywords: &'a str,
}

fn escape(text: &str) -> String {
    text.replace('|', "/").replace('"', "'")
}

fn chart_label(text: &str) -> String {
    let short: String = text.chars().take(28).collect();
    format!("\"{}\"", escape(&short))
}

pub fn markdown(page: &Page) -> String {
    let failing = page.probes.iter().filter(|p| !p.problems.is_empty()).count();
    let mut md = format!(
        "{} 更新。页面健康 {}/{} 正常；已检查收录 {}/{}；Search Console 最近 24 个完整小时展示 {}、点击 {}（前 24 小时 {} / {}）。\n\n",
        page.at_sgt, page.probes.len() - failing, page.probes.len(), page.indexed.0, page.indexed.1,
        page.last.impressions, page.last.clicks, page.previous.impressions, page.previous.clicks,
    );
    md.push_str("## 本小时建议\n\n");
    if page.findings.is_empty() { md.push_str("没有需要处理的事项。\n\n"); }
    for f in page.findings { md.push_str(&format!("- **{}** {} → {}\n", f.severity, f.title, f.action)); }
    md.push_str(&format!("\n派工：{}\n\n", page.dispatch));
    md.push_str(page.keywords);

    md.push_str("## 每小时搜索展示（Google，新加坡时间）\n\n");
    if page.hourly.is_empty() {
        md.push_str("Search Console 这段时间没有小时数据。\n\n");
    } else {
        let labels: Vec<String> = page.hourly.iter().map(|(hour, _)| chart_label(hour)).collect();
        let values: Vec<String> = page.hourly.iter().map(|(_, v)| format!("{v}")).collect();
        let max = page.hourly.iter().map(|(_, v)| *v).fold(1.0_f64, f64::max);
        md.push_str(&format!("```mermaid\nxychart-beta\ntitle \"Impressions per hour\"\nx-axis [{}]\ny-axis \"Impressions\" 0 --> {}\nbar [{}]\n```\n\n",
            labels.join(", "), max.ceil(), values.join(", ")));
    }

    if let Some(t) = page.traffic {
        md.push_str(&super::watch_segment_view::section(&t.segments, t.segments_error.as_deref()));
        md.push_str(&super::watch_payments::section(&t.payments, t.payments_error.as_deref()));
    }
    md.push_str(&traffic_section(page.traffic));

    md.push_str("## 目标关键词（近 28 天）\n\n");
    let shown: Vec<&TargetRow> = page.targets.iter().filter(|t| t.impressions > 0.0).collect();
    if shown.is_empty() {
        md.push_str("目标词还没有展示数据：新页面通常需要 1-4 周进入索引并开始获得展示。\n\n");
    } else {
        md.push_str(&format!("```mermaid\nxychart-beta horizontal\ntitle \"Impressions per target keyword (28 days)\"\nx-axis [{}]\ny-axis \"Impressions\" 0 --> {}\nbar [{}]\n```\n\n",
            shown.iter().map(|t| chart_label(&t.keyword)).collect::<Vec<_>>().join(", "),
            shown.iter().map(|t| t.impressions).fold(1.0_f64, f64::max).ceil(),
            shown.iter().map(|t| format!("{}", t.impressions)).collect::<Vec<_>>().join(", ")));
    }
    md.push_str("| 关键词 | 落地页 | 展示 | 点击 | 平均排名 |\n|---|---|---|---|---|\n");
    for t in page.targets {
        let position = if t.position > 0.0 { format!("{:.1}", t.position) } else { "—".into() };
        md.push_str(&format!("| {} | {} | {} | {} | {} |\n", escape(&t.keyword), t.page, t.impressions, t.clicks, position));
    }

    md.push_str("\n## 实际搜索词（近 28 天，按展示排序）\n\n| 搜索词 | 页面 | 展示 | 点击 | 排名 |\n|---|---|---|---|---|\n");
    for q in page.top { md.push_str(&format!("| {} | {} | {} | {} | {:.1} |\n", escape(&q.query), escape(&q.page), q.impressions, q.clicks, q.position)); }

    md.push_str("\n## 页面健康\n\n");
    if failing == 0 { md.push_str(&format!("站点地图里 {} 个地址全部 HTTP 200，语言声明、canonical 与 hreflang 都正确。\n", page.probes.len())); }
    for p in page.probes.iter().filter(|p| !p.problems.is_empty()) { md.push_str(&format!("- {}：{}\n", p.url, p.problems.join("；"))); }
    if !page.errors.is_empty() { md.push_str(&format!("\n数据源错误：{}\n", page.errors.join("；"))); }
    md
}

/// Real visitors per day and the top sources of the recent days, from PostHog.
fn traffic_section(traffic: Option<&Traffic>) -> String {
    let mut md = String::from("## 流量来源（PostHog，生产站真实访客）\n\n");
    let Some(t) = traffic.filter(|t| !t.days.is_empty()) else { return md + "PostHog 本次没有返回数据，见页面健康一节的数据源错误。\n\n"; };
    let agents = super::watch_traffic::MACHINE_AGENTS.iter().chain(super::watch_fingerprints::MACHINE_FINGERPRINTS).map(|(_, name)| name.to_string())
        .chain(std::iter::once(super::watch_fingerprints::IDC_SINGLE_HIT.to_string()));
    let machines = t.machine_ip_prefixes.iter().map(|m| format!("{}*", m.prefix)).chain(agents).collect::<Vec<_>>().join("、");
    md.push_str(&format!("每天的独立访客（新加坡时间），已扣除我们自己机器的访问（{}、局域网地址）和已查明的机器流量（{}，见 traffic-sources.md）。当天还没结束。\n\n",
        t.own_ips.join("、"), if machines.is_empty() { "无".into() } else { machines }));
    md.push_str(&format!("```mermaid\nxychart-beta\ntitle \"Real visitors per day\"\n%% series: Real visitors, Own visits, Machine visits\nx-axis [{}]\ny-axis \"Visitors\" 0 --> {}\nline [{}]\nline [{}]\nline [{}]\n```\n\n",
        t.days.iter().map(|d| chart_label(d.day.get(5..).unwrap_or(&d.day))).collect::<Vec<_>>().join(", "),
        t.days.iter().map(|d| d.visitors.max(d.own).max(d.machine)).fold(1.0_f64, f64::max).ceil(),
        t.days.iter().map(|d| format!("{}", d.visitors)).collect::<Vec<_>>().join(", "),
        t.days.iter().map(|d| format!("{}", d.own)).collect::<Vec<_>>().join(", "),
        t.days.iter().map(|d| format!("{}", d.machine)).collect::<Vec<_>>().join(", ")));
    md.push_str("| 日期 | 来源 | 访客 | 只看一页 | 当天首次访问 | 首次落地页 |\n|---|---|---|---|---|---|\n");
    let mut shown = std::collections::BTreeMap::<&str, usize>::new();
    for s in &t.sources {
        let n = shown.entry(&s.day).or_default();
        *n += 1;
        if *n <= 8 { md.push_str(&format!("| {} | {} | {} | {}{} | {} | {} |\n", s.day, escape(&s.source), s.visitors, s.pageview_only,
            if s.suspected_bot() { "（疑似机器）" } else { "" }, s.first_seen, escape(&s.landing))); }
    }
    md + "\n"
}

fn task_lines(kind: &str, run_dir: &str) -> [String; 2] {
    if kind == "找词" {
        return [
            "授权（founder 2026-11-02 与 2026-11-04「第一周直接根据词进行修改，每天迭代一次」）：直接按下面的词修改关键词落地页与 SEO 元数据（Mars_CRM/web/src/screens/marketing/keywords/**、Mars_CRM/web/src/seo/**、.claude/skills/seo-competitor/references/watch.config.json 的 targets），跑 web 测试，commit/push，交付 beta，验收后交付 Canary，Canary 验收通过后直接 promote 到生产 example.com，无需再问（founder 2026-11-04「找词的 skill 可以不用我授权，直接更新生产」）；每天最多新建 1 个落地页。".into(),
            format!("步骤：1. 加载 mars-crm 与 seo-competitor skill（关键词页只复用 Wiki 详情页与 PricingFaq 样式，见「Public pages reuse the existing styles」）；2. 对抗唱反调（Opus 5.5 主导 × Gemini 4 Neon，founder 2026-11-17；辩手只有这两方）：主会话自己逐个看词与 SERP 文件，写出做 / 并入 / 不做的初判（读文件可按 subagent-routing 派快模型子代理整理数据，但子代理不给做或不做的结论，报告也不把它列为唱反调一方），把拟做词与并入方案写入临时文件 `<scratch>/picks-draft.md` 并运行 `mars-skill-tools seo-competitor advisor --run {run_dir} --mode keywords --draft <scratch>/picks-draft.md --out {run_dir}/advisor-keywords.md`{ADVISOR_WAIT}（调用 ag1 的 Gemini 4 Neon 站在搜索生态外部视角对抗审查：意图不对口、巨头或 UGC 霸屏、AI Overview 零点击、与现有页内耗、量太小；{CORE_MARKET}唱反调不能以真人或本地服务意图否决这类词）；经源码与 SERP 核实确应否决的词向 ~/.local/state/seo-watch/keyword-vetoes.jsonl 追加一行 {{\"keyword\":…,\"reason\":…,\"at\":…}}，不做；3. 其余每个词默认并入「最接近页面」：en、zh-CN、zh-TW 三份文案各加一个含该词的 H2、一段具体用例和一条 FAQ（FAQ 与 FAQPage JSON-LD 一致），标题与描述只在该词比现有主词更强时改；只有该词意图与该页主词明显不同（例如 best X 对比清单对工具页）时新建关键词页，每天最多 1 个（KEYWORD_PAGE_SLUGS 新 slug、三种语言文案、sitemap 三语言 hreflang）；4. 每个已改的词在 watch.config.json 的 targets 加一行 {{keyword, page}}，Watchdog 据此记为已上线并开始 28 天复盘；5. beta 上 1440 与 390 截图验收，并按 mars-crm 交付第 3b 步派 feature-verifier（本轮提交区间 <first sha>^..<last sha>），通过后交付 Canary，Canary 截图验收后 `mars deliver accept <canary run> --verdict=passed --note=\"<本提交 sha> …\"`；6. 上生产：`git log <生产 commit>..<canary commit> -- Mars_CRM` 列出同车的产品提交，每个都要有负责会话在 Canary 的 passed 结论（缺的等该会话或 release-queue，不问创始人）；已有会话在主导 promote 时只用 session-send 告诉它你的结论，不重复 promote；否则 `mars canary promote --dry-run` 再 `mars canary promote`，之后 curl example.com 改动页的非 JS HTML 与 FAQPage JSON-LD 并截图；7. 发布报告（report skill）：每个词改了哪页哪段、依据的数据、「对抗唱反调（Opus 5.5 × Gemini 4 Neon）」一节写分歧、核查证据与采纳 / 驳回 / 折中裁决、生产验收、7/14/28 天看哪个指标。"),
        ];
    }
    if kind == "流量溯源" {
        return [
            "授权：只读调查与发布报告；不改代码。X 搜索用 x-dev skill（Monid），读 @example_handle 的通知、提及与引用可在 debug Chrome 9223 的后台页面里做（x.com 已登录）。".into(),
            "步骤：1. 加载 seo-competitor 与 conversion-analytics skill；2. 对每个突增来源按小时拆 PostHog 访客，找到首次访问时刻；3. t.co / x.com 来源：`x-dev search 'example.com' --days 7 --sort top` 与 `x-dev search '@example_handle' --replies` 列出链接我们或提及 @example_handle 的帖子；来源一栏的 https://t.co/<code> 是某条帖子正文里的短链，正文含同一短链的帖子就是它（只有 https://t.co/ 的访客来自 X 应用内浏览器，按时间分摊）；PostHog 生产站采集从 2026-11-02 22:38 开始，更早的帖子只能看到它之后的访客；其他来源打开 referrer 页面；4. 发布溯源报告（report skill）：每个来源的发帖人、帖子链接、发帖时间、帖子的浏览与互动数、带来的访客与注册数，以及是否值得跟进（回复、转发、联系作者），并把来源写进 seo-competitor skill 的流量来源记录。".into(),
        ];
    }
    [
        "授权（founder 2026-11-02）：可直接修改 SEO 文案与元数据（Mars_CRM/web/src/screens/marketing/keywords/**、Mars_CRM/web/src/seo/**），跑 web 测试，commit/push，交付 beta，验收并按 mars-crm 交付第 3b 步派 feature-verifier（本轮提交区间）后交付 Canary；绝不 promote 到生产（example.com 需 founder 单独要求）。".into(),
        format!("步骤：1. 加载 mars-crm 与 seo-competitor skill；2. 先读 {run_dir}/analysis.json 的 `anomalies`（监控页「异常指标捕捉」6 个维度：量价背离/首页低 CTR、关键词内耗、排名骤降与 SERP/GSC 背离、Google 规范页冲突、新访客 CTA→注册漏斗断裂与疑似机器批次、Google/Bing 跨引擎断层）与下面事项，结合对应文案文件 `git log -3`（排除 7 天内已改标题或已在 Canary 待 promote 的项），把 Fable 5.2 主导会话的初判、拟改方案与拟跳过项写到临时文件 `<scratch>/fable-draft.md`；3. 双顾问对抗性分析（founder 2026-11-11，Fable 5.2 主导必跑）：执行 `mars-skill-tools seo-competitor advisor --run {run_dir} --mode daily --draft <scratch>/fable-draft.md --out {run_dir}/advisor-neon.md`{ADVISOR_WAIT}，由 ag1 的 Gemini 4 Neon 站在外部红队视角反驳 Fable 5.2 的假设、指出遗漏异常与否决项；主会话逐条对照源码、GSC 与 PostHog 原始数据核实分歧（采纳 / 驳回 / 折中，证据优先，不盲从），只对经对抗检验且与 Mars 已上线功能对口的改动动手，三种语言同步，页面只复用现有样式（关键词页 = Mars Wiki 详情页 changelog-detail-page.css + 定价页 PricingFaq 卡片），不新建 CSS 文件、不改与 SEO 无关的代码；4. 复盘监控页「流量来源」与「转化与异常批次」：比较昨天与之前各来源访客与 CTA→注册漏斗，未溯源突增按流量溯源步骤查明，确认的固定机房 IP 段或机器 UA 记入 traffic-sources.md 与配置；5. 看监控页「异常上升词（Google Trends）」一节（判断记录 ~/.local/state/seo-watch/trend-labels.jsonl）：还没判断的未知新词先查清是什么再补判断（不认识不等于噪声），判为机会的词是否已进找词看板，连续 3 天以上判观察的词是否该改判（{CORE_MARKET}此前只因真人或本地服务意图判观察的这类词改判机会），升温的关注领域给出原因和对口页面；6. Bing 与 DuckDuckGo（DuckDuckGo 没有站长平台，结果主要来自 Bing 索引，Yahoo 也用 Bing，所以 Bing 收录就是 DuckDuckGo 收录）：先读监控页「Bing 排名与收录」一节（每小时由 Webmaster API 与 SerpApi 轮查，状态在 ~/.local/state/seo-watch/bing.json）：比较目标词的 Bing 名次与 Google 名次，Bing 首页外而 Google 前 10 的词说明 Bing 缺的是收录或外链而不是文案；从未抓取的目标页用 Bing Webmaster GetUrlInfo 复核（key 在 ~/.config/mars-seo/bing-webmaster.apikey，顺序调用约 2.5 s 一次，否则 Throttle；LastCrawledDate 为第 1 年即从未抓取），用 URL Submission API 重新提交（每站每天 100 条），`mars-skill-tools seo-competitor push` 重推 IndexNow；curl https://example.com/robots.txt 确认 Bingbot 与 DuckDuckBot 仍是 Allow；比较流量来源里 duckduckgo.com 与 bing.com 的访客；在 9223 lease 页面用 duckduckgo.com/?kl=us-en 搜品牌词与 2 个 targets 词并记录 example.com 名次（遇验证码记“未测量”，不记 0；site: 查询必被拦截，不用）；文案只按 Google 与 Bing 共用的标题、描述、H2 改，不为 DuckDuckGo 单独写；7. 发布复盘报告（report skill）：含一节「双顾问对抗复盘（Fable 5.2 × Gemini 4 Neon）」展示分歧、核查证据与最终裁决，写清改了什么、依据哪个指标、预期哪个指标在几天内变化，并更新 seo-competitor skill 的 Watchdog 一节如有新规律。"),
    ]
}

/// The Google billing step of the daily retro only: GKE, Gemini and the hkgcp account's real spend.
fn billing_step(run_dir: &str) -> String {
    format!("8. 谷歌账单复盘（founder 2026-11-12，在第 7 步发布前完成）：运行 `bun ~/.codex/skills/gcp-billing/scripts/retro.ts --md {run_dir}/billing.md`（只读 BigQuery 账单导出，约 20 秒），把输出的「## 谷歌账单复盘」一节整节放进复盘报告（自带每日折线图和表格）；在该节开头补 2–4 句结论：最近一个计费日比前 7 天日均，GKE（按集群）、Gemini（按模型）、其他（按服务）哪项变化最大、为什么（对照当天部署与 Cloud Build 次数、Mars CRM 对话量与模型路由），hkgcp（user@example.com 名下，USD，净额是真实花费）和 gfs（GFS 赠金，SGD，用量是赠金消耗速度）各自是否异常；两种币种不相加；脚本标注未完整的计费日照实写。脚本失败时把错误写进报告并按 gcp-billing skill 排查（`bun ~/.codex/skills/gcp-billing/scripts/billing.ts status`），不手写 SQL，不改任何账单、预算或导出设置。")
}

/// The prompt of an optimization, review or traffic-attribution tab; `findings` are the new actionable ones.
pub fn prompt(kind: &str, page_url: &str, run_dir: &str, findings: &[Finding]) -> String {
    let list: Vec<String> = findings.iter().map(|f| format!("- [{}] {} → {}", f.severity, f.title, f.action)).collect();
    let [authority, steps] = task_lines(kind, run_dir);
    let mut lines = vec![
        format!("SEO Watchdog（{kind}）：example.com 每小时巡检发现需要处理的事项。监控页：{page_url}；本次数据：{run_dir}/analysis.json。"),
        authority,
        steps,
    ];
    if kind == "每日复盘" { lines.push(billing_step(run_dir)); lines.push(super::watch_ads_retro::daily_step(run_dir)); }
    lines.extend(["事项：".into(), list.join("\n")]);
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_hour_says_so_and_still_lists_targets() {
        let target = TargetRow { keyword: "personal crm".into(), page: "/personal-crm".into(), impressions: 0.0, clicks: 0.0, position: 0.0 };
        let window = Window::default();
        let md = markdown(&Page { at_sgt: "2026-11-02 14:00", probes: &[], indexed: (0, 0), last: &window, previous: &window,
            hourly: &[], targets: &[target], top: &[], traffic: None, findings: &[], errors: &[], dispatch: "无", keywords: "" });
        assert!(md.contains("没有需要处理的事项"));
        assert!(md.contains("| personal crm | /personal-crm | 0 | 0 | — |"));
    }

    #[test]
    fn the_prompt_carries_the_authority_and_every_finding() {
        let f = Finding { key: "rank:x".into(), severity: "P1".into(), title: "t".into(), action: "a".into() };
        let text = prompt("优化", "https://report", "/tmp/run", &[f]);
        assert!(text.contains("绝不 promote 到生产"));
        assert!(text.contains("- [P1] t → a"));
        assert!(prompt("每日复盘", "https://report", "/tmp/run", &[]).contains("trend-labels.jsonl"));
        let daily = prompt("每日复盘", "https://report", "/tmp/run", &[]);
        assert!(daily.contains("DuckDuckGo") && daily.contains("GetUrlInfo") && daily.contains("DuckDuckBot"));
        assert!(daily.contains(CORE_MARKET) && daily.contains("改判机会"), "the daily retro re-judges human-service 观察 words");
        assert!(daily.contains("gcp-billing/scripts/retro.ts --md /tmp/run/billing.md") && daily.contains("user@example.com"), "the daily retro reviews Google billing");
        assert!(["复盘", "关键词优化", "健康修复", "找词", "流量溯源"].iter().all(|k| !prompt(k, "https://report", "/tmp/run", &[]).contains("retro.ts")), "only the daily retro runs the billing step");
        let room = prompt("找词", "https://report", "/tmp/run", &[]);
        assert!(room.contains(CORE_MARKET) && room.contains("不能以真人或本地服务意图否决"), "the keyword room keeps counselling words");
        assert!(room.contains("Opus 5.5 主导 × Gemini 4 Neon") && room.contains("子代理不给做或不做的结论") && !room.contains("双重唱反调"), "founder 2026-11-17: the keyword room debate is Opus 5.5 against Neon only");
    }

    #[test]
    fn only_the_keyword_room_promotes_to_production() {
        let text = prompt("找词", "https://report", "/tmp/run", &[]);
        assert!(text.contains("直接 promote 到生产") && !text.contains("绝不 promote"));
        assert!(prompt("复盘", "https://report", "/tmp/run", &[]).contains("绝不 promote 到生产"));
    }

    #[test]
    fn the_attribution_prompt_is_read_only_and_starts_from_the_founder_account() {
        let text = prompt("流量溯源", "https://report", "/tmp/run", &[]);
        assert!(text.contains("不改代码") && text.contains("@example_handle"));
    }

    #[test]
    fn daily_retro_and_keywords_invoke_gemini_neon_adversarial_advisor() {
        let daily = prompt("每日复盘", "https://report", "/tmp/run-09", &[]);
        assert!(daily.contains("seo-competitor advisor --run /tmp/run-09 --mode daily"));
        assert!(daily.contains("Gemini 4 Neon") && daily.contains("anomalies"));
        let kw = prompt("找词", "https://report", "/tmp/run-08", &[]);
        assert!(kw.contains("seo-competitor advisor --run /tmp/run-08 --mode keywords"));
        for text in [&daily, &kw] {
            assert!(text.contains(super::ADVISOR_WAIT) && text.contains("--out /tmp/run-"), "conversation retro 813130305414909650913: one background wait per advisor run");
        }
    }
}
