//! `.claude/skills/seo-competitor/scripts/report.ts`: writes the competitor report into a report-skill
//! spec: the overview cases, the master table, and one chart card plus one keyword-detail section per
//! competitor. Sections and cases it did not generate are kept. The spec file itself is edited by
//! `mars-skill-tools report spec` (the port of the report skill's `spec.ts`), run as a child of this binary.

use super::cards::{CARD_PREFIX, DETAIL_SUFFIX, competitor_sections};
use super::dashboard::{Mrr, growth_case, master_table, mrr_case, share_case, win_case};
use super::files::{at, fail, items, num_or, read_json, stringify, text, truthy};
use super::format::label_of;
use super::self_case::self_case;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

const GENERATED_CASES: [&str; 5] = ["MRR", "GROWTH", "KWC", "WIN", "SELF"];
const MASTER_TITLE: &str = "竞对总表";

/// The parsed command line, as `parseArgs` reads it with `--slug` and `--title` as strings.
#[derive(Default)]
struct Args {
    slug: Option<String>,
    title: Option<String>,
    positionals: Vec<String>,
}

/// A value must follow the flag and must not look like another option.
fn option_value(argv: &[String], i: &mut usize, name: &str, inline: Option<&str>) -> Result<String, String> {
    if let Some(value) = inline {
        return Ok(value.to_string());
    }
    let next = argv.get(*i + 1).filter(|v| !v.starts_with('-') || v.len() == 1);
    match next {
        Some(value) => {
            *i += 1;
            Ok(value.clone())
        }
        None if argv.get(*i + 1).is_some() => Err(format!("Option '--{name} <value>' argument is ambiguous.")),
        None => Err(format!("Option '--{name} <value>' argument missing")),
    }
}

/// `parseArgs({ args, allowPositionals: true, options: { slug, title } })`, strict: an unknown
/// option is an error, `--` ends the options.
fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut args = Args::default();
    let mut only_positionals = false;
    let mut i = 0;
    while i < argv.len() {
        let token = argv[i].as_str();
        if only_positionals || token == "-" || !token.starts_with('-') {
            args.positionals.push(token.to_string());
        } else if token == "--" {
            only_positionals = true;
        } else {
            let body = token.strip_prefix("--").ok_or_else(|| format!("Unknown option '{token}'."))?;
            let (name, inline) = match body.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (body, None),
            };
            match name {
                "slug" => args.slug = Some(option_value(argv, &mut i, name, inline)?),
                "title" => args.title = Some(option_value(argv, &mut i, name, inline)?),
                _ => return Err(format!("Unknown option '{token}'.")),
            }
        }
        i += 1;
    }
    Ok(args)
}

