//! Whether the sweep after a NONE proves the searched name existed (a false NONE). Search-retro
//! 84763104493312366879 read two right NONEs counted as false (`rg -F` found neither name in the
//! indexed tree): `grep -c "record.example.com" $tmp` inside a heredoc written for a remote host
//! (17a40a19…jsonl:1530), and `grep -n -i "index.lock\|retry…" scripts/utils/…*.ts` that printed
//! nothing while the step's `which`, `file` and `ls` printed lines (66f454ce…jsonl:478). A rule that
//! also required the query literal in the printed text was rejected: it dropped a real false NONE
//! (4aedaa8c…jsonl:168, `enum MarsSymbol` exists) whose text lines came from another rg in the step.
use super::quality;
use super::steps::{Kind, Step};

/// True when a sweep printed a match. A sweep whose rg/grep all ask for line numbers (`-n`) and no
/// file lists or counts, in a step that printed no `N:text`/`path:N:text` line and no bare number,
/// matched nothing: the step's printed lines came from its other commands. Lines an `ls` of the step
/// printed are not matches either (`listed`).
pub(super) fn confirms(output: &str, sweep: &str) -> bool {
    // The step's own `echo` markers, and what printed before the one ahead of the sweep (`echoed.rs`).
    let output = super::echoed::own(output, sweep);
    let args = ls_args(sweep);
    let kept: Vec<&str> = output.lines().filter(|l| args.as_ref().is_none_or(|a| !listed(l.trim(), a))).collect();
    let output = kept.join("\n");
    printed(&output) && !(numbered(sweep) && !output.lines().any(|l| text_of(l).is_some() || is_number(l.trim())))
}

/// `confirms`, and when the step's rg/grep ask only for line numbers, some printed line names one of
/// the searched `names` (or is a bare count). Search-retro 81053780001780858038
/// (agent-ae1424d9…jsonl:52): the NONE for `heroVerify|heroEnter` was right (commit 0c1361d92c
/// removed both), and the next step's `grep -n "heroVerify\|verify" BindContactFlow.tsx` printed two
/// `verify` lines, which read as a false NONE. Unlike the rule rejected above, it applies only to
/// `-n` sweeps (no `-l`/`-c`), so 4aedaa8c…jsonl:168 (`-l`) stays a false NONE; any printed line
/// counts, so a `find` in the same step that prints the path (b48eb9f7…jsonl:404,
/// `…/AuthGlass/auth-glass-phone.imageset`) also does.
pub(super) fn confirms_names(output: &str, sweep: &str, names: &[String]) -> bool {
    // A local-search NONE inside the sweep step printed its own lines (`none_block.rs`).
    let output = &super::none_block::strip(output, sweep);
    if !confirms(output, sweep) {
        return false;
    }
    if names.is_empty() || !numbered(sweep) {
        return true;
    }
    // Lines printed before the step's echo marker ahead of the sweep are its other commands' output
    // (`echoed.rs`), so they cannot show the sweep found a name. Search-retro 812798351439866126096
    // (9f3ea62e…jsonl:81): after a right code and docs NONE for a two-character brand name, the
    // step ran `local-search search <name> --corpus history …; echo ----; rg -n "<name>|<other>"
    // traffic-sources.md`; the history hits above the marker held the name, the rg printed only one
    // `<other>` line, and the NONE counted false.
    let own = super::echoed::own(output, sweep);
    let lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    own.lines().any(|l| is_number(l.trim()) || lower.iter().any(|n| l.to_lowercase().contains(n.as_str())))
}

/// True when the sweep reads a file that a later local-search call (in `earlier`, before the sweep)
/// returned: it follows that hit, not the NONE. Search-retro 87040299049855422752: a docs NONE for
/// `web.cafe`, then a history search that returned 6f535f15…jsonl:136, then `grep -o … web\.cafe`
/// of that transcript (58a0fe71…jsonl:71, :80, :92); `web.cafe` is in no code or docs file.
/// A `local-search tools` call locates a transcript the same way: it is the route a history NONE
/// names for tool calls. Search-retro 813204256598053172535, read in the window before it
/// (ca9752b0…jsonl:1101): a history NONE for `测试归类其他信息1106`, then `local-search tools` (:1103)
/// printed c06bd891…jsonl:962, then `grep -n` of that transcript (:1105); the name was only in tool calls.
pub(super) fn reads_located(items: &[Step], earlier: &[&Step], sweep: &str) -> bool {
    earlier.iter().filter(|s| s.kind == Kind::Shell && (quality::invocation(&s.text).is_some() || runs_tools(&s.text))).any(|s| {
        quality::result_of(items, &s.id).is_some_and(|r| quality::returned_files(&r.text).iter().any(|f| sweep.contains(f.as_str())))
    })
}

