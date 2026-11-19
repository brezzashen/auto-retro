//! What a local-search call returned, read from its transcript output. A hit is a line whose first
//! token names a file (`path:line…`, `path:start-end`) or a report page (`https://…/<slug>/…`).
//! When the call's own output went through a reshaping pipe (`awk`, `cut`, `sort`, `uniq`), a bare
//! path, optionally after a `uniq -c` count, is a hit too. Search-retro 8971788586442395373: 4 of 7
//! false NONEs were hits printed in these shapes (1a58e2e9…jsonl:146 report URLs; 77fbd9bf…jsonl:261,
//! 275, 2654 `awk -F: '{print $1}' | sort | uniq -c`).
use super::quality::commands;

/// Programs that rewrite a search's `path:line:text` lines into bare paths or counted paths.
const RESHAPERS: &[&str] = &["awk", "cut", "sort", "uniq"];

/// Files a search printed, as their last two path components; a report page as its slug.
pub(crate) fn returned_files(output: &str) -> Vec<String> {
    keys(output, false)
}

/// `returned_files`, plus bare and counted paths when `command` pipes the search into a reshaper
/// and the output carries no NONE line (the bare paths would then come from another program).
#[cfg(test)]
pub(crate) fn returned_by(command: &str, output: &str) -> Vec<String> {
    returned_or_used(command, output, |_| false)
}

/// True when the local-search call's stdout is captured by `$(…)` or backticks, so the
/// transcript never shows what it returned (a5652a60…jsonl:889 fed a `for f in $(…)` loop).
pub(crate) fn captured(command: &str) -> bool {
    let Some(at) = command.find("local-search") else { return false };
    let before = &command[..at];
    let mut open: Vec<bool> = Vec::new();
    let chars: Vec<char> = before.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '(' => open.push(i > 0 && chars[i - 1] == '$'),
            ')' => { open.pop(); }
            _ => {}
        }
    }
    open.contains(&true) || before.matches('`').count() % 2 == 1
}

/// True when the local-search call's stdout goes through a selecting line filter (`| grep -E
/// '^\S+:[0-9]'`) and no NONE line reached the transcript: the filter, not local-search, decided
/// that nothing printed. Search-retro 82450840685001354157, c82d6abb…jsonl:697 filtered a `query`
/// whose hit lines start with two spaces and printed nothing; the same query unfiltered at :702
/// returned 20 hits. Such a call is unobserved, as a captured one is. An excluding filter
/// (`grep -v "^note"`, 8ce2f885…jsonl:1089) passes NONE and near lines, so the agent saw the answer.
pub(crate) fn filtered(command: &str, output: &str) -> bool {
    line_filtered(command) && !output.lines().any(|l| l.trim_start().starts_with("NONE"))
}

/// True when the call's stdout goes through a selecting (not inverted) rg/grep line filter.
pub(crate) fn line_filtered(command: &str) -> bool {
    piped_into(command, |program: &str, segment: &str| FILTERS.contains(&program) && !inverted(segment))
}

/// True when the call's stdout goes through an excluding (inverted) rg/grep line filter (`excluded.rs`).
pub(crate) fn excluded(command: &str) -> bool {
    piped_into(command, |program: &str, segment: &str| FILTERS.contains(&program) && inverted(segment))
}

/// Line filters that pass only some of a search's lines.
const FILTERS: &[&str] = &["grep", "egrep", "rg", "ugrep"];

/// True when an rg/grep segment inverts its match (`-v`, `-iv`, `--invert-match`).
fn inverted(segment: &str) -> bool {
    segment.split_whitespace().any(|w| w == "--invert-match" || (w.starts_with('-') && !w.starts_with("--") && w.contains('v')))
}

/// True when the call's stdout is piped into `wc`: only counts reach the transcript, so it shows
/// neither a hit list nor a NONE. Search-retro 813242367899389877458 (79d40a04…jsonl:364):
/// `search 'data-testid' … | wc -l` printed `60` and `153` and counted as a false NONE.
pub(crate) fn counted(command: &str) -> bool {
    piped_into(command, |program, _| program == "wc")
}