/// `report spec <args…>`: its stdout, with its stderr passed through.
fn spec_ts(args: &[String]) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("report spec: {e}"))?;
    let output = Command::new(exe)
        .args(["report", "spec"])
        .env("MARS_SKILL_TOOLS_RELAUNCHED", "1")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("report spec: {e}"))?;
    if !output.status.success() {
        return Err(format!("spec.ts {} failed", args.first().map_or("", String::as_str)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `JSON.parse(specTs("get", spec, path) || "null")`.
fn spec_get(spec: &str, path: &str) -> Result<Value, String> {
    let text = spec_ts(&["get".to_string(), spec.to_string(), path.to_string()])?;
    if text.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// `[x].flat()` of a spec value: a list gives its elements, anything else is one element.
fn flat_one(value: Value) -> Vec<Value> {
    match value {
        Value::Null => Vec::new(),
        Value::Array(list) => list,
        other => vec![other],
    }
}

/// Sections the generator owns: card, detail and the template note.
fn generated_section(section: &Value) -> bool {
    let title = label_of(section, "title");
    title.starts_with(CARD_PREFIX) || title.ends_with(DETAIL_SUFFIX) || title.starts_with("模板说明")
}

/// One `cases.<id>.<field>=<value>` assignment per field, in the case's own key order.
fn case_assignments(cases: &[(&str, Value)]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (id, case) in cases {
        for (field, value) in case.as_object().into_iter().flatten() {
            let rendered = match value.as_str() {
                Some(s) => s.to_string(),
                None => stringify(value)?,
            };
            out.push(format!("cases.{id}.{field}={rendered}"));
        }
    }
    Ok(out)
}

fn run(run_dir: &str, report_dir: &str, args: &Args) -> Result<String, String> {
    let analysis = read_json(&format!("{run_dir}/analysis.json"))?;
    let config = at(&analysis, &["config"]).clone();
    let target = at(&analysis, &["target"]).clone();
    let competitors = items(at(&analysis, &["competitors"]));
    let keywords = items(at(&analysis, &["keywords"]));
    let mrr: Mrr = match read_json(&format!("{run_dir}/mrr.json"))? {
        Value::Array(rows) => rows.into_iter().map(|m| (text(at(&m, &["domain"])).to_string(), m)).collect::<HashMap<_, _>>(),
        _ => HashMap::new(),
    };
    let spec = format!("{report_dir}/spec.json");
    if !Path::new(&spec).exists() {
        let (Some(slug), Some(title)) = (args.slug.as_deref().filter(|s| !s.is_empty()), args.title.as_deref().filter(|s| !s.is_empty())) else {
            return Err("a new report needs --slug and --title".to_string());
        };
        let goal = format!("{} 的 SEO 竞对分析", text(at(&config, &["target", "name"])));
        let init: Vec<String> = ["init", report_dir, "--slug", slug, "--title", title, "--group", "research", "--env", "prod", "--goal", &goal]
            .iter()
            .map(|s| s.to_string())
            .collect();
        spec_ts(&init)?;
    }
    let mut cases: Vec<(&str, Value)> = vec![
        ("MRR", mrr_case(competitors, &mrr)),
        ("GROWTH", growth_case(competitors, &target)),
        ("KWC", share_case(competitors)),
        ("WIN", win_case(keywords, &target, num_or(at(&config, &["plannedRank"]), 0.0))),
    ];
    if let Some(self_section) = self_case(&target, text(at(&config, &["target", "domain"]))) {
        cases.push(("SELF", self_section));
    }
    let kept_sections: Vec<Value> = items(&spec_get(&spec, "sections")?).iter().filter(|s| !generated_section(s)).cloned().collect();
    let kept_matrices: Vec<Value> = flat_one(spec_get(&spec, "matrix")?)
        .into_iter()
        .filter(|m| truthy(m) && at(m, &["title"]).as_str() != Some(MASTER_TITLE) && !label_of(m, "title").starts_with(MASTER_TITLE))
        .collect();
    let mut matrix = vec![master_table(competitors, &mrr)];
    matrix.extend(kept_matrices);
    let mut sections = competitor_sections(competitors, &mrr);
    sections.extend(kept_sections.iter().cloned());
    let mut assignments = vec![format!("matrix={}", stringify(&matrix)?), format!("sections={}", stringify(&sections)?)];
    assignments.extend(case_assignments(&cases)?);
    let mut set = vec!["set".to_string(), spec.clone()];
    set.extend(assignments);
    spec_ts(&set)?;
    let all = items(&spec_get(&spec, "cases")?).to_vec();
    let mut ordered: Vec<Value> = GENERATED_CASES.iter().filter_map(|id| all.iter().find(|c| at(c, &["id"]).as_str() == Some(id)).cloned()).collect();
    ordered.extend(all.iter().filter(|c| !GENERATED_CASES.contains(&at(c, &["id"]).as_str().unwrap_or_default())).cloned());
    spec_ts(&["set".to_string(), spec.clone(), format!("cases={}", stringify(&ordered)?)])?;
    if !truthy(&spec_get(&spec, "summary")?) {
        let name = text(at(&config, &["target", "name"]));
        spec_ts(&["set".to_string(), spec.clone(), format!("summary={name} SEO 竞对分析：{} 个竞品", competitors.len())])?;
    }
    let ids: Vec<Value> = ordered.iter().map(|c| at(c, &["id"]).clone()).collect();
    stringify(&json!({"spec": spec, "cases": ids, "sections": kept_sections.len() + competitors.len() * 2}))
}

/// `bun report.ts <run-dir> <report-dir> [--slug <slug> --title <title>]`.
pub fn main(argv: &[String]) -> i32 {
    let args = match parse_args(argv) {
        Ok(args) => args,
        Err(message) => return fail(&message),
    };
    let (Some(run_dir), Some(report_dir)) = (args.positionals.first(), args.positionals.get(1)) else {
        eprintln!("usage: bun report.ts <run-dir> <report-dir> [--slug <slug> --title <title>]");
        return 2;
    };
    if run_dir.is_empty() || report_dir.is_empty() {
        eprintln!("usage: bun report.ts <run-dir> <report-dir> [--slug <slug> --title <title>]");
        return 2;
    }
    match run(run_dir, report_dir, &args) {
        Ok(line) => {
            println!("{line}");
            0
        }
        Err(message) => fail(&message),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn slug_and_title_take_separate_or_inline_values() {
        let args = parse_args(&argv(&["run", "--slug", "s", "--title=t", "rest"])).unwrap();
        assert_eq!(args.slug.as_deref(), Some("s"));
        assert_eq!(args.title.as_deref(), Some("t"));
        assert_eq!(args.positionals, ["run", "rest"]);
    }

    #[test]
    fn a_flag_without_a_value_or_an_unknown_option_is_an_error() {
        assert!(parse_args(&argv(&["run", "--slug"])).is_err());
        assert!(parse_args(&argv(&["run", "--slug", "--title", "t"])).is_err());
        assert!(parse_args(&argv(&["run", "--help"])).is_err());
        assert!(parse_args(&argv(&["run", "-x"])).is_err());
    }

    #[test]
    fn double_dash_ends_the_options() {
        let args = parse_args(&argv(&["--", "--slug", "a"])).unwrap();
        assert_eq!(args.slug, None);
        assert_eq!(args.positionals, ["--slug", "a"]);
    }

    #[test]
    fn generated_sections_are_recognised_by_their_titles() {
        assert!(generated_section(&json!({"title": "竞对卡片 · Alpha"})));
        assert!(generated_section(&json!({"title": "Alpha · 关键词明细"})));
        assert!(generated_section(&json!({"title": "模板说明 A"})));
        assert!(!generated_section(&json!({"title": "我的备注"})));
    }
}
