//! The semantic duplicate pass over one capture: units from the body and the bundled skill sources,
//! vectors, cross-place pairs, groups, and `file:line` for every grouped unit.
use crate::{dup, embed, locate::Corpus, rerank, units};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

pub struct Pass {
    pub units: Vec<units::Unit>,
    pub pairs: Vec<dup::Pair>,
    pub groups: Vec<dup::Group>,
    pub candidates: usize,
}

/// Units of the capture at the tree's current commit.
pub fn collect(body: &Value, tree: &Path) -> Result<Vec<units::Unit>> {
    let mut all = units::from_body(body);
    for (rel, text) in crate::tree::skill_sources(tree)? {
        all.extend(units::from_skill_source(&rel, &text));
    }
    Ok(all)
}

/// Previous run's pairs as unit pairs, for `dup::mark_new`.
pub fn previous_pairs(saved: &Value) -> Vec<(units::Unit, units::Unit)> {
    saved["pairs"].as_array().into_iter().flatten().filter_map(|p| {
        let unit = |k: &str| serde_json::from_value::<units::Unit>(p[k].clone()).ok();
        Some((unit("a")?, unit("b")?))
    }).collect()
}

pub fn run(all: Vec<units::Unit>, tree: &Path, previous: Option<&[(units::Unit, units::Unit)]>) -> Result<Pass> {
    let texts: Vec<String> = all.iter().map(|u| u.text.clone()).collect();
    let vecs = embed::vectors(&texts)?;
    let mut pairs = dup::pairs(&all, &vecs);
    let candidates: Vec<(usize, usize)> = pairs.iter().map(|p| (p.a, p.b)).collect();
    for (p, s) in pairs.iter_mut().zip(rerank::judge(&texts, &candidates)?) {
        p.judge = s;
    }
    let candidates = pairs.len();
    pairs.retain(|p| p.judge >= dup::SAME);
    if let Some(prev) = previous {
        dup::mark_new(&all, &mut pairs, prev);
    }
    let groups = dup::groups(&all, &vecs, &pairs);
    let mut all = all;
    let corpus = Corpus::load(tree)?;
    for g in &groups {
        for m in &g.members {
            if all[*m].src.is_empty() {
                all[*m].src = corpus.find(&all[*m].text);
            }
        }
    }
    Ok(Pass { units: all, pairs, groups, candidates })
}

impl Pass {
    pub fn standing_waste(&self) -> usize { self.groups.iter().map(|g| g.standing_waste).sum() }
    pub fn on_demand_waste(&self) -> usize { self.groups.iter().map(|g| g.on_demand_waste).sum() }

    pub fn to_json(&self) -> Value {
        let unit = |i: usize| serde_json::to_value(&self.units[i]).unwrap_or(Value::Null);
        json!({
            "threshold": dup::similar(), "same": dup::SAME, "link": dup::LINK, "candidates": self.candidates, "reranker": rerank::MODEL, "embedder": embed::MODEL, "units": self.units.len(),
            "standing_waste": self.standing_waste(), "on_demand_waste": self.on_demand_waste(),
            "pairs": self.pairs.iter().map(|p| json!({"a": unit(p.a), "b": unit(p.b), "score": p.score, "judge": p.judge, "new": p.new})).collect::<Vec<_>>(),
            "groups": self.groups.iter().map(|g| json!({
                "id": g.id, "places": g.places, "standing_waste": g.standing_waste, "on_demand_waste": g.on_demand_waste,
                "new_pairs": g.new_pairs, "new_standing_waste": g.new_standing_waste, "oversized": g.members.len() > dup::MAX_GROUP,
                "members": g.members.iter().map(|m| unit(*m)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }
}
