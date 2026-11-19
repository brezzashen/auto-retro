//! Search quality of one window (founder, 2026-11-02): whether local-search answers right, fast,
//! and with results the agent uses. Every rate is driven toward TARGET (99.999 %); a 100-call
//! window lists every failure so the retro classifies each one. The policy score (`score.rs`) only
//! says which tool ran.
//!
//! - `engine` (`engine.rs`): queries.jsonl rows; `speed_rate` is the share within budget.
//! - `calls`: every shell command running `local-search search|query` (benchmark and replay runs
//!   with LOCAL_SEARCH_NO_LOG excluded) in the transcripts of `steps.rs`, paired with its result.
//!   Hit: the output names at least one file or report page (`returned.rs`). Fallback: no hit, then rg, grep or find within
//!   FALLBACK_CALLS tool calls. False NONE: that sweep printed something, so the name existed.
//!   `answer_rate` = 1 - false NONE / calls (a NONE the agent accepted or a sweep confirmed is an
//!   answer). Adopted: a returned file is named by a tool call of the next ADOPT_TURNS turns (path
//!   tail) or by assistant text before them (file name), within ADOPT_MS, or its printed lines
//!   answered and no search for the same names followed (`answered.rs`, founder 2026-11-02), or a
//!   file or identifier its hit lines printed is named by those calls (`followed.rs`). Wall: result time
//!   minus call time, only for commands whose single program besides a leading `cd` and output
//!   filters is local-search.
use super::adopt::{adopts_with, ADOPT_MS, ADOPT_TURNS};
use super::{answered, edit_check, followed, none, pattern, returned, transcripts, unindexed, values};
pub(super) use super::returned::returned_files;
use super::steps::{self, Kind, Step};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// Founder goal (2026-11-02): every rate approaches 99.999 %.
pub(super) const TARGET: f64 = 0.99999;
/// Failure locators listed per kind; a 100-call window lists every failure.
pub(super) const FAILURES: usize = 100;
const FALLBACK_CALLS: usize = 3;
/// Declaration keywords a query carries beside the name (`search "function DesktopEventChip"`); a
/// later `grep "function Row"` shares only the keyword, so it is no sweep for the same name
/// (search-retro 83012811238074165395, 1f4c3841:557).
const KEYWORDS: &[&str] = &["function", "struct", "class", "enum", "const", "static", "async", "export", "import", "interface", "impl", "trait", "type", "func", "protocol", "extension", "return", "self", "Self", "public", "private"];
/// Upper-case SQL keywords a pasted statement carries (`search "SELECT enabled, day_reminder FROM
/// crm.birthday_settings"`); a later search for another statement shares only `FROM`, so it is no
/// re-search of the same names (search-retro 86517430927628776430, 60390666…jsonl:330 then :337).
/// Only the upper-case spelling: a lower-case `update` or `order` may be a real name.
const SQL_KEYWORDS: &[&str] = &["SELECT", "FROM", "WHERE", "JOIN", "LEFT", "INNER", "OUTER", "LIMIT", "ORDER", "GROUP", "HAVING", "INSERT", "INTO", "UPDATE", "DELETE", "VALUES", "UNION", "CASE", "WHEN", "THEN", "ELSE", "NULL", "LIKE", "WITH", "DISTINCT", "RETURNING", "FILTER", "COUNT", "EXISTS", "LATERAL", "CROSS", "COALESCE"];

