//! AIsa (aisa.one) calls of the AI recommendation watch: one web-searching answer per engine and the judge that reads it.
//! The charged amount comes back in `x-aisa-customer-cost-micros-usd` (micro-US$).

use crate::http;
use serde_json::{Value, json};
use std::time::Duration;

const BASE: &str = "https://api.aisa.one";
const COST_HEADER: &str = "x-aisa-customer-cost-micros-usd";
const ANSWER_TIMEOUT: Duration = Duration::from_secs(150);
const JUDGE_TIMEOUT: Duration = Duration::from_secs(60);

/// One engine's answer to one question.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Answer {
    pub text: String,
    /// Cited URLs, or source domains when the engine hides the URL (Gemini grounding redirects).
    pub cited: Vec<String>,
    /// Search queries the engine ran (Claude `server_tool_use`, Gemini `webSearchQueries`, Ark `web_search_call`); GPT returns none.
    pub fan_out: Vec<String>,
    pub cost_usd: f64,
    /// Why the answer is not whole (token limit, filter, empty text); such an answer proves nothing either way.
    pub incomplete: Option<String>,
}

impl Answer {
    /// Marks the answer incomplete unless the engine stopped with its `natural` reason and some text.
    pub fn checked(self, stop: Option<&str>, natural: &str) -> Self {
        let incomplete = if self.text.trim().is_empty() { Some("empty answer".to_string()) }
            else if stop != Some(natural) { Some(format!("stopped with {}", stop.unwrap_or("no stop reason"))) } else { None };
        Self { incomplete, ..self }
    }
}

/// The judge's reading of one answer.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Verdict {
    pub mentioned: bool,
    /// Position of Mars among the products the answer names (1 = first pick).
    pub rank: Option<u32>,
    pub products: Vec<String>,
    pub cost_usd: f64,
}

fn post(key: &str, path: &str, body: &Value, extra: &[(&str, String)], timeout: Duration) -> Result<(Value, f64), String> {
    let mut headers = vec![("authorization", format!("Bearer {key}")), ("content-type", "application/json".to_string())];
    headers.extend(extra.iter().cloned());
    let response = http::send(reqwest::Method::POST, &format!("{BASE}{path}"), &headers, Some(body.to_string().into_bytes()), Some(timeout))?;
    let cost = response.header(COST_HEADER).and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.0) / 1_000_000.0;
    let parsed: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    if !response.ok() {
        let detail = parsed["error"]["message"].as_str().or(parsed["error"].as_str()).unwrap_or("no detail");
        return Err(format!("AIsa {path}: HTTP {} {}", response.status, detail.chars().take(160).collect::<String>()));
    }
    Ok((parsed, cost))
}

/// Asks `model` the question with web search on, as the engine `kind` (`openai-search`, `anthropic-search`, `gemini-search`).
pub fn ask(key: &str, kind: &str, model: &str, question: &str) -> Result<Answer, String> {
    match kind {
        "openai-search" => {
            let body = json!({"model": model, "messages": [{"role": "user", "content": question}],
                "web_search_options": {"user_location": {"type": "approximate", "approximate": {"country": "SG"}}}});
            let (value, cost) = post(key, "/v1/chat/completions", &body, &[], ANSWER_TIMEOUT)?;
            Ok(Answer { cost_usd: cost, ..parse_openai(&value) })
        }
        "anthropic-search" => {
            // Anthropic web search refuses `user_location.country` SG (HTTP 400, 2026-11-12), so Claude searches without a location.
            let body = json!({"model": model, "max_tokens": 4000, "messages": [{"role": "user", "content": question}],
                "tools": [{"type": "web_search_20250305", "name": "web_search", "max_uses": 3}]});
            let (value, cost) = post(key, "/v1/messages", &body, &[("anthropic-version", "2023-06-01".into())], ANSWER_TIMEOUT)?;
            Ok(Answer { cost_usd: cost, ..parse_anthropic(&value) })
        }
        "gemini-search" => {
            let body = json!({"contents": [{"role": "user", "parts": [{"text": question}]}], "tools": [{"google_search": {}}]});
            let (value, cost) = post(key, &format!("/v1beta/models/{model}:generateContent"), &body, &[], ANSWER_TIMEOUT)?;
            Ok(Answer { cost_usd: cost, ..parse_gemini(&value) })
        }
        other => Err(format!("unknown engine kind {other}")),
    }
}

