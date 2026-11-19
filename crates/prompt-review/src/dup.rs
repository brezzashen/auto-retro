//! Semantic duplicates: the same rule written in two places in different words. A pair is two units
//! in different places (another tool, system section, skill or catalog entry) whose vectors are at
//! least SIMILAR apart; pairs join into groups. Exact copies the analyzer already counts are pairs too.
use crate::embed::cosine;
use crate::units::Unit;
use serde::{Deserialize, Serialize};

/// Cosine floor of a candidate pair. Calibrated on the 2026-11-09 capture against the hand-found
/// duplicates of report mars-crm-prompt-skill-audit-20261109: the delete rule's copies scored
/// 0.80-0.84, the Feishu guidance copies 0.87-0.97.
pub const SIMILAR: f32 = 0.80;
/// Reranker floor of a duplicate (rerank.rs): copies scored ≥ 0.995, shared-vocabulary rows 0.06-0.98.
pub const SAME: f32 = 0.99;
/// Cosine floor every pair inside one group keeps (complete linkage).
pub const LINK: f32 = 0.78;

fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}
/// PROMPT_REVIEW_SIMILAR and PROMPT_REVIEW_LINK override the floors for calibration only.
pub fn similar() -> f32 { env_f32("PROMPT_REVIEW_SIMILAR", SIMILAR) }
fn link_floor() -> f32 { env_f32("PROMPT_REVIEW_LINK", LINK) }
/// A group larger than this is a topic cluster, not one rule; it is reported but split no further.
pub const MAX_GROUP: usize = 12;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pair {
    pub a: usize,
    pub b: usize,
    pub score: f32,
    /// Reranker score: same rule in other words.
    #[serde(default)]
    pub judge: f32,
    /// No near-identical pair existed in the previous run.
    pub new: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub members: Vec<usize>,
    pub places: Vec<String>,
    /// Characters of standing copies beyond the longest one: paid on every call.
    pub standing_waste: usize,
    /// Characters of on-demand skill copies of a rule the standing prefix already holds.
    pub on_demand_waste: usize,
    pub new_pairs: usize,
    /// Standing waste carried by members that only occur in new pairs.
    pub new_standing_waste: usize,
}

fn eligible(a: &Unit, b: &Unit) -> bool {
    a.place != b.place && !(a.surface == "session" && b.surface == "session") && (a.standing || b.standing || a.place.starts_with("skill:") && b.place.starts_with("skill:"))
}

pub fn pairs(units: &[Unit], vecs: &[Vec<f32>]) -> Vec<Pair> {
    let mut out = Vec::new();
    for i in 0..units.len() {
        for j in i + 1..units.len() {
            if eligible(&units[i], &units[j]) {
                let score = cosine(&vecs[i], &vecs[j]);
                if score >= similar() {
                    out.push(Pair { a: i, b: j, score, judge: 0.0, new: true });
                }
            }
        }
    }
    out
}

/// Complete linkage: pairs are taken strongest first, and two clusters merge only when every member
/// of one is at least LINK from every member of the other, so a chain of loosely related lines (the
/// rows of one resource table) never becomes one group.
fn link(n: usize, vecs: &[Vec<f32>], pairs: &[Pair]) -> Vec<Vec<usize>> {
    let mut order: Vec<&Pair> = pairs.iter().collect();
    order.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut of: Vec<Option<usize>> = vec![None; n];
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for p in order {
        let (ca, cb) = (of[p.a], of[p.b]);
        if ca.is_some() && ca == cb {
            continue;
        }
        let a = ca.map(|c| clusters[c].clone()).unwrap_or_else(|| vec![p.a]);
        let b = cb.map(|c| clusters[c].clone()).unwrap_or_else(|| vec![p.b]);
        if !a.iter().all(|x| b.iter().all(|y| cosine(&vecs[*x], &vecs[*y]) >= link_floor())) {
            continue;
        }
        let target = ca.or(cb).unwrap_or_else(|| { clusters.push(Vec::new()); clusters.len() - 1 });
        let mut merged = a;
        merged.extend(b);
        if let (Some(x), Some(y)) = (ca, cb) {
            clusters[if x == target { y } else { x }].clear();
        }
        for m in &merged {
            of[*m] = Some(target);
        }
        clusters[target] = merged;
    }
    clusters.into_iter().filter(|c| c.len() >= 2).collect()
}

