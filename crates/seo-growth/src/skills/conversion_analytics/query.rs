//! `mars-skill-tools conversion-analytics query [<HogQL> | --file <f> | -] [--json] [--config <file>]`:
//! one ad hoc HogQL query against the PostHog project of `watch.config.json`, printed as a
//! tab-separated table with a header row. It replaces the hand-built curl + Keychain + `jq .results`
//! pipeline, which hid PostHog's validation errors as `jq: Cannot iterate over null`
//! (conversation-retro 817041268211456232350: 64 calls in 12 sessions). The key never leaves the process.

use super::config::read_config;
use super::posthog::{PostHog, text};
use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};

const CONFIG: &str = ".claude/skills/conversion-analytics/references/watch.config.json";
const USAGE: &str = "usage: mars-skill-tools conversion-analytics query [<HogQL> | --file <f> | -] [--json] [--config <file>]";

/// `--config`, else the skill's config found from the working directory upwards.
pub(crate) fn config_path(argv: &[String], start: &Path) -> Option<PathBuf> {
    if let Some(i) = argv.iter().position(|a| a == "--config") { return argv.get(i + 1).map(PathBuf::from); }
    start.ancestors().map(|dir| dir.join(CONFIG)).find(|p| p.is_file())
}

/// The HogQL text: `--file <f>`, a positional argument, or stdin for `-` or no argument.
fn hogql(argv: &[String]) -> Result<String, String> {
    if let Some(i) = argv.iter().position(|a| a == "--file") {
        let path = argv.get(i + 1).ok_or("--file needs a path")?;
        return std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"));
    }
    let mut skip = false;
    let positional = argv.iter().find(|a| {
        let take = !skip && !a.starts_with("--");
        skip = a.as_str() == "--config";
        take
    });
    match positional.map(String::as_str) {
        Some(q) if q != "-" => Ok(q.to_string()),
        _ => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text).map_err(|e| format!("stdin: {e}"))?;
            Ok(text)
        }
    }
}

/// Header plus one tab-separated line per row; tabs and newlines inside a cell become spaces.
pub fn table(columns: &[String], rows: &[Vec<Value>]) -> String {
    let clean = |s: String| s.replace(['\t', '\n', '\r'], " ");
    let mut out = columns.join("\t");
    for row in rows {
        out.push('\n');
        out.push_str(&(0..row.len()).map(|i| clean(text(row, i))).collect::<Vec<_>>().join("\t"));
    }
    out
}

fn run(argv: &[String]) -> Result<(), String> {
    if argv.iter().any(|a| a == "--help" || a == "-h") { println!("{USAGE}"); return Ok(()); }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let path = config_path(argv, &cwd).ok_or(format!("{CONFIG} not found above {}; pass --config", cwd.display()))?;
    let config = read_config(&path.to_string_lossy())?;
    let query = hogql(argv)?;
    if query.trim().is_empty() { return Err(format!("empty HogQL\n{USAGE}")); }
    let ph = PostHog::from_keychain(&config.host, config.project, &config.keychain_service)?;
    let (columns, rows) = ph.query_named(query.trim(), "conversion-analytics-query")?;
    if argv.iter().any(|a| a == "--json") {
        println!("{}", json!({ "columns": columns, "results": rows }));
    } else {
        println!("{}", table(&columns, &rows));
    }
    eprintln!("({} rows, project {})", rows.len(), config.project);
    Ok(())
}

pub fn main(argv: &[String]) -> i32 {
    match run(argv) {
        Ok(()) => 0,
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
    fn positional_query_skips_flag_values() {
        assert_eq!(hogql(&args(&["--config", "c.json", "select 1", "--json"])).unwrap(), "select 1");
        assert_eq!(hogql(&args(&["--json", "select 2"])).unwrap(), "select 2");
    }

    #[test]
    fn table_has_header_and_flattens_cells() {
        let rows = vec![vec![json!("a\tb"), json!(3), Value::Null], vec![json!("x\ny"), json!(true), json!(1.5)]];
        let cols = args(&["k", "n", "v"]);
        assert_eq!(table(&cols, &rows), "k\tn\tv\na b\t3\t\nx y\ttrue\t1.5");
    }

    #[test]
    fn config_found_from_a_subdirectory() {
        let root = std::env::temp_dir().join(format!("ca-query-{}", std::process::id()));
        let sub = root.join("Mars_CRM/web");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(root.join(".claude/skills/conversion-analytics/references")).unwrap();
        std::fs::write(root.join(CONFIG), "{}").unwrap();
        assert_eq!(config_path(&[], &sub), Some(root.join(CONFIG)));
        assert_eq!(config_path(&args(&["--config", "x.json"]), &sub), Some(PathBuf::from("x.json")));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
