//! The places local-search's cut line names (`local-search/src/cut_alts.rs`, ~/automation, search-retro
//! 818072776481006282864): under `--limit`'s cut note, `-- cut by --limit before any line of: a (first
//! path:line), … --` names each alternative of an `a|b` word that had matches in scope but no printed
//! line. Those files hold the asked alternative's line, so they are returned hits, like the list's own
//! rows: 5217f027…jsonl:132 asked `interaction-only|appearance`, the cut hid `useTurnstile.ts:170`,
//! and the agent then grepped that file (:140); with the line printed, that read adopts the call.

/// Start of the line, after any timing prefix.
const LABEL: &str = "-- cut by --limit before any line of:";

/// The `path:line` places of a cut line, or `None` for any other line.
pub(super) fn places(line: &str) -> Option<Vec<&str>> {
    let rest = line.trim_start().strip_prefix(LABEL)?;
    Some(rest.split("(first ").skip(1).filter_map(|p| p.split(')').next()).map(str::trim).collect())
}

#[cfg(test)]
mod tests {
    use super::super::returned::returned_or_used;
    use super::places;

    const LINE: &str = "-- cut by --limit before any line of: interaction-only (first Mars_CRM/web/src/screens/Intro/useTurnstile.ts:170), b (first x/y.rs:3); search it apart or narrow --path --";

    #[test]
    fn the_cut_line_names_its_places() {
        assert_eq!(places(LINE), Some(vec!["Mars_CRM/web/src/screens/Intro/useTurnstile.ts:170", "x/y.rs:3"]));
        assert_eq!(places("-- more match than --limit 30: this list is cut; rerun with --limit 150 for every match --"), None);
    }

    /// 5217f027…jsonl:132 with the new line: the named file is a returned file, so reading it adopts the call.
    #[test]
    fn reading_a_file_the_cut_line_names_adopts_the_hit() {
        let command = "cd /Volumes/MarsWork/Workspaces/Mars_agent; ~/.local/bin/local-search search \"interaction-only|appearance\" --corpus code --path Mars_CRM/web/src --per-source 1 --limit 30 --lines 2>&1 | head -30";
        let output = format!("-- more match than --limit 30: this list is cut; rerun with --limit 150 for every match --\n{LINE}\nMars_CRM/web/src/components/library/markdown/useChartTones.ts:1:/** The appearance-aware colours");
        let returned = returned_or_used(command, &output, |_| false);
        // The list's own row first, then the files the cut line names: a read of either adopts it.
        assert_eq!(returned, ["markdown/useChartTones.ts", "Intro/useTurnstile.ts", "x/y.rs"]);
    }
}
