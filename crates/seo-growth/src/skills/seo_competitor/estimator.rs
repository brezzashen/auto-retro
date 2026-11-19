//! `.claude/skills/seo-competitor/scripts/estimator.ts`: the market-research mrr estimator core
//! (TypeScript port of mrr_estimator.py): same tiers, weights and log-space aggregation.
//! Only the fields mrrEstimate writes are kept; the `missing` list is not computed.

use super::files::{at, items, num, text};
use crate::js_wire::js;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug)]
pub struct Range {
    pub low: f64,
    pub base: f64,
    pub high: f64,
}

struct Tier {
    reliability: f64,
    uncertainty: f64,
    authority: f64,
}

fn tier_of(name: &str) -> Option<Tier> {
    let (reliability, uncertainty, authority) = match name {
        "payment_provider" => (0.98, 0.05, 4.0),
        "third_party_verified" => (0.92, 0.3, 3.0),
        "regulatory_filing" => (0.92, 0.1, 2.4),
        "company_disclosure" => (0.8, 0.2, 1.7),
        "reputable_press" => (0.7, 0.3, 1.2),
        "third_party_estimate" => (0.58, 0.4, 1.0),
        "derived_public" => (0.48, 0.5, 0.9),
        "market_prior" => (0.3, 0.7, 0.7),
        _ => return None,
    };
    Some(Tier { reliability, uncertainty, authority })
}

fn metric_spec(metric: &str) -> Option<f64> {
    Some(match metric {
        "mrr" => 1.0,
        "arr" => 0.95,
        "monthly_revenue" => 0.78,
        "revenue_30d" => 0.8,
        "annual_revenue" => 0.72,
        "customers_x_arpa" => 0.62,
        "traffic_funnel" => 0.4,
        "peer_mrr" => 0.32,
        _ => return None,
    })
}

/// `asRange(v, unc)`: a `{low, base, high}` object, or a number widened by `unc`.
fn range(v: &Value, unc: f64) -> Result<Range, String> {
    if v.is_object() {
        let (low, base, high) = (num(&v["low"]), num(&v["base"]), num(&v["high"]));
        return match (low, base, high) {
            (Some(low), Some(base), Some(high)) if low <= base && base <= high => Ok(Range { low, base, high }),
            _ => Err(format!("bad range {}", crate::js_wire::to_string(v).unwrap_or_default())),
        };
    }
    match num(v) {
        Some(n) if n >= 0.0 => Ok(Range { low: js::max(0.0, n * (1.0 - unc)), base: n, high: n * (1.0 + unc) }),
        _ => Err(format!("bad number {}", super::files::show(v))),
    }
}

fn mul(a: Range, b: Range) -> Range {
    Range { low: a.low * b.low, base: a.base * b.base, high: a.high * b.high }
}

fn div(a: Range, d: f64) -> Range {
    Range { low: a.low / d, base: a.base / d, high: a.high / d }
}

/// Freshness weight by the age of the evidence in days.
fn fresh(days: f64) -> f64 {
    if days <= 90.0 {
        1.0
    } else if days <= 180.0 {
        0.9
    } else if days <= 365.0 {
        0.75
    } else if days <= 730.0 {
        0.5
    } else {
        0.25
    }
}

/// One normalised signal with its weight.
#[derive(Clone, Debug)]
pub struct Signal {
    pub id: Option<Value>,
    pub dimension: Option<String>,
    pub metric: String,
    pub group: Option<String>,
    pub tier: String,
    pub url: Option<Value>,
    pub date: Option<Value>,
    pub mrr: Range,
    pub freshness: f64,
    pub weight: f64,
    pub supporting: Vec<Value>,
}

fn optional_text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

/// Normalises one signal: its MRR range, freshness and weight.
fn normalize(s: &Value, as_of: f64) -> Result<Signal, String> {
    let source = at(s, &["source"]);
    let tier_name = text(at(source, &["tier"])).to_string();
    let tier = tier_of(&tier_name).ok_or_else(|| format!("{}: tier {tier_name}", super::files::show(at(s, &["id"]))))?;
    let metric = text(at(s, &["metric"])).to_string();
    let spec = metric_spec(&metric).ok_or_else(|| format!("{}: metric {metric}", super::files::show(at(s, &["id"]))))?;
    let unc = num(at(s, &["uncertainty"])).unwrap_or(tier.uncertainty);
    let mrr = match metric.as_str() {
        "mrr" | "arr" | "monthly_revenue" | "revenue_30d" | "annual_revenue" | "peer_mrr" => {
            let mut mrr = range(at(s, &["amount"]), unc)?;
            if metric == "arr" || metric == "annual_revenue" {
                mrr = div(mrr, 12.0);
            }
            if matches!(metric.as_str(), "monthly_revenue" | "revenue_30d" | "annual_revenue") {
                let share = s.get("recurring_share").ok_or_else(|| "recurring_share required".to_string())?;
                mrr = mul(mrr, range(share, 0.0)?);
            }
            mrr
        }
        "customers_x_arpa" => mul(range(at(s, &["customers"]), 0.0)?, range(at(s, &["monthly_arpa"]), 0.0)?),
        _ => mul(mul(range(at(s, &["monthly_visits"]), 0.0)?, range(at(s, &["paid_conversion_rate"]), 0.0)?), range(at(s, &["monthly_arpa"]), 0.0)?),
    };
    if mrr.base.is_nan() || mrr.base <= 0.0 {
        return Err("non-positive base".to_string());
    }
    let reliability = num(at(s, &["reliability"])).unwrap_or(tier.reliability);
    let age = js::round((as_of - js::parse_iso_ms(text(at(source, &["date"])))) / 86_400_000.0);
    let freshness = fresh(age);
    Ok(Signal {
        id: s.get("id").cloned(),
        dimension: optional_text(at(s, &["dimension"])),
        metric,
        group: optional_text(at(s, &["independence_group"])),
        tier: tier_name,
        url: source.get("url").cloned(),
        date: source.get("date").cloned(),
        mrr,
        freshness,
        weight: reliability * freshness * spec * tier.authority,
        supporting: items(at(s, &["supporting_sources"])).to_vec(),
    })
}

