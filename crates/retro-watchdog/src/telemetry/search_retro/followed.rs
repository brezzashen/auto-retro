//! A hit is adopted when the agent follows what its printed lines named (search-retro
//! 84763104493312366879): a file the hit text referenced (`from '../lib/api.ts'`, ``
//! (`mars-dev-tools/src/db/db_local.rs`) ``) or an identifier it printed that the query did not
//! hold (`addDictionaryWord` from a search for `dictionary/words`), named by one of the next
//! tool calls in the adoption window. 3 of that window's 13 unadopted hits were read this way:
//! a grep inside the referenced file, or the next search for the printed name.
use super::steps::{Kind, Step};

const MIN_IDENT: usize = 8;

/// Leads printed in the text of `path:line:text` hit lines: referenced file paths (last two
/// components of a token with `/` and a file extension) and identifiers of MIN_IDENT or more
/// characters with an inner capital or underscore. A lead equal to a query name is left out, so a
/// later search for the same name stays a re-search; a longer name holding it (`addDictionaryWord`
/// after `dictionary/words`) is the hit's answer.
pub(super) fn leads(output: &str, names: &[String]) -> Vec<String> {
    let names: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let mut out: Vec<String> = Vec::new();
    let mut in_hit = false;
    for line in output.lines() {
        let text = match hit_text(line) {
            Some(text) => { in_hit = true; text }
            None if in_hit => match body_text(line) { Some(text) => text, None => { in_hit = false; continue } },
            None => continue,
        };
        for token in text.split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))) {
            let lead = file_lead(token).or_else(|| ident_lead(token)).or_else(|| han_lead(token));
            if let Some(lead) = lead {
                let lower = lead.to_lowercase();
                if !names.contains(&lower) && !out.contains(&lead) {
                    out.push(lead);
                }
            }
        }
    }
    out
}

/// True when a tool call of the next `turns` assistant turns after the message `own`, within
/// `window_ms`, names a lead.
pub(super) fn followed(after: &[Step], at_ms: f64, own: &str, leads: &[String], turns: usize, window_ms: f64) -> bool {
    !leads.is_empty()
        && super::adopt::window(after, at_ms, own, turns, window_ms)
            .iter()
            .filter(|s| matches!(s.kind, Kind::Shell | Kind::Tool))
            .any(|s| leads.iter().any(|l| (s.text.contains(l.as_str()) || super::package::named(&s.text, l)) && (!is_han_run(l) || searches(&s.text))))
}

/// A run of MIN_HAN or more Han characters printed in a hit line (a localization key such as
/// `"查看“%@”的来源"`). Search-retro 82450840685001354157, 782afcb5…jsonl:523: the hit printed that
/// key from Localizable.strings and the next search, `local-search search "的来源"` (:534), looked
/// for its caller with the fixed part after the placeholder.
fn han_lead(token: &str) -> Option<String> {
    (token.chars().count() >= MIN_HAN && is_han_run(token)).then(|| token.to_string())
}

pub(super) const MIN_HAN: usize = 3;