/// Rates without the labelled synthetic sessions (`synthetic.rs`); `with_synthetic` repeats the goal with them.
pub(super) fn measure(home: &Path, from_ms: f64, to_ms: f64) -> Value {
    let synthetic = super::synthetic::sessions(home, from_ms, to_ms);
    let (engine, calls) = (super::engine::measure(home, from_ms, to_ms, false), calls(home, from_ms, to_ms, &synthetic));
    let (engine_all, calls_all) = if synthetic.is_empty() { (engine.clone(), calls.clone()) } else { (super::engine::measure(home, from_ms, to_ms, true), self::calls(home, from_ms, to_ms, &BTreeMap::new())) };
    let with_synthetic = json!({"goal": goal(&engine_all, &calls_all), "calls": calls_all["total"], "engine_rows": engine_all["search"]["calls"].as_u64().unwrap_or(0) + engine_all["query"]["calls"].as_u64().unwrap_or(0)});
    json!({"window": {"from_ms": from_ms, "to_ms": to_ms}, "goal": goal(&engine, &calls), "engine": engine, "calls": calls,
        "synthetic": {"sessions": synthetic, "env": super::synthetic::ENV}, "with_synthetic": with_synthetic,
        "definitions": {"hit": "output names at least one file (since 2026-11-16 a call whose excluding `| grep -v` filter left no file and no NONE while its engine row logged hits is unobserved, `excluded.rs`, search-retro 82856125487639539959); since 2026-11-03 a report page URL, and a bare or `uniq -c` counted path when the search is piped into awk, cut, sort or uniq (`returned.rs`); a call whose output `$(…)` captures, or (since 2026-11-03) that names no file after a `| grep`/`| rg` line filter, is unobserved, outside every rate; since 2026-11-07 a call counted by `| wc` is unobserved too, a fold marker `<path>: … same line(s) in N more files: <path>:<line>, …` names its file and every copy, and a Codex JavaScript tool result (`{\"chunk_id\"…,\"output\":…}` or a settled promise) is read as its command output (`steps.rs` unwrap_settled; search-retro 813242367899389877458); since 2026-11-13 a call whose `| grep` line filter dropped the looser-retry label, with an engine row logged `looser`, is read with the label, so its files are suggestions (`looser.rs`, search-retro 88110673498121705918)",
            "false_none": format!("no hit (since 2026-11-16 lines a local-search NONE block of the sweep step printed are no match, `none_block.rs`, search-retro 82856125487639539959), then within {FALLBACK_CALLS} tool calls an rg/grep/find not fed by a pipe, naming a query name (4+ characters, not a declaration keyword) in its own command (heredoc bodies excluded; since 2026-11-03 in its pattern, not only a file argument, `pattern.rs`), printed something; since 2026-11-03 a sweep whose rg/grep ask only for line numbers must print a numbered line, and since 2026-11-04 some printed line must hold a query name or be a count (`none.rs` confirms_names; since 2026-11-17 a line below the step's echo marker ahead of the sweep, search-retro 812798351439866126096); since 2026-11-04 an rg/grep reading only files its own command edited in place (`sed -i`, `perl -i`) checks that edit, not the NONE (`edit_check.rs`); since 2026-11-05 lines the step printed with its own `echo` (`done`, `exit=$?`), and lines printed before the echo marker it wrote ahead of the sweep, are not matches (`echoed.rs`); since 2026-11-05 a sweep whose every path lies in a tree no corpus indexes (`~/automation/config`, `~/automation/host`, `~/.local/bin`, `/Applications`, system trees; since 2026-11-08 cargo and rustup dependency sources, also through a `$var` the command assigns) is the rg the NONE asked for, not a re-search (`unindexed.rs`); since 2026-11-10 a sweep whose every path lies outside each `--path` scope of the call (neither inside nor above it) is no re-search either (`scoped.rs`); since 2026-11-07 a sweep reading only agent transcripts (`.jsonl` under the history roots) after a NONE whose `--corpus` left out history cannot refute it (`transcripts.rs`), and a `(words spread over the chunk)` line is a near suggestion, a hit when the agent opens it (`returned.rs`)"),
            "answer_rate": "1 - false_none / calls",
            "not_run": "since 2026-11-04 a search written after `||` whose query the engine never logged between call and result did not run, outside every rate; since 2026-11-05 also a search in a line a PreToolUse hook refused (`ran.rs`); since 2026-11-08 an engine row logged after the next turn's tool call started is that call's, not the refused or skipped one's, and a sweep a PreToolUse hook refused confirms no NONE (search-retro 814194729051765393311)",
            "speed_rate": format!("engine rows within budget (search {} ms, query {} ms)", super::engine::SEARCH_BUDGET_MS, super::engine::QUERY_BUDGET_MS),
            "adopted": format!("a returned file named by a tool call of the next {ADOPT_TURNS} assistant turns (since 2026-11-05; parallel calls of one message are one turn, `adopt.rs`; before, the next 8 tool calls) or by assistant text among them, within 15 min; or, since 2026-11-02, printed lines no search for the same names followed in that window (adopted_by_lines); or, since 2026-11-03, a file or identifier its hit lines printed is named by those calls (adopted_by_leads; since 2026-11-03 also a printed run of 3+ Han characters that a following search names; since 2026-11-08 also the numbered body lines a `query` prints under a hit). Near lines `(words spread over the chunk)` are not hits; since 2026-11-04 every line under a looser retry's label (`no chunk holds the exact pattern`) is a near line, a hit only when the agent then names its file (a NONE with suggestions, `returned.rs`; since 2026-11-10 that includes an `-- outside --path` group under the label, `retry_scope_tests.rs`), and a `query` is re-searched only by two of its words, an identifier-shaped one, or a search for nothing but its words (`answered.rs`); a run of 3+ Han characters is a name; since 2026-11-04 an upper-case SQL keyword (`SELECT`, `FROM`, `WHERE`, …) is no name (`quality.rs` SQL_KEYWORDS); since 2026-11-05 a returned file is also named by its module path (`crate::dev::deliver_wait_facts::`, `from '../people'`, `module_ref.rs`); since 2026-11-05 two shared pieces inside a sibling identifier, where each side has a piece the other lacks (`MarsDebugEventsView` after `MarsDebugMemoryTab`), are no re-search (`sibling.rs`), and since 2026-11-08 a kebab-case program whose words open a snake name (`cpu-build-gate` for `CPU_BUILD_GATE_PRIORITY`) counts as one piece standing for its words (`owner.rs`, search-retro 813280046605134040490); since 2026-11-05 `--lines` near lines (`-- no line holds every word as written`) with no exact line before them are near lines too (`returned.rs` NEAR_LINES); since 2026-11-05 an rg/grep re-search whose output holds no line with a query name is no re-search (`answered.rs` silent), and a printed identifier-shaped value (backticked or quoted) that the next tool call, a non-search shell command, uses verbatim adopts the hit (adopted_by_values, `values.rs`); since 2026-11-05 a query that is one two-character Han word (a person's name, `何棠`) is a name, so a hit that confirmed absence and drew no re-search is answered (founder decision on search-retro 87276522378180065818, `quality.rs` names_of); since 2026-11-06 a one-word query that is a three-character upper-case acronym (`ICP`) is that name too, re-searched only as a whole word (`pattern.rs` acronym); a session-status or agent-memory search re-searches only by its arguments, not its program name (`pattern.rs` arguments); and an rg/grep/find over a tree no corpus indexes (Mars_studio, `~/automation/config`, after a `cd` into one) is no re-search (`unindexed.rs`, search-retro 814125901426638775162); since 2026-11-06 a host name in a query (`apps.apple.com`) is one name, not its labels, and an escaped `\\.` in a later pattern reads as a dot (`domain.rs`, search-retro 815399477109309239632); since 2026-11-07 lines under the `--path` label `as a whole word first` with no whole-word line above it are look-alike near lines, a hit only when the agent opens one (`returned.rs`), a non-search shell command naming a returned file by its kebab or snake stem (`bun test seo-locales`) adopts it (`adopt.rs` names_stem), and an rg/grep fed by a pipe is a line filter, not a search, for adopted_by_values (`values.rs`; search-retro 85383374938275326400); since 2026-11-07 a non-search tool call citing a returned file as `name.ext:<line>` without its directory adopts it (`adopt.rs` cites, search-retro 813242367899389877458); since 2026-11-07 a hit line inside a package (`crates/<name>/`, `packages/`, `skills/`) the search command did not name is a lead a later call inside that package follows (`package.rs`), and a loop-variable query (`\"$v\"` of `for v in …`) asks for the loop words, a dotted version (`1.64.1`) being one name (`loop_var.rs`; search-retro 816857502053349117708); since 2026-11-07 a non-search shell command naming a returned file's directory and stem as two consecutive words, kebab or snake (`mars-skill-tools conversion-analytics query` runs `conversion_analytics/query.rs`), adopts it (`subcommand.rs`, search-retro 89200397237788824863); since 2026-11-09 a sibling import with a distinctive stem (`from './crmServer.ts'`) is a file lead by its bare name (`followed.rs` file_lead, search-retro 85874229037822047064), and a one-word query of any three ASCII characters (`278`, `/man/`) is that name like an acronym (founder decision 1A on search-retro 85874229037822047064, `pattern.rs` short_word); since 2026-11-10 a non-search command naming a returned file by its bare name, a kebab or snake stem of 6+ characters with its extension (`sed -n 105,130p watch_trends_surge.rs` with the shell already in its directory), adopts it (`bare.rs`, search-retro 813279914114122985562); since 2026-11-10 a source of another corpus under a NONE's label `the <asked> corpus holds none of it, <other> does; rerun with --corpus <other>` is a suggestion, a hit only when the agent opens it (`elsewhere.rs`), and a search the same assistant message ran beside the call is no re-search of its result (`answered.rs` accepted_after; search-retro 813204256598053172535); since 2026-11-10 the adoption turns are counted from the message after the search, so the calls its own message ran beside it fill no turn (founder decision 1A on search-retro 813204256598053172535, `adopt.rs` window); since 2026-11-10 a glob path whose directory ends with the file's two directories (`MarsCRM/Shell/*.swift`), or with its parent and a six-character literal stem (`Shell/AppRouter*.swift`), names the file (`glob.rs`), and a backslash escapes the next character outside quotes and inside double quotes when a command is split, a backslash-newline continuing the line (`split.rs`; search-retro 816476999821716281017); since 2026-11-10 a call naming a package root without its trailing slash (`--path …/crates/limit-resume`) follows that package lead (`package.rs` named, search-retro 88631398572330521580); since 2026-11-12 a plain one-word name inside a longer identifier of a later search (`UsageDotCanvas` for `canvas`, `save_cargo_target` for `target`; not its plural, not an identifier joining two query names) is no re-search, and an rg/grep reading only files the hit printed (a path by its last two components, a bare file by its name or a 6+ character stem) reads the answer (founder decision A on search-retro 86114628083256871521, `inner.rs`); since 2026-11-14 a Skill tool call loading the skill of a returned `SKILL.md` (`{{\"skill\":\"swipe-arbitration\"}}`) adopts it (`skill_load.rs`, search-retro 813866010669096751911); since 2026-11-15 that glob rule reads the full printed path, so its two-directory branch applies to a two-component key (`stripe/gateway.rs` read by `…/src/stripe/*.rs`, `adopt.rs` adopts_with, search-retro 88394746224259496385); since 2026-11-17 a `./` or `../` path that resolves from the command's `cd` directory to the returned file names it (`cd …/lib/api && grep … ../composerAttachments.ts`, `adopt.rs` relative_names, search-retro 812798351439866126096); since 2026-11-15 a later rg/grep or local-search whose every path or `--path` lies outside each `--path` scope of the hit is no re-search, an unscoped or wider one still is (`scoped.rs` elsewhere_from, search-retro 82104642038523697224); since 2026-11-16 a `NONE:` line right above the literal-absent note (`literal: … but no line of their files has …`) stays part of the search's own output, so an `-- outside --path` group under them is a suggestion like any scope NONE's (`returned.rs` own_output), and a NONE for a pattern with a punctuation-only part (`%% series`) is refuted only by a sweep line holding that text as written (`phrase.rs` literal; search-retro 815124133498867247874)"),
            "synthetic": "since 2026-11-05 sessions with an event labelled by SEARCH_WATCH_SYNTHETIC (benchmarks, hook probes, guard replays) and labelled engine rows are left out of every rate; with_synthetic keeps them (founder decision on search-retro 810495355461229102702, `synthetic.rs`)",
            "wall": "single local-search command, result time minus call time"}})
}