fn chars(u: &Unit) -> usize { u.text.chars().count() }

pub fn groups(units: &[Unit], vecs: &[Vec<f32>], pairs: &[Pair]) -> Vec<Group> {
    let clusters = link(units.len(), vecs, pairs);
    let mut out: Vec<Group> = clusters.into_iter().map(|mut members| {
        members.sort();
        let inside: Vec<&Pair> = pairs.iter().filter(|p| members.contains(&p.a) && members.contains(&p.b)).collect();
        let standing: Vec<usize> = members.iter().copied().filter(|m| units[*m].standing).collect();
        let longest = standing.iter().map(|m| chars(&units[*m])).max().unwrap_or(0);
        let standing_waste = standing.iter().map(|m| chars(&units[*m])).sum::<usize>().saturating_sub(longest);
        let on_demand_waste = if standing.is_empty() { 0 } else { members.iter().filter(|m| !units[**m].standing).map(|m| chars(&units[*m])).sum() };
        let new_pairs = inside.iter().filter(|p| p.new).count();
        let old: Vec<usize> = inside.iter().filter(|p| !p.new).flat_map(|p| [p.a, p.b]).collect();
        let fresh: Vec<usize> = standing.iter().copied().filter(|m| !old.contains(m)).collect();
        let new_standing_waste = if old.is_empty() { standing_waste } else { fresh.iter().map(|m| chars(&units[*m])).sum() };
        let mut places: Vec<String> = members.iter().map(|m| units[*m].place.clone()).collect();
        places.sort();
        places.dedup();
        Group { id: String::new(), members, places, standing_waste, on_demand_waste, new_pairs, new_standing_waste }
    }).collect();
    out.sort_by(|a, b| (b.standing_waste + b.on_demand_waste).cmp(&(a.standing_waste + a.on_demand_waste)));
    for (i, g) in out.iter_mut().enumerate() {
        g.id = format!("S{}", i + 1);
    }
    out
}

/// Character trigrams of the normalized text, for matching a unit across weeks after small edits.
fn trigrams(text: &str) -> std::collections::HashSet<[char; 3]> {
    let c: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    c.windows(3).map(|w| [w[0], w[1], w[2]]).collect()
}

pub fn same_text(a: &str, b: &str) -> bool {
    let (x, y) = (trigrams(a), trigrams(b));
    let union = x.union(&y).count().max(1);
    x.intersection(&y).count() as f32 / union as f32 >= 0.6
}

/// Marks a pair old when the previous run held a pair between the same two places whose texts match.
pub fn mark_new(units: &[Unit], pairs: &mut [Pair], previous: &[(Unit, Unit)]) {
    for p in pairs.iter_mut() {
        let (a, b) = (&units[p.a], &units[p.b]);
        p.new = !previous.iter().any(|(x, y)| {
            (x.place == a.place && y.place == b.place && same_text(&x.text, &a.text) && same_text(&y.text, &b.text))
                || (x.place == b.place && y.place == a.place && same_text(&x.text, &b.text) && same_text(&y.text, &a.text))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(place: &str, text: &str, standing: bool) -> Unit {
        Unit { surface: place.split(':').next().unwrap().into(), place: place.into(), path: String::new(), text: text.into(), standing, src: vec![] }
    }

    #[test]
    fn groups_count_waste_and_newness() {
        let units = vec![unit("system:a", "Confirm before deleting anything.", true), unit("tool:delete", "Always confirm before you delete.", true),
            unit("skill:x", "Ask the user to confirm deletes.", false)];
        let mut ps = vec![Pair { a: 0, b: 1, score: 0.9, judge: 1.0, new: true }, Pair { a: 1, b: 2, score: 0.88, judge: 1.0, new: true }];
        mark_new(&units, &mut ps, &[(units[0].clone(), units[1].clone())]);
        assert!(!ps[0].new && ps[1].new);
        let vecs = vec![vec![1.0, 0.0]; 3];
        let g = &groups(&units, &vecs, &ps)[0];
        assert_eq!(g.members, vec![0, 1, 2]);
        assert_eq!(g.standing_waste, 33);
        assert_eq!(g.on_demand_waste, 32);
        assert_eq!((g.new_pairs, g.new_standing_waste), (1, 0));
    }
}
