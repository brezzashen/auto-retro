//! Two later searches that do not look for a hit's name again (founder decision A on search-retro
//! 86114628083256871521, 2026-11-12).
//!
//! 1. A plain one-word ASCII name (`canvas`, `target`) inside a longer identifier of the later
//!    search (`UsageDotCanvas`, `save_cargo_target`) names that other identifier. 7beb74e8…jsonl:149
//!    printed `SettingsTokens.swift:7 static let canvas = …0xF1F2F6`, the agent used it, and :164's
//!    `search "struct UsageDotCanvas|usageColumnCompact"` counted as a re-search of `canvas`; the same
//!    happened to f616ef29…jsonl:886 (`save_cargo_target` after `target`) in search-retro
//!    813111088844585114560. Names with pieces keep the sibling rule (`sibling.rs`): a superset such
//!    as `HomeGreetingTests` still re-searches `HomeGreeting`.
//! 2. An rg/grep that reads only files the hit's printed output named (by file name, or by a stem of
//!    six or more characters) reads inside the answer, it does not search again. 033f090b…jsonl:469
//!    searched history for `account founder`, :487 states the clue its lines gave (`demo-session
//!    --account founder`), and :504's `grep -n -i -E "founder|holder|…" demo-session.ts` counted as a
//!    re-search.

/// True when `name` (lowercased) stands in `asked` (the later search's pattern, original case) as a
/// whole token, or inside a token that also holds another of the query's `names` (`parseChatRequest`
/// for `chat` and `request`, 35565d7d…jsonl:3905), or is not a plain one-word ASCII name, so the usual
/// substring test applies.
pub(super) fn stands_alone(asked: &str, name: &str, plain: bool, names: &[&str]) -> bool {
    if !plain || !name.is_ascii() {
        return true;
    }
    tokens(asked).iter().any(|t| same_word(t, name) || (t.contains(name) && names.iter().any(|o| *o != name && o.len() >= 4 && t.contains(o))))
}

/// A token that is the name or its plural (`referrals` for `referral`, 6feb3543…jsonl:151 then :166
/// searched `api/referrals` for the same feature) is the same word.
fn same_word(token: &str, name: &str) -> bool {
    token.strip_prefix(name).is_some_and(|rest| matches!(rest, "" | "s" | "es"))
}

/// Lowercased ASCII identifier tokens of `text` (a Han character splits, as `answered::holds` reads
/// a short name); a regex escape letter (`\b`, `\w`) splits like a space.
fn tokens(text: &str) -> Vec<String> {
    let mut clean = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek().is_some_and(|n| n.is_ascii_alphabetic()) {
            chars.next();
            clean.push(' ');
        } else {
            clean.push(c);
        }
    }
    clean.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-').map(|t| t.trim_matches('-')).filter(|t| !t.is_empty()).map(str::to_lowercase).collect()
}

/// True when `files` (an rg/grep's file operands) are all files, not directories or globs, that the
/// hit's `printed` output named: a path by its last two components (`release-queue/SKILL.md` is not
/// the `product-listing/…` file a hit printed, 499f9e3f…jsonl:3277), a bare file by its name or stem
/// when the stem has six or more characters (`demo-session.ts`, not `SKILL.md`).
pub(super) fn reads_pointed(files: &[String], printed: &str) -> bool {
    !files.is_empty()
        && files.iter().all(|f| {
            let parts: Vec<&str> = f.trim_start_matches("./").split('/').filter(|p| !p.is_empty()).collect();
            let Some(base) = parts.last() else { return false };
            let Some((stem, ext)) = base.rsplit_once('.') else { return false };
            if stem.is_empty() || ext.is_empty() || f.contains('*') {
                return false;
            }
            match parts.len() {
                1 => stem.chars().count() >= 6 && (printed.contains(base) || names_stem(printed, stem)),
                n => printed.contains(&parts[n - 2..].join("/")),
            }
        })
}

/// True when `stem` stands in `printed` as a word that is not a directory (`demo-session `--account`,
/// not `canary/preflight/run.rs` for `preflight.rs`, f616ef29…jsonl:952 then :959).
fn names_stem(printed: &str, stem: &str) -> bool {
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/' | '.'));
    printed.match_indices(stem).any(|(i, _)| !word(printed[..i].chars().next_back()) && !word(printed[i + stem.len()..].chars().next()))
}

#[cfg(test)]
mod tests {
    use super::super::answered::{accepted, accepted_after};
    use super::super::steps::{Kind, Step};
    use super::*;