/// The three rates against TARGET, their nines (-log10 of the miss share) and failure counts.
fn goal(engine: &Value, calls: &Value) -> Value {
    let nines = |r: Option<f64>| r.map(|r| if r >= 1.0 { json!("all") } else { json!((-(1.0 - r).log10() * 100.0).round() / 100.0) });
    let (answer, speed, adoption) = (calls["answer_rate"].as_f64(), engine["speed_rate"].as_f64(), calls["adoption_rate"].as_f64());
    let unadopted = calls["hits"].as_u64().unwrap_or(0).saturating_sub(calls["adopted"].as_u64().unwrap_or(0));
    let met = [answer, speed, adoption].iter().all(|r| r.is_some_and(|r| r >= TARGET));
    json!({"target": TARGET, "answer_rate": answer, "speed_rate": speed, "adoption_rate": adoption,
        "nines": {"answer": nines(answer), "speed": nines(speed), "adoption": nines(adoption), "target": 5},
        "met": met,
        "failures": {"false_none": calls["false_none"], "slow": engine["slow_calls"], "unadopted": unadopted}})
}

/// `cpu-watchdog search-quality <incident-id> | <from_ms> <to_ms>`: prints `measure` as JSON.
pub(crate) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let (from_ms, to_ms) = match args.as_slice() {
        [id] => {
            let path = crate::monitor::root(home).join("incidents").join(format!("{id}.json"));
            let incident: Value = serde_json::from_str(&std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?)?;
            (incident["window"]["from_ms"].as_f64().unwrap_or(0.0), incident["window"]["to_ms"].as_f64().unwrap_or(0.0))
        }
        [from, to] => (from.parse()?, to.parse()?),
        _ => bail!("usage: cpu-watchdog search-quality <incident-id> | <from_ms> <to_ms>"),
    };
    println!("{}", serde_json::to_string_pretty(&measure(home, from_ms, to_ms))?);
    Ok(())
}