fn reshaped(command: &str) -> bool {
    piped_into(command, |program, _| RESHAPERS.contains(&program))
}

/// True when a local-search call's stdout is piped, directly or through earlier pipe stages, into
/// a segment `wanted` accepts (given its program name and text).
fn piped_into(command: &str, wanted: impl Fn(&str, &str) -> bool) -> bool {
    let mut after_search = false;
    for (piped, segment) in commands(command) {
        let program = segment.split_whitespace().find(|w| !w.contains('=')).unwrap_or_default();
        let program = program.rsplit('/').next().unwrap_or(program);
        if program == "local-search" {
            after_search = true;
        } else if program.chars().all(|c| c.is_ascii_digit()) {
            // `2>&1` splits at `&` into `… 2>` and `1`: still the search's own redirection.
        } else if after_search && piped && wanted(program, &segment) {
            return true;
        } else if !piped {
            after_search = false;
        }
    }
    false
}

/// The label `relax.rs` prints above a looser retry's lines: the exact pattern matched nowhere.
pub(super) const RELAXED: &str = "no chunk holds the exact pattern";

/// The label `--lines` prints above the lines of chunks that hold every word but on no one line
/// (local-search `output/lines.rs` NEAR_LABEL), the `--lines` form of `(words spread over the
/// chunk)`. With no exact line before it, the output is a NONE with suggestions. Search-retro
/// 87508011737703381146: b0bbe2c0…jsonl:111 `export function InformationGroup` printed only
/// `informationGroupTone` look-alikes under it, and 21775191…jsonl:443 `paper texture` only lines
/// 546-548 holding one word each; both counted as hits left unused.
const NEAR_LINES: &str = "-- no line holds every word as written";

/// The fold marker local-search `output/same_line.rs` prints under a block whose lines other files
/// repeat word for word: `<path>: … same line(s) in N more files: <path>:<line>, …`. It names the
/// block's file and every folded copy. Search-retro 813242367899389877458 (bfff573f…jsonl:245): the
/// agent's `grep -v node_modules` dropped `probe.js:32`, only the marker stayed, the next grep read
/// that probe.js, and the hit counted as a false NONE.
const SAME_LINES: &str = ": … same line(s) in ";

/// The marker local-search `output/same_line.rs` prints under a block whose translations (other
/// `.lproj` or locale copies of the file) folded into it: `<path>: … same keys in N more
/// translations: <path>:<line>,<line>, …`. It names every folded translation, so they are returned
/// files like SAME_LINES copies (search-retro 89259434869436960053).
const SAME_KEYS: &str = ": … same keys in ";

/// The `--path` label local-search prints above in-scope lines that hold the words only inside
/// longer names (`outside.rs` WHOLE: `-- outside --path: N more file(s) hold it, as a whole word
/// first …`). Its look-alikes are near lines; with no whole-word line above it, the call is a NONE
/// for the scope with suggestions. Search-retro 85383374938275326400: 8a06d9aa…jsonl:223
/// `"Submission {" --path tests` printed only `OpenSubmission {` and :230 `fn modified` only
/// `pub fn modified_expression`; both agents rightly moved on and the calls counted as unadopted hits.
fn look_alike_label(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("-- outside --path:") && line.contains("as a whole word first")
}

