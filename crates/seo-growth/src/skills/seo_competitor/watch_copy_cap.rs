//! Report-only rule for over-edited keyword pages (founder 2026-11-12, report
//! `seo-daily-retro-20261112` item 3): a rank or CTR finding of a page whose English copy block took
//! `commits` or more commits in `days` days is shown but never handed to a tab. New pages of the
//! keyword room and an anomaly word's first merge are other tabs' work and stay unlimited.

use super::watch_analyze::Finding;
use super::watch_config::TargetKeyword;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::process::Command;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CopyCap {
    pub days: u32,
    pub commits: usize,
    /// English keyword copy table, relative to the repository root the watchdog runs in.
    pub file: String,
}

/// Key suffix of a held finding: the plain key stays unhandled, so it is new work once the page has cooled.
const HOLD: &str = ":hold";

/// The copy block of a page and its locale copies: `/ja/text-message-analyzer` → `text-message-analyzer`.
fn slug(page: &str, site: &str) -> Option<String> {
    let last = page.strip_prefix(site).unwrap_or(page).trim_end_matches('/').rsplit('/').next()?;
    (!last.is_empty() && last.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')).then(|| last.to_string())
}

/// The page a rank or CTR finding is about; other findings have none.
fn page_of(key: &str, targets: &[TargetKeyword]) -> Option<String> {
    if let Some(page) = key.strip_prefix("ctr:").or_else(|| key.strip_prefix("anomaly:page1-ctr:")) { return Some(page.to_string()); }
    let rest = key.strip_prefix("rank:").or_else(|| key.strip_prefix("stall:"))?;
    let names = |k: &str| rest == k || rest.ends_with(&format!(":{k}")) || rest.starts_with(&format!("{k}:")) || rest.contains(&format!(":{k}:"));
    targets.iter().filter(|t| names(&t.keyword)).max_by_key(|t| t.keyword.len()).map(|t| t.page.clone())
}

/// Commits of the last `days` days that touched the page's block in the copy table.
pub fn commits(cap: &CopyCap, slug: &str) -> usize {
    let range = format!("/^  .{slug}.: {{/,/^  }},/:{}", cap.file);
    let since = format!("--since={} days ago", cap.days);
    Command::new("git").args(["log", &since, "--format=%H", "-s", "-L", &range]).output().ok().filter(|o| o.status.success())
        .map_or(0, |o| String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.len() == 40 && l.bytes().all(|b| b.is_ascii_hexdigit())).count())
}

/// Holds the P1 rank and CTR findings of pages at or over the cap and returns those pages with their counts.
pub fn apply(findings: &mut [Finding], targets: &[TargetKeyword], site: &str, cap: &CopyCap, count: &dyn Fn(&str) -> usize) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for finding in findings.iter_mut().filter(|f| f.severity == "P1") {
        let Some(slug) = page_of(&finding.key, targets).and_then(|page| slug(&page, site)) else { continue };
        let n = *counts.entry(slug.clone()).or_insert_with(|| count(&slug));
        if n < cap.commits { continue; }
        finding.key.push_str(HOLD);
        finding.severity = "P2".into();
        finding.title = format!("{}（该页近 {} 天已有 {n} 次文案提交，只报告不改）", finding.title, cap.days);
        finding.action = format!("不改文案：排名类与 CTR 类事项对 {} 天内已有 {} 次以上文案提交的页面只报告（创始人 2026-11-12）；提交数降下来后本项会重新报出", cap.days, cap.commits);
    }
    counts.retain(|_, n| *n >= cap.commits);
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(key: &str) -> Finding {
        Finding { key: key.into(), severity: "P1".into(), title: "t".into(), action: "a".into() }
    }

    fn target(keyword: &str, page: &str) -> TargetKeyword {
        TargetKeyword { keyword: keyword.into(), page: page.into() }
    }

    #[test]
    fn rank_and_ctr_findings_of_an_over_edited_page_are_held_and_others_pass() {
        let cap = CopyCap { days: 7, commits: 3, file: "keywordPages.en.ts".into() };
        let targets = [target("ai email", "/ai-email-reply"), target("ai email assistant", "/ai-email-reply"),
            target("line トーク 分析", "/ja/text-message-analyzer"), target("personal crm", "/personal-crm")];
        let mut findings = vec![
            finding("rank:absent:ai email assistant"), finding("rank:deep:line トーク 分析"), finding("stall:personal crm:1"),
            finding("ctr:https://example.com/text-message-analyzer"), finding("anomaly:page1-ctr:/personal-crm"),
            finding("index:https://example.com/text-message-analyzer"), finding("anomaly:page-cliff:/text-message-analyzer"),
        ];
        let count = |slug: &str| match slug { "text-message-analyzer" => 15, "ai-email-reply" => 3, _ => 2 };
        let held = apply(&mut findings, &targets, "https://example.com", &cap, &count);
        assert_eq!(held, BTreeMap::from([("ai-email-reply".to_string(), 3), ("text-message-analyzer".to_string(), 15)]));
        let keys: Vec<(&str, &str)> = findings.iter().map(|f| (f.key.as_str(), f.severity.as_str())).collect();
        assert_eq!(keys, vec![
            ("rank:absent:ai email assistant:hold", "P2"), ("rank:deep:line トーク 分析:hold", "P2"), ("stall:personal crm:1", "P1"),
            ("ctr:https://example.com/text-message-analyzer:hold", "P2"), ("anomaly:page1-ctr:/personal-crm", "P1"),
            ("index:https://example.com/text-message-analyzer", "P1"), ("anomaly:page-cliff:/text-message-analyzer", "P1"),
        ]);
        assert!(findings[0].title.ends_with("（该页近 7 天已有 3 次文案提交，只报告不改）") && findings[0].action.starts_with("不改文案"));
    }

    #[test]
    fn the_longest_matching_target_names_the_page_and_odd_paths_have_no_block() {
        let targets = [target("ai email", "/a"), target("ai email assistant", "/b")];
        assert_eq!(page_of("rank:ai email", &targets).as_deref(), Some("/a"));
        assert_eq!(page_of("rank:lost-top:ai email assistant:2026-11-12", &targets).as_deref(), Some("/b"));
        assert_eq!(page_of("trend:heat:x", &targets), None);
        assert_eq!(slug("https://example.com/zh-TW/win-him-back", "https://example.com").as_deref(), Some("win-him-back"));
        assert_eq!(slug("https://example.com/", "https://example.com"), None);
        assert_eq!(slug("/wiki/Some Page", "https://example.com"), None);
    }
}