#[derive(Default)]
struct Tally {
    total: usize, unobserved: usize, hits: usize, adopted: usize, answered: usize, led: usize, valued: usize, misses: usize, fallback: usize, false_none: usize, not_run: usize,
    wall: Vec<f64>, captured: Vec<String>, unadopted: Vec<String>, fell_back: Vec<String>, wrong_none: Vec<String>, skipped: Vec<String>,
}

fn calls(home: &Path, from_ms: f64, to_ms: f64, synthetic: &BTreeMap<String, String>) -> Value {
    let (mut seen, mut t, engine_rows) = (HashSet::new(), Tally::default(), super::ran::rows(home, from_ms, to_ms));
    let (looser_rows, hit_rows) = (super::looser::rows(home, from_ms, to_ms), super::excluded::hit_rows(home, from_ms, to_ms));
    for path in steps::transcripts(home, from_ms).into_iter().filter(|p| !super::synthetic::in_sessions(p, synthetic)) {
        let items = steps::read(&path, from_ms, to_ms, ADOPT_MS);
        let mut cwds = None;
        for (i, call) in items.iter().enumerate() {
            if call.kind != Kind::Shell || call.at_ms > to_ms || !seen.insert(call.id.clone()) { continue; }
            // A heredoc body that mentions local-search (an edit script) is no call (281469a7…jsonl:649).
            let text = none::strip_heredocs(&call.text);
            let Some(names) = invocation(&text) else { continue };
            let Some(answer) = result_of(&items, &call.id) else { continue };
            // A `||` fallback or hook-refused search the engine never logged did not run (`ran.rs`).
            let next_ms = items[i + 1..].iter().find(|s| s.turn != call.turn && matches!(s.kind, Kind::Shell | Kind::Tool)).map(|s| s.at_ms);
            if super::ran::skipped(&text, &answer.text, &engine_rows, call.at_ms, answer.at_ms, next_ms) { t.not_run += 1; push(&mut t.skipped, format!("{}:{}", path.display(), call.line)); continue; }
            let after = &items[i + 1..];
            // Stdout sent to a file: later reads of that file are its output too (`redirected.rs`).
            let output = super::redirected::output(&text, &answer.text, &items, after, call.at_ms, &call.turn);
            // A looser retry whose label the call's own `| grep` dropped (`looser.rs`).
            let output = super::looser::relabel(&text, &output, &looser_rows, call.at_ms, answer.at_ms).map_or(output, std::borrow::Cow::Owned);
            let full = returned::full_paths(&output);
            let files = returned::returned_or_used(&call.text, &output, |near| adopts_with(after, call.at_ms, &call.turn, near, &full));
            // Captured by `$(…)`, counted by `wc`, or no file left after the agent's own line filter (`excluded.rs`): unobserved.
            if returned::captured(&text) || (files.is_empty() && (returned::counted(&text) || returned::filtered(&text, &answer.text) || super::excluded::emptied(&text, &answer.text, &hit_rows, call.at_ms, answer.at_ms))) {
                t.unobserved += 1;
                push(&mut t.captured, format!("{}:{}", path.display(), call.line));
                continue;
            }
            t.total += 1;
            let locator = format!("{}:{}", path.display(), call.line);
            if single_command(&call.text) { t.wall.push(answer.at_ms - call.at_ms); }
            if files.is_empty() {
                t.misses += 1;
                let window: Vec<&Step> = after.iter().filter(|s| matches!(s.kind, Kind::Shell | Kind::Tool)).take(FALLBACK_CALLS).collect();
                let cwds = cwds.get_or_insert_with(|| steps::cwds(&path));
                let at = |s: &Step| cwds.get(&s.line).map(String::as_str);
                let sweep = window.iter().enumerate().find(|(j, s)| s.kind == Kind::Shell && sweeps_for_at(&s.text, &names, &home.to_string_lossy(), &text, (at(call), at(s))) && !none::reads_located(&items, &window[..*j], &s.text)).map(|(_, s)| *s);
                if let Some(sweep) = sweep {
                    t.fallback += 1;
                    push(&mut t.fell_back, locator.clone());
                    // A sweep a PreToolUse hook refused printed only the refusal (884cd1bf…jsonl:161).
                    if result_of(&items, &sweep.id).is_some_and(|r| !super::ran::hook_blocked(&r.text) && none::confirms_names(&r.text, &sweep.text, &names) && super::phrase::refuted(&r.text, &sweep.text, &text) != Some(false)) {
                        t.false_none += 1;
                        push(&mut t.wrong_none, locator);
                    }
                }
                continue;
            }
            t.hits += 1;
            if adopts_with(after, call.at_ms, &call.turn, &files, &full) {
                t.adopted += 1;
            } else if followed::followed(after, call.at_ms, &call.turn, &[followed::leads(&answer.text, &names), super::package::leads(&answer.text, &text)].concat(), ADOPT_TURNS, ADOPT_MS) {
                (t.adopted, t.led) = (t.adopted + 1, t.led + 1);
            } else if !answered::truncated(&answer.text) && {
                // A later search only outside the call's `--path` scopes is no re-search (`scoped.rs`).
                let cwds = cwds.get_or_insert_with(|| steps::cwds(&path));
                let home = home.to_string_lossy();
                let elsewhere = |s: &Step, segment: &str| super::scoped::elsewhere_from(&text, &s.text, segment, &home, cwds.get(&call.line).map(String::as_str), cwds.get(&s.line).map(String::as_str));
                answered::accepted_scoped(&items, after, call.at_ms, &call.turn, &names, answered::sentence(&text), ADOPT_TURNS, ADOPT_MS, &answer.text, &elsewhere)
            } {
                (t.adopted, t.answered) = (t.adopted + 1, t.answered + 1);
            } else if values::used(after, &answer.text) {
                (t.adopted, t.valued) = (t.adopted + 1, t.valued + 1);
            } else {
                push(&mut t.unadopted, locator);
            }
        }
    }
    let answer_rate = if t.total == 0 { Value::Null } else { ratio(t.total - t.false_none, t.total) };
    json!({"total": t.total, "unobserved": t.unobserved, "unobserved_calls": t.captured, "not_run": t.not_run, "not_run_calls": t.skipped, "hits": t.hits, "hit_rate": ratio(t.hits, t.total), "false_none": t.false_none, "answer_rate": answer_rate,
        "no_hit": t.misses, "adopted": t.adopted, "adopted_by_lines": t.answered, "adopted_by_leads": t.led, "adopted_by_values": t.valued, "adoption_rate": ratio(t.adopted, t.hits),
        "fallback_after_no_hit": t.fallback, "fallback_rate": ratio(t.fallback, t.misses),
        "wall_ms": {"n": t.wall.len(), "p50": percentile(&mut t.wall, 0.5), "p95": percentile(&mut t.wall, 0.95)},
        "failures": {"false_none": t.wrong_none, "unadopted": t.unadopted, "fallback": t.fell_back}})
}

