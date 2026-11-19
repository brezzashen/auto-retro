//! Step 4.8 of the hourly SEO retro (founder 2026-11-18): the Google Ads bid review and results review, read from
//! the watchdog's `googleAds` block (`watch_ads.rs`) and checked against PostHog and the hour's database rows.

pub fn step(run_dir: &str) -> String {
    format!("4.8 谷歌广告：出价复盘与实际效果复盘（账户 594-270-3856，founder 2026-11-18）：读 {run_dir}/analysis.json 的 googleAds（tabs.campaigns_today 是今天各系列的预算、花费、点击、展示、CTR、平均 CPC、转化与搜索展示份额，以及因预算、因排名损失的份额；campaigns_hourly 是今天按小时；campaigns_7d 是近 7 天每天；keywords_bids 是每个词的当前出价 effective_cpc_bid_micros、首页出价 first_page_cpc_micros、页首出价 top_of_page_cpc_micros、首位出价、质量得分、审核与投放状态；keywords_today、keywords_7d 是每个词的展示、点击、花费与展示份额；search_terms_today 是今天的真实搜索词；ads_review 是广告审核；meta.updated 是 Ads 脚本最后写入的时间；*_micros 除以 1,000,000 是美元），不要自己去 Ads 后台抓页面；googleAds.source = ui 表示 Ads 脚本还没授权，数据是看门狗从 Ads 后台表格读的，只有 campaigns_today、campaigns_7d、keywords_today、keywords_7d，字段是表格列（name、budget_amount、primary_status、keyword_text、match_type、bid_config.effective_cpc_bid 是当前最高出价、stats.impressions、stats.clicks、stats.cost、stats.cost_per_click、stats.conversions，金额已带 $），没有首页出价估计和搜索词，出价复盘就按展示为 0、CTR 和花费判断，并在报告里写一句脚本仍待授权；googleAds 为 null，或带 skipped、error，就写明原因并跳过数字部分；带 partial 表示后台读表超时，只有 failed.pending 以外的表可用，照实写明缺哪几张表；meta.updated 早于本轮整点一小时以上要写明数据停在哪个时间。预算按现有系列与日预算照常花（founder 2026-11-18），复盘重点是异常：先把本小时与今天每个系列、每个词的展示、CTR、平均 CPC、花费与点击→注册对比近 7 天同系列的水平，偏离两倍以上、从 0 到有或从有到 0 的写成「异常好」或「异常差」并给出可能原因与证据，没有异常就写一句「广告本小时无异常」，不要逐词罗列平稳的数字。同行出价对比：keywords_bids 有数据时，用 first_page_cpc_micros 与 top_of_page_cpc_micros 作同行竞价门槛，和我们的当前出价、实际平均 CPC 对比，标出低于首页出价和高于页首出价一倍以上的词；没有这些估计（如 source = ui）就写一句「同行出价估计缺失（Ads 脚本待授权）」，不要拿别的数字冒充。新词建议优先工作与高价值商务场景（识别人、名片与对话录入 CRM、识别客户意向），情感与约会词留给自然搜索，不进广告。报告加两节。「出价复盘」：列出出价低于首页出价、拿不到展示的词，因排名损失展示份额高的词，花费高但无转化的词，以及今天该加否定或该加成新词的搜索词；按系列给出调价建议（具体词、当前出价→建议出价、理由）；只写建议，不改账户。「实际效果复盘」：本小时和今天累计的花费、点击、平均 CPC、Google Ads 记录的 Sign-up 与 Purchase 转化；广告今天有点击时，用 mars-skill-tools conversion-analytics query 写一条 HogQL，查本小时 properties.$current_url 含 'gclid=' 的 $pageview 访客，按人去重，带上落地页、国家、是否 cta_clicked、auth_otp_verified、message_sent；账户从 2026-11-19 起有最终网址后缀 utm_source=google&utm_medium=cpc&utm_campaign={{campaignid}}&utm_term={{keyword}}，再从 $current_url 取出 utm_term 与 utm_campaign，按关键词写出访客、注册与付费，词池里的词以此判断哪个词带来注册；再和 traffic.hour.db 的新注册与订单对照，算出每次注册成本和各国家、各系列的效果；没有点击就不查 PostHog；今天花费为 0 时说明原因是广告审核中、预算、出价还是展示份额。不写赠金门槛与进度（founder 2026-11-18：赠金月底才到账，先不管）。关键词池（founder 2026-11-18）：读 {run_dir}/analysis.json 的 cpcPool，只写 newFlags（今天新出现标记的词）：先写低投入高收益、低成本转化、搜索词高效或带来转化的词（词、系列、点击、CPC 与同系列对比、转化），再写花费无转化、建议加否定词与长期零展示的词；没有 newFlags 就写一句「词池无新异常」。词池提案（founder 2026-11-19：每小时只预览，写进本报告，等 founder 确认后才应用）：cpcPool.proposals.addRows、pauseRows 是提案的词（系列、广告组、词），add、pause 是对应的 CSV，preview 是看门狗已在 Ads 批量上传里预览的结果（kind、file、ok 是否预览通过、validChanges 有效条数、fresh 是否本小时新预览、error 预览失败原因）；有 fresh 的预览时，报告加一节「词池提案预览」，列出每个提案词、所在系列与广告组、为什么提（对应的标记与数字）和预览结果，并在 confirm 里加一项「是否应用 <file 文件名>」，选项 A 应用、B 不应用，按证据标出推荐项；没有 fresh 的预览只写一句「提案未变（<at 时间> 已预览）」，不重复提问；preview 带 error 就写明失败原因。复盘本身不上传、不应用，复盘只写建议，不上传；founder 在报告里选了应用（消息形如 `<slug> 第N项选A：应用`），才运行 `ADS_UPLOAD_FILE=<该 file> ADS_APPLY=1 bun .claude/skills/agent-browser/scripts/lease-page.js --label ads-bulk --out <scratch> --profile 9223 --timeout 70000 --budget 600000 .claude/skills/seo-competitor/scripts/ads-bulk.js`（它直接应用已预览的那一次，不重复上传；<scratch>/bulk-<文件名>.json 的 applied 为 true 才算成功），再用 ads-ui.js 回读新词状态并写进报告；否定词不能批量上传，用 ads-negatives.js（见 references/cpc-pool.md）。")
}

