//! Official list prices of Mars CRM models, read from their owner at analysis time:
//! `mars-crm-commerce/src/official_model_prices.rs` (US$ per 1M tokens, dated bands, long-context tiers).
//! The credit rule mirrors `pricing.rs` `credits_for`: cost x 3 markup x 100 credits per US$, rounded up,
//! at least one credit when the call used any token. Reasoning tokens bill at the output rate.
//! A model the table does not list is unpriced (`None`), never free.
use anyhow::{Context, Result};
use regex::Regex;
use std::{collections::BTreeMap, sync::OnceLock};

pub(super) const SOURCE: &str = "/path/to/workspace/Mars_CRM/rust/crates/mars-crm-commerce/src/official_model_prices.rs";
const CREDITS_PER_USD_BILLED: f64 = 300.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Rates { pub(super) input: f64, pub(super) cached: f64, pub(super) output: f64 }
struct Band { from: Option<String>, rates: Rates, long: Option<(f64, Rates)> }
struct Price { bands: Vec<Band>, image_output: Option<f64> }
pub(super) struct Prices { models: BTreeMap<String, Price>, pub(super) source: String }

/// One call's list cost by component, in US$.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Cost { pub(super) uncached_input: f64, pub(super) cached_input: f64, pub(super) output: f64, pub(super) reasoning: f64 }
impl Cost { pub(super) fn total(&self) -> f64 { self.uncached_input + self.cached_input + self.output + self.reasoning } }

/// The table this process reads once; `Err` keeps the reason so every summary can show it.
pub(super) fn current() -> &'static Result<Prices, String> {
    static PRICES: OnceLock<Result<Prices, String>> = OnceLock::new();
    PRICES.get_or_init(|| {
        if cfg!(test) { return parse(FIXTURE, "fixture").map_err(|e| format!("{e:#}")); }
        std::fs::read_to_string(SOURCE).context(SOURCE).and_then(|text| parse(&text, SOURCE)).map_err(|e| format!("{e:#}"))
    })
}

fn rates(text: &str) -> Option<Rates> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"TokenRates \{ input: ([\d.]+), cached_input: ([\d.]+), output: ([\d.]+)").unwrap());
    let c = re.captures(text)?;
    Some(Rates { input: c[1].parse().ok()?, cached: c[2].parse().ok()?, output: c[3].parse().ok()? })
}

fn bands(text: &str) -> Vec<Band> {
    let from = Regex::new(r#"from: (?:None|Some\("([^"]+)"\))"#).unwrap();
    let long = Regex::new(r"threshold_tokens: ([\d_.]+),").unwrap();
    text.split("PriceBand {").skip(1).filter_map(|piece| {
        let rates_at = piece.find("TokenRates {")?;
        let base = rates(&piece[rates_at..])?;
        let tier = long.captures(piece).and_then(|c| {
            let at = c.get(0)?.end();
            Some((c[1].replace('_', "").parse().ok()?, rates(&piece[at..])?))
        });
        Some(Band { from: from.captures(piece).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()), rates: base, long: tier })
    }).collect()
}