pub(super) fn result_of<'a>(items: &'a [Step], id: &str) -> Option<&'a Step> {
    items.iter().find(|s| s.kind == Kind::Result && s.id == id)
}

fn push(list: &mut Vec<String>, locator: String) {
    if list.len() < FAILURES { list.push(locator); }
}

/// A search an agent ran to find something: a `local-search` program word whose subcommand (after
/// any `--db <path>`) is search or query. Returns the names in its first query (identifiers of
/// four or more characters), which a fallback sweep must share to count as looking for the same
/// thing. Text that only mentions local-search (an edit, a heredoc, a commit message) has no such
/// word pair; benchmark and replay runs set LOCAL_SEARCH_NO_LOG.
pub(super) fn invocation(command: &str) -> Option<Vec<String>> {
    query_of(command).map(|q| names_of(&q))
}

/// The first query text of a local-search search|query command.
pub(super) fn query_of(command: &str) -> Option<String> {
    if command.contains("LOCAL_SEARCH_NO_LOG") { return None; }
    segments(command).into_iter().find_map(|segment| {
        let words: Vec<&str> = segment.split_whitespace().skip_while(|w| w.contains('=') && !w.starts_with('-')).collect();
        if words.first()?.rsplit('/').next() != Some("local-search") { return None; }
        let mut sub = 1;
        while words.get(sub).is_some_and(|w| *w == "--db") { sub += 2; }
        if !matches!(words.get(sub), Some(&("search" | "query"))) { return None; }
        let rest = words[sub + 1..].join(" ");
        let query = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => steps::quoted(&rest[1..], q),
            _ => rest.split_whitespace().next().unwrap_or_default().to_string(),
        };
        Some(super::loop_var::resolve(command, &query))
    })
}