/// Step 9 of the daily retro: the keyword pool review (founder 2026-11-18), proposals previewed but never applied.
pub fn daily_step(run_dir: &str) -> String {
    format!("9. CPC 关键词池复盘（founder 2026-11-18，在第 7 步发布前完成）：读 {run_dir}/analysis.json 的 cpcPool（pool 是词池文件路径；counts 是在投、搜索词、候选、排除的数量；expand 是今天的规划师补数与扩词；flaggedRows 是带标记的词，字段 impressions、clicks、cost、cpc、ctr、conversions 是 Ads 近 7 天加今天，plannerVolume 与 plannerBidHigh 是新马菲港台合计月搜索量与最高首页出价；proposals.add 与 proposals.pause 是 Google Ads Editor 格式的 CSV，proposals.negatives 是建议否定的搜索词；阈值在 .claude/skills/seo-competitor/references/watch.config.json 的 googleAds.pool.thresholds）。报告加一节「CPC 关键词池复盘」，用柱状图比较异常词与同系列的 CPC：先列异常特征词，即低投入高收益、低成本转化、搜索词高效、搜索词带来转化，每个写所在系列、点击、CPC 与同系列 CPC、转化、规划师搜索量与首页出价，以及可能原因；再列花费无转化、长期零展示、点击率低的词；然后列便宜有量的新候选前 10 个。建议动作写清加哪些词、暂停哪些词、哪些搜索词加否定；proposals.preview 是看门狗每小时已在 Ads 预览的结果（founder 2026-11-19：提案在每小时复盘里预览并等确认），日复盘不再上传预览，只预览、不设 ADS_APPLY，汇总今天各小时提案的预览结果和 founder 的应用决定，没确认的提案照列一次。情感、约会、挽回类词不进广告；client-intent 主题还没有广告组，出现高效词时建议新建广告组。")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_step_reads_the_collected_block_and_never_changes_the_account() {
        let text = step("/s/runs/x");
        assert!(text.contains("/s/runs/x/analysis.json 的 googleAds") && text.contains("不要自己去 Ads 后台抓页面"));
        assert!(text.contains("「出价复盘」") && text.contains("「实际效果复盘」") && text.contains("只写建议，不改账户"));
        assert!(text.contains("googleAds.source = ui") && text.contains("bid_config.effective_cpc_bid"), "the UI fallback names its own fields");
        assert!(text.contains("utm_term={keyword}") && text.contains("按关键词写出访客"), "founder 2026-11-19: the final URL suffix ties sign-ups to keywords");
        assert!(text.contains("gclid=") && text.contains("没有点击就不查 PostHog"), "PostHog's hourly read budget is spent only when ads drew clicks");
        assert!(text.contains("不写赠金门槛与进度") && !text.contains("1,911.74"), "the promo credit is not tracked hourly");
        assert!(text.contains("「异常好」") && text.contains("广告本小时无异常"), "the review leads with anomalies, not steady numbers");
        assert!(text.contains("同行出价估计缺失") && text.contains("情感与约会词留给自然搜索"), "peer bids come only from Google's estimates; paid words stay business");
        assert!(text.contains("cpcPool") && text.contains("词池无新异常") && text.contains("复盘只写建议，不上传"), "the hourly review names only new pool flags");
        assert!(text.contains("「词池提案预览」") && text.contains("fresh") && text.contains("ADS_APPLY=1") && text.contains("选了应用"), "founder 2026-11-19: proposals are previewed hourly and applied only after the founder picks it");
        let daily = daily_step("/s/runs/x");
        assert!(daily.contains("/s/runs/x/analysis.json 的 cpcPool") && daily.contains("proposals.preview") && daily.contains("只预览、不设 ADS_APPLY"), "the daily review previews proposals, never applies them");
    }
}
