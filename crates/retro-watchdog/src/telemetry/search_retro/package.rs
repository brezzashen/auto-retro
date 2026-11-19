//! A hit that points the agent at a package its search did not name is followed when a later call
//! works inside that package. Search-retro 816857502053349117708 (f0f03132…jsonl:1089):
//! `search "rev-parse --verify" --path .claude/skills/report/scripts` was a NONE whose `outside
//! --path` line named `crates/mars-skill-tools/tests/fixtures/report_spec/golden.json:77`; the next
//! call searched `--path Mars_CRM/rust/crates/mars-skill-tools/src/report` and found the code
//! (`spec/io.rs:133`). The fixture file itself was never opened, so the hit counted unadopted.
//!
//! A call names the package by its root too. Search-retro 88631398572330521580 (2f3de906…jsonl:215):
//! a search printed lines of `automation/crates/limit-resume/src/main.rs`, the next call searched
//! `--path /home/user/automation/crates/limit-resume` with no trailing slash, and the
//! `crates/limit-resume/` lead went unmatched, so the hit counted unadopted (`named`).

/// Directory names whose child is one package.
const ROOTS: &[&str] = &["crates", "packages", "skills"];

/// `<root>/<package>/` of every `path:line:` hit line in `output` whose package the search command
/// `command` did not name.
pub(super) fn leads(output: &str, command: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in output.lines() {
        let Some((path, rest)) = line.trim_start().split_once(':') else { continue };
        if path.contains(' ') || !rest.split(':').next().is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
            continue;
        }
        let parts: Vec<&str> = path.split('/').collect();
        for (i, part) in parts.iter().enumerate() {
            let Some(package) = parts.get(i + 1).filter(|_| ROOTS.contains(part) && i + 2 < parts.len()) else { continue };
            let lead = format!("{part}/{package}/");
            if !command.contains(package) && !out.contains(&lead) {
                out.push(lead);
            }
        }
    }
    out
}

/// True when `text` names the package `lead` (`crates/<name>/`): inside it, or its root followed by
/// whitespace, a quote, a shell operator or the end (`--path …/crates/limit-resume --lines`).
/// A longer sibling (`crates/limit-resume-x`) does not name it.
pub(super) fn named(text: &str, lead: &str) -> bool {
    let Some(root) = lead.strip_suffix('/') else { return text.contains(lead) };
    text.match_indices(root).any(|(i, _)| {
        text[i + root.len()..].chars().next().is_none_or(|c| c == '/' || c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ';' | '|' | '&' | ')'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_outside_line_in_an_unnamed_crate_leads_to_it() {
        let command = "~/.local/bin/local-search search \"rev-parse --verify\" --corpus code --lines --path .claude/skills/report/scripts | head";
        let output = "NONE: no code chunk has every word.\n  -- outside --path: 1 file(s) hold it on one line, first 1 --\nMars_CRM/rust/crates/mars-skill-tools/tests/fixtures/report_spec/golden.json:77:        \"stderr\": \"git rev-parse\"";
        assert_eq!(leads(output, command), ["crates/mars-skill-tools/"]);
        let next = "~/.local/bin/local-search search \"rev-parse\" --path Mars_CRM/rust/crates/mars-skill-tools/src/report";
        let leads = leads(output, command);
        assert!(leads.iter().any(|l| next.contains(l.as_str())));
    }

    #[test]
    fn a_call_on_the_package_root_follows_the_lead() {
        // 2f3de906…jsonl:215 and the next call at :228.
        let command = r#"cd ~; ~/.local/bin/local-search search "\"--tab\"|terminal\", \"close\"|\"close\", \"--terminal\"" --corpus code --per-source 1 --limit 40 --lines 2>&1 | grep -v '^\s*$' | head -30"#;
        let output = "automation/crates/limit-resume/src/main.rs:899:            let _ = orca(&[\"terminal\", \"close\", \"--terminal\", handle, \"--tab\", \"--json\"]);\nautomation/crates/session-send/src/main.rs:163:        println!(\"{}\", close::receipt(&term, &orca(&[\"terminal\", \"close\"])?)?);";
        let leads = leads(output, command);
        assert_eq!(leads, ["crates/limit-resume/", "crates/session-send/"]);
        let next = r#"cd ~; ~/.local/bin/local-search search "exec claude|; exit" --corpus code --path /home/user/automation/crates/limit-resume --per-source 1 --limit 20 --lines 2>&1 | head -15"#;
        assert!(named(next, &leads[0]));
        assert!(named("rg -n x ~/automation/crates/limit-resume", &leads[0]));
        assert!(named("cd \"$HOME/automation/crates/limit-resume\"; cargo test", &leads[0]));
        assert!(named("sed -n 1,9p crates/limit-resume/src/notice.rs", &leads[0]));
    }

    #[test]
    fn a_longer_sibling_package_does_not_follow_the_lead() {
        assert!(!named("rg -n x ~/automation/crates/limit-resume-x", "crates/limit-resume/"));
        assert!(!named("rg -n x ~/automation/crates/limit-resume_tools/src", "crates/limit-resume/"));
        assert!(!named("cargo test -p limit-resume", "crates/limit-resume/"));
    }

    #[test]
    fn a_package_the_search_already_named_or_a_non_hit_line_is_no_lead() {
        let output = "src/telemetry/x.rs:5:fn a()\n/Users/b/automation/crates/cpu-watchdog/src/x.rs:5:fn a()";
        assert!(leads(output, "local-search search a --path ~/automation/crates/cpu-watchdog").is_empty());
        assert!(leads("note: crates/foo/src/a.rs is not here", "x").is_empty());
        assert!(leads("crates/foo:3:x", "x").is_empty());
    }
}
