//! Lines a sweep step printed with its own `echo`, and the lines its commands printed before the
//! marker it echoed ahead of its first rg/grep/find. Neither is a match. Search-retro
//! 81412365984068943558 read six right NONEs for `planRetryBackoffV2` (it exists in no file; a
//! whole-repository `rg -l -F --hidden` returned exit 1) counted as false: the sweeps printed only
//! `done` (5cd1f99c…jsonl:62, `…; done; echo done`), `exit=1` (045d181b…jsonl:52,
//! `rg -l …; echo "exit=$?"`) and a `cat` of the path list before `echo ---` (dd7d52f5…jsonl:63).
use super::none::strip_heredocs;
use super::quality::commands;

/// The output without the step's echoed lines and without what preceded the last echo marker
/// written before the first sweep command.
pub(super) fn own(output: &str, sweep: &str) -> String {
    let segments: Vec<(String, Vec<String>)> = commands(&strip_heredocs(sweep)).into_iter().map(|(_, s)| program(&s)).collect();
    let templates: Vec<(usize, Vec<Piece>)> = segments.iter().enumerate().filter(|(_, (p, _))| p == "echo").filter_map(|(i, (_, args))| template(args).map(|t| (i, t))).collect();
    if templates.is_empty() {
        return output.to_string();
    }
    let first_sweep = segments.iter().position(|(p, _)| matches!(p.as_str(), "rg" | "grep" | "egrep" | "ugrep" | "find"));
    let marker = first_sweep.and_then(|f| templates.iter().filter(|(i, _)| *i < f).last());
    let lines: Vec<&str> = output.lines().collect();
    let start = marker.and_then(|(_, t)| lines.iter().position(|l| matches(t, l.trim()))).map_or(0, |p| p + 1);
    lines[start..].iter().filter(|l| !templates.iter().any(|(_, t)| matches(t, l.trim()))).copied().collect::<Vec<_>>().join("\n")
}

/// A local-search call's output up to the first line an `echo` written after its segment printed:
/// what follows that marker came from the step's later commands, never from the search. Search-retro
/// 817612922386767070928 (agent-adb4502c…jsonl:156): `local-search search "MarkdownView|…" --path
/// <worktree> | head -60; echo ---; grep -n "^import" features/people/PersonDossier.kt; grep -rn …`
/// printed the search's right NONE, `---`, then the greps' `features/people/PersonDossier.kt:3:…`
/// lines, which counted as an unadopted hit of the NONE.
pub(super) fn search_part<'a>(output: &'a str, command: &str) -> &'a str {
    let segments: Vec<(String, Vec<String>)> = commands(&strip_heredocs(command)).into_iter().map(|(_, s)| program(&s)).collect();
    let Some(search) = segments.iter().position(|(p, _)| p == "local-search") else { return output };
    let Some(marker) = segments.iter().enumerate().skip(search + 1).filter(|(_, (p, _))| p == "echo").find_map(|(_, (_, args))| template(args)) else { return output };
    let mut at = 0;
    for line in output.split_inclusive('\n') {
        if matches(&marker, line.trim()) {
            return &output[..at];
        }
        at += line.len();
    }
    output
}

#[derive(Debug, PartialEq)]
enum Piece {
    Text(String),
    Any,
}

/// The program word (after `VAR=` assignments and shell keywords) and the rest of the words.
fn program(segment: &str) -> (String, Vec<String>) {
    let mut words = segment.split_whitespace().skip_while(|w| w.contains('=') && !w.starts_with('-') || matches!(*w, "do" | "then" | "else" | "{" | "!" | "time"));
    let program = words.next().map(|p| p.rsplit('/').next().unwrap_or(p).to_string()).unwrap_or_default();
    (program, words.map(str::to_string).collect())
}

/// An `echo`'s printed line as literal text and `$…` wildcards. None when it could match a real
/// match line: a variable with fewer than three letters or digits of literal text (`echo "$f"`,
/// `echo "$a:$b"`), or when it echoes nothing.
fn template(args: &[String]) -> Option<Vec<Piece>> {
    let text = args.iter().skip_while(|a| matches!(a.as_str(), "-n" | "-e" | "-E" | "-ne")).map(|a| a.replace(['"', '\''], "")).collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = text.trim().chars().collect();
    let (mut pieces, mut literal, mut i) = (Vec::new(), String::new(), 0);
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            if !literal.is_empty() { pieces.push(Piece::Text(std::mem::take(&mut literal))); }
            pieces.push(Piece::Any);
            i += 1;
            match chars[i] {
                '{' => { while i < chars.len() && chars[i] != '}' { i += 1; } i += 1; }
                c if c.is_alphanumeric() || c == '_' => { while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') { i += 1; } }
                _ => i += 1,
            }
            continue;
        }
        literal.push(chars[i]);
        i += 1;
    }
    if !literal.is_empty() { pieces.push(Piece::Text(literal)); }
    let alnum = pieces.iter().map(|p| match p { Piece::Text(t) => t.chars().filter(|c| c.is_alphanumeric()).count(), Piece::Any => 0 }).sum::<usize>();
    let variable = pieces.contains(&Piece::Any);
    (!pieces.is_empty() && (!variable || alnum >= 3)).then_some(pieces)
}

