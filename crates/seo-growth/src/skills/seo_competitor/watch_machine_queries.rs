//! Machine-generated Search Console queries (long templated phrasings such as
//! `relationship <x> and interpretation assistant`) that inflate a page's impressions at positions
//! 5–10 with zero clicks. When they hold a large share of a page's visible query impressions, the
//! Page-1 CTR check judges that page on its visible non-machine queries instead of the page row.

use super::watch_analyze::QueryRow;
use serde::Deserialize;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct MachineQueryConfig {
    /// Lowercase substrings; a query containing any of them is machine-generated.
    pub patterns: Vec<String>,
    /// Share of a page's visible query impressions above which the page is judged on human rows.
    pub min_share: f64,
}

impl Default for MachineQueryConfig {
    fn default() -> Self {
        Self { patterns: vec!["interpretation assistant".into()], min_share: 0.25 }
    }
}

/// A page whose visible queries are dominated by machine phrasings.
#[derive(Clone, Debug, PartialEq)]
pub struct MachineMix {
    /// Visible non-machine queries: impressions, clicks, impression-weighted position.
    pub human: (f64, f64, f64),
    pub machine_impressions: f64,
    pub visible_impressions: f64,
}

pub fn is_machine(query: &str, config: &MachineQueryConfig) -> bool {
    let q = query.to_lowercase();
    config.patterns.iter().any(|p| !p.is_empty() && q.contains(p.as_str()))
}

/// The page's machine mix when machine queries reach `min_share` of its visible impressions.
pub fn machine_mix(rows: &[QueryRow], page_url: &str, config: &MachineQueryConfig) -> Option<MachineMix> {
    let page_rows: Vec<&QueryRow> = rows.iter().filter(|r| r.page == page_url).collect();
    let visible: f64 = page_rows.iter().map(|r| r.impressions).sum();
    let (machine, human): (Vec<&QueryRow>, Vec<&QueryRow>) = page_rows.into_iter().partition(|r| is_machine(&r.query, config));
    let machine_impressions: f64 = machine.iter().map(|r| r.impressions).sum();
    if visible <= 0.0 || machine_impressions / visible < config.min_share { return None; }
    let impressions: f64 = human.iter().map(|r| r.impressions).sum();
    let clicks: f64 = human.iter().map(|r| r.clicks).sum();
    let position = if impressions > 0.0 { human.iter().map(|r| r.impressions * r.position).sum::<f64>() / impressions } else { 0.0 };
    Some(MachineMix { human: (impressions, clicks, position), machine_impressions, visible_impressions: visible })
}