/// `returned_by`, plus the near lines that `used` accepts. Under exact lines a spread chunk the agent
/// then opened is the answer too: search-retro 818364334650240656506 (215c6438…jsonl:113) printed
/// `gcloud_auth` exactly only in cpu-watchdog's classifier, and the agent read the spread line
/// `mars-ops/src/canary/gcloud_reauth.rs:1-36` next (:127); it counted as an unadopted hit. When the
/// exact list is empty, only the opened near lines are returned. A
/// retry's lines are a NONE for the exact pattern with suggestions: a suggestion the agent then
/// opened is a hit it adopted, one it left is the NONE it took. Search-retro 816506381674351735418:
/// c5ce4338…jsonl:105, d3cc147d…jsonl:103 and b48eb9f7…jsonl:390 printed only near lines for names
/// that did not exist yet and each agent rightly moved on, while 9b1a1318…jsonl:478 opened the
/// `chunks naming LocalSearch` line `main.rs:51` and found its answer there.
pub(crate) fn returned_or_used(command: &str, output: &str, used: impl Fn(&[String]) -> bool) -> Vec<String> {
    let output = own_output(command, output);
    let reshaped = reshaped(command) && !output.lines().any(|l| l.trim_start().starts_with("NONE:"));
    let (mut exact, near) = split_keys(output, reshaped);
    exact.extend(near.into_iter().filter(|f| !exact.contains(f) && used(std::slice::from_ref(f))).collect::<Vec<_>>());
    exact
}

/// The first line `local-search search` prints only when its exact lookup found nothing: the
/// literal-absent note (`literal.rs`) or a looser retry's label (`relax.rs`).
fn no_exact_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with(RELAXED) || (line.starts_with("literal: ") && line.contains("but no line of their files has"))
}

/// The part of a one-search command's output the search printed. Its no-exact label is the first
/// line it prints, so `path:line` lines above that label came from another program of the same
/// command. Search-retro 85654515729827601357 (b21509cc…jsonl:5277): `grep -n reqwest
/// crates/*/Cargo.toml; local-search search "multipart::Form" …` printed two Cargo.toml lines, then
/// the literal-absent note; the Cargo.toml lines counted as an unadopted hit of a right NONE.
/// A `NONE:` line right above the note is the search's own first line and stays: search-retro
/// 815124133498867247874 (7b135b8c…jsonl:567) printed NONE, the literal-absent note, then an
/// `-- outside --path` group; cut below the NONE, that group read as exact hits of a right NONE.
fn own_output<'a>(command: &str, output: &'a str) -> &'a str {
    let searches = super::quality::commands(command).iter().filter(|(_, s)| s.split_whitespace().any(|w| w.rsplit('/').next() == Some("local-search"))).count();
    if searches != 1 {
        return output;
    }
    // Lines after an `echo` marker written after the search came from later commands (`echoed.rs`).
    let output = super::echoed::search_part(output, command);
    let (mut at, mut none_at) = (0, None::<usize>);
    for line in output.split_inclusive('\n') {
        if no_exact_line(line) {
            return &output[none_at.unwrap_or(at)..];
        }
        none_at = line.trim_start().starts_with("NONE:").then_some(at);
        at += line.len();
    }
    output
}

/// Every `path:line…` path the output printed in full (a line's first token), for rules that need
/// more than the last two components (`adopt.rs` adopts_with). Not a hit list: `returned_or_used`
/// decides which files are hits, and only those are looked up here by their key.
pub(crate) fn full_paths(output: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in output.lines().filter_map(|l| l.split_whitespace().next()) {
        let Some((path, rest)) = token.split_once(':') else { continue };
        if rest.starts_with(|c: char| c.is_ascii_digit()) && path.matches('/').count() >= 2 && !out.iter().any(|p| p == path) {
            out.push(path.to_string());
        }
    }
    out
}

fn keys(output: &str, bare: bool) -> Vec<String> {
    split_keys(output, bare).0
}

