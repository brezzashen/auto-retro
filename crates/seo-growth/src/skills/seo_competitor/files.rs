//! Shared JSON helpers of the seo-competitor scripts: `Bun.file(...).text()` + `JSON.parse`,
//! `JSON.stringify(x, null, 1)` for the files they write, and the `?.`/`??` reads they do.

use crate::js_wire::{JsFormatter, js};
use serde::Serialize;
use serde_json::Value;
use serde_json::ser::PrettyFormatter;

pub static NULL: Value = Value::Null;

/// `a?.b?.c` on JSON; a missing key reads as null.
pub fn at<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    let mut current = value;
    for key in path {
        match current.get(*key) {
            Some(next) => current = next,
            None => return &NULL,
        }
    }
    current
}

/// `Number(value)` for a JSON number, `None` for anything else.
pub fn num(value: &Value) -> Option<f64> {
    value.as_f64()
}

/// `value ?? default` for a number field.
pub fn num_or(value: &Value, default: f64) -> f64 {
    value.as_f64().unwrap_or(default)
}

/// A string field, or "" when it is not one.
pub fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}

/// `value ?? []` as a slice; empty for anything but an array.
pub fn items(value: &Value) -> &[Value] {
    match value.as_array() {
        Some(list) => list.as_slice(),
        None => &[],
    }
}

/// JavaScript truthiness of a JSON value.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `String(value)` as a template literal prints it; `null` for JSON null.
pub fn show(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => js::number_string(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => s.clone(),
        other => crate::js_wire::to_string(other).unwrap_or_default(),
    }
}

/// `JSON.parse(await Bun.file(path).text())`.
pub fn read_json(path: &str) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

/// `JSON.stringify(value, null, 1)`: one-space indent, no trailing newline.
pub fn to_pretty_one<T: Serialize + ?Sized>(value: &T) -> Result<String, String> {
    let mut out = Vec::with_capacity(4096);
    let formatter = JsFormatter(PrettyFormatter::with_indent(b" "));
    value.serialize(&mut serde_json::Serializer::with_formatter(&mut out, formatter)).map_err(|e| e.to_string())?;
    String::from_utf8(out).map_err(|e| e.to_string())
}

/// `JSON.stringify(value)`.
pub fn stringify<T: Serialize + ?Sized>(value: &T) -> Result<String, String> {
    crate::js_wire::to_string(value).map_err(|e| e.to_string())
}

/// `Bun.write(path, text)`, creating nothing else.
pub fn write_text(path: &str, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{path}: {e}"))
}

/// A script failure: Bun prints the uncaught error and exits 1.
pub fn fail(message: &str) -> i32 {
    eprintln!("{message}");
    1
}

/// The real host of one SERP result, without `www.`. DataForSEO now reports many organic results
/// as `domain: "google.com"` with a `google.com/goto?url=…` link (verified 2026-11-04), leaving the
/// site only in `breadcrumb` ("https://www.example.com › path"). A forum or social post carries a
/// count there instead ("240+ comments · 1 year ago") and its platform in `website_name`
/// ("Reddit · r/dating"), verified 2026-11-07.
pub fn serp_domain(item: &Value) -> String {
    let raw = text(at(item, &["domain"]));
    let host = if raw.is_empty() || raw == "google.com" {
        let crumb = text(at(item, &["breadcrumb"]));
        let rest = crumb.split_once("://").map_or(crumb, |(_, r)| r);
        rest.split([' ', '/', '›']).next().filter(|h| is_host(h)).or_else(|| platform_host(text(at(item, &["website_name"])))).unwrap_or(raw)
    } else {
        raw
    };
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

/// Forums and social networks: their results are single posts, not sites built for the query.
const FORUMS: [(&str, &str); 12] = [("reddit", "reddit.com"), ("quora", "quora.com"), ("youtube", "youtube.com"), ("facebook", "facebook.com"),
    ("instagram", "instagram.com"), ("tiktok", "tiktok.com"), ("medium", "medium.com"), ("linkedin", "linkedin.com"),
    ("pinterest", "pinterest.com"), ("x", "x.com"), ("threads", "threads.net"), ("tumblr", "tumblr.com")];

/// The host of a forum named by a result's `website_name` ("Reddit · r/x" → reddit.com).
fn platform_host(name: &str) -> Option<&'static str> {
    let platform = name.split('·').next().unwrap_or_default().trim().to_lowercase();
    FORUMS.iter().find(|(n, _)| *n == platform).map(|(_, h)| *h)
}

/// True when `domain` (from [`serp_domain`]) is a forum or social network, any subdomain included.
pub fn is_forum(domain: &str) -> bool {
    FORUMS.iter().any(|(_, h)| domain == *h || domain.ends_with(&format!(".{h}")))
}

/// A hostname such as `www.example.co`: letters, digits, `-` and dots, ending in an alphabetic label.
/// A breadcrumb can open with a follower count instead ("30.5K+ followers").
fn is_host(h: &str) -> bool {
    let tld = h.rsplit('.').next().unwrap_or_default();
    h.contains('.') && tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())
        && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

#[cfg(test)]
mod serp_domain_tests {
    use super::serp_domain;
    use serde_json::json;

    #[test]
    fn a_google_goto_result_takes_its_host_from_the_breadcrumb() {
        let goto = json!({"domain": "google.com", "url": "https://google.com/goto?url=x", "breadcrumb": "https://www.ourrelationship.com › i-asked"});
        assert_eq!(serp_domain(&goto), "ourrelationship.com");
        assert_eq!(serp_domain(&json!({"domain": "www.reddit.com"})), "reddit.com");
        assert_eq!(serp_domain(&json!({"domain": "google.com", "breadcrumb": "Google"})), "google.com");
        assert_eq!(serp_domain(&json!({"domain": "google.com", "breadcrumb": "30.5K+ followers"})), "google.com");
    }

    #[test]
    fn a_forum_post_takes_its_platform_from_the_website_name() {
        let post = json!({"domain": "google.com", "breadcrumb": "240+ comments  ·  1 year ago", "website_name": "Reddit · r/preguntaleareddit"});
        assert_eq!(serp_domain(&post), "reddit.com");
        assert_eq!(serp_domain(&json!({"domain": "google.com", "breadcrumb": "40+ answers", "website_name": "Quora"})), "quora.com");
        assert_eq!(serp_domain(&json!({"domain": "google.com", "breadcrumb": "5 views", "website_name": "Stephan Speaks"})), "google.com");
        assert!(super::is_forum("reddit.com") && super::is_forum("whatsappupdates1.quora.com") && !super::is_forum("openup.com"));
    }
}