/// True when a simple command of `command` runs `local-search tools`.
fn runs_tools(command: &str) -> bool {
    quality::commands(command).iter().any(|(_, segment)| {
        let words: Vec<&str> = segment.split_whitespace().skip_while(|w| w.contains('=') && !w.starts_with('-')).collect();
        words.first().is_some_and(|p| p.rsplit('/').next() == Some("local-search")) && words.get(1) == Some(&"tools")
    })
}

/// The arguments of every `ls` in the step, or None when it ran no `ls`.
fn ls_args(sweep: &str) -> Option<Vec<String>> {
    let mut out: Option<Vec<String>> = None;
    for (_, segment) in super::quality::commands(&strip_heredocs(sweep)) {
        let words: Vec<&str> = segment.split_whitespace().skip_while(|w| w.contains('=')).collect();
        if words.first().is_some_and(|p| p.rsplit('/').next() == Some("ls")) {
            let args = words[1..].iter().filter(|w| !w.starts_with('-') && !w.contains('>')).map(|w| w.trim_matches(|c| c == '"' || c == '\'').trim_end_matches('/').to_string());
            out.get_or_insert_with(Vec::new).extend(args);
        }
    }
    out
}

/// A line `ls` prints: a `dir:` header, a bare entry name (no `/` or `:`), or one of its own
/// arguments, a glob matching within one directory. Search-retro 87040299049855422752:
/// `ls Mars_CRM/ios/appstore Mars_CRM/ios/appstore/*; rg -l "关系与工作助手" …` printed only ls lines,
/// `Mars_CRM/ios/appstore/README.md` among them, and the phrase lived only in App Store Connect
/// (c330a851…jsonl:51). A sweep listing top-level files by bare name next to an `ls` is missed.
fn listed(line: &str, args: &[String]) -> bool {
    if line.is_empty() || (line.ends_with(':') && !line[..line.len() - 1].contains(':')) || (!(line.contains('/') || line.contains(':')) && !is_number(line)) {
        return true;
    }
    args.iter().any(|a| match a.split_once('*') {
        Some((head, tail)) => line.len() >= head.len() + tail.len() && line.starts_with(head) && line.ends_with(tail) && !line[head.len()..line.len() - tail.len()].contains('/'),
        None => line == a,
    })
}

fn is_number(line: &str) -> bool {
    !line.is_empty() && line.chars().all(|c| c.is_ascii_digit())
}

/// A line that is not empty, an exit-code notice, NONE or a count of 0.
fn printed(output: &str) -> bool {
    output.lines().map(str::trim).any(|l| !l.is_empty() && !l.starts_with("Exit code") && !l.starts_with("NONE") && l != "0" && !l.starts_with("[crosstalk]"))
}

/// True when the step's rg/grep flags ask for line numbers (`-n`, `-rn`) and none lists files or
/// counts. Other programs' flags (`cut -c1-120`, `ls -l`) do not count.
pub(super) fn numbered(sweep: &str) -> bool {
    let flags: Vec<String> = super::quality::commands(&strip_heredocs(sweep))
        .into_iter()
        .filter(|(_, segment)| segment.split_whitespace().find(|w| !w.contains('=')).is_some_and(|p| matches!(p.rsplit('/').next().unwrap_or(p), "rg" | "grep" | "egrep")))
        .flat_map(|(_, segment)| segment.split_whitespace().filter(|w| w.starts_with('-') && !w.starts_with("--")).map(str::to_string).collect::<Vec<_>>())
        .collect();
    flags.iter().any(|f| f.contains('n')) && !flags.iter().any(|f| f.contains('l') || f.contains('c'))
}

