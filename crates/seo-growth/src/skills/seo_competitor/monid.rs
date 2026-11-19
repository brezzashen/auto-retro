//! `.claude/skills/seo-competitor/scripts/monid.ts`: runs one Monid endpoint, follows its download
//! link and caches the JSON result on disk. A file already present is reused without a call. A call AIsa
//! carries (`aisa::aisa_target`) goes to AIsa first and to `monid run` only when AIsa fails.

use super::files::{NULL, at, read_json, stringify, truthy, write_text};
use crate::http;
use crate::js_wire::js;
use serde_json::Value;
use std::collections::VecDeque;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct MonidCall {
    pub provider: String,
    pub endpoint: String,
    pub body: Value,
    pub out: String,
}

/// Arguments of `monid run` for one call, without the program name.
pub fn monid_args(call: &MonidCall) -> Result<Vec<String>, String> {
    Ok(vec![
        "run".to_string(),
        "-p".to_string(),
        call.provider.clone(),
        "-e".to_string(),
        call.endpoint.clone(),
        "-i".to_string(),
        stringify(&call.body)?,
        "-w".to_string(),
        "120".to_string(),
        "-o".to_string(),
        call.out.clone(),
    ])
}

/// `runMonid(call)`: the cached file when present, else AIsa, else one `monid run` plus its download.
pub fn run_monid(call: &MonidCall) -> Result<Value, String> {
    if Path::new(&call.out).exists() {
        return read_json(&call.out);
    }
    if let Some(parent) = Path::new(&call.out).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    match super::aisa::run_aisa(call) {
        Some(Ok(value)) => return Ok(value),
        Some(Err(error)) => eprintln!("aisa {} failed, retrying on monid: {error}", call.endpoint),
        None => {}
    }
    let output = Command::new("monid")
        .args(monid_args(call)?)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("monid {} failed: {e}", call.endpoint))?;
    let code = output.status.code();
    if code != Some(0) || !Path::new(&call.out).exists() {
        let err = String::from_utf8_lossy(&output.stderr);
        let code_text = code.map_or("null".to_string(), |c| c.to_string());
        return Err(format!("monid {} failed ({code_text}): {}", call.endpoint, js::tail_lossy(&err, 400)));
    }
    let first = read_json(&call.out)?;
    let link = at(&first, &["data", "download_link"]);
    if !truthy(link) {
        return Ok(first);
    }
    let link_text = link.as_str().unwrap_or_default();
    let res = http::get(link_text, &[], None)?;
    if !res.ok() {
        return Err(format!("download {} failed: HTTP {}", call.endpoint, res.status));
    }
    let text = res.text();
    write_text(&call.out, &text)?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", call.out))
}

/// `runAll(calls, concurrency)`: every call runs; failures are collected and reported together.
pub fn run_all(calls: Vec<MonidCall>, concurrency: usize) -> Result<(), String> {
    let queue = Mutex::new(VecDeque::from(calls));
    let failures: Mutex<Vec<String>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..concurrency {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
                    let Some(call) = next else { break };
                    if let Err(error) = run_monid(&call) {
                        // String(error) of a JavaScript Error.
                        failures.lock().unwrap_or_else(|e| e.into_inner()).push(format!("Error: {error}"));
                    }
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap_or_else(|e| e.into_inner());
    if failures.is_empty() {
        return Ok(());
    }
    Err(format!("{} Monid call(s) failed:\n{}", failures.len(), failures.join("\n")))
}

/// DataForSEO wraps most results in a one-element array.
pub fn first(json: &Value) -> &Value {
    match json.as_array() {
        Some(list) => list.first().unwrap_or(&NULL),
        None => json,
    }
}

/// The URL slug of a keyword: lowercase, runs of other characters as `-`, at most 60 characters.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in text.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    out.chars().take(60).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DOWNLOAD: &str = include_str!("../../../tests/fixtures/seo_competitor/api/monid_download_link.json");

    fn call(out: &str) -> MonidCall {
        MonidCall { provider: "dataforseo".into(), endpoint: "/serp/google-organic".into(), body: json!({"keyword": "personal crm", "depth": 10}), out: out.into() }
    }

    #[test]
    fn arguments_match_monid_run_with_the_wait_and_output_flags() {
        assert_eq!(
            monid_args(&call("/tmp/x.json")).unwrap(),
            ["run", "-p", "dataforseo", "-e", "/serp/google-organic", "-i", r#"{"keyword":"personal crm","depth":10}"#, "-w", "120", "-o", "/tmp/x.json"]
        );
    }

    #[test]
    fn cached_output_is_read_without_a_call() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("serp.json");
        std::fs::write(&out, DOWNLOAD).unwrap();
        let value = run_monid(&call(&out.to_string_lossy())).unwrap();
        assert_eq!(value["data"]["download_link"], "https://storage.example/monid/out.json");
    }

    #[test]
    fn run_all_with_every_output_cached_succeeds_without_a_call() {
        let dir = tempfile::tempdir().unwrap();
        let mut calls = vec![];
        for name in ["a.json", "b.json"] {
            let out = dir.path().join(name);
            std::fs::write(&out, "[]").unwrap();
            calls.push(call(&out.to_string_lossy()));
        }
        run_all(calls, 8).unwrap();
    }

    #[test]
    fn first_unwraps_a_one_element_array_only() {
        assert_eq!(first(&json!([{"items": [1]}])), &json!({"items": [1]}));
        assert_eq!(first(&json!({"items": []})), &json!({"items": []}));
        assert_eq!(first(&json!([])), &Value::Null);
    }

    #[test]
    fn slug_lowercases_and_joins_runs_of_other_characters() {
        assert_eq!(slug("personal crm"), "personal-crm");
        assert_eq!(slug("  Crm, for  networking!  "), "crm-for-networking");
        assert_eq!(slug("中文 only"), "only");
        assert_eq!(slug(&"a".repeat(80)).len(), 60);
    }
}