/// Files of exact hits, and files of the near lines under a looser retry's label.
fn split_keys(output: &str, bare: bool) -> (Vec<String>, Vec<String>) {
    let (mut out, mut near, mut relaxed): (Vec<String>, Vec<String>, bool) = (Vec::new(), Vec::new(), false);
    // Whether the retry's own first line is indented: history, docs and report hits print as
    // `  <locator>  [corpus/kind]` with their text under them, code hits flush left.
    let mut indented: Option<bool> = None;
    // Inside the NEAR_LINES section, where a chunk line without its number prints as `path: text`.
    let mut after_near = false;
    // Whether the search printed its NONE line, so an `outside --path` group is the scope's NONE.
    let mut scope_none = false;
    // The upstream named by the last upstream note (`-- origin/mastra holds … this checkout lacks`).
    let mut upstream: Option<String> = None;
    // Files the cut line names (`cut_hidden.rs`): returned hits, added after the list's own.
    let mut cut: Vec<String> = Vec::new();
    for line in output.lines().map(super::timing_prefix::strip) {
        // local-search counts an oversized `--lines` output in one label line (`output/oversize.rs`, 6cf9ab1): no hit, no state.
        if line.starts_with("-- ") && line.contains("over the ~30 KB a caller may show whole") { continue; }
        if let Some(places) = super::cut_hidden::places(line) { cut.extend(places.into_iter().filter_map(|p| file_key(p, false))); continue; }
        // The retry's lines run up to the other corpus's `  -- docs: … --` section, an indented hit
        // (a following `query`'s, b9b85bcd…jsonl:1867) or the first unindented line that names no file
        // (another program's output or an `echo ---`).
        if line.trim_start().starts_with(RELAXED) { (relaxed, indented) = (true, None); continue; }
        if line.trim_start().starts_with(NEAR_LINES) {
            // Like a retry, the first line under it says whether the near lines are indented: a
            // history `--lines` search prints `  <transcript>:<line>  [history/turn]` with text
            // under it. Search-retro 811147635907382047180 (rollout-…-01a1b96e…jsonl:258): the
            // fingerprint search '7pzyN|…' printed only such lines and counted as an unadopted hit.
            if out.is_empty() { (relaxed, indented) = (true, None); }
            after_near = true;
            continue;
        }
        // Like NEAR_LINES, look-alikes are suggestions only when no whole-word line came first;
        // under a whole-word hit they stay returned files (7b999c5e…jsonl:130 printed `0.648` whole,
        // then `max-width: 648px;` as a look-alike, and edited that look-alike's file).
        if look_alike_label(line) {
            if out.is_empty() { (relaxed, indented) = (true, Some(false)); }
            after_near = false;
            continue;
        }
        if line.trim_start().starts_with("NONE:") { scope_none = true; }
        // Files outside `--path` under the scope's NONE are suggestions, like a retry's lines.
        // Search-retro 85799442753078631747 (db32365e…jsonl:4169): `search "file-lock" --path
        // web/src` printed NONE, then three contract files outside the scope; the agent took the NONE
        // (Web draws lucide `FileLock`, :4184) and the call counted as an unadopted hit.
        // Under a retry's label too: search-retro 812899216858410875382 (`retry_scope_tests.rs`).
        if (scope_none || relaxed) && out.is_empty() && line.trim_start().starts_with("-- outside --path:") {
            (relaxed, indented, after_near) = (true, None, false);
            continue;
        }
        // A source of another corpus under the NONE is a suggestion too (`elsewhere.rs`).
        if out.is_empty() && (super::elsewhere::label(line) || super::elsewhere::switched(line)) {
            (relaxed, indented, after_near) = (true, None, false);
            continue;
        }
        // An upstream note's `<upstream>:<path>` lines are suggestions: a file the agent then reads
        // with `git show <upstream>:<path>` is a hit it adopted (`upstream_suggestion`).
        if let Some(key) = upstream_suggestion(line, &mut upstream) {
            if !near.contains(&key) { near.push(key); }
            continue;
        }
        if line.trim_start().starts_with("-- ") { after_near = false; }
        if relaxed && indented.is_none() && !line.trim().is_empty() { indented = Some(line.starts_with(char::is_whitespace)); }
        // Relaxed near lines are not answers: the exact pattern matched nowhere (search-retro
        // 84763104493312366879, 17a40a19…jsonl:1530 `record.example.com`).
        // Since 2026-11-05 a spread chunk is a suggestion, near like a retry's lines: a hit only when
        // the agent then opens it. Search-retro 8986486579033029752, 17ad7f85…jsonl:391: `install
        // script-guard` printed `…/references/maintenance.md:7  (words spread over the chunk)` and
        // the next grep read that file; it counted as a false NONE.
        if line.trim_end().ends_with("(words spread over the chunk)") {
            if let Some(key) = line.split_whitespace().next().and_then(|t| file_key(t, false)) {
                if !near.contains(&key) { near.push(key); }
            }
            continue;
        }
        if let Some((shown, rest)) = line.split_once(SAME_LINES).or_else(|| line.split_once(SAME_KEYS)) {
            let places = rest.split_once(": ").map_or("", |(_, p)| p);
            let shown = unnumbered_key(&format!("{}:", shown.trim()));
            let list = if relaxed { &mut near } else { &mut out };
            for key in shown.into_iter().chain(places.split(", ").filter_map(|p| file_key(p.trim(), false))) {
                if !list.contains(&key) { list.push(key); }
            }
            continue;
        }
        // A `query` block header `==> path:start-end` names a hit. Output filtered to the headers
        // (`| grep -v "^ "`) holds no other file line: search-retro 813866010669096751911,
        // afdb1701…jsonl:373 printed three headers, read the first file next and counted as no hit.
        if !relaxed {
            if let Some(key) = line.strip_prefix("==> ").and_then(|h| h.split_whitespace().next()).and_then(|t| file_key(t, false)) {
                if !out.contains(&key) { out.push(key); }
                continue;
            }
        }
        // A flush-left local-search label (`-- no chunk in --path holds: …; the lines below match the
        // other alternatives --`) inside suggestions is the search's own line, not another program's.
        if relaxed && line.starts_with("-- ") && line.trim_end().ends_with(" --") { continue; }
        let mut tokens = line.split_whitespace();
        let Some(mut token) = tokens.next() else { continue };
        if bare && token.chars().all(|c| c.is_ascii_digit()) {
            let Some(next) = tokens.next() else { continue };
            token = next;
        }
        let unnumbered = || (after_near || relaxed).then(|| unnumbered_key(token)).flatten();
        let key = report_slug(token).or_else(|| file_key(token, bare)).or_else(unnumbered);
        if relaxed {
            // `path: text` (a line of a chunk without its number) still belongs to the retry.
            let path_led = token.split_once(':').is_some_and(|(p, _)| p.contains('/') || p.contains('.'));
            // In an indented retry (a history `search`, a02094a4…jsonl:64 `singleorb`) its indented
            // locators are near lines too; in a flush-left one an indented locator is the next hit list.
            relaxed = if line.starts_with(char::is_whitespace) {
                !line.trim_start().starts_with("-- ") && (key.is_none() || indented == Some(true))
            } else {
                key.is_some() || path_led
            };
        }
        let Some(key) = key else { continue };
        let list = if relaxed { &mut near } else { &mut out };
        if !list.contains(&key) { list.push(key); }
    }
    for key in cut { if !out.contains(&key) { out.push(key); } }
    (out, near)
}