/// Words of four or more characters (since search-retro 816506381674351735418 also a run of three or
/// more Han characters: `子任务` at a789a969…jsonl:1956 had no name, so its answer could never count), a kebab-case slug whole: `personal-crm` split to `personal` matched an
/// unrelated `query "personal memory goals …"` as a re-search (search-retro 87040299049855422752, f36acff7…jsonl:85).
/// A query that is one Han word of two characters (a person's name, `何棠`) is that name: founder
/// decision on search-retro 87276522378180065818 ("确认不存在计入"). d02e115b…jsonl:517 searched `何棠`,
/// saw it only in onboarding fixtures, and switched to another demo person without a re-search;
/// with no name the hit could never count as answered.
/// In a query made only of one-word `a|b` alternatives, a three-character alternative is a name too:
/// founder decision 第1项选A on search-retro 812685146552855153702 (2026-11-11). 40c0a03c…jsonl:29
/// searched `ag1|agy` inside script-guard, read the answer in its printed lines and searched no
/// more, yet had no name, so the hit could never count as answered.
fn names_of(query: &str) -> Vec<String> {
    // A host name is one name (`domain.rs`); its labels are not.
    let (hosts, rest) = super::domain::hosts(query);
    if !hosts.is_empty() {
        return hosts.into_iter().chain(names_of(&rest)).collect();
    }
    // A member chain names its type, not its plain member words (`chain.rs`).
    let (chains, rest) = super::chain::split(query);
    if !chains.is_empty() {
        return chains.into_iter().chain(names_of(&rest)).collect();
    }
    let words: Vec<&str> = query.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-').map(|n| n.trim_matches('-')).filter(|n| !n.is_empty()).collect();
    if let [only] = words.as_slice() {
        if answered::han_short_name(only) || pattern::short_word(only) {
            return vec![only.to_string()];
        }
    }
    // Only an alternative that is itself a three-character word counts: `api` inside
    // `/api/threads\?limit` is not one (e265c2a6…jsonl:3883).
    let one_word = query.contains('|') && query.split('|').all(|a| !a.trim().is_empty() && !a.trim().contains(char::is_whitespace));
    let short_alternatives: Vec<&str> = if one_word { query.split('|').map(str::trim).filter(|a| pattern::short_word(a)).collect() } else { Vec::new() };
    words.into_iter().filter(|n| (n.chars().count() >= 4 || answered::han_name(n) || short_alternatives.contains(n)) && !KEYWORDS.contains(n) && !SQL_KEYWORDS.contains(n)).map(str::to_string).collect()
}

