//! The group label local-search prints under an exact NONE when another corpus holds the words
//! (`local-search/src/query/extras.rs` `elsewhere`, since search-retro 85636608367849986859):
//! `-- report: the code corpus holds none of it, report does; rerun with --corpus report --`.
//! The source under it lives outside the corpus asked, so it is a suggestion like a file outside
//! `--path`: a hit only when the agent then opens it (`returned.rs`). Search-retro
//! 813204256598053172535 (11197a04…jsonl:100): `search "token.example.com" --corpus code` printed
//! NONE, then the report page `token-collection-speed-20261029` under this label; the agent reran
//! in `--corpus report` as the label told it (:116), never opened that page, and the right NONE
//! counted as an unadopted hit. Search-retro 813279914114122985562 left the same shape open at one call.

/// True for the `elsewhere` group label of a NONE.
pub(super) fn label(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("-- ") && line.contains(" corpus holds none of it, ") && line.contains("rerun with --corpus ")
}

/// True for the note a scoped search prints when the corpus asked holds no source under `--path`
/// and it shows the other corpus instead (`local-search/src/scope_corpus.rs` mixed_note): the
/// corpus asked answered NONE, so the files below are suggestions too. Search-retro
/// 815285992441756886172 (a4990917…jsonl:243): `search "Glass|glass|blur|…" --corpus code --path
/// Mars_CRM/android` printed this note and one parity README while 531 Kotlin files were unindexed;
/// the next `rg -i glass Mars_CRM/android/app/src` printed 80 lines, and the call counted as an
/// unadopted hit instead of the false NONE it was.
pub(super) fn switched(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("note: no ") && line.contains(" source under --path ") && line.contains(" files do, so this shows --corpus ")
}

#[cfg(test)]
mod tests {
    use super::super::returned::returned_or_used;

    /// 11197a04…jsonl:102, the output of two searches in one command.
    const OUTPUT: &str = "NONE: no code chunk has every word. The code corpus holds Mars_agent and ~/automation (not its config/ or host/); build output is unindexed, so rg there.\n  -- report: the code corpus holds none of it, report does; rerun with --corpus report --\n  https://reports.example.com/token-collection-speed-20261029/  [report/overview]\n    https://token.example.com:8023\n  -- history: the code corpus holds none of it, history does; rerun with --corpus history --\n  /home/user/Library/Application Support/orca/codex-runtime-home/home/sessions/2026/10/21/rollout-2026-10-21T01-06-31-01a15a47-6644-7aa3-af9d-a9511f8c3e73.jsonl:13  [history/assistant]\n    https://token.example.com:8023/ 舒服一下，重启 Docker · 2026-10-20\n    assistant: 我先按重启本机 Docker 处理，并检查 token.example.com:8023 的恢复情况。\n---\nNONE: no docs chunk has every word.\n  -- report: the docs corpus holds none of it, report does; rerun with --corpus report --\n  https://reports.example.com/token-collection-speed-20261029/  [report/overview]\n    https://token.example.com:8023";
    const COMMAND: &str = "~/.local/bin/local-search search \"token.example.com\" --corpus code --per-source 1 --limit 100 --lines | head -40; echo ---; ~/.local/bin/local-search search \"token.example.com\" --corpus docs --per-source 1 --limit 100 --lines | head -20";

    #[test]
    fn the_label_is_recognised() {
        assert!(super::label("  -- report: the code corpus holds none of it, report does; rerun with --corpus report --"));
        // A query's absent exact name (`extras.rs` absent_elsewhere, search-retro 815942659352666385145).
        assert!(super::label("  -- report names `ExampleApp_5g`: the docs corpus holds none of it, report does; rerun with --corpus report --"));
        assert!(!super::label("  -- docs: rules and notes naming the same words --"));
        assert!(!super::label("  -- outside --path: 6 file(s) hold it on one line, first 3 --"));
    }

    #[test]
    fn a_source_in_another_corpus_under_a_none_is_a_suggestion() {
        // The agent took the NONE and asked the report corpus: no hit of this call.
        assert!(returned_or_used(COMMAND, OUTPUT, |_| false).is_empty());
        // Opening the suggested page adopts it.
        let opened = returned_or_used(COMMAND, OUTPUT, |f| f.iter().any(|k| k == "token-collection-speed-20261029"));
        assert_eq!(opened, ["token-collection-speed-20261029"]);
        // Under an exact hit the other corpus's source stays a returned file.
        let found = "web/src/a.ts:3:const host = 'token.example.com'\n  -- report: the code corpus holds none of it, report does; rerun with --corpus report --\n  https://reports.example.com/token-collection-speed-20261029/  [report/overview]";
        assert_eq!(returned_or_used("local-search search x --corpus code --lines", found, |_| false), ["src/a.ts", "token-collection-speed-20261029"]);
    }

    #[test]
    fn files_of_the_other_corpus_under_a_scope_switch_note_are_suggestions() {
        // a4990917…jsonl:243 (Kotlin was not indexed): the code corpus held nothing under the scope.
        let output = "note: no code source under --path Mars_CRM/android holds it; its docs files do, so this shows --corpus docs\n-- no chunk in --path holds: blur, RenderEffect; the lines below match the other alternatives --\nMars_CRM/android/docs/parity/README.md:11:| Deviations | every place 1:1 is not achievable (Metal shader, liquid glass, system font) |\n  -- outside --path: 30 more file(s) hold it, first Mars_CRM/web/src/components/library/GlassMaterial/README.md:1 --";
        let command = "cd /r; ls Mars_CRM/android | head; ~/.local/bin/local-search search \"Glass|glass|blur|RenderEffect\" --corpus code --path Mars_CRM/android --per-source 1 --limit 100 --lines 2>&1 | head -80";
        assert!(super::switched(output.lines().next().unwrap()));
        assert!(returned_or_used(command, output, |_| false).is_empty());
        assert_eq!(returned_or_used(command, output, |f| f.iter().any(|k| k == "parity/README.md")), ["parity/README.md"]);
    }
}
