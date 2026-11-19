//! A hit whose printed lines answered the question counts as adopted (founder, 2026-11-02, search
//! retro 815462549835379528073: 11 of 19 unadopted hits were existence, survey or signature
//! answers read straight from `path:line:text`, such as `--lock-status` existing or no analytics
//! SDK in the code).
//!
//! The answer is accepted when the agent does not search for the same names again: none of the
//! next ADOPT_CALLS tool calls within ADOPT_MS is a search (local-search search|query, rg, grep,
//! find, agent-memory recall, session-status) whose text holds a query name or two of its
//! camelCase or snake_case pieces of four or more letters, lowercased, so `ComposerDraftStore` is
//! re-searched by `query "composer text draft"`. One piece alone is not enough (search-retro
//! 87040299049855422752): `ComposerTakeoverContext` was not re-searched by
//! `search "PaneActiveContext|usePaneActive"` (d308ac40…jsonl:174), which shares only `context`. A re-search keeps the hit
//! unadopted; a wrong answer the agent silently accepted is the remaining over-count.
use super::steps::{Kind, Step};

/// Output the agent did not see whole: a harness truncation or a saved-to-file notice. Its printed
/// lines cannot have answered (search-retro 83012811238074165395: an Antigravity report query whose
/// output went to steps/16/output.txt).
pub(super) fn truncated(output: &str) -> bool {
    ["output was truncated", "Output too large", "<persisted-output>"].iter().any(|m| output.contains(m))
}

/// A run of three or more Han characters is a name: a Chinese word is short (`子任务`).
pub(super) fn han_name(word: &str) -> bool {
    word.chars().count() >= super::followed::MIN_HAN && super::followed::is_han_run(word)
}

/// Two Han characters: a name only when it is the whole query (`quality::names_of`), such as a
/// person's name; inside a longer query a two-character word is too common to mark a re-search.
pub(super) fn han_short_name(word: &str) -> bool {
    word.chars().count() == 2 && super::followed::is_han_run(word)
}

/// True when `command` runs `local-search query`: its names are the words of a sentence. A `search`
/// for a prose phrase of three or more plain words with no `|` alternatives is one too: search-retro
/// 815285992441756886172 (9963235a…jsonl:529) found `search "Page reloaded after the code was sent"
/// --corpus watchdog` in a sign-up dossier, and the next call `query "iOS verify OTP code relay
/// fallback …" --corpus code` asked a different question that shared only the word `code`.
pub(super) fn sentence(command: &str) -> bool {
    super::quality::commands(command).iter().any(|(_, segment)| {
        let words: Vec<&str> = segment.split_whitespace().skip_while(|w| w.contains('=') && !w.starts_with('-')).collect();
        let local = words.first().is_some_and(|p| p.rsplit('/').next() == Some("local-search"));
        local && (words.get(1) == Some(&"query") || (words.get(1) == Some(&"search") && asked(segment).is_some_and(|q| prose(&q))))
    })
}

/// Three or more words of letters only, no regex or alternatives: `Page reloaded after the code was sent`.
fn prose(phrase: &str) -> bool {
    let words: Vec<&str> = phrase.split_whitespace().collect();
    words.len() >= 3 && words.iter().all(|w| w.chars().all(char::is_alphabetic) || han_name(w))
}

/// True when no later search within the adoption window looks for one of `names`. For a `query`
/// (`sentence`) one shared plain word is not enough: the later search must hold two of its words or
/// one identifier-shaped word (camelCase, or a digit, `_`, `-`, `.` or `/` inside). Search-retro
/// 816506381674351735418: `query "event chip right click context menu …"` was "re-searched" by
/// `rg '"event-status": \{'` (00150745…jsonl:5617), and `query "… API route /api/home"` by a grep
/// for `route.fulfill` (3a1577a9…jsonl:2387), each a different question.
#[cfg(test)]
pub(super) fn accepted(items: &[Step], after: &[Step], at_ms: f64, names: &[String], sentence: bool, turns: usize, window_ms: f64) -> bool {
    accepted_after(items, after, at_ms, "", names, sentence, turns, window_ms, "")
}