pub(super) fn is_han_run(text: &str) -> bool {
    text.chars().all(|c| matches!(c, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'))
}

/// A Han lead is followed only by a search for it: Chinese words also fill commit messages, report
/// text and edits, where naming the run means nothing about the hit.
fn searches(command: &str) -> bool {
    super::quality::commands(command).iter().any(|(_, segment)| {
        let program = segment.split_whitespace().find(|w| !w.contains('=')).unwrap_or_default();
        matches!(program.rsplit('/').next().unwrap_or(program), "local-search" | "rg" | "grep" | "egrep" | "ugrep")
    })
}

/// The text after `path:line:` (or `path:start-end`) of a hit line, where the path names a file.
fn hit_text(line: &str) -> Option<&str> {
    let mut parts = line.trim_start().splitn(3, ':');
    let (path, place, text) = (parts.next()?, parts.next()?, parts.next().unwrap_or_default());
    let numeric = !place.is_empty() && place.split_whitespace().next()?.chars().all(|c| c.is_ascii_digit() || c == '-');
    (numeric && !path.contains(' ') && path.rsplit('/').next().is_some_and(|n| n.contains('.'))).then_some(text)
}

/// The text of a `local-search query` body line under a hit header: indent, line number, tab.
/// Search-retro 84886809318527941381 (fb52f3c7…jsonl:266): a query hit printed
/// `        70\t  beforeEach() { initializeAppearance().setPreference('dark') },` and the next
/// call searched `export function initializeAppearance`; only header lines were read for leads.
fn body_text(line: &str) -> Option<&str> {
    let rest = line.strip_prefix(|c: char| c == ' ' || c == '\t')?.trim_start_matches(' ');
    let digits = rest.find(|c: char| !c.is_ascii_digit())?;
    (digits > 0 && rest[digits..].starts_with('\t')).then(|| &rest[digits + 1..])
}

fn file_lead(token: &str) -> Option<String> {
    let token = token.trim_end_matches('.');
    let parts: Vec<&str> = token.split('/').filter(|p| !p.is_empty() && *p != "." && *p != "..").collect();
    let name = parts.last()?;
    let (stem, ext) = name.rsplit_once('.')?;
    let ext_ok = !ext.is_empty() && ext.len() <= 6 && ext.chars().all(|c| c.is_ascii_alphanumeric()) && ext.chars().any(|c| c.is_ascii_alphabetic());
    if !ext_ok {
        return None;
    }
    if parts.len() >= 2 {
        return Some(parts[parts.len() - 2..].join("/"));
    }
    // A sibling import (`from './crmServer.ts'`) names one file explicitly; its bare name is the lead
    // when the stem is distinctive (search-retro 85874229037822047064, aec4ef1c…jsonl:249 → :262
    // `sed -n 1,60p crmServer.ts`). `./index.ts` or `./types.ts` would tie unrelated reads to the hit.
    ((token.starts_with("./") || token.starts_with("../")) && distinctive_stem(stem)).then(|| name.to_string())
}

/// A file stem that names one module: an inner capital, `_` or `-`, or eight or more characters.
fn distinctive_stem(stem: &str) -> bool {
    let chars: Vec<char> = stem.chars().collect();
    chars.len() >= 8 || chars.windows(2).any(|w| (w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase()) || (w[0].is_ascii_alphanumeric() && matches!(w[1], '_' | '-')))
}

fn ident_lead(token: &str) -> Option<String> {
    let chars: Vec<char> = token.chars().collect();
    let word = chars.iter().all(|c| c.is_ascii_alphanumeric() || *c == '_') && chars.first().is_some_and(|c| c.is_ascii_alphabetic());
    let inner = chars.windows(2).any(|w| (w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase()) || (w[0].is_ascii_alphanumeric() && w[1] == '_'));
    (word && inner && chars.len() >= MIN_IDENT).then(|| token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(at_ms: f64, text: &str) -> Step {
        Step { at_ms, id: format!("{at_ms}"), kind: Kind::Shell, text: text.into(), line: 1, turn: format!("{at_ms}") }
    }

    #[test]
    fn a_grep_inside_a_file_the_hit_imported_follows_it() {
        // ef7b1784…jsonl:68: the import line named ../lib/api.ts; the agent grepped src/lib/api.ts.
        let out = "src/components/useDeletedPeople.ts:5:import { fetchPersonFate, type PersonFate } from '../lib/api.ts'";
        let leads = leads(out, &["PersonFate".into()]);
        assert_eq!(leads, ["fetchPersonFate", "lib/api.ts"]);
        assert!(followed(&[shell(1.0, "grep -n \"PersonFate\\|people\" src/lib/api.ts | head")], 0.0, "", &leads, 8, 9e5));
    }

    #[test]
    fn a_file_a_docs_line_referenced_and_a_printed_identifier_are_leads() {
        // 59f017bb…jsonl:1210 and 8e542550…jsonl:100.
        let docs = ".claude/skills/x/references/source-map.md:181:  admin URL is loopback port 8038 (`mars-dev-tools/src/db/db_local.rs`).";
        assert_eq!(leads(docs, &["MARS_RUST_TEST_DATABASE_URL".into()]), ["db/db_local.rs"]);
        let code = "Mars_CRM/web/src/lib/api/personalDictionary.ts:10:export const addDictionaryWord = (text: string) => send<DictionaryWord>('POST', '/api/me/dictionary/words', { text })";
        let leads = leads(code, &["dictionary".into(), "words".into()]);
        assert_eq!(leads, ["addDictionaryWord", "DictionaryWord"]);
        assert!(!super::leads("a/b.ts:1:const personFate = 1", &["PersonFate".into()]).contains(&"personFate".to_string()));
        assert!(followed(&[shell(1.0, "local-search search \"addDictionaryWord\" --corpus code")], 0.0, "", &leads, 8, 9e5));
    }

    #[test]
    fn a_printed_han_key_is_a_lead_only_a_search_follows() {
        // 782afcb5…jsonl:523 → :534.
        let out = "MarsCRM/App/Localization/en.lproj/Localizable.strings:2046:\"查看“%@”的来源\" = \"View the source of “%@”\";";
        let names = ["View".to_string(), "source".to_string(), "evidenceSource".to_string()];
        let leads = leads(out, &names);
        assert_eq!(leads, ["的来源"]);
        assert!(followed(&[shell(1.0, "cd ios; ~/.local/bin/local-search search \"的来源\" --corpus code --path MarsCRM/Features --lines | head")], 0.0, "", &leads, 8, 9e5));
        assert!(!followed(&[shell(1.0, "git commit -m \"修复的来源标签\"")], 0.0, "", &leads, 8, 9e5));
        // Two-character runs are too common to tie a call to the hit.
        assert!(super::leads("a/b.strings:1:\"查看\" = \"View\"", &[]).is_empty());
    }

    #[test]
    fn a_query_body_line_under_a_hit_is_hit_text() {
        // fb52f3c7…jsonl:266 → :282.
        let out = "  /u/Mars_agent/Mars_CRM/web/src/components/library/markdown/CssSummaryPreview.stories.tsx:68-71  [code/block] export const Dark: Story = · 2026-09-19\n        68\texport const Dark: Story = {\n        70\t  beforeEach() { initializeAppearance().setPreference('dark') },";
        let leads = leads(out, &["dark".into(), "theme".into()]);
        assert!(leads.contains(&"initializeAppearance".to_string()), "{leads:?}");
        assert!(followed(&[shell(1.0, "local-search search \"export function initializeAppearance\" --corpus code")], 0.0, "", &leads, 8, 9e5));
        // A numbered line with no hit header above it is no hit text.
        assert!(super::leads("        70\t  initializeAppearance()", &[]).is_empty());
        assert!(super::leads("a/b.rs:3:x\nnote\n        70\t  initializeAppearance()", &[]).is_empty());
    }

    #[test]
    fn a_sibling_import_with_a_distinctive_stem_is_a_lead() {
        // aec4ef1c…jsonl:249 → :262: the hits imported './crmServer.ts'; the agent read crmServer.ts.
        let out = "Mars_CRM/agent/tests/insightExecution.test.ts:9:import { crmServerMissing, liveCrm } from './crmServer.ts'";
        let leads = leads(out, &["crmServer".into()]);
        assert!(leads.contains(&"crmServer.ts".to_string()), "{leads:?}");
        assert!(followed(&[shell(1.0, "cd Mars_CRM/agent/tests; sed -n 1,60p crmServer.ts")], 0.0, "", &leads, 8, 9e5));
        // Generic sibling names and bare file names without `./` stay out.
        assert!(super::leads("a/b.ts:1:export * from './index.ts'", &[]).is_empty());
        assert!(super::leads("a/b.ts:1:import t from './types.ts'", &[]).is_empty());
        assert!(super::leads("a/b.md:1:see crmServer.ts", &[]).is_empty());
    }

    #[test]
    fn non_hit_lines_short_words_and_late_calls_are_not_leads() {
        assert!(leads("37:export async function clickOrExplain(page)", &[]).is_empty());
        assert!(leads("note: --path a/b.rs is not under c", &[]).is_empty());
        assert!(leads("a/b.rs:1:let userId = 1; see 1.5 and v2.0/x", &[]).is_empty());
        let leads = leads("a/b.rs:3:call(fetchDossier)", &[]);
        assert!(!followed(&[shell(1e6, "rg fetchDossier")], 0.0, "", &leads, 8, 9e5));
        assert!(!followed(&[shell(1.0, "ls")], 0.0, "", &leads, 8, 9e5));
    }
}
