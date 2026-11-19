//! The geo-test question bank as the AI recommendation watch reads it: unbranded questions of one market, in file
//! order, rotated a few per hour. Each question carries a `bucket` (report geo-optimization-20261112, founder
//! 2026-11-12): `tool` questions ask for a product and form the headline rate; `advice` questions ask how to handle
//! a situation and `dating` questions are a separate track, so neither dilutes the headline rate.

use serde::Deserialize;

/// The bucket of a question without one: the questions asked before buckets existed all asked for a tool.
pub const TOOL: &str = "tool";

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Question {
    pub id: String,
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub market: String,
    #[serde(default)]
    pub branded: bool,
    #[serde(default)]
    pub bucket: String,
    pub question: String,
}

impl Question {
    pub fn bucket(&self) -> &str {
        if self.bucket.is_empty() { TOOL } else { &self.bucket }
    }
}

/// Unbranded questions of `market` (`global`, `cn`): the denominator of "does an engine recommend Mars unprompted".
pub fn read_bank(path: &str, market: &str) -> Result<Vec<Question>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    Ok(text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str::<Question>(l).ok())
        .filter(|q| !q.branded && q.market == market).collect())
}

/// The next `n` questions after `cursor`, wrapping; the new cursor.
pub fn rotate(bank: &[Question], cursor: usize, n: usize) -> (Vec<Question>, usize) {
    if bank.is_empty() { return (Vec::new(), 0); }
    let take = n.min(bank.len());
    ((0..take).map(|i| bank[(cursor + i) % bank.len()].clone()).collect(), (cursor + take) % bank.len())
}