/// `accepted` for a search issued by the assistant message `turn`. The calls that message ran
/// beside it were written before its result existed, so none of them re-searches what it printed;
/// they fill no turn of the window either (`adopt.rs` window). Search-retro 813204256598053172535 (66fc47b4…jsonl:96):
/// one message ran `search "claude-opus-5-5|ANTHROPIC_DEFAULT_FABLE_MODEL|…"` and `agent-memory
/// recall "… ANTHROPIC_DEFAULT_FABLE_MODEL …"` (:100); the hit lines showed the ids only in
/// benchmark lists, the agent went on to the settings files, and the recall counted as a re-search.
/// `printed` is the hit's output: an rg/grep inside a file it named is no re-search (`inner.rs`).
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) fn accepted_after(items: &[Step], after: &[Step], at_ms: f64, turn: &str, names: &[String], sentence: bool, turns: usize, window_ms: f64, printed: &str) -> bool {
    accepted_scoped(items, after, at_ms, turn, names, sentence, turns, window_ms, printed, &|_, _| false)
}

/// `accepted_after` where `elsewhere(step, segment)` says a later search segment looks only outside
/// the hit's `--path` scopes, so it is no re-search (`scoped::elsewhere_from`).
#[allow(clippy::too_many_arguments)]
pub(super) fn accepted_scoped(items: &[Step], after: &[Step], at_ms: f64, turn: &str, names: &[String], sentence: bool, turns: usize, window_ms: f64, printed: &str, elsewhere: &dyn Fn(&Step, &str) -> bool) -> bool {
    let shapes = shapes(names);
    if shapes.is_empty() {
        return false;
    }
    !super::adopt::window(after, at_ms, turn, turns, window_ms)
        .iter()
        .filter(|s| s.kind == Kind::Shell && s.turn != turn)
        .any(|s| {
            let programs = researching(&s.text, &shapes, sentence, printed, &|segment| elsewhere(s, segment));
            !programs.is_empty() && !silent(items, s, &programs, names)
        })
}

/// An rg/grep-only re-search whose output holds no line with a query name found nothing the hit
/// missed (founder decision 2A on search-retro 813111088844585114560, the mirror of
/// `none::confirms_names`): f616ef29…jsonl:922 grepped `"release"` in seed_sources.rs after
/// affected.rs:138 had answered, and printed no release line. The step's output holds every
/// command of the call, so a name printed by another command still counts as found.
fn silent(items: &[Step], step: &Step, programs: &[String], names: &[String]) -> bool {
    let greps = programs.iter().all(|p| matches!(p.as_str(), "rg" | "grep" | "egrep" | "ugrep"));
    let Some(result) = super::quality::result_of(items, &step.id) else { return false };
    let lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    greps && !result.text.lines().any(|l| { let l = l.to_lowercase(); lower.iter().any(|n| l.contains(n.as_str())) })
}

