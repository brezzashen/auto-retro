//! `.claude/skills/seo-competitor/scripts/gsc.ts`: reads the target's Search Console data through a
//! short-lived token of a read-only service account (IAM Credentials `generateAccessToken`). No key file.

use super::config::{Gsc, RunConfig};
use super::files::{stringify, write_text};
use crate::js_wire::js;
use serde_json::{Value, json};

pub const SCOPE: &str = "https://www.googleapis.com/auth/webmasters.readonly";

/// The `generateAccessToken` URL for a service account.
pub fn token_url(service_account: &str) -> String {
    format!("https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts/{service_account}:generateAccessToken")
}

/// The body that asks for a 10-minute read-only token.
pub fn token_body() -> Value {
    json!({"scope": [SCOPE], "lifetime": "600s"})
}

/// The `accessToken` field of a `generateAccessToken` response.
pub fn parse_access_token(text: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    value["accessToken"].as_str().map(str::to_string).ok_or_else(|| "generateAccessToken response has no accessToken".to_string())
}

/// `encodeURIComponent`: everything but the unreserved set, as uppercase UTF-8 percent escapes.
pub fn encode_uri_component(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte);
        if unreserved {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The `searchAnalytics/query` URL of a property.
pub fn query_url(property: &str) -> String {
    format!("https://www.googleapis.com/webmasters/v3/sites/{}/searchAnalytics/query", encode_uri_component(property))
}

/// The request body for one dimension over the configured window.
pub fn query_body(start: &str, end: &str, dimension: &str, row_limit: u32) -> Value {
    json!({"startDate": start, "endDate": end, "dimensions": [dimension], "rowLimit": row_limit})
}

/// `startDate ?? historyFrom` and `historyTo`: the window the three queries cover.
pub fn window(gsc: &Gsc, config: &RunConfig) -> (String, String) {
    (gsc.start_date.clone().unwrap_or_else(|| config.history_from.clone()), config.history_to.clone())
}

/// One HTTP POST with a bearer token and a JSON body; returns the status and the text of the answer.
pub(super) fn post_json(url: &str, bearer: &str, body: &str) -> Result<(u16, String), String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let response = reqwest::Client::new()
            .post(url)
            .header("Authorization", format!("Bearer {bearer}"))
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let text = response.text().await.map_err(|e| e.to_string())?;
        Ok((status, text))
    })
}

/// `gcloud auth print-access-token`, exchanged for a service-account token.
pub(super) fn access_token(gsc: &Gsc) -> Result<String, String> {
    let user = std::process::Command::new("gcloud")
        .args(["auth", "print-access-token", "--account", &gsc.caller_account])
        .output()
        .map_err(|e| e.to_string())?;
    if !user.status.success() {
        return Err(format!("gcloud token for {} failed", gsc.caller_account));
    }
    let caller_token = js::trim(&String::from_utf8_lossy(&user.stdout)).to_string();
    let body = stringify(&token_body())?;
    let (status, text) = post_json(&token_url(&gsc.service_account), &caller_token, &body)?;
    if !(200..300).contains(&status) {
        return Err(format!("generateAccessToken failed: HTTP {status}"));
    }
    parse_access_token(&text)
}

