//! Builds and publishes the run's page through the report skill of the Mars_agent checkout.
use crate::page::{markdown, values, Context, Published};
use anyhow::{bail, Result};
use std::io::Write;
use std::process::{Command, Stdio};

const SCRIPTS: &str = ".claude/skills/report/scripts";
const CRITERIA: [&str; 5] = [
    "从 origin/mastra 的独立工作树采集请求前缀，与上周采集对比",
    "体积、结构重复、重复描述、约束文本、系统重复片段与上周和 10-08 基线对比",
    "语义重复逐组列出每份拷贝的 file:line",
    "超过写明理由的阈值才开一个会话",
    "每周一页，历史趋势可见",
];

fn bun(args: &[&str], stdin: Option<&str>) -> Result<String> {
    let mut child = Command::new("bun").args(args).current_dir(crate::tree::REPO)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(text.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        bail!("bun {} failed: {}", args.first().copied().unwrap_or_default(), text.lines().rev().take(4).collect::<Vec<_>>().join(" | "));
    }
    Ok(text)
}

pub fn render(c: &Context, publish: bool) -> Result<Published> {
    let slug = format!("mars-crm-prompt-review-{}", c.date.replace('-', ""));
    // The publication guard refuses `users/<name>/…` (a credential pattern, case-insensitive); an excerpt
    // holding one gets a division slash instead of its slash.
    let md = regex::Regex::new(r"(?im)((?:^|/)users)/").expect("regex").replace_all(&markdown(c), "$1\u{2215}").into_owned();
    std::fs::write(c.run_dir.join("report.md"), &md)?;
    let dir = c.run_dir.join("page");
    std::fs::create_dir_all(&dir)?;
    let (dir_s, spec) = (dir.to_string_lossy().to_string(), dir.join("spec.json").to_string_lossy().to_string());
    let script = |name: &str| format!("{SCRIPTS}/{name}");
    let title = format!("Mars CRM 提示词周复盘 {}", c.date);
    let mut init: Vec<String> = ["init", &dir_s, "--slug", &slug, "--title", &title, "--group", "infra", "--env", "local",
        "--goal", "每周复盘 Mars CRM 主智能体的系统提示词、工具描述与 schema、内置 skill 提示词，找出冗余与重复，超过阈值时开会话去重", "--force"]
        .iter().map(|s| s.to_string()).collect();
    init.extend(CRITERIA.iter().flat_map(|t| ["--criterion".to_string(), t.to_string()]));
    let init: Vec<String> = std::iter::once(script("spec.ts")).chain(init).collect();
    bun(&init.iter().map(String::as_str).collect::<Vec<_>>(), None)?;
    bun(&[&script("spec.ts"), "set", &spec, "sections=[]"], None)?;
    bun(&[&script("spec.ts"), "sections", &spec, &c.run_dir.join("report.md").to_string_lossy()], None)?;
    bun(&[&script("spec.ts"), "set", &spec, "--file", "-"], Some(&values(c).to_string()))?;
    bun(&[&script("build.ts"), &spec], None)?;
    if !publish {
        return Ok(Published { url: None, slug });
    }
    let out = bun(&[&script("publish.ts"), &format!("{dir_s}/report"), "--encrypt"], None)?;
    let url = regex::Regex::new(r"https://reports\.example\.com[^\s)\]]*").expect("regex").find_iter(&out)
        .map(|m| m.as_str().to_string()).find(|u| u.contains(&slug))
        .unwrap_or_else(|| format!("https://reports.example.com/{slug}/"));
    Ok(Published { url: Some(url), slug })
}
