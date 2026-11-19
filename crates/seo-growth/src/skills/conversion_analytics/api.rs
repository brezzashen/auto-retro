//! `mars-skill-tools conversion-analytics api <path> [-X <METHOD>] [--data <json> | --data-file <f>] [--out <f>]
//! [--header <name>]... [--config <file>]`: one authenticated PostHog REST call under
//! `/api/projects/<project>/` (session_recordings/<id>, its snapshots, insights, project settings). The body
//! goes to stdout (or `--out`), `HTTP <status>` and each requested response header to stderr; exit 1 on a
//! non-2xx status. The Keychain key stays inside the process.

use super::config::read_config;
use super::query::config_path;
use crate::http;
use std::process::Command;
use std::time::Duration;

const USAGE: &str = "usage: mars-skill-tools conversion-analytics api <path> [-X GET|POST|PATCH|DELETE] [--data <json> | --data-file <f>] [--out <f>] [--header <name>]... [--config <file>]\n  <path> is relative to /api/projects/<project>/, e.g. session_recordings/<id> or '' for the project settings";

#[derive(Debug, PartialEq)]
pub struct Args {
    pub path: String,
    pub method: String,
    pub data: Option<String>,
    pub data_file: Option<String>,
    pub out: Option<String>,
    pub headers: Vec<String>,
}

pub fn parse(argv: &[String]) -> Result<Args, String> {
    let mut args = Args { path: String::new(), method: String::new(), data: None, data_file: None, out: None, headers: vec![] };
    let mut path = None;
    let mut i = 0;
    while i < argv.len() {
        let value = || argv.get(i + 1).cloned().ok_or(format!("{} needs a value\n{USAGE}", argv[i]));
        match argv[i].as_str() {
            "-X" | "--method" => { args.method = value()?.to_uppercase(); i += 1 }
            "--data" | "-d" => { args.data = Some(value()?); i += 1 }
            "--data-file" => { args.data_file = Some(value()?); i += 1 }
            "--out" | "-o" => { args.out = Some(value()?); i += 1 }
            "--header" => { args.headers.push(value()?); i += 1 }
            "--config" => i += 1,
            flag if flag.starts_with('-') && flag.len() > 1 => return Err(format!("unknown flag {flag}\n{USAGE}")),
            positional if path.is_none() => path = Some(positional.to_string()),
            extra => return Err(format!("unexpected argument {extra}\n{USAGE}")),
        }
        i += 1;
    }
    args.path = normalize(&path.ok_or(USAGE.to_string())?);
    if args.method.is_empty() {
        args.method = if args.data.is_some() || args.data_file.is_some() { "POST".into() } else { "GET".into() };
    }
    Ok(args)
}

/// The path under `/api/projects/<project>/`: a full URL or `/api/projects/<n>/` prefix is stripped.
pub fn normalize(raw: &str) -> String {
    let trimmed = raw.split("/api/projects/").nth(1).map(|rest| rest.split_once('/').map_or("", |(_, p)| p)).unwrap_or(raw);
    trimmed.trim_start_matches('/').to_string()
}

fn keychain(service: &str) -> Result<String, String> {
    let out = Command::new("security").args(["find-generic-password", "-s", service, "-w"]).output().map_err(|e| format!("security: {e}"))?;
    if !out.status.success() { return Err(format!("Keychain entry {service} not readable")); }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn run(argv: &[String]) -> Result<bool, String> {
    if argv.iter().any(|a| a == "--help" || a == "-h") { println!("{USAGE}"); return Ok(true); }
    let args = parse(argv)?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let path = config_path(argv, &cwd).ok_or("watch.config.json not found above the working directory; pass --config")?;
    let config = read_config(&path.to_string_lossy())?;
    let body = match (&args.data, &args.data_file) {
        (Some(d), _) => Some(d.clone().into_bytes()),
        (None, Some(f)) => Some(std::fs::read(f).map_err(|e| format!("{f}: {e}"))?),
        _ => None,
    };
    let method = reqwest::Method::from_bytes(args.method.as_bytes()).map_err(|e| format!("-X {}: {e}", args.method))?;
    let url = format!("{}/api/projects/{}/{}", config.host.trim_end_matches('/'), config.project, args.path);
    let headers = [("authorization", format!("Bearer {}", keychain(&config.keychain_service)?)), ("content-type", "application/json".to_string())];
    let response = http::send(method, &url, &headers, body, Some(Duration::from_secs(120)))?;
    eprintln!("HTTP {} {} /api/projects/{}/{}", response.status, args.method, config.project, args.path);
    for name in &args.headers {
        eprintln!("{name}: {}", response.header(name).unwrap_or_else(|| "(absent)".into()));
    }
    match &args.out {
        Some(file) => {
            std::fs::write(file, &response.body).map_err(|e| format!("{file}: {e}"))?;
            eprintln!("({} bytes written to {file})", response.body.len());
        }
        None => println!("{}", response.text()),
    }
    Ok(response.ok())
}

pub fn main(argv: &[String]) -> i32 {
    match run(argv) {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(message) => {
            eprintln!("{message}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn method_defaults_from_body() {
        assert_eq!(parse(&args(&["session_recordings/01a1"])).unwrap().method, "GET");
        let patch = parse(&args(&["", "-X", "patch", "--data", "{\"a\":1}", "--config", "c.json"])).unwrap();
        assert_eq!((patch.path.as_str(), patch.method.as_str(), patch.data.as_deref()), ("", "PATCH", Some("{\"a\":1}")));
        assert_eq!(parse(&args(&["query/", "--data-file", "q.json"])).unwrap().method, "POST");
    }

    #[test]
    fn full_urls_and_prefixes_reduce_to_the_project_path() {
        assert_eq!(normalize("https://us.posthog.com/api/projects/704719/session_recordings/x/snapshots?source=blob_v2"), "session_recordings/x/snapshots?source=blob_v2");
        assert_eq!(normalize("/api/projects/704719/"), "");
        assert_eq!(normalize("/insights/?limit=5"), "insights/?limit=5");
    }

    #[test]
    fn headers_repeat_and_unknown_flags_fail() {
        assert_eq!(parse(&args(&["query/", "--header", "a", "--header", "b"])).unwrap().headers, vec!["a", "b"]);
        assert!(parse(&args(&["x", "--bogus"])).is_err());
        assert!(parse(&args(&[])).is_err());
    }
}
