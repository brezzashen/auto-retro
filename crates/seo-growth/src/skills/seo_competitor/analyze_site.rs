//! `.claude/skills/seo-competitor/scripts/analyze.ts` `siteOf` and `keywordRows`: one site's US
//! keyword rows, keyword shares, top pages and Similarweb figures.

use super::analyze::Ctx;
use super::files::{at, items, num_or, read_json, show};
use super::metrics::{WINNABLE_KD, Growth, growth_of};
use super::monid::first;
use crate::js_wire::js;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Keyword {
    pub keyword: String,
    pub volume: f64,
    pub kd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Value>,
    pub etv: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<Value>,
    pub group: &'static str,
}

#[derive(Serialize, Clone, Debug)]
pub struct Shares {
    pub total: f64,
    pub brand: f64,
    pub winnable: f64,
    pub hard: f64,
}

#[derive(Serialize, Clone, Debug)]
pub struct Page {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<Value>,
    pub etv: f64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    pub domain: String,
    pub url: String,
    pub visits: f64,
    pub organic_share: Value,
    pub paid_share: Value,
    pub top_countries: Vec<String>,
    pub us_etv: f64,
    pub us_keywords: f64,
    pub top10: f64,
    pub rank: Value,
    pub referring_domains: Value,
    pub growth: Growth,
    pub shares: Shares,
    pub keywords: Vec<Keyword>,
    pub pages: Vec<Page>,
}

/// One keyword row: brand when it contains a brand substring, winnable when KD is at most 20.
fn keyword_of(it: &Value, brand: &[String]) -> Keyword {
    let kd = at(it, &["keyword_data", "keyword_properties", "keyword_difficulty"]).as_f64();
    let serp = at(it, &["ranked_serp_element", "serp_item"]);
    let keyword = show(at(it, &["keyword_data", "keyword"]));
    let is_brand = brand.iter().any(|b| keyword.contains(b.as_str()));
    let group = if is_brand {
        "brand"
    } else if kd.is_some_and(|k| k <= WINNABLE_KD) {
        "winnable"
    } else {
        "hard"
    };
    Keyword {
        volume: num_or(at(it, &["keyword_data", "keyword_info", "search_volume"]), 0.0),
        kd,
        position: serp.get("rank_group").cloned(),
        etv: js::round(num_or(at(serp, &["etv"]), 0.0)),
        url: serp.get("url").cloned(),
        keyword,
        group,
    }
}

/// `keywordRows(domain, brand)`: the rows of `rk_<domain>.json` (none when the file is absent) and
/// the `first()` of that file.
fn keyword_rows(ctx: &Ctx, domain: &str, brand: &[String]) -> Result<(Value, Vec<Keyword>), String> {
    let path = format!("{}/rk_{domain}.json", ctx.raw);
    let rk = if std::path::Path::new(&path).exists() { first(&read_json(&path)?).clone() } else { serde_json::json!({}) };
    let rows = items(at(&rk, &["items"])).iter().map(|it| keyword_of(it, brand)).collect();
    Ok((rk, rows))
}

/// Share of each group in the site's total estimated traffic (the total is at least 1).
fn shares(rows: &[Keyword]) -> Shares {
    let sum = |group: Option<&str>| rows.iter().filter(|r| group.is_none_or(|g| r.group == g)).fold(0.0, |acc, r| acc + r.etv);
    let total = js::max(1.0, sum(None));
    Shares { total: sum(None), brand: sum(Some("brand")) / total, winnable: sum(Some("winnable")) / total, hard: sum(Some("hard")) / total }
}

/// The top five pages by estimated traffic, summed over the keyword rows that point at them.
fn top_pages(rows: &[Keyword]) -> Vec<Page> {
    let mut pages: Vec<(Option<Value>, f64)> = Vec::new();
    for r in rows {
        match pages.iter_mut().find(|(url, _)| *url == r.url) {
            Some(entry) => entry.1 += r.etv,
            None => pages.push((r.url.clone(), r.etv)),
        }
    }
    pages.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    pages.into_iter().take(5).map(|(url, etv)| Page { url, etv }).collect()
}

/// `siteOf(domain, brand)`: one site's record for analysis.json.
pub fn site_of(ctx: &Ctx, domain: &str, brand: &[String]) -> Result<Site, String> {
    let sw = at(&read_json(&format!("{}/sw/sw_{domain}.json", ctx.raw))?, &["data"]).clone();
    let (rk, rows) = keyword_rows(ctx, domain, brand)?;
    let organic = at(&rk, &["metrics", "organic"]);
    let top_countries = items(at(&sw, &["topCountries"]))
        .iter()
        .take(3)
        .map(|c| format!("{} {}%", show(at(c, &["countryCode"])), js::number_string(js::round(num_or(at(c, &["share"]), 0.0) * 100.0))))
        .collect();
    let history = ctx.history.get(domain).cloned().unwrap_or_default();
    Ok(Site {
        domain: domain.to_string(),
        url: format!("https://{domain}/"),
        visits: num_or(at(&sw, &["monthlyVisits"]), 0.0),
        organic_share: at(&sw, &["trafficSources", "searchOrganic"]).clone(),
        paid_share: at(&sw, &["trafficSources", "searchPaid"]).clone(),
        top_countries,
        us_etv: js::round(num_or(at(organic, &["etv"]), 0.0)),
        us_keywords: num_or(at(&rk, &["total_count"]), 0.0),
        top10: num_or(at(organic, &["pos_1"]), 0.0) + num_or(at(organic, &["pos_2_3"]), 0.0) + num_or(at(organic, &["pos_4_10"]), 0.0),
        rank: ctx.ranks.get(domain).cloned().unwrap_or(Value::Null),
        referring_domains: ctx.refdom.get(domain).cloned().unwrap_or(Value::Null),
        growth: growth_of(&history),
        shares: shares(&rows),
        pages: top_pages(&rows),
        keywords: rows.iter().take(15).cloned().collect(),
    })
}