/// The matched text of a `N:text` or `path:N:text` line (grep/rg with line numbers).
pub(super) fn text_of(line: &str) -> Option<&str> {
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let mut parts = line.splitn(3, ':');
    let (a, b, c) = (parts.next()?, parts.next(), parts.next());
    match (b, c) {
        (Some(b), Some(c)) if digits(b) => Some(c),
        (Some(b), _) if digits(a) => Some(b),
        _ => None,
    }
}

/// The command without heredoc bodies: lines after `<<WORD` (or `<<'WORD'`, `<<-WORD`) up to the
/// line holding WORD alone are a file or a remote script being written, not a sweep that ran here.
pub(super) fn strip_heredocs(command: &str) -> String {
    let (mut out, mut until) = (String::new(), None::<String>);
    for line in command.lines() {
        if let Some(word) = &until {
            if line.trim() == word {
                until = None;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
        if let Some((_, after)) = line.split_once("<<").filter(|(_, a)| !a.starts_with('<')) {
            let word: String = after.trim_start_matches('-').trim_start().trim_start_matches(['\'', '"']).chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if !word.is_empty() {
                until = Some(word);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_numbered_sweep_that_printed_no_line_matched_nothing() {
        // 66f454ce…jsonl:478: only `which`/`file`/`ls` lines; the numbered grep printed nothing.
        let other = "/Users/b/.local/bin/commit-owned-paths: Mach-O 64-bit executable arm64\nscripts/utils/commit_owned_paths.ts";
        assert!(!confirms(other, "which c; file $(which c) | cut -c1-120; ls -l scripts/utils/c*; grep -n -i \"index.lock\\|retry\" scripts/utils/c*.ts | head"));
        assert!(confirms("scripts/utils/git.ts:40:  rm .git/index.lock", "rg -n index.lock scripts"));
        assert!(confirms("40:  rm .git/index.lock", "grep -n index.lock f"));
        assert!(confirms("3", "grep -n x f | wc -l"));
    }

    #[test]
    fn a_numbered_sweep_must_print_a_searched_name() {
        // agent-ae1424d9…jsonl:52: the grep added a broader `verify`; heroVerify was already removed.
        let names = vec!["heroVerify".to_string(), "heroEnter".to_string()];
        let sweep = "git diff --stat a HEAD -- x | tail -3; grep -n \"heroVerify\\|verify\" web/BindContactFlow.tsx | head -20";
        let out = " web/a.tsx | 7 ++++---\n 3 files changed\n21:export function BindContactFlow({ step }: { step: 'enter' | 'verify' })\n58:      navigate(config.verifyPath, {";
        assert!(!confirms_names(out, sweep, &names));
        assert!(confirms_names(&format!("{out}\n9:  var heroVerify: String"), sweep, &names));
        assert!(confirms_names("3", "grep -n heroVerify f | wc -l", &names));
        // File lists and unnumbered sweeps keep the printed rule (4aedaa8c…jsonl:168).
        assert!(confirms_names("MarsCRM/DesignSystem/MarsSymbol.swift", "rg -n \"MarsSymbol\\b\" MarsCRM -l", &["MarsSymbol".to_string()]));
        assert!(confirms_names("some text", "rg heroVerify a.ts", &names));
        // b48eb9f7…jsonl:404: a find in the step printed the folder; a real false NONE.
        let glass = vec!["auth-glass-phone".to_string(), "imageset".to_string(), "AuthGlass".to_string()];
        let out = "a.rs:55:pub fn render_png(svg: &str)\nios/MarsCRM/App/Assets.xcassets/AuthGlass/auth-glass-phone.imageset";
        assert!(confirms_names(out, "grep -n \"render_png(\" a.rs; find ios/MarsCRM -name \"auth-glass-phone.imageset\"", &glass));
    }

    #[test]
    fn file_lists_counts_and_unnumbered_sweeps_keep_the_printed_rule() {
        // 4aedaa8c…jsonl:168: `rg -n "MarsSymbol\b" … -l` listed MarsSymbol.swift; a real false NONE.
        assert!(confirms("MarsCRM/DesignSystem/MarsSymbol.swift\nMarsCRM/DesignSystem/MarsRowMenu.swift:135: func rank", "rg -n \"MarsSymbol\\b\" MarsCRM/DesignSystem -l"));
        assert!(confirms("Mars_CRM/web/src/a.ts", "rg -l index.lock"));
        assert!(confirms("a.ts:3", "grep -c index.lock a.ts"));
        assert!(confirms("some text", "rg index.lock a.ts"));
        assert!(!confirms("0", "grep -c x"));
        assert!(!confirms("NONE: no code chunk has every word.", "rg x"));
    }

    fn step(id: &str, kind: Kind, text: &str) -> Step {
        Step { at_ms: 1.0, id: id.into(), kind, text: text.into(), line: 1, turn: id.into() }
    }

    #[test]
    fn a_sweep_of_a_file_a_later_search_returned_follows_that_hit() {
        let t = "/u/.claude/projects/-r/6f535f15.jsonl";
        let items = vec![
            step("b", Kind::Shell, "~/.local/bin/local-search search \"web.cafe\" --corpus history --lines"),
            step("b", Kind::Result, &format!("  {t}:136  [history/assistant]")),
        ];
        let earlier = vec![&items[0]];
        assert!(reads_located(&items, &earlier, &format!("grep -o 'web\\.cafe' {t} | sort")));
        assert!(!reads_located(&items, &earlier, "rg -n 'web\\.cafe' .claude/skills"));
        assert!(!reads_located(&items, &[], &format!("grep -o 'web\\.cafe' {t}")));
        // ca9752b0…jsonl:1103 and :1105: the transcript came from `local-search tools`.
        let c = "/Users/b/.claude-profiles/team4/projects/-Volumes-r/c06bd891.jsonl";
        let items = vec![
            step("t", Kind::Shell, "~/.local/bin/local-search tools \"测试归类其他信息1106\" --sessions 2>&1 | cut -c1-600 | head -30"),
            step("t", Kind::Result, &format!("  {c}:962  5 calls  [Bash] 2026-11-06\ntools    8 calls in 2 transcripts matched")),
        ];
        let sweep = format!("T={c}; grep -n '测试归类其他信息1106' $T | cut -d: -f1 | head");
        assert!(reads_located(&items, &[&items[0]], &sweep));
        assert!(!reads_located(&items, &[&items[0]], "grep -rn '测试归类其他信息1106' Mars_CRM/rust"));
        assert!(runs_tools("cd /r; LOCAL_SEARCH_NO_LOG=1 local-search tools x") && !runs_tools("local-search search tools --lines"));
    }

    #[test]
    fn lines_an_ls_printed_are_not_matches() {
        let sweep = "cd /r; ls Mars_CRM/ios/appstore Mars_CRM/ios/appstore/* 2>&1 | head -50; rg -l \"关系与工作助手\" Mars_CRM/ios/appstore .claude/skills/mars-crm-ios 2>&1";
        let out = "Mars_CRM/ios/appstore/README.md\n\nMars_CRM/ios/appstore:\nmetadata\nREADME.md\n\nMars_CRM/ios/appstore/metadata:\napp-info\nversion";
        assert!(!confirms(out, sweep));
        assert!(confirms(&format!("{out}\n.claude/skills/mars-crm-ios/SKILL.md"), sweep));
        assert!(confirms("Mars_CRM/ios/appstore/metadata/version/zh-Hans.json", sweep));
        // Without an ls the printed rule is unchanged.
        assert!(confirms("README.md", "rg -l x"));
        assert!(confirms("3", "ls src; grep -c x src/a.rs"));
    }

    #[test]
    fn heredoc_bodies_are_not_sweeps() {
        let command = "cat > $TMPDIR/retire.sh <<'EOF'\nset -e\ngrep -c \"record.example.com\" $tmp\nEOF\nssh macmini 'bash -s' < $TMPDIR/retire.sh";
        let kept = strip_heredocs(command);
        assert!(!kept.contains("grep -c"));
        assert!(kept.contains("ssh macmini"));
        assert_eq!(strip_heredocs("rg -n foo src <<< \"x\"\ngrep bar a"), "rg -n foo src <<< \"x\"\ngrep bar a\n");
    }
}