pub fn parse_openai(value: &Value) -> Answer {
    let choice = &value["choices"][0];
    let cited = choice["message"]["annotations"].as_array().into_iter().flatten()
        .filter_map(|a| a["url_citation"]["url"].as_str().map(String::from)).collect();
    Answer { text: choice["message"]["content"].as_str().unwrap_or_default().to_string(), cited, ..Default::default() }
        .checked(choice["finish_reason"].as_str(), "stop")
}

pub fn parse_anthropic(value: &Value) -> Answer {
    let blocks = value["content"].as_array().cloned().unwrap_or_default();
    let text = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<String>();
    let mut cited: Vec<String> = blocks.iter().filter(|b| b["type"] == "text")
        .flat_map(|b| b["citations"].as_array().cloned().unwrap_or_default()).filter_map(|c| c["url"].as_str().map(String::from)).collect();
    cited.dedup();
    let fan_out = blocks.iter().filter(|b| b["type"] == "server_tool_use").filter_map(|b| b["input"]["query"].as_str().map(String::from)).collect();
    Answer { text, cited, fan_out, ..Default::default() }.checked(value["stop_reason"].as_str(), "end_turn")
}

pub fn parse_gemini(value: &Value) -> Answer {
    let candidate = &value["candidates"][0];
    let text = candidate["content"]["parts"].as_array().into_iter().flatten().filter_map(|p| p["text"].as_str()).collect::<String>();
    let cited = candidate["groundingMetadata"]["groundingChunks"].as_array().into_iter().flatten()
        .filter_map(|c| c["web"]["title"].as_str().or(c["web"]["uri"].as_str()).map(String::from)).collect();
    let fan_out = candidate["groundingMetadata"]["webSearchQueries"].as_array().into_iter().flatten().filter_map(|q| q.as_str().map(String::from)).collect();
    Answer { text, cited, fan_out, ..Default::default() }.checked(candidate["finishReason"].as_str(), "STOP")
}

const JUDGE_RULES: &str = "You read an AI assistant's answer and report whether it recommends the product Mars (also written ExampleAI or Mars CRM, by ExampleApp, website example.com), an AI relationship assistant. The planet Mars, Mars Inc. and other products named Mars do not count. Return JSON only: {\"mentioned\": true|false, \"rank\": the 1-based position of Mars among the products the answer names, or null, \"products\": [product names in the order the answer names them, at most 12]}.";

/// Reads one answer with the judge model. A `mentioned` the text cannot back (no "mars" in it) is dropped.
pub fn judge(key: &str, model: &str, question: &str, answer: &str) -> Result<Verdict, String> {
    let prompt = format!("{JUDGE_RULES}\n\nQuestion: {question}\n\nAnswer:\n{answer}");
    let body = json!({"model": model, "messages": [{"role": "user", "content": prompt}], "response_format": {"type": "json_object"}, "max_tokens": 600});
    let (value, cost) = post(key, "/v1/chat/completions", &body, &[], JUDGE_TIMEOUT)?;
    let raw = value["choices"][0]["message"]["content"].as_str().unwrap_or_default();
    Ok(Verdict { cost_usd: cost, ..parse_verdict(raw, answer)? })
}

pub fn parse_verdict(raw: &str, answer: &str) -> Result<Verdict, String> {
    let trimmed = raw.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    let value: Value = serde_json::from_str(trimmed).map_err(|e| format!("judge JSON: {e}"))?;
    let backed = answer.to_lowercase().contains("mars");
    let mentioned = value["mentioned"].as_bool().unwrap_or(false) && backed;
    let rank = if mentioned { value["rank"].as_u64().map(|r| r as u32) } else { None };
    let products = value["products"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(String::from)).take(12).collect();
    Ok(Verdict { mentioned, rank, products, cost_usd: 0.0 })
}

/// A cited URL or domain on example.com.
pub fn is_ours(cited: &str) -> bool {
    cited.to_lowercase().contains("example.com")
}
