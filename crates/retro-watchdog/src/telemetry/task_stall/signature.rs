//! Signature of a failing command for the `repeat_failure` stall.
//!
//! Agents often open every shell call with the same preamble (`S=<scratchpad>`, `LEASE=$(cat ...)`,
//! a one-line helper `ab() { ... }`). Keyed on the raw command head, different commands that share
//! that preamble collapsed into one signature and raised a false repeat failure (incident
//! 810451496554981403331). The signature therefore skips pure assignment, helper-definition and
//! comment lines, and leading `NAME=value` prefixes, and keys on the first real command.
//! A leading `cd <dir>;` / `cd <dir> &&` is preamble too: one-line calls that all start in the same
//! scripts directory collapsed into one signature (incident 818109913379204620192).

/// Byte length of a `NAME=value` assignment at the start of `s`, value included; 0 when `s` does
/// not start with one.
fn assignment_len(s: &str) -> usize {
    let b = s.as_bytes();
    let name = b.iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'_').count();
    if name == 0 || b[0].is_ascii_digit() || b.get(name) != Some(&b'=') { return 0; }
    word_end(b, name + 1)
}

/// Byte length of a leading `cd <dir>` that is followed by `;` or `&&`; 0 otherwise, so a bare
/// `cd <dir>` stays the command.
fn cd_len(s: &str) -> usize {
    let Some(arg) = s.strip_prefix("cd ") else { return 0 };
    let start = s.len() - arg.trim_start().len();
    let end = word_end(s.as_bytes(), start);
    let tail = s[end..].trim_start();
    if tail.starts_with(';') || tail.starts_with("&&") { end } else { 0 }
}

/// End of the shell word starting at `i`: the first unquoted whitespace, `;` or `&` outside
/// `$( )`.
fn word_end(b: &[u8], mut i: usize) -> usize {
    let (mut depth, mut quote) = (0usize, 0u8);
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == b'\\' && quote == b'"' { i += 1; } else if c == quote { quote = 0; }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'(' {
            depth += 1;
        } else if c == b')' {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && (c.is_ascii_whitespace() || c == b';' || c == b'&') {
            break;
        }
        i += 1;
    }
    i.min(b.len())
}

/// The line without its leading assignments; empty when it is only a preamble.
fn command_part(line: &str) -> &str {
    let mut rest = line.trim_start().strip_prefix("export ").unwrap_or(line.trim_start());
    loop {
        let n = assignment_len(rest).max(cd_len(rest));
        if n == 0 { return rest; }
        rest = rest[n..].trim_start().trim_start_matches([';', '&']).trim_start();
    }
}

fn helper_definition(line: &str) -> bool {
    let head = line.split_once('{').map(|(h, _)| h.trim()).unwrap_or("");
    head.ends_with("()") && head[..head.len() - 2].chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && line.trim_end().ends_with('}')
}

/// The command that identifies a call: the first line that is not a preamble, without leading
/// assignments. Falls back to the whole command when every line is preamble.
pub(super) fn core(command: &str) -> &str {
    command.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#') && !helper_definition(l))
        .map(command_part).find(|c| !c.is_empty()).unwrap_or(command)
}

#[cfg(test)]
mod tests {
    use super::core;

    #[test]
    fn a_shared_preamble_is_skipped_and_the_first_real_command_keys_the_call() {
        let pre = "S=/private/tmp/claude-501/x/scratchpad\nLEASE=$(cat $S/lease.txt)\nab() { agent-browser --cdp \"$LEASE\" \"$@\"; }\n";
        assert_eq!(core(&format!("{pre}ab screenshot $S/a.jpg")), "ab screenshot $S/a.jpg");
        assert_eq!(core(&format!("{pre}# insert text\nab eval 'x'")), "ab eval 'x'");
        assert_eq!(core("S=\"a b\"; K=$(dirname \"$S\") && bun run deliver canary"), "bun run deliver canary");
        assert_eq!(core("LOCAL_SEARCH_MISS=1 local-search query x"), "local-search query x");
        assert_eq!(core("export A=1\nls -la"), "ls -la");
        assert_eq!(core("A=1"), "A=1", "an all-preamble command keeps itself");
        assert_eq!(core("cargo test -p x"), "cargo test -p x");
    }

    #[test]
    fn a_leading_cd_into_a_shared_directory_is_preamble() {
        let a = "cd /w/scripts; R=~/m/S07/cap; G='{\"at\":[1,2]}'; for st in \"grid:[$G]\"; do bun day_capture.js; done";
        let b = "cd /w/scripts; R=~/m/S08/cap; B='{\"at\":[3,4]}'; for st in \"pv1:[$B]\"; do bun day_capture.js; done";
        assert_eq!(core(a), "for st in \"grid:[$G]\"; do bun day_capture.js; done");
        assert_ne!(core(a), core(b));
        assert_eq!(core("cd \"/a b\" && bun test x"), "bun test x");
        assert_eq!(core("cd /tmp"), "cd /tmp", "a bare cd stays the command");
        assert_eq!(core("cdk deploy; ls"), "cdk deploy; ls");
    }
}