/// Lowercased names and their camelCase/snake_case pieces of four or more letters.
pub(super) fn parts(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in names {
        let mut pieces = vec![name.to_lowercase()];
        let mut current = String::new();
        let chars: Vec<char> = name.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            let boundary = c == '_' || (c.is_uppercase() && i > 0 && chars[i - 1].is_lowercase());
            if boundary && !current.is_empty() {
                pieces.push(std::mem::take(&mut current).to_lowercase());
            }
            if c != '_' {
                current.push(c);
            }
        }
        pieces.push(current.to_lowercase());
        for p in pieces {
            if p.chars().filter(|c| c.is_alphanumeric()).count() >= 4 && !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// Each name as its lowercased whole (when four or more letters) and its other `parts`.
fn shapes(names: &[String]) -> Vec<(Option<String>, Vec<String>)> {
    names
        .iter()
        .map(|n| {
            let whole = n.to_lowercase();
            let pieces = parts(std::slice::from_ref(n)).into_iter().filter(|p| *p != whole).collect();
            // A two-character Han name reaches here only as a whole query (`quality::names_of`).
            let named = whole.chars().filter(|c| c.is_alphanumeric()).count() >= 4 || han_name(&whole) || han_short_name(&whole) || super::pattern::short_word(n);
            (named.then_some(whole), pieces)
        })
        .filter(|(whole, pieces): &(Option<String>, Vec<String>)| whole.is_some() || pieces.len() >= 2)
        .collect()
}

/// A simple command that is a search (program word first, not fed by a single `|`) and whose own
/// text holds a whole name or two pieces of one. `cd ~/automation/crates/cpu-watchdog` or `echo "$out" | grep -q Published` is not one.
/// For rg/grep/find only the pattern counts (`pattern.rs`), as in `quality::sweeps_for`: a file
/// argument is not what the agent looked for. Search-retro 8114242514842044170, 57305590…jsonl:1515:
/// `grep -n "08:59:10Z" $D/report.md` re-searched `report` of the query "report needs duplicate_analysis".
fn researching(command: &str, shapes: &[(Option<String>, Vec<String>)], sentence: bool, printed: &str, elsewhere: &dyn Fn(&str) -> bool) -> Vec<String> {
    super::quality::commands(command).iter().filter(|(_, segment)| !elsewhere(segment)).filter_map(|(piped, segment)| {
        let lower = segment.to_lowercase();
        let words: Vec<&str> = lower.split_whitespace().skip_while(|w| w.contains('=') && !w.starts_with('-')).collect();
        let program = words.first().map(|p| p.rsplit('/').next().unwrap_or(p)).unwrap_or("");
        let home = super::unindexed::home();
        let search = match program {
            // A sweep of a tree no corpus indexes reads what the hit could not hold (`unindexed.rs`).
            "rg" | "grep" | "egrep" | "find" | "ugrep" => !piped && !super::unindexed::only_unindexed_in(command, segment, &home) && !super::unindexed::within(command, segment, &home) && !super::inner::reads_pointed(&super::pattern::files(segment), printed),
            "session-status" => !piped,
            "local-search" => matches!(words.get(1), Some(&("search" | "query"))),
            "agent-memory" => words.get(1) == Some(&"recall"),
            _ => false,
        };
        // Original case: `sibling` splits camelCase names of the later search.
        let asked_text = if matches!(program, "rg" | "grep" | "egrep" | "find" | "ugrep") {
            super::pattern::of(segment).unwrap_or_else(|| segment.clone())
        } else if program == "local-search" {
            asked(segment).unwrap_or_else(|| segment.clone())
        } else {
            super::pattern::arguments(segment)
        };
        let looked_for = super::domain::unescape(&asked_text.to_lowercase());
        let all: Vec<&str> = shapes.iter().filter_map(|(w, _)| w.as_deref()).collect();
        let mut wholes: Vec<(&String, bool)> = shapes.iter().filter_map(|(w, p)| w.as_ref().map(|w| (w, !p.is_empty() || identifier(w)))).filter(|(w, named)| holds(&looked_for, w) && super::inner::stands_alone(&asked_text, w, !*named, &all)).collect();
        wholes.sort();
        wholes.dedup();
        let by_wholes = if sentence {
            wholes.len() >= 2 || wholes.iter().any(|(_, named)| *named) || (!wholes.is_empty() && only_these(&looked_for, shapes))
        } else {
            !wholes.is_empty()
        };
        // Two pieces count only outside a sibling identifier of the name (`sibling.rs`); the
        // kebab program that opens a snake name is one piece (`owner.rs`).
        let by_pieces = || shapes.iter().any(|(whole, pieces)| {
            let (text, owned) = super::owner::without_owners(&super::sibling::without_siblings(&asked_text, whole.as_deref(), pieces), whole.as_deref());
            pieces.iter().filter(|p| !owned.contains(p) && text.contains(p.as_str())).count() + usize::from(!owned.is_empty()) >= 2
        });
        (search && (by_wholes || by_pieces())).then(|| program.to_string())
    }).collect()
}

/// The query text of a `local-search search|query` segment, replays (LOCAL_SEARCH_NO_LOG) included.
fn asked(segment: &str) -> Option<String> {
    let words: Vec<&str> = segment.split_whitespace().collect();
    let sub = words.iter().position(|w| matches!(*w, "search" | "query"))?;
    let rest = words[sub + 1..].join(" ");
    Some(match rest.chars().next() {
        Some(q @ ('"' | '\'')) => super::steps::quoted(&rest[1..], q),
        _ => rest.split_whitespace().next().unwrap_or_default().to_string(),
    })
}

/// True when every word of four or more characters the later search asked for is a name of the
/// sentence: `search releases` after `query "how releases queue"` asks for nothing else.
fn only_these(looked_for: &str, shapes: &[(Option<String>, Vec<String>)]) -> bool {
    let mut words = looked_for.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 4 || han_name(w)).peekable();
    words.peek().is_some() && words.all(|w| shapes.iter().any(|(whole, _)| whole.as_deref() == Some(w)))
}

/// True when `text` holds `name`: a short ASCII name (an acronym) only as a whole word.
fn holds(text: &str, name: &str) -> bool {
    if name.chars().count() >= 4 || !name.is_ascii() {
        return text.contains(name);
    }
    text.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == name)
}

