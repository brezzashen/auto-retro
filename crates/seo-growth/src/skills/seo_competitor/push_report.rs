//! The push watchdog's report page (reports.example.com `seo-push`) and the prompts of the two tabs it
//! opens: a repair tab for a broken sitemap, verification file or push, and the Baidu sitemap tab,
//! because Baidu accepts a sitemap only through its web console.

use super::push_site::SiteRound;

fn show(line: &Option<String>) -> String {
    line.clone().unwrap_or_else(|| "未接入".into()).replace('|', "/")
}

pub fn markdown(at_sgt: &str, rounds: &[SiteRound], dispatch: &[String]) -> String {
    let failures: usize = rounds.iter().map(|r| r.failures.len()).sum();
    let mut md = format!("{at_sgt} 更新。{} 个站点，{} 个问题。百度、IndexNow（Bing、Yandex 等）与 Bing URL 提交只推送尚未被接收的 sitemap 地址；百度与 Bing 提交按每日配额分批。\n\n", rounds.len(), failures);
    md.push_str("## 推送进度\n\n");
    let bars: Vec<(String, usize, usize)> = rounds.iter().flat_map(|r| {
        let host = r.site.trim_start_matches("https://").to_string();
        r.accepted.iter().map(move |(engine, n)| (format!("{host} {engine}"), *n, r.urls))
    }).collect();
    if bars.is_empty() {
        md.push_str("还没有可推送的站点。\n\n");
    } else {
        let labels: Vec<String> = bars.iter().map(|(label, _, _)| format!("\"{label}\"")).collect();
        let max = bars.iter().map(|(_, _, total)| *total).max().unwrap_or(1).max(1);
        md.push_str(&format!("```mermaid\nxychart-beta\n%% series: accepted, sitemap\ntitle \"Accepted URLs (bar) vs sitemap URLs (line)\"\nx-axis [{}]\ny-axis \"URLs\" 0 --> {max}\nbar [{}]\nline [{}]\n```\n\n",
            labels.join(", "), bars.iter().map(|b| b.1.to_string()).collect::<Vec<_>>().join(", "), bars.iter().map(|b| b.2.to_string()).collect::<Vec<_>>().join(", ")));
    }
    md.push_str("## 推送状态\n\n| 站点 | sitemap 地址 | 百度 | IndexNow | Bing 提交 | Bing 已索引 | 验证文件 |\n|---|---|---|---|---|---|---|\n");
    for r in rounds {
        let files: Vec<String> = r.files.iter().map(|(path, status)| format!("{path} {status}")).collect();
        let indexed = r.bing_indexed.map_or_else(|| "—".to_string(), |n| n.to_string());
        md.push_str(&format!("| {} | {} | {} | {} | {} | {indexed} | {} |\n", r.site, r.urls, show(&r.baidu), show(&r.indexnow), show(&r.bing), files.join("<br>")));
    }
    md.push_str("\n## 问题\n\n");
    if failures == 0 { md.push_str("没有问题。\n"); }
    for (_, text) in rounds.iter().flat_map(|r| r.failures.iter()) { md.push_str(&format!("- {text}\n")); }
    md.push_str(&format!("\n## 派工\n\n{}\n", if dispatch.is_empty() { "本次不派工".to_string() } else { dispatch.join("\n\n") }));
    md
}

pub fn repair_prompt(page_url: &str, run_dir: &str, failures: &[(String, String)]) -> String {
    let list: Vec<String> = failures.iter().map(|(key, text)| format!("- `{key}` {text}")).collect();
    [
        format!("SEO 推送看门狗（local.autoretro.seo-push）发现问题。监控页：{page_url}；本次数据：{run_dir}/analysis.json。"),
        "步骤：1. 加载 seo-competitor skill 的 Push watchdog 一节；2. 逐条找出原因并修复：验证文件在杭州主机 /var/www/exampleapp-verify/（example.cn）或 Mars_CRM/web/public/（example.com），百度 token 在 ~/.config/mars-seo/，token 失效时在 Chrome 9223 的百度搜索资源平台「普通收录」页重新读取；3. 修好后运行 `mars-skill-tools seo-competitor push` 确认问题消失；4. 发布报告（report skill）说明原因与修复。生产发布与 DNS 变更先问 founder。".into(),
        "问题：".into(),
        list.join("\n"),
    ].join("\n")
}

pub fn sitemap_prompt(page_url: &str, marker: &str, sites: &[String]) -> String {
    [
        format!("SEO 推送看门狗：百度已开始接收 API 推送，现在提交 sitemap。站点：{}。监控页：{page_url}。", sites.join("、")),
        "百度的 sitemap 只能在网页后台提交。步骤：1. 加载 agent-browser skill，用 Chrome 9223（已登录百度账号）打开 https://ziyuan.baidu.com/linksubmit/index?site=<站点>/ 的 sitemap 页；2. 读出「今日提交上限」：为 0 就停下，在回复里说明，看门狗过几天再开标签；3. 大于 0 就提交 <站点>/sitemap.xml，刷新确认列表里出现该地址且状态正常，截图；".into(),
        format!("4. 成功后运行 `touch {marker}`，看门狗不再开这个标签；5. 在 seo 技能 references/engines.md 的百度一行记下提交日期与结果。"),
    ].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_lists_each_site_and_says_when_nothing_is_wrong() {
        let round = SiteRound { site: "https://example.cn".into(), urls: 16, baidu: Some("本次推送 9 个".into()), indexnow: None,
            bing: None, bing_indexed: Some(3), files: vec![("/BingSiteAuth.xml".into(), 200)], accepted: vec![("百度".into(), 10)], failures: vec![] };
        let md = markdown("2026-11-03 22:00", &[round], &[]);
        assert!(md.contains("| https://example.cn | 16 | 本次推送 9 个 | 未接入 | 未接入 | 3 | /BingSiteAuth.xml 200 |"));
        assert!(md.contains("没有问题"));
        assert!(md.contains("bar [10]") && md.contains("line [16]"));
    }

    #[test]
    fn the_sitemap_prompt_stops_on_zero_quota_and_names_the_marker() {
        let text = sitemap_prompt("https://report", "/s/marker", &["https://example.cn".into()]);
        assert!(text.contains("为 0 就停下"));
        assert!(text.contains("touch /s/marker"));
    }
}
