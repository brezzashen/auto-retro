//! AIsa (aisa.one) for the Monid calls it carries: DataForSEO under its native live paths and search1api's sitemap
//! through the tool router. A DataForSEO answer's `tasks[0].result` is what `monid run` wrote. The key is
//! `AISA_API_KEY`, else the Keychain item `aisa-api-token` / `api@example.com:dev`.

use super::files::{stringify, write_text};
use super::monid::MonidCall;
use crate::http;
use serde_json::{json, Value};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

const REST_BASE_URL: &str = "https://api.aisa.one/apis/v1/dataforseo";
const ROUTER_URL: &str = "https://tools.aisa.one/v1/tool-router/aisa-batch-use";
const KEYCHAIN_ACCOUNT: &str = "api@example.com:dev";
const TIMEOUT: Duration = Duration::from_secs(120);

/// Monid's DataForSEO endpoint and its native live path on AIsa.
const DATAFORSEO: [(&str, &str); 14] = [
    ("/keywords/google-ads-keywords-for-keywords", "/keywords_data/google_ads/keywords_for_keywords/live"),
    ("/keywords/google-trends-explore", "/keywords_data/google_trends/explore/live"),
    ("/keywords/google-ads-search-volume", "/keywords_data/google_ads/search_volume/live"),
    ("/serp/google-organic", "/serp/google/organic/live/advanced"),
    ("/labs/ranked-keywords", "/dataforseo_labs/google/ranked_keywords/live"),
    ("/labs/historical-bulk-traffic-estimation", "/dataforseo_labs/google/historical_bulk_traffic_estimation/live"),
    ("/labs/keyword-overview", "/dataforseo_labs/google/keyword_overview/live"),
    ("/labs/related-keywords", "/dataforseo_labs/google/related_keywords/live"),
    ("/backlinks/bulk-ranks", "/backlinks/bulk_ranks/live"),
    ("/backlinks/bulk-referring-domains", "/backlinks/bulk_referring_domains/live"),
    ("/backlinks/bulk-spam-score", "/backlinks/bulk_spam_score/live"),
    ("/backlinks/domain-intersection", "/backlinks/domain_intersection/live"),
    ("/backlinks/referring-domains", "/backlinks/referring_domains/live"),
    ("/domain/whois", "/domain_analytics/whois/overview/live"),
];

/// The AIsa path (DataForSEO) or tool (search1api) for a Monid call, or `None` when it stays on Monid.
pub fn aisa_target(provider: &str, endpoint: &str) -> Option<&'static str> {
    match provider {
        "dataforseo" => DATAFORSEO.iter().find(|(monid, _)| *monid == endpoint).map(|(_, path)| *path),
        "search1api" if endpoint == "/sitemap" => Some("search1api.web-sitemap"),
        _ => None,
    }
}

fn aisa_key() -> Option<&'static str> {
    static KEY: OnceLock<Option<String>> = OnceLock::new();
    KEY.get_or_init(|| {
        let from_env = std::env::var("AISA_API_KEY").ok().map(|k| k.trim().to_owned()).filter(|k| !k.is_empty());
        from_env.or_else(|| {
            let out = Command::new("security").args(["find-generic-password", "-s", "aisa-api-token", "-a", KEYCHAIN_ACCOUNT, "-w"]).output().ok()?;
            let key = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            (out.status.success() && !key.is_empty()).then_some(key)
        })
    })
    .as_deref()
}

/// The DataForSEO task's result, or why AIsa could not answer.
pub fn dataforseo_result(answer: &Value) -> Result<Value, String> {
    let task = &answer["tasks"][0];
    if task["status_code"].as_i64() != Some(20000) {
        let message = task["status_message"].as_str().or_else(|| answer["status_message"].as_str()).unwrap_or("no task");
        return Err(format!("dataforseo task {}: {message}", task["status_code"]));
    }
    Ok(task.get("result").cloned().unwrap_or(Value::Null))
}