    fn shell(text: &str) -> Step {
        Step { at_ms: 1.0, id: "1".into(), kind: Kind::Shell, text: text.into(), line: 1, turn: "2".into() }
    }

    #[test]
    fn a_plain_name_inside_a_longer_identifier_is_no_re_search() {
        // 7beb74e8…jsonl:149 then :164; f616ef29…jsonl:886.
        let names = vec!["canvas".to_string(), "track".to_string()];
        assert!(accepted(&[], &[shell("~/.local/bin/local-search search \"struct UsageDotCanvas|usageColumnCompact\" --corpus code")], 0.0, &names, false, 8, 9e5));
        assert!(accepted(&[], &[shell("rg -n save_cargo_target src")], 0.0, &["target".into()], false, 8, 9e5));
        // The word itself, also as a regex word, still re-searches.
        assert!(!accepted(&[], &[shell("rg -n 'canvas' ios")], 0.0, &names, false, 8, 9e5));
        assert!(!accepted(&[], &[shell(r"rg -n '\bcanvas\b' ios")], 0.0, &names, false, 8, 9e5));
        assert!(!accepted(&[], &[shell("~/.local/bin/local-search search \"static let canvas\" --corpus code")], 0.0, &names, false, 8, 9e5));
        // A name with pieces keeps the superset rule (sibling.rs).
        assert!(!accepted(&[], &[shell("rg -n HomeGreetingTests web")], 0.0, &["HomeGreeting".into()], false, 8, 9e5));
        // A plural is the same word; a kebab identifier is one longer identifier.
        assert!(!accepted(&[], &[shell("~/.local/bin/local-search search \"api/referrals|me/invite\" --corpus code")], 0.0, &["referral".into()], false, 8, 9e5));
        assert!(accepted(&[], &[shell("rg -n invite-demo-canary-founder ~/.cache")], 0.0, &["founder".into()], false, 8, 9e5));
        // Two names joined in one identifier still re-search them (35565d7d…jsonl:3905).
        assert!(!accepted(&[], &[shell("rg -n parseChatRequest src")], 0.0, &["chat".into(), "request".into()], false, 8, 9e5));
    }

    #[test]
    fn a_grep_inside_a_file_the_answer_named_is_no_re_search() {
        // 033f090b…jsonl:469 printed `demo-session --account founder`; :504 grepped demo-session.ts.
        let names = vec!["account".to_string(), "founder".to_string()];
        let printed = "    …QA is authorized: via demo-session `--account founder` (privacy lock on iOS…";
        let grep = shell("cd /r/scripts && grep -n -i -E \"founder|holder|cache\" demo-session.ts | head -40");
        let after = [grep];
        assert!(accepted_after(&[], &after, 0.0, "1", &names, false, 8, 9e5, printed));
        // Unnamed files, directories and trees still re-search.
        assert!(!accepted_after(&[], &[shell("grep -n -i founder other-script.ts")], 0.0, "1", &names, false, 8, 9e5, printed));
        assert!(!accepted_after(&[], &[shell("grep -rn founder scripts")], 0.0, "1", &names, false, 8, 9e5, printed));
        assert!(!accepted_after(&[], &[shell("rg -n founder")], 0.0, "1", &names, false, 8, 9e5, printed));
    }

    #[test]
    fn pointed_files_need_a_named_file_or_long_stem() {
        assert!(reads_pointed(&["demo-session.ts".into()], "see demo-session for the flag"));
        assert!(reads_pointed(&["./web/src/a.rs".into()], "web/src/a.rs:3: fn a"));
        assert!(reads_pointed(&["/r/web/src/a.rs".into()], "src/a.rs:3: fn a"));
        // A short bare name or a path in another directory is not the printed file (499f9e3f…jsonl:3277).
        assert!(!reads_pointed(&["SKILL.md".into()], ".claude/skills/a/SKILL.md:3"));
        assert!(!reads_pointed(&[".claude/skills/release-queue/SKILL.md".into()], ".claude/skills/product-listing/SKILL.md:3"));
        assert!(!reads_pointed(&["a.rs".into()], "the a module"));
        assert!(!reads_pointed(&["preflight.rs".into()], "mars-ops/src/canary/preflight/run.rs:205: label"));
        assert!(!reads_pointed(&["src".into()], "src/a.rs"));
        assert!(!reads_pointed(&["*.rs".into()], "x.rs"));
        assert!(!reads_pointed(&[], "x"));
    }
}