/// Simple commands of a shell line (`split.rs`), escapes kept.
fn segments(command: &str) -> Vec<String> {
    commands(command).into_iter().map(|(_, segment)| segment).collect()
}

#[cfg(test)]
pub(super) fn is_local_search(command: &str) -> bool {
    invocation(command).is_some()
}

/// True when local-search is the only program, apart from a leading `cd` and pipes into filters.
pub(super) fn single_command(command: &str) -> bool {
    let body = match command.trim_start().strip_prefix("cd ") {
        Some(rest) => rest.split_once("&&").or_else(|| rest.split_once(';')).map_or("", |(_, after)| after),
        None => command,
    };
    body.matches("local-search").count() == 1 && !body.contains("&&") && !body.contains(';')
}

/// True when one simple command of `command` is an rg/grep/find sweep for one of `names`: the
/// program at command position, not fed by a single `|` (a filter reading stdin, such as
/// `local-search … | grep -i report`), with the name in that command's own text. Search-retro
/// 83012811238074165395: 4 of 8 false NONEs were a pipe filter, a `grep -c` of one memory file whose
/// heredoc elsewhere in the step named the word, or a grep for a different name. The name must be
/// in the sweep's pattern, not only in a file it reads (`pattern.rs`, search-retro 82450840685001354157).
/// Since 2026-11-05 a sweep reading only unindexed trees is the rg the NONE asked for (`unindexed.rs`);
/// since 2026-11-07 one reading only transcripts cannot refute a NONE that skipped history (`transcripts.rs`).
/// `cwds`: the shell directories the transcript recorded for the search and the sweep (`scoped.rs`).
fn sweeps_for_at(command: &str, names: &[String], home: &str, search: &str, cwds: (Option<&str>, Option<&str>)) -> bool {
    commands(&none::strip_heredocs(command)).iter().any(|(piped, segment)| {
        let words: Vec<&str> = segment.split_whitespace().filter(|w| !w.contains('=')).collect();
        let sweep = !piped && !edit_check::verifies(command, segment) && words.first().is_some_and(|p| matches!(p.rsplit('/').next().unwrap_or(p), "rg" | "grep" | "egrep" | "find" | "ugrep")) && !unindexed::only_unindexed_in(command, segment, home)
            && !(transcripts::none_skips_history(search) && transcripts::only_transcripts(segment, home))
            && !super::scoped::outside_from(search, &none::strip_heredocs(command), segment, home, cwds.0, cwds.1);
        sweep && {
            let looked_for = super::domain::unescape(&pattern::of(segment).unwrap_or_else(|| segment.clone()));
            names.iter().any(|n| looked_for.contains(n.as_str()))
        }
    })
}

/// Simple commands, each with whether a single `|` fed it (`||` does not); `split.rs`.
pub(super) fn commands(command: &str) -> Vec<(bool, String)> {
    super::split::commands(command)
}

/// Six decimals, enough to tell 99.999 % from 99.99 %.
pub(super) fn ratio(n: usize, d: usize) -> Value {
    if d == 0 { Value::Null } else { json!((n as f64 / d as f64 * 1e6).round() / 1e6) }
}

pub(super) fn percentile(values: &mut [f64], p: f64) -> Value {
    if values.is_empty() { return Value::Null; }
    values.sort_by(|a, b| a.total_cmp(b));
    json!(values[((values.len() - 1) as f64 * p).round() as usize].round())
}

#[cfg(test)]
#[path = "quality_tests.rs"]
mod tests;