/// `collectGsc(config, raw)`: writes `gsc_query.json`, `gsc_page.json` and `gsc_date.json`.
pub fn collect_gsc(config: &RunConfig, raw: &str) -> Result<(), String> {
    let Some(gsc) = &config.gsc else { return Ok(()) };
    std::fs::create_dir_all(raw).map_err(|e| e.to_string())?;
    let token = access_token(gsc)?;
    let (start, end) = window(gsc, config);
    for dimension in ["query", "page", "date"] {
        let row_limit = if dimension == "date" { 1000 } else { 100 };
        let body = stringify(&query_body(&start, &end, dimension, row_limit))?;
        let (status, text) = post_json(&query_url(&gsc.property), &token, &body)?;
        if !(200..300).contains(&status) {
            return Err(format!("searchAnalytics {} failed: HTTP {status}", gsc.property));
        }
        let result: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        write_text(&format!("{raw}/gsc_{dimension}.json"), &stringify(&result)?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::seo_competitor::config::{Gsc, RunConfig};

    const TOKEN_RESPONSE: &str = include_str!("../../../tests/fixtures/seo_competitor/api/gsc_token_response.json");
    const QUERY_RESPONSE: &str = include_str!("../../../tests/fixtures/seo_competitor/api/gsc_query_response.json");

    fn gsc(start: Option<&str>) -> Gsc {
        Gsc {
            property: "sc-domain:example.com".to_string(),
            service_account: "gsc-readonly@example.iam.gserviceaccount.com".to_string(),
            caller_account: "caller@example.com".to_string(),
            start_date: start.map(str::to_string),
        }
    }

    fn config(gsc: Option<Gsc>) -> RunConfig {
        serde_json::from_value(serde_json::json!({
            "target": {"domain": "example.com", "name": "Example"},
            "market": {"locationCode": 2840, "languageCode": "en", "label": "US"},
            "historyFrom": "2025-09-01", "historyTo": "2026-10-01",
            "competitors": [], "targetKeywords": ["personal crm"], "plannedRank": 40,
            "gsc": gsc.map(|g| serde_json::json!({"property": g.property, "serviceAccount": g.service_account, "callerAccount": g.caller_account, "startDate": g.start_date})),
        }))
        .unwrap()
    }

    #[test]
    fn token_request_asks_for_a_read_only_ten_minute_token() {
        assert_eq!(token_url("sa@x.iam"), "https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts/sa@x.iam:generateAccessToken");
        assert_eq!(stringify(&token_body()).unwrap(), r#"{"scope":["https://www.googleapis.com/auth/webmasters.readonly"],"lifetime":"600s"}"#);
    }

    #[test]
    fn token_response_yields_the_access_token_and_rejects_a_missing_one() {
        assert_eq!(parse_access_token(TOKEN_RESPONSE).unwrap(), "test-bearer-fixture");
        assert!(parse_access_token(r#"{"error": "denied"}"#).is_err());
        assert!(parse_access_token("not json").is_err());
    }

    #[test]
    fn property_is_encoded_like_encode_uri_component() {
        assert_eq!(encode_uri_component("sc-domain:example.com"), "sc-domain%3Aexample.com");
        assert_eq!(encode_uri_component("a b/é!*()'~"), "a%20b%2F%C3%A9!*()'~");
        assert_eq!(
            query_url("sc-domain:example.com"),
            "https://www.googleapis.com/webmasters/v3/sites/sc-domain%3Aexample.com/searchAnalytics/query"
        );
    }

    #[test]
    fn query_body_keeps_the_window_then_dimension_then_row_limit_order() {
        assert_eq!(
            stringify(&query_body("2025-07-01", "2026-10-01", "query", 100)).unwrap(),
            r#"{"startDate":"2025-07-01","endDate":"2026-10-01","dimensions":["query"],"rowLimit":100}"#
        );
    }

    #[test]
    fn window_starts_at_the_gsc_start_date_or_the_history_start() {
        let cfg = config(Some(gsc(Some("2025-07-01"))));
        assert_eq!(window(cfg.gsc.as_ref().unwrap(), &cfg), ("2025-07-01".to_string(), "2026-10-01".to_string()));
        let cfg = config(Some(gsc(None)));
        assert_eq!(window(cfg.gsc.as_ref().unwrap(), &cfg), ("2025-09-01".to_string(), "2026-10-01".to_string()));
    }

    #[test]
    fn query_response_fixture_round_trips_through_json_text() {
        let value: Value = serde_json::from_str(QUERY_RESPONSE).unwrap();
        assert_eq!(value["rows"][0]["keys"][0], "example crm");
        assert_eq!(stringify(&value).unwrap(), QUERY_RESPONSE.replace('\n', "").replace("  ", "").replace(": ", ":").replace(", ", ","));
    }

    #[test]
    fn without_a_gsc_block_nothing_is_fetched_or_written() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        collect_gsc(&config(None), &raw.to_string_lossy()).unwrap();
        assert!(!raw.exists());
    }
}