/// The file of a `  <upstream>:<path>` line under local-search's upstream note (`upstream.rs`), whose
/// label sets `upstream`; any other line ends the note. Search-retro 813254723049142099088:
/// 51d32cbc…jsonl:56 printed iOS invite lines and the note naming
/// `origin/mastra:Mars_CRM/web/src/screens/invite/Invite.css`; five turns later (:118) the agent read
/// that file with `git show origin/mastra:…/Invite.css`, and the call counted as an unadopted hit.
fn upstream_suggestion(line: &str, upstream: &mut Option<String>) -> Option<String> {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("-- ").filter(|r| r.contains(" this checkout lacks")) {
        *upstream = rest.split_once(" holds ").map(|(u, _)| u.to_string());
        return None;
    }
    let name = upstream.as_deref()?;
    match trimmed.strip_prefix(name).and_then(|r| r.strip_prefix(':')) {
        Some(path) if line.starts_with(char::is_whitespace) => file_key(&format!("{}:1", path.trim_end()), false),
        _ => {
            if !(line.starts_with(char::is_whitespace) && trimmed.starts_with("… ")) { *upstream = None; }
            None
        }
    }
}

/// A near line without its number, `dir/name.ext: text` → `dir/name.ext`. Search-retro
/// 85845825094965061070 (98a8572e…jsonl:113): `Browser context management is not supported` printed
/// `…/references/kit.md: …` under NEAR_LINES, the next turn grepped kit.md, and the hit counted unadopted.
fn unnumbered_key(token: &str) -> Option<String> {
    let path = token.strip_suffix(':')?;
    if !path.contains('/') || path.contains("://") { return None; }
    file_key(&format!("{path}:1"), false)
}