/// The estimate of one competitor.
pub struct Estimate {
    pub low: f64,
    pub base: f64,
    pub high: f64,
    pub status: &'static str,
    pub score: f64,
    pub label: &'static str,
    pub contradiction: bool,
    pub signals: Vec<Signal>,
}

/// Keeps the heaviest signal of each independence group, then aggregates them in log space.
fn select(norm: Vec<Signal>) -> Vec<Signal> {
    let mut picked: Vec<Signal> = Vec::new();
    for s in norm {
        match picked.iter().position(|p| p.group == s.group) {
            Some(i) if s.weight > picked[i].weight => picked[i] = s,
            Some(_) => {}
            None => picked.push(s),
        }
    }
    picked.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
    picked
}

/// `estimate(doc)`: the MRR range, status, score and label of one evidence document.
pub fn estimate(doc: &Value) -> Result<Estimate, String> {
    let as_of = js::parse_iso_ms(text(at(doc, &["as_of"])));
    let norm = items(at(doc, &["signals"])).iter().map(|s| normalize(s, as_of)).collect::<Result<Vec<_>, _>>()?;
    let sel = select(norm);
    let w: Vec<f64> = sel.iter().map(|s| s.weight).collect();
    let total: f64 = w.iter().fold(0.0, |a, b| a + b);
    let lmean = |vals: &[f64]| {
        let acc = vals.iter().enumerate().fold(0.0, |a, (i, v)| a + w[i] * js::max(*v, 0.01).ln());
        (acc / total).exp()
    };
    let bases: Vec<f64> = sel.iter().map(|s| s.mrr.base).collect();
    let base = lmean(&bases);
    let cl = lmean(&sel.iter().map(|s| s.mrr.low).collect::<Vec<_>>());
    let ch = lmean(&sel.iter().map(|s| s.mrr.high).collect::<Vec<_>>());
    let spread = bases.iter().enumerate().fold(0.0, |a, (i, v)| a + w[i] * js::pow(v.ln() - base.ln(), 2.0));
    let disp = js::max(0.0, spread / total).sqrt();
    let f = js::min(10f64.ln(), 1.25 * disp).exp();
    let low = js::min(cl, base / f);
    let high = js::max(ch, base * f);
    let status = status_of(&sel);
    let dims = dimensions(&sel, at(doc, &["funding"]));
    let quality = js::min(1.0, total / 3.5);
    let indep = js::min(1.0, sel.len() as f64 / 4.0);
    let cov = js::min(1.0, dims as f64 / 6.0);
    let recency = sel.iter().fold(0.0, |a, s| a + s.weight * s.freshness) / total;
    let agree = js::max(0.0, 1.0 - disp / 4f64.ln());
    let raw_score = js::round(100.0 * (0.35 * quality + 0.2 * indep + 0.2 * cov + 0.15 * recency + 0.1 * agree));
    let cap = match status {
        "estimated" => 74.0,
        "disclosed" => 86.0,
        _ => 96.0,
    };
    let score = js::min(raw_score, cap);
    let label = if score >= 80.0 {
        "high"
    } else if score >= 60.0 {
        "medium"
    } else if score >= 40.0 {
        "low"
    } else {
        "very low"
    };
    let top = bases.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let bottom = bases.iter().copied().fold(f64::INFINITY, f64::min);
    Ok(Estimate { low, base, high, status, score, label, contradiction: top / bottom >= 3.0, signals: sel })
}

/// `verified`, `disclosed` or `estimated`, from the strongest selected evidence.
fn status_of(sel: &[Signal]) -> &'static str {
    let verified = sel.iter().any(|s| s.metric == "mrr" && matches!(s.tier.as_str(), "payment_provider" | "third_party_verified") && s.freshness == 1.0);
    if verified {
        return "verified";
    }
    let disclosed = sel.iter().any(|s| matches!(s.metric.as_str(), "mrr" | "arr") && matches!(s.tier.as_str(), "regulatory_filing" | "company_disclosure") && s.freshness >= 0.75);
    if disclosed { "disclosed" } else { "estimated" }
}

/// Distinct dimensions of the selected signals and their supporting sources, plus funding.
fn dimensions(sel: &[Signal], funding: &Value) -> usize {
    let mut dims: HashSet<Option<String>> = HashSet::new();
    for s in sel {
        dims.insert(s.dimension.clone());
        for x in &s.supporting {
            dims.insert(optional_text(at(x, &["dimension"])));
        }
    }
    if !items(funding).is_empty() {
        dims.insert(Some("funding".to_string()));
    }
    dims.len()
}
