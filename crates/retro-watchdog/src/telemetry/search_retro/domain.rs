//! Host names in a query (`apps.apple.com`) are one name, not their labels. Search-retro
//! 815399477109309239632: `search "apps.apple.com|itunes.apple.com|appStoreUrl|APP_STORE"`
//! (be857861…jsonl:51) printed that the repository has no App Store link; the agent took that
//! answer and looked up the app id instead (`grep "appleId\|apple_id"`, be857861…jsonl:72, and
//! `search "ascAppId|apple_app_id"`, :77). The labels `apps` and `apple` made both a re-search
//! of the same names, so the answered hit counted as unadopted.

/// Top-level labels that mark a two-label word (`example.com`) as a host rather than `file.ext`.
const TLDS: &[&str] = &["com", "net", "org", "io", "ai", "dev", "fun", "cn", "sg", "co", "us"];

/// The host names of `query` (lowercased) and the query with them blanked out.
pub(super) fn hosts(query: &str) -> (Vec<String>, String) {
    let mut found = Vec::new();
    let mut rest = String::with_capacity(query.len());
    let mut run = String::new();
    let flush = |run: &mut String, rest: &mut String, found: &mut Vec<String>| {
        // A dotted version (`1.64.1`) is one name too (`loop_var.rs`, search-retro 816857502053349117708).
        if is_host(run) || super::loop_var::is_version(run) {
            found.push(run.to_lowercase());
            rest.push_str(&" ".repeat(run.len()));
        } else {
            rest.push_str(run);
        }
        run.clear();
    };
    for c in query.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '-') {
            run.push(c);
        } else {
            flush(&mut run, &mut rest, &mut found);
            rest.push(c);
        }
    }
    flush(&mut run, &mut rest, &mut found);
    (found, rest)
}

/// Lowercase labels of letters, digits and `-` joined by single dots: three or more labels, or two
/// whose last is a common top-level domain. Upper case marks a bundle or type, and `.app` is a
/// bundle suffix: `UpdateWatch.app` (6c3fb2b2…jsonl:249) was re-searched by its pieces at :258.
fn is_host(word: &str) -> bool {
    let labels: Vec<&str> = word.split('.').collect();
    let well_formed = !word.chars().any(|c| c.is_ascii_uppercase()) && labels.iter().all(|l| !l.is_empty() && l.chars().any(|c| c.is_ascii_alphabetic()));
    let last = labels.last().map(|l| l.to_ascii_lowercase()).unwrap_or_default();
    well_formed && (labels.len() >= 3 || (labels.len() == 2 && TLDS.contains(&last.as_str())))
}

/// Pattern text with regex-escaped dots read as dots, so `apps\.apple\.com` holds the host.
pub(super) fn unescape(text: &str) -> String {
    text.replace("\\.", ".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_is_one_name() {
        let (found, rest) = hosts("apps.apple.com|itunes.apple.com|appStoreUrl|APP_STORE");
        assert_eq!(found, vec!["apps.apple.com".to_string(), "itunes.apple.com".to_string()]);
        assert!(!rest.contains("apple") && rest.contains("appStoreUrl"));
        assert_eq!(hosts("canary.example.com deploy").0, vec!["canary.example.com".to_string()]);
        assert_eq!(hosts("example.com").0, vec!["example.com".to_string()]);
    }

    #[test]
    fn a_file_name_or_method_call_is_not() {
        assert!(hosts("report.md").0.is_empty());
        assert!(hosts("route.fulfill").0.is_empty());
        assert!(hosts("v1.2.3").0.is_empty());
        assert!(hosts("UpdateWatch.app").0.is_empty() && hosts("CPUWatchdog.app").0.is_empty());
        assert_eq!(unescape("apps\\.apple\\.com"), "apps.apple.com");
    }
}