/// `https://host/<slug>/#case` → `<slug>`.
fn report_slug(token: &str) -> Option<String> {
    let rest = token.strip_prefix("https://")?;
    let path = rest.split_once('/')?.1.split('#').next()?;
    path.split('/').find(|s| !s.is_empty()).map(str::to_string)
}

/// `dir/name.ext:12…` (or, with `bare`, `dir/name.ext`) → `dir/name.ext`.
fn file_key(token: &str, bare: bool) -> Option<String> {
    let path = match token.split_once(':') {
        Some((path, rest)) if rest.starts_with(|c: char| c.is_ascii_digit()) => path,
        Some(_) => return None,
        None if bare && token.contains('/') => token,
        None => return None,
    };
    if !path.rsplit('/').next().is_some_and(|n| n.contains('.')) { return None; }
    let parts: Vec<&str> = path.rsplit('/').take(2).collect();
    Some(parts.iter().rev().copied().collect::<Vec<_>>().join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_pages_are_hits_by_slug() {
        let out = "  https://reports.example.com/mars-seo-competitor-20261023/  [report/section] Mars …\n  https://reports.example.com/mars-seo-competitor-20261023/#COMP  [report/case] …\n  https://reports.example.com/marscrm-seo-competitors-20261023/  [report/section] M";
        assert_eq!(returned_files(out), ["mars-seo-competitor-20261023", "marscrm-seo-competitors-20261023"]);
    }

    #[test]
    fn reshaped_output_counts_bare_and_counted_paths() {
        let cmd = r#"cd /r && LOCAL_SEARCH_MISS=1 ~/.local/bin/local-search search "import-production/" --corpus code --lines 2>&1 | awk -F: '{print $1}' | sort | uniq -c | sort -rn | head -80"#;
        let out = "  32 /Volumes/W/Mars_agent/Mars_CRM/docs/figma-annotation-repair.json\n   2 .claude/skills/table-design/references/naming.md";
        assert_eq!(returned_by(cmd, out), ["docs/figma-annotation-repair.json", "references/naming.md"]);
        assert_eq!(returned_by(cmd, "  -- docs\n/Volumes/W/docs/B_prd/1028/tasks.json"), ["1028/tasks.json"]);
        // Without a reshaping pipe, bare paths come from another program (find, ls) and are no hit.
        assert!(returned_by("local-search search x --lines; find . -name '*.rs'", "./src/a.rs").is_empty());
        // A NONE line in the output means the bare paths were printed by something else.
        assert!(returned_by(cmd, "NONE: no code chunk has every word.\n/Volumes/W/a/b.rs").is_empty());
        assert!(returned_files("  32 /Volumes/W/a/b.rs").is_empty());
    }

    #[test]
    fn a_spread_chunk_is_a_near_suggestion_used_only_when_opened() {
        // 17ad7f85…jsonl:391: the only chunk with both words was maintenance.md, and the agent read it.
        let out = "no chunk holds the exact pattern; chunks with all of its words:\n  -- no line holds every word as written; nearest lines of chunks with the words --\n/Users/b/.codex/skills/watchdog/references/maintenance.md:7  (words spread over the chunk)\nMars_CRM/rust/docs/ts-archive.md: lls/report/indexer/classify.ts` stays";
        let cmd = "local-search search \"install script-guard\" --corpus docs --lines";
        let opened = returned_or_used(cmd, out, |f| f.iter().any(|k| k == "references/maintenance.md"));
        assert_eq!(opened, ["references/maintenance.md"]);
        assert!(returned_or_used(cmd, out, |_| false).is_empty());
        // Exact hits still win; a spread line beside them is no hit unless the agent opens it.
        assert_eq!(returned_files("a/b.rs:3:fn x()\nc/d.rs:1-9  (words spread over the chunk)"), ["a/b.rs"]);
        // 215c6438…jsonl:113: `gcloud_auth` exact only in a classifier; the agent read the spread
        // line's gcloud_reauth.rs next, so the call adopted that file.
        let both = "w/release_retro/stats.rs:60:    (\"gcloud_auth\", \"environment\", &[])\n  -- no line holds every word as written; nearest lines of chunks with the words --\nMars_CRM/rust/crates/mars-ops/src/canary/gcloud_reauth.rs:1-36  (words spread over the chunk)";
        let cmd = "local-search search \"gcloud_auth\" --corpus code --lines";
        assert_eq!(returned_or_used(cmd, both, |f| f.iter().any(|k| k == "canary/gcloud_reauth.rs")), ["release_retro/stats.rs", "canary/gcloud_reauth.rs"]);
        assert_eq!(returned_or_used(cmd, both, |_| false), ["release_retro/stats.rs"]);
    }

    #[test]
    fn an_unnumbered_near_line_names_its_file() {
        // 98a8572e…jsonl:113: exact lines, the docs section, then kit.md only as `path: text` near lines.
        let out = "a/x/toolverto.com.json:58:  \"notes\": \"Public form\"\n  -- docs: rules and notes naming the same words --\n  -- no line holds every word as written; nearest lines of chunks with the words --\n.claude/skills/product-listing/references/kit.md: When the 9225 lease fails\n.claude/skills/product-listing/references/kit.md: management is not supported";
        assert_eq!(returned_files(out), ["x/toolverto.com.json", "references/kit.md"]);
        // With no exact line it stays a suggestion, a hit only when opened.
        let only_near = "  -- no line holds every word as written; nearest lines of chunks with the words --\n.claude/skills/p/references/kit.md: When the lease fails";
        assert!(returned_or_used("local-search search x --lines", only_near, |_| false).is_empty());
        assert_eq!(returned_or_used("local-search search x --lines", only_near, |f| f.iter().any(|k| k == "references/kit.md")), ["references/kit.md"]);
        // Outside the near section, `path: text` from another program is no hit.
        assert!(returned_files("a/b/README.md: ASCII text").is_empty());
    }

    #[test]
    fn files_outside_the_path_under_its_none_are_suggestions() {
        // db32365e…jsonl:4169: NONE in web/src, then contract files outside it; the agent took the NONE.
        let out = "doc.text.fill.svg\nNONE: no code chunk has every word. The code corpus holds Mars_agent.\n  -- outside --path: 6 file(s) hold it on one line, first 3 --\npackages/contracts/src/informationIconNames.ts:764:  \"file-lock\",\nrust/crates/mars-contracts/src/data/information_icon_names.txt:761:file-lock";
        let cmd = "ls x | head; local-search search \"file-lock\" --corpus code --lines --path web/src 2>&1 | head -5";
        assert!(returned_or_used(cmd, out, |_| false).is_empty());
        assert_eq!(returned_or_used(cmd, out, |f| f.iter().any(|k| k == "src/informationIconNames.ts")), ["src/informationIconNames.ts"]);
        // Under an in-scope hit the outside group's files stay returned files.
        let found = "web/src/i18n/settings.ts:633:    AI 数据共享\n  -- outside --path: 29 more file(s) hold it, first rust/crates/mars-contracts/src/lib.rs:10 --";
        assert_eq!(returned_files(found), ["i18n/settings.ts"]);
    }

    #[test]
    fn a_fold_marker_names_its_file_and_copies() {
        // bfff573f…jsonl:245: the block's own line was filtered away; the marker alone stayed.
        let out = ".claude/skills/loading/scripts/probe.js: … same line(s) in 2 more files: .claude/skills/a/scripts/probe.js:41, b/long-frame/probe.js:58\n  -- docs: rules and notes naming the same words --";
        assert_eq!(returned_files(out), ["scripts/probe.js", "long-frame/probe.js"]);
        // Under a near label the marker's files stay suggestions.
        let near = "  -- no line holds every word as written; nearest lines of chunks with the words --\nx/y/a.md: … same line(s) in 1 more file: z/b.md:3";
        assert!(returned_or_used("local-search search x --lines", near, |_| false).is_empty());
    }

    #[test]
    fn query_block_headers_alone_are_hits() {
        // afdb1701…jsonl:373: `query … | grep -v "^ "` kept only the block headers and the timing line.
        let out = "==> /r/Features/Events/DesktopEventsScreen.swift:1-25\n\n==> /r/Features/Events/EventEditorSheet.swift:1-30\nroute vector, embed 199.8 ms";
        assert_eq!(returned_files(out), ["Events/DesktopEventsScreen.swift", "Events/EventEditorSheet.swift"]);
        // `head` file banners and headers without a line range name no hit.
        assert!(returned_files("==> a/b.rs <==\nfn x()").is_empty());
        assert!(returned_files("==> notes").is_empty());
    }

    #[test]
    fn a_translations_marker_names_its_file_and_each_translation() {
        // Search-retro 89259434869436960053: local-search folds `.lproj` translations under the first.
        let out = "ios/App/th.lproj/Localizable.strings: … same keys in 2 more translations: ios/App/ko.lproj/Localizable.strings:861,862, ios/App/en.lproj/Localizable.strings:861,862\nios/Features/MemoryEventsTab.swift:26:Button(\"加载失败\")";
        assert_eq!(returned_files(out), ["th.lproj/Localizable.strings", "ko.lproj/Localizable.strings", "en.lproj/Localizable.strings", "Features/MemoryEventsTab.swift"]);
    }

    #[test]
    fn a_search_counted_by_wc_is_detected() {
        assert!(counted("local-search search a --lines 2>/dev/null | grep -v '^ *--' | wc -l"));
        assert!(!counted("local-search search a --lines | head; ls | wc -l"));
        assert!(!counted("local-search search a --lines"));
    }

    #[test]
    fn captured_output_is_detected() {
        assert!(captured(r#"for f in $(LOCAL_SEARCH_MISS=1 ~/.local/bin/local-search search "X {" --lines | cut -d: -f1); do echo $f; done"#));
        assert!(captured("x=`local-search search a`"));
        assert!(!captured("echo $(date); local-search search a --lines | head"));
        assert!(!captured("local-search search a --lines"));
    }

    #[test]
    fn a_search_piped_into_a_line_filter_is_filtered() {
        // c82d6abb…jsonl:697: the help text of another program was all that printed.
        let usage = "Usage: bun demo-session.ts <command> --product crm|hackathon";
        assert!(filtered(r#"bun d.ts --help 2>&1 | head -40; ~/.local/bin/local-search query "delete labelled test threads" --corpus code 2>&1 | grep -E '^\S+:[0-9]' | head -10"#, usage));
        assert!(filtered("local-search search a --lines | head -5 | grep -i test", ""));
        // A NONE that reached the transcript, or an excluding filter (8ce2f885…jsonl:1089), was seen.
        assert!(!filtered("local-search search a --lines | grep -i a", "NONE: no code chunk has every word."));
        assert!(!filtered("local-search search a --lines | grep -v \"^note\" | head", "pg/migrate.rs:30-58  (words spread over the chunk)"));
        assert!(!filtered("local-search search a --lines | rg --invert-match x", ""));
        // A grep of another program's output, or after the search's pipeline ended, is no filter of it.
        assert!(!filtered("bun d.ts --help | grep usage; local-search search a --lines | head", ""));
        assert!(!filtered("local-search search a --lines; grep -n a f.ts", ""));
    }
}