pub(super) fn parse(text: &str, source: &str) -> Result<Prices> {
    let consts = Regex::new(r"(?s)const (\w+): &\[PriceBand\] = &\[(.*?)\];\s*\n").unwrap();
    let named: BTreeMap<&str, &str> = consts.captures_iter(text).map(|c| (c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str())).collect();
    let start = text.find("OFFICIAL_MODEL_PRICES").context("no OFFICIAL_MODEL_PRICES table")?;
    let table = &text[start..start + text[start..].find("\n];").context("OFFICIAL_MODEL_PRICES is not closed")?];
    let entry = Regex::new(r#"\(\s*"([^"]+)","#).unwrap();
    let starts: Vec<(usize, String)> = entry.captures_iter(table).map(|c| (c.get(0).unwrap().start(), c[1].to_string())).collect();
    let reference = Regex::new(r"(?:google|openai)\((\w+)").unwrap();
    let image = Regex::new(r"image_output: Some\(([\d.]+)\)").unwrap();
    let mut models = BTreeMap::new();
    for (i, (at, name)) in starts.iter().enumerate() {
        let body = &table[*at..starts.get(i + 1).map_or(table.len(), |n| n.0)];
        let own = bands(body);
        let list = if own.is_empty() {
            let r = reference.captures(body).with_context(|| format!("{name}: no bands"))?;
            bands(named.get(&r[1]).with_context(|| format!("{name}: unknown band list {}", &r[1]))?)
        } else { own };
        anyhow::ensure!(!list.is_empty(), "{name}: no parsable band");
        models.insert(name.clone(), Price { bands: list, image_output: image.captures(body).and_then(|c| c[1].parse().ok()) });
    }
    anyhow::ensure!(!models.is_empty(), "OFFICIAL_MODEL_PRICES has no entry");
    Ok(Prices { models, source: source.to_string() })
}

/// `gemini-3.8-flash`, `models/gemini-3.8-flash` and `gemini-3.8-flash-2026-09-01` are one price.
fn normalize(model: &str) -> String {
    let lower = model.trim().to_lowercase();
    let name = lower.strip_prefix("models/").unwrap_or(&lower);
    let b = name.as_bytes();
    let dated = b.len() >= 11 && b[b.len() - 11..].iter().enumerate().all(|(i, c)| if matches!(i, 0 | 5 | 8) { *c == b'-' } else { c.is_ascii_digit() });
    if dated { name[..name.len() - 11].to_string() } else { name.to_string() }
}

/// `YYYY-MM-DD` of epoch ms (UTC).
pub(super) fn day(at_ms: f64) -> String {
    let z = (at_ms / 86_400_000.0).floor() as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    format!("{:04}-{:02}-{:02}", yoe + era * 400 + i64::from(m <= 2), m, d)
}

impl Prices {
    fn band(&self, model: &str, on: &str) -> Option<(&Price, &Band)> {
        let price = self.models.get(&normalize(model))?;
        let band = price.bands.iter().rev().find(|b| b.from.as_deref().is_none_or(|f| f <= on)).unwrap_or(&price.bands[0]);
        Some((price, band))
    }

    /// The rates one call of `input` tokens billed at `at_ms`.
    pub(super) fn rates(&self, model: &str, input: f64, at_ms: f64) -> Option<Rates> {
        let (_, band) = self.band(model, &day(at_ms))?;
        Some(match band.long { Some((threshold, long)) if input > threshold => long, _ => band.rates })
    }

    /// One call's list cost; `on` overrides the billing day (a coming band's projection).
    pub(super) fn cost(&self, model: &str, input: f64, cached: f64, output: f64, reasoning: f64, at_ms: f64, on: Option<&str>) -> Option<Cost> {
        let today = day(at_ms);
        let (price, band) = self.band(model, on.unwrap_or(&today))?;
        let r = match band.long { Some((threshold, long)) if input > threshold => long, _ => band.rates };
        let out = price.image_output.unwrap_or(r.output);
        let cached = cached.min(input).max(0.0);
        Some(Cost { uncached_input: (input - cached) * r.input / 1e6, cached_input: cached * r.cached / 1e6, output: output * out / 1e6, reasoning: reasoning * out / 1e6 })
    }

    /// Credits `pricing.rs` would charge for this cost (tool fees excluded).
    pub(super) fn credits(cost: &Cost, tokens: f64) -> f64 {
        if tokens <= 0.0 { 0.0 } else { (cost.total() * CREDITS_PER_USD_BILLED - 1e-9).ceil().max(1.0) }
    }

    /// The next dated band of `model` after `at_ms`, within `days`.
    pub(super) fn upcoming(&self, model: &str, at_ms: f64, days: f64) -> Option<String> {
        let (today, until) = (day(at_ms), day(at_ms + days * 86_400_000.0));
        self.models.get(&normalize(model))?.bands.iter().filter_map(|b| b.from.clone()).find(|f| *f > today && *f <= until)
    }
}

/// The table's shape with the prices of 2026-11-12, for hermetic tests.
#[cfg(test)]
pub(super) const FIXTURE: &str = r#"
const FLASH: &[PriceBand] = &[
    PriceBand { from: None, rates: TokenRates { input: 0.75, cached_input: 0.075, output: 3.75, cache_write: None }, long_context: None },
    PriceBand { from: Some("2027-01-01"), rates: TokenRates { input: 1.5, cached_input: 0.15, output: 7.5, cache_write: None }, long_context: None },
];
const GEMINI_4: &[PriceBand] = &[
    PriceBand { from: None, rates: TokenRates { input: 2.0, cached_input: 0.1, output: 10.0, cache_write: None }, long_context: None },
    PriceBand { from: Some("2026-12-01"), rates: TokenRates { input: 4.0, cached_input: 0.2, output: 20.0, cache_write: None }, long_context: None },
];
const PRO: &[PriceBand] = &[PriceBand {
    from: None,
    rates: TokenRates { input: 2.0, cached_input: 0.2, output: 12.0, cache_write: None },
    long_context: Some(LongContext {
        threshold_tokens: 200_000.0,
        rates: TokenRates { input: 4.0, cached_input: 0.4, output: 18.0, cache_write: None },
    }),
}];
pub const OFFICIAL_MODEL_PRICES: &[(&str, OfficialModelPrice)] = &[
    ("gemini-4-argon", google(GEMINI_4)),
    ("gemini-3.8-flash", google(FLASH)),
    ("gemini-3.1-pro-preview", google(PRO)),
    ("gemini-3.5-flash-lite", google(&[PriceBand {
        from: None,
        rates: TokenRates { input: 0.3, cached_input: 0.3, output: 2.5, cache_write: None },
        long_context: None,
    }])),
    ("gemini-3.1-flash-image", OfficialModelPrice {
        image_output: Some(60.0),
        bands: &[PriceBand { from: None, rates: TokenRates { input: 0.5, cached_input: 0.5, output: 3.0, cache_write: None }, long_context: None }],
    }),
];
"#;
#[cfg(not(test))]
const FIXTURE: &str = "";
