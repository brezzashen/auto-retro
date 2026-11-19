//! A visitor's traffic source as one HogQL expression, shared by the daily sources, the visitor-type
//! segments and the hourly visitor detail, so the three always name a source the same way.

/// Referring domains that are a step inside our own flow (another Mars host, the sign-in return),
/// not a way people found us; a session entering from one is `(internal)`.
const INTERNAL_DOMAINS: &str = "'example.com', 'example.cn', 'accounts.google.com', 'appleid.apple.com'";

/// In-app browsers that open our links without a referrer, so their sessions would read `$direct`:
/// raw User-Agent `ilike` pattern, then the source name. Each pattern was checked against production
/// page views (2026-11-18, 30 days without referrer: WeChat 730 people, Doubao 2; the 2 Douyin hits were
/// ByteDance link checks, which `watch_traffic::MACHINE_AGENTS` excludes before this applies), and
/// `references/traffic-sources.md` records what each app sends. Xiaohongshu had no visit in 90 days, so
/// its two tokens come from public UA checks (iOS `discover/<version>`, Android `XiaoHongShu`); neither
/// matched any production page view in those 90 days. Order matters: the first match wins.
pub const APP_SOURCES: &[(&str, &str)] = &[
    ("%AppName/doubao%", "doubao (app)"),
    ("%aweme%", "douyin (app)"),
    ("%XiaoHongShu%", "xiaohongshu (app)"),
    ("%discover/%", "xiaohongshu (app)"),
    ("%MicroMessenger%", "wechat (app)"),
];

/// A page view's source is its session's entry: `utm_source`, else the referrer (a t.co short link
/// keeps its full URL, our own hosts are `(internal)`). A session without either is named by the
/// in-app browser it ran in (`APP_SOURCES`), else it stays `$direct`. A page view's own `$referrer`
/// names our own page after a full reload, and its `utm_source` is gone after the first navigation,
/// which split one visitor over two sources.
pub fn expr() -> String {
    let apps: String = APP_SOURCES.iter()
        .map(|(pattern, name)| format!("ifNull(properties.$raw_user_agent, '') ilike '{pattern}', '{name}', "))
        .collect();
    format!("coalesce(nullIf(properties.$session_entry_utm_source, ''), \
     if(properties.$session_entry_referring_domain = 't.co' and properties.$session_entry_referrer != 'https://t.co/', properties.$session_entry_referrer, \
     if(properties.$session_entry_referring_domain in ({INTERNAL_DOMAINS}) or endsWith(properties.$session_entry_referring_domain, '.example.com'), '(internal)', \
     if(ifNull(nullIf(properties.$session_entry_referring_domain, ''), '$direct') = '$direct', \
     multiIf({apps}nullIf(properties.$session_entry_referring_domain, '')), \
     properties.$session_entry_referring_domain))), '(none)')")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_referrer_or_utm_wins_over_the_in_app_browser() {
        let sql = expr();
        let utm = sql.find("$session_entry_utm_source").unwrap();
        let app = sql.find("AppName/doubao").unwrap();
        assert!(utm < app, "{sql}");
        assert!(sql.contains("ifNull(nullIf(properties.$session_entry_referring_domain, ''), '$direct') = '$direct'"));
    }

    #[test]
    fn direct_sessions_are_named_by_their_app_in_order() {
        let sql = expr();
        assert!(sql.contains("ilike '%AppName/doubao%', 'doubao (app)'"));
        assert!(sql.contains("ilike '%aweme%', 'douyin (app)'"));
        assert!(sql.contains("ilike '%MicroMessenger%', 'wechat (app)', nullIf(properties.$session_entry_referring_domain, ''))"));
        assert!(sql.contains("ilike '%XiaoHongShu%', 'xiaohongshu (app)'") && sql.contains("ilike '%discover/%', 'xiaohongshu (app)'"));
        assert!(sql.find("douyin (app)").unwrap() < sql.find("xiaohongshu (app)").unwrap());
        assert!(sql.find("xiaohongshu (app)").unwrap() < sql.find("wechat (app)").unwrap());
    }

    #[test]
    fn our_own_hosts_stay_internal_and_t_co_keeps_its_post() {
        let sql = expr();
        assert!(sql.contains("'accounts.google.com'") && sql.contains("'(internal)'"));
        assert!(sql.contains("properties.$session_entry_referrer != 'https://t.co/', properties.$session_entry_referrer"));
    }
}