/// Whether `line` is one the template prints: its literal pieces in order, anchored at both ends
/// unless a wildcard sits there.
fn matches(template: &[Piece], line: &str) -> bool {
    let mut rest = line;
    for (k, piece) in template.iter().enumerate() {
        let Piece::Text(t) = piece else { continue };
        let t = t.trim();
        let anchored = k == 0 || template[k - 1] != Piece::Any;
        match rest.find(t) {
            Some(0) if anchored => rest = &rest[t.len()..],
            Some(p) if !anchored => rest = &rest[p + t.len()..],
            _ => return false,
        }
        if rest.starts_with(' ') && k + 1 < template.len() { rest = rest.trim_start(); }
    }
    rest.is_empty() || template.last() == Some(&Piece::Any)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echoed_markers_are_not_matches() {
        // 5cd1f99c…jsonl:43: only the trailing `echo done` printed.
        let loop_sweep = "CROSSTALK_ALL=1 git status --porcelain | awk '{print $NF}' | while read -r f; do [ -f \"$f\" ] && grep -l -F planRetryBackoffV2 \"$f\"; done; echo done";
        assert_eq!(own("done", loop_sweep), "");
        // 045d181b…jsonl:44: rg found nothing and exited 1.
        assert_eq!(own("exit=1", "LOCAL_SEARCH_MISS=1 rg -l -F --hidden planRetryBackoffV2 /r; echo \"exit=$?\""), "");
        // A real file list survives the markers.
        assert_eq!(own("Mars_CRM/a.ts\nexit=0", "rg -l x /r; echo \"exit=$?\""), "Mars_CRM/a.ts");
    }

    #[test]
    fn lines_before_the_marker_ahead_of_the_sweep_are_not_matches() {
        // dd7d52f5…jsonl:44: `cat` of the path list, `echo ---`, then the per-file grep printed nothing.
        let sweep = "CROSSTALK_ALL=1 git status --porcelain --untracked-files=all | awk '{print $NF}' > \"$TMPDIR/p.txt\"; cat \"$TMPDIR/p.txt\"; echo ---; while read -r f; do [ -f \"$f\" ] && grep -l -F planRetryBackoffV2 \"$f\"; done < \"$TMPDIR/p.txt\"; echo \"done\"";
        assert_eq!(own("Mars_CRM/json\ncpu.txt\n---\ndone", sweep), "");
        assert_eq!(own("Mars_CRM/json\ncpu.txt\n---\nMars_CRM/json\ndone", sweep), "Mars_CRM/json");
        // A marker echoed after the sweep cuts nothing before it.
        assert_eq!(own("a.rs:3:x\n---", "rg -n x .; echo ---"), "a.rs:3:x");
    }

    #[test]
    fn a_marker_echoed_after_the_search_ends_its_output() {
        // agent-adb4502c…jsonl:156: the greps after `echo ---` printed PersonDossier.kt lines.
        let cmd = "cd /w/mars && ~/.local/bin/local-search search \"MarkdownView|ChatImageGrid\" --corpus code --path /w | head -60; echo ---; grep -n \"^import\" features/people/PersonDossier.kt | head";
        let out = "unindexed: the index holds no file under /w; rg results:\nNONE: rg found no line either.\n---\n3:import a.b\nfeatures/people/PersonDossier.kt:3:import a.b\n";
        assert_eq!(search_part(out, cmd), "unindexed: the index holds no file under /w; rg results:\nNONE: rg found no line either.\n");
        // A marker echoed before the search, or none at all, cuts nothing.
        assert_eq!(search_part("---\na/b.rs:1:x\n", "echo ---; local-search search x"), "---\na/b.rs:1:x\n");
        assert_eq!(search_part("a/b.rs:1:x\n", "local-search search x | head"), "a/b.rs:1:x\n");
        // An echo that could print a match line is no marker.
        assert_eq!(search_part("a/b.rs:1:x\n", "local-search search x; echo \"$f\""), "a/b.rs:1:x\n");
    }

    #[test]
    fn echoes_that_could_print_a_match_are_kept() {
        assert_eq!(own("Mars_CRM/a.ts", "rg -l x . | while read -r f; do echo \"$f\"; done"), "Mars_CRM/a.ts");
        assert_eq!(own("a.ts:3", "echo \"$a:$b\"; rg -c x a.ts"), "a.ts:3");
        assert_eq!(own("a.rs:1:x", "rg -n x a.rs"), "a.rs:1:x");
        assert!(template(&["\"==".into(), "$f".into(), "==\"".into()]).is_none());
        let exit = template(&["\"exit=$?\"".into()]).expect("literal exit=");
        assert!(matches(&exit, "exit=1"));
        assert!(!matches(&exit, "src/exit=1.rs:3:x"));
        let done = template(&["done".into()]).expect("literal done");
        assert!(matches(&done, "done") && !matches(&done, "src/done.rs"));
    }
}