/// A word shaped like an identifier rather than prose: a digit, `_`, `-`, `.` or `/` inside (a
/// camelCase word shows as its pieces in `shapes`).
fn identifier(word: &str) -> bool {
    word.chars().any(|c| c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(at_ms: f64, text: &str) -> Step {
        Step { at_ms, id: format!("{at_ms}"), kind: Kind::Shell, text: text.into(), line: 1, turn: format!("{at_ms}") }
    }

    #[test]
    fn truncated_output_is_not_an_answer() {
        assert!(truncated("The tool's output was truncated due to being too long."));
        assert!(truncated("<persisted-output>\nOutput too large (46.7KB)."));
        assert!(!truncated("web/src/a.ts:3:const a = 1"));
    }

    #[test]
    fn names_split_into_lowercase_parts() {
        assert_eq!(parts(&["ComposerDraftStore".into(), "lock_status".into(), "id".into()]), ["composerdraftstore", "composer", "draft", "store", "lock_status", "lock", "status"]);
    }

    #[test]
    fn a_later_search_for_the_same_name_refuses_the_answer() {
        let names = vec!["simRun".to_string()];
        // Search-retro 815462549835379528073: df3c74dd…jsonl:386 then a grep for simRun in registry.rs.
        assert!(!accepted(&[], &[shell(1.0, "cd /r; grep -n 'simRun\\|workspaceSync' src/registry.rs")], 0.0, &names, false, 8, 9e5));
        assert!(!accepted(&[], &[shell(1.0, "LOCAL_SEARCH_MISS=1 rg -n 'hour' locales")], 0.0, &["hour".into()], false, 8, 9e5));
        assert!(!accepted(&[], &[shell(1.0, "~/.local/bin/session-status macmini ssh")], 0.0, &["macmini".into()], false, 8, 9e5));
        let draft = vec!["ComposerDraftStore".to_string()];
        assert!(!accepted(&[], &[shell(1.0, "local-search query \"composer text draft\" --corpus code")], 0.0, &draft, false, 8, 9e5));
    }

    #[test]
    fn one_shared_piece_is_not_a_search_for_the_same_name() {
        // Search-retro 87040299049855422752, d308ac40…jsonl:174 then :187.
        let names = vec!["useComposerTakeoverHost".to_string(), "ComposerTakeoverContext".to_string(), "value".to_string()];
        assert!(accepted(&[], &[shell(1.0, "~/.local/bin/local-search search \"PaneActiveContext|usePaneActive\" --corpus code")], 0.0, &names, false, 8, 9e5));
        assert!(!accepted(&[], &[shell(1.0, "rg -n 'TakeoverContext' src")], 0.0, &names, false, 8, 9e5));
        assert!(!accepted(&[], &[shell(1.0, "rg -n 'takeover_host|composer' src")], 0.0, &names, false, 8, 9e5));
    }

    #[test]
    fn using_the_answer_without_searching_again_accepts_it() {
        // 61d75dce…jsonl:226: `cpu-watchdog` in a cd and a pipe filter are not searches for parityWatch.
        assert!(accepted(&[], &[shell(1.0, "cd ~/automation/crates/cpu-watchdog; rg -n 'fn sweep' quality.rs")], 0.0, &["parityWatch".into()], false, 8, 9e5));
        assert!(accepted(&[], &[shell(1.0, "out=$(bun publish.ts); echo \"$out\" | grep -q lock")], 0.0, &["lock".into()], false, 8, 9e5));
        // 59f017bb…jsonl:522: a migration number is a name too.
        assert!(!accepted(&[], &[shell(1.0, "rg -n 20270224000000 supabase")], 0.0, &["20270224000000".into()], false, 8, 9e5));
        let names = vec!["lock-status".to_string(), "lock".to_string(), "status".to_string()];
        // cd1d3980…jsonl:6432: the flag existed, so the agent ran it.
        assert!(accepted(&[], &[shell(1.0, "bun .claude/skills/report/scripts/publish.ts --lock-status")], 0.0, &names, false, 8, 9e5));
        assert!(accepted(&[], &[shell(1.0, "grep -n deploymentTarget project.yml")], 0.0, &["segmentMuted".into()], false, 8, 9e5));
        // A re-search after the window or after the call budget does not count.
        assert!(accepted(&[], &[shell(1e6, "rg simRun")], 0.0, &["simRun".into()], false, 8, 9e5));
        assert!(!accepted(&[], &[], 0.0, &[], false, 8, 9e5));
    }

    #[test]
    fn a_call_the_same_message_ran_beside_the_search_is_no_re_search() {
        // Search-retro 813204256598053172535, 66fc47b4…jsonl:96 and :100 share one message id.
        let names: Vec<String> = ["claude-opus-5-5", "ANTHROPIC_DEFAULT_FABLE_MODEL", "claude-fable-5-1", "claude-fable-5-2"].map(String::from).to_vec();
        let mut recall = shell(1.0, "~/.local/bin/agent-memory recall \"Claude Code default model fable pin ANTHROPIC_DEFAULT_FABLE_MODEL cc1 settings.json\" 2>&1 | cut -c1-600 | head -40; echo; sed -n 1,20p ~/.claude/settings.json");
        recall.turn = "msg_011CgtcMsp79WW6rrR5jMdxX".into();
        let after = [recall];
        // Read without its turn, the recall is the re-search that kept the hit unadopted.
        assert!(!accepted(&[], &after, 0.0, &names, false, 8, 9e5));
        assert!(accepted_after(&[], &after, 0.0, "msg_011CgtcMsp79WW6rrR5jMdxX", &names, false, 8, 9e5, ""));
        // The same recall issued by the next message saw the result: a re-search.
        assert!(!accepted_after(&[], &after, 0.0, "msg_before", &names, false, 8, 9e5, ""));
    }

    #[test]
    fn a_search_outside_the_hit_scope_is_no_re_search() {
        // Search-retro 82104642038523697224, 24596541…jsonl:249 (`scoped::elsewhere_from`).
        let names = vec!["done".to_string()];
        let after = [shell(1.0, "cd /r; local-search search \"type: 'done'\" --path Mars_CRM/agent/src | head")];
        assert!(!accepted(&[], &after, 0.0, &names, false, 8, 9e5));
        let elsewhere = |_: &Step, segment: &str| segment.contains("agent/src");
        assert!(accepted_scoped(&[], &after, 0.0, "", &names, false, 8, 9e5, "", &elsewhere));
    }

    #[test]
    fn a_name_only_in_a_grep_file_argument_is_no_re_search() {
        // Search-retro 8114242514842044170, 57305590…jsonl:1515: the answer was read from the printed line.
        let names = vec!["report".to_string(), "needs".to_string(), "duplicate_analysis".to_string()];
        assert!(accepted(&[], &[shell(1.0, "grep -n \"08:59:10Z\" $D/report.md")], 0.0, &names, false, 8, 9e5));
        assert!(!accepted(&[], &[shell(1.0, "grep -n duplicate_analysis $D/report.json")], 0.0, &names, false, 8, 9e5));
        assert!(!accepted(&[], &[shell(1.0, "~/.local/bin/local-search search \"report needs\" --corpus code")], 0.0, &names, false, 8, 9e5));
    }

    #[test]
    fn one_plain_word_of_a_sentence_query_is_no_re_search() {
        // Search-retro 816506381674351735418, 00150745…jsonl:5617 and 3a1577a9…jsonl:2387.
        let words = |q: &str| q.split_whitespace().filter(|w| w.chars().count() >= 4).map(str::to_string).collect::<Vec<_>>();
        let event = words("event chip right click context menu edit item label desktop events");
        assert!(accepted(&[], &[shell(1.0, "rg -n '\"event-status\": \\{' -A3 scenes.json")], 0.0, &event, true, 8, 9e5));
        // The word itself after a `search` for it is still a re-search; inside the longer identifier
        // `event-status` it is not (founder decision A on search-retro 86114628083256871521, `inner.rs`).
        assert!(!accepted(&[], &[shell(1.0, "rg -n '\"event\": \\{' -A3 scenes.json")], 0.0, &event, false, 8, 9e5));
        assert!(accepted(&[], &[shell(1.0, "rg -n '\"event-status\": \\{' -A3 scenes.json")], 0.0, &event, false, 8, 9e5));
        let home = words("web home greeting overdue contacts count and suggestion rows route home");
        assert!(accepted(&[], &[shell(1.0, "grep -n \"return route.fulfill({ response\" day_capture.js")], 0.0, &home, true, 8, 9e5));
        // A name the sentence says twice (`home`, `/api/home`) is still one word (3a1577a9…jsonl:2505).
        assert!(accepted(&[], &[shell(1.0, "grep -n \"HOME_STEPS=\" s08_composer_cap.sh")], 0.0, &home, true, 8, 9e5));
        let real = r#"cd /s/scripts; grep -n "if (drop || values.prefs)\|return route.fulfill({ response, json: drop\|prefs: { type\|const drop = " day_capture.js"#;
        assert!(accepted(&[], &[shell(1.0, real)], 0.0, &home, true, 8, 9e5));
        // Two words of the sentence (35565d7d…jsonl:3905), or one identifier-shaped word, still re-search.
        let chat = vec!["demo-session".to_string(), "request".into(), "chat".into(), "threadId".into(), "message".into()];
        assert!(!accepted(&[], &[shell(1.0, "~/.local/bin/local-search search \"parseChatRequest\" --corpus code")], 0.0, &chat, true, 8, 9e5));
        assert!(!accepted(&[], &[shell(1.0, "rg -n threadId src/routes")], 0.0, &chat, true, 8, 9e5));
        assert!(!accepted(&[], &[shell(1.0, "grep -n demo-session SKILL.md")], 0.0, &chat, true, 8, 9e5));
    }

    #[test]
    fn a_short_han_word_is_a_name() {
        // Search-retro 816506381674351735418, a789a969…jsonl:1956: `search "子任务："` had no name at all.
        assert!(han_name("子任务") && !han_name("子任") && !han_name("ab子任务"));
        let names = vec!["子任务".to_string()];
        assert!(accepted(&[], &[shell(1.0, "grep -n \"export function StepCard\" StepCard.tsx")], 0.0, &names, false, 8, 9e5));
        assert!(!accepted(&[], &[shell(1.0, "~/.local/bin/local-search search \"子任务\" --corpus code")], 0.0, &names, false, 8, 9e5));
        assert!(sentence("cd /r; ~/.local/bin/local-search query \"how\" --corpus code | head") && !sentence("local-search search a"));
    }

    #[test]
    fn a_two_character_han_query_confirms_absence_unless_searched_again() {
        // Search-retro 87276522378180065818, d02e115b…jsonl:517: `何棠` printed only onboarding
        // fixtures and the agent switched person (:540) without searching the name again.
        assert!(han_short_name("何棠") && !han_short_name("子任务") && !han_short_name("ab"));
        let names = vec!["何棠".to_string()];
        assert!(accepted(&[], &[shell(1.0, "~/.local/bin/local-search search \"segmentId:\" --corpus code")], 0.0, &names, false, 8, 9e5));
        assert!(!accepted(&[], &[shell(1.0, "rg -n 何棠 Mars_CRM/rust")], 0.0, &names, false, 8, 9e5));
    }

    #[test]
    fn a_prose_phrase_search_is_a_sentence() {
        // Search-retro 815285992441756886172, 9963235a…jsonl:529: the phrase search and the next query shared only `code`.
        let call = "~/.local/bin/local-search search \"Page reloaded after the code was sent\" --corpus watchdog --per-source 1 --limit 50 --lines 2>&1 | cut -c1-200 | head -20";
        assert!(sentence(call));
        // An identifier, an `a|b` pattern or a two-word phrase stays a name search.
        for other in ["local-search search \"tool_step_labels_zh_cn\" --corpus code", "local-search search \"insert into vectors|from vectors\" --corpus code", "local-search search \"fun MarsTopBar\" --corpus code", "local-search search \"Page reloaded after code.sent\""] {
            assert!(!sentence(other), "{other}");
        }
        let names: Vec<String> = ["Page", "reloaded", "after", "code", "sent"].map(String::from).to_vec();
        let next = "cd /r; ~/.local/bin/local-search query \"iOS verify OTP code relay fallback direct Supabase URLSession\" --corpus code --path Mars_CRM/ios 2>&1 | head -14";
        assert!(accepted(&[], &[shell(1.0, next)], 0.0, &names, sentence(call), 8, 9e5));
        // As a name search the shared `code` was a re-search; two shared words still are.
        assert!(!accepted(&[], &[shell(1.0, next)], 0.0, &names, false, 8, 9e5));
        assert!(!accepted(&[], &[shell(1.0, "local-search search \"page reloaded\" --corpus watchdog")], 0.0, &names, true, 8, 9e5));
    }
}
