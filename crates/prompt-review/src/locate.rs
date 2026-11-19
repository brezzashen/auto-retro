//! Maps a unit's rendered text back to `file:line` in the capture tree's TypeScript sources. The text
//! reaches the model through template literals, zod `.describe()` strings and string joins, so the
//! match is a plain substring probe of the unit (start, middle, end) that holds no quote, escape or
//! interpolation character; every file holding the probe is listed, because a second hit in source
//! is itself a copy.
use std::path::Path;

/// Directories whose `.ts` files assemble the agent's prompt (tests excluded).
pub const ROOTS: [&str; 2] = ["Mars_CRM/agent/src", "Mars_CRM/packages/contracts/src"];
const PROBE: usize = 18;
const MAX_HITS: usize = 4;

pub struct Corpus {
    files: Vec<(String, String, Vec<usize>)>,
}

impl Corpus {
    pub fn load(tree: &Path) -> anyhow::Result<Self> {
        let out = std::process::Command::new("git").arg("-C").arg(tree).args(["ls-files", "--"]).args(ROOTS).output()?;
        anyhow::ensure!(out.status.success(), "git ls-files failed in {}", tree.display());
        let mut files = Vec::new();
        for rel in String::from_utf8_lossy(&out.stdout).lines().filter(|p| p.ends_with(".ts") && !p.ends_with(".test.ts")) {
            if let Ok(text) = std::fs::read_to_string(tree.join(rel)) {
                let starts = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
                files.push((rel.to_string(), text, starts));
            }
        }
        Ok(Self { files })
    }

    fn line(starts: &[usize], offset: usize) -> usize {
        starts.partition_point(|s| *s <= offset)
    }

    /// `file:line` of every file holding a probe of `text`; empty when none matched.
    pub fn find(&self, text: &str) -> Vec<String> {
        for probe in probes(text) {
            let hits: Vec<String> = self.files.iter()
                .filter_map(|(rel, body, starts)| body.find(&probe).map(|at| format!("{rel}:{}", Self::line(starts, at))))
                .take(MAX_HITS).collect();
            if !hits.is_empty() {
                return hits;
            }
        }
        Vec::new()
    }
}

/// Up to three windows of PROBE characters free of characters that source escapes or interpolates.
fn probes(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let bad = |c: &char| matches!(c, '`' | '\'' | '"' | '\\' | '$' | '{' | '}' | '\n' | '…');
    let mut out: Vec<String> = Vec::new();
    if chars.len() < PROBE {
        return out;
    }
    for start in [0, (chars.len() - PROBE) / 2, chars.len() - PROBE] {
        // Slide right to the first clean window from this start.
        let found = (start..=chars.len() - PROBE).find(|s| !chars[*s..s + PROBE].iter().any(bad));
        if let Some(s) = found {
            let w: String = chars[s..s + PROBE].iter().collect();
            if !out.contains(&w) { out.push(w); }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_skip_quotes_and_lines_are_one_based() {
        let p = probes("Say 'yes' before deleting any person record from the CRM store.");
        assert!(p.iter().all(|w| !w.contains('\'')));
        let corpus = Corpus { files: vec![("a.ts".into(), "x\ny\nSay before deleting any person record\n".into(), vec![0, 2, 4])] };
        assert_eq!(corpus.find("before deleting any person record"), vec!["a.ts:3"]);
    }
}