fn post(url: &str, key: &str, body: &Value) -> Result<Value, String> {
    let headers = [("authorization", format!("Bearer {key}")), ("content-type", "application/json".to_owned())];
    let res = http::send(reqwest::Method::POST, url, &headers, Some(stringify(body)?.into_bytes()), Some(TIMEOUT))?;
    if !res.ok() {
        return Err(format!("aisa HTTP {}: {}", res.status, crate::js_wire::js::tail_lossy(&res.text(), 300)));
    }
    serde_json::from_str(&res.text()).map_err(|e| format!("aisa answer: {e}"))
}

/// One AIsa tool-router call: the tool's `data` and the charged US$ (`customer_cost_micros_usd`, 0 when the
/// router leaves it out), or why it failed; `None` without a key.
pub fn run_tool_charged(tool: &str, arguments: Value) -> Option<Result<(Value, f64), String>> {
    let key = aisa_key()?;
    let request = json!({"calls": [{"call_id": "c", "tool": tool, "arguments": arguments}]});
    Some(post(ROUTER_URL, key, &request).and_then(|answer| {
        let result = &answer["results"][0];
        let charged = result["customer_cost_micros_usd"].as_f64().unwrap_or(0.0) / 1_000_000.0;
        match result["successful"].as_bool() {
            Some(true) => Ok((result.get("data").cloned().unwrap_or(Value::Null), charged)),
            _ => Err(format!("aisa {tool}: {}", result["error"]["message"].as_str().unwrap_or("call failed"))),
        }
    }))
}

/// One AIsa tool-router call: the tool's `data`, or why it failed; `None` without a key.
pub fn run_tool(tool: &str, arguments: Value) -> Option<Result<Value, String>> {
    Some(run_tool_charged(tool, arguments)?.map(|(data, _)| data))
}

/// One DataForSEO tool (an AIsa tool id with no Monid twin) through the router: the task result and the
/// charged US$ (DataForSEO's `cost`, which AIsa bills one to one); `None` without a key.
pub fn dataforseo_tool(tool: &str, body: Value) -> Option<Result<(Value, f64), String>> {
    let answer = run_tool(tool, json!({"body": [body]}))?;
    Some(answer.and_then(|data| dataforseo_result(&data).map(|result| (result, data["cost"].as_f64().unwrap_or(0.0)))))
}

/// The call's output through AIsa, written to `call.out`; `None` when AIsa does not carry it or has no key.
pub fn run_aisa(call: &MonidCall) -> Option<Result<Value, String>> {
    let target = aisa_target(&call.provider, &call.endpoint)?;
    let key = aisa_key()?;
    let output = if call.provider == "dataforseo" {
        post(&format!("{REST_BASE_URL}{target}"), key, &json!([call.body])).and_then(|answer| dataforseo_result(&answer))
    } else {
        run_tool(target, json!({"body": call.body}))?
    };
    Some(output.and_then(|value| {
        write_text(&call.out, &serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?)?;
        Ok(value)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_monid_endpoints_to_aisa() {
        assert_eq!(aisa_target("dataforseo", "/serp/google-organic"), Some("/serp/google/organic/live/advanced"));
        assert_eq!(aisa_target("dataforseo", "/keywords/google-trends-explore"), Some("/keywords_data/google_trends/explore/live"));
        assert_eq!(aisa_target("search1api", "/sitemap"), Some("search1api.web-sitemap"));
        assert_eq!(aisa_target("dataforseo", "/serp/google-jobs"), None);
        assert_eq!(aisa_target("serpapi", "/bing/search"), None);
    }

    #[test]
    fn dataforseo_result_takes_the_first_task_result() {
        let ok = json!({"tasks": [{"status_code": 20000, "status_message": "Ok.", "result": [{"items": [1]}]}]});
        assert_eq!(dataforseo_result(&ok).unwrap(), json!([{"items": [1]}]));
        let bad = json!({"tasks": [{"status_code": 40501, "status_message": "Invalid Field"}]});
        assert_eq!(dataforseo_result(&bad).unwrap_err(), "dataforseo task 40501: Invalid Field");
        assert!(dataforseo_result(&json!({"status_code": 40100, "status_message": "auth"})).is_err());
    }
}
