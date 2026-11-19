//! Volcengine Ark (火山方舟) calls of the AI recommendation watch: Doubao answers through the Responses API with the
//! `web_search` tool (联网内容插件, activated per account). Ark returns no charged amount, so the cost is the
//! token usage times the engine's configured US$ per million tokens.

use super::watch_geo_aisa::Answer;
use crate::http;
use serde_json::{Value, json};
use std::time::Duration;

const URL: &str = "https://ark.cn-beijing.volces.com/api/v3/responses";
const TIMEOUT: Duration = Duration::from_secs(180);

/// US$ per million input and output tokens of one Ark model.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Price {
    pub input_per_m: f64,
    pub output_per_m: f64,
}

/// Asks `model` the question with Ark's web search on.
pub fn ask(key: &str, model: &str, question: &str, price: Price) -> Result<Answer, String> {
    let body = json!({"model": model, "input": [{"role": "user", "content": question}],
        "tools": [{"type": "web_search", "max_keyword": 3}], "stream": false});
    let headers = [("authorization", format!("Bearer {key}")), ("content-type", "application/json".to_string())];
    let response = http::send(reqwest::Method::POST, URL, &headers, Some(body.to_string().into_bytes()), Some(TIMEOUT))?;
    let value: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    if !response.ok() {
        let error = &value["error"];
        let detail = format!("{} {}", error["code"].as_str().unwrap_or(""), error["message"].as_str().unwrap_or("no detail"));
        return Err(format!("Ark responses: HTTP {} {}", response.status, detail.trim().chars().take(160).collect::<String>()));
    }
    Ok(parse(&value, price))
}

/// Reads a Responses API body: message text, its `url_citation` annotations, and the cost from `usage`.
pub fn parse(value: &Value, price: Price) -> Answer {
    let items = value["output"].as_array().cloned().unwrap_or_default();
    let parts: Vec<Value> = items.iter().filter(|i| i["type"] == "message")
        .flat_map(|i| i["content"].as_array().cloned().unwrap_or_default()).filter(|c| c["type"] == "output_text").collect();
    let text = parts.iter().filter_map(|c| c["text"].as_str()).collect::<String>();
    let mut cited: Vec<String> = parts.iter().flat_map(|c| c["annotations"].as_array().cloned().unwrap_or_default())
        .filter_map(|a| a["url"].as_str().map(String::from)).collect();
    cited.sort();
    cited.dedup();
    let fan_out = items.iter().filter(|i| i["type"] == "web_search_call")
        .filter_map(|i| i["action"]["query"].as_str().or(i["query"].as_str()).map(String::from)).collect();
    let usage = &value["usage"];
    let tokens = |k: &str| usage[k].as_f64().unwrap_or(0.0);
    let cost_usd = (tokens("input_tokens") * price.input_per_m + tokens("output_tokens") * price.output_per_m) / 1_000_000.0;
    let stop = match value["status"].as_str() {
        Some("incomplete") => value["incomplete_details"]["reason"].as_str().or(Some("incomplete")),
        other => other,
    };
    Answer { text, cited, fan_out, cost_usd, incomplete: None }.checked(stop, "completed")
}
