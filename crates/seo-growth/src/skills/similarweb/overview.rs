//! Website overview from Similarweb Pro (`pro.similarweb.com`): engagement, monthly visits, channel mix, top
//! countries and similar sites, and the per-site snapshot the competitor report reads.

use super::client::request_json;
use serde_json::{Map, Value, json};

const SIM_ORIGIN: &str = "https://pro.similarweb.com";
/// Recorded as `source` in a snapshot, and as the publisher of the figures built on it.
pub const SOURCE: &str = "Similarweb Pro via pro.similarweb.com";

/// `TrafficSourcesOverview` channel names and the share keys the competitor report reads.
const CHANNELS: &[(&str, &str)] = &[
    ("Direct", "direct"), ("Search - Organic", "searchOrganic"), ("Search - Paid", "searchPaid"),
    ("Social - Organic", "socialOrganic"), ("Social - Paid", "socialPaid"), ("Referrals", "referrals"),
    ("Email", "mail"), ("Display Ads", "displayAds"), ("Gen AI", "genAi"), ("Affiliates", "affiliate"),
];

/// The first `limit` rows of a `GeographyExtended` table, already in share order. `Data[].Country` is a
/// numeric id; `Filters.country[]` maps it to a name and an `icon` of `flag flag-<alpha-2>`.
fn top_countries(geo: &Value, limit: usize) -> Vec<Value> {
    let names = geo["Filters"]["country"].as_array().cloned().unwrap_or_default();
    geo["Data"].as_array().into_iter().flatten().take(limit).map(|row| {
        let id = row["Country"].as_i64().unwrap_or_default().to_string();
        let meta = names.iter().find(|c| c["id"].as_str() == Some(id.as_str()));
        let code = meta.and_then(|c| c["icon"].as_str()).and_then(|i| i.rsplit("flag-").next()).unwrap_or_default().to_uppercase();
        json!({"countryCode": code, "countryName": meta.map_or(Value::Null, |c| c["text"].clone()), "share": row["Share"]})
    }).collect()
}

/// One site over the monthly window `from`..`to` (`YYYY%7CMM%7CDD`). Only the engagement call fails the
/// whole overview; the others come back `null`.
pub fn fetch_overview(domain: &str, country: u32, from: &str, to: &str, auto_sync: bool) -> Result<Value, String> {
    let page = format!("https://pro.similarweb.com/#/digitalsuite/websiteanalysis/overview/website-performance/*/999/3m?webSource=Total&key={domain}");
    let window = format!("country={country}&from={from}&to={to}&isWindow=false&includeSubDomains=true&timeGranularity=Monthly&keys={domain}");
    let eng_url = format!("{SIM_ORIGIN}/widgetApi/WebsiteOverview/EngagementOverview/Table?{window}&webSource=Total&ignoreFilterConsistency=false&ShouldGetVerifiedData=false");
    let graph_url = format!("{SIM_ORIGIN}/widgetApi/WebsiteOverview/EngagementVisits/Graph?{window}&webSource=Total&ShouldGetVerifiedData=false");
    let mix_url = format!("{SIM_ORIGIN}/widgetApi/MarketingMixTotal/TrafficSourcesOverview/PieChart?{window}");
    let geo_url = format!("{SIM_ORIGIN}/widgetApi/WebsiteGeographyExtended/GeographyExtended/Table?{window}&webSource=Total&orderBy=TotalShare%20desc&page=1&pageSize=5");
    let sim_url = format!("{SIM_ORIGIN}/api/WebsiteOverview/getsimilarsites?key={domain}&limit=10&country={country}&webSource=Total");
    let engagement = request_json("GET", &eng_url, &page, auto_sync)?;
    let rest = |url: &str| request_json("GET", url, &page, false).unwrap_or(Value::Null);
    let (visits_graph, traffic_sources, geo, similar_sites) = (rest(&graph_url), rest(&mix_url), rest(&geo_url), rest(&sim_url));
    Ok(json!({
        "domain": domain,
        "country": country,
        "from": from.replace("%7C", "-"),
        "to": to.replace("%7C", "-"),
        "engagement": engagement["Data"].get(0).cloned().unwrap_or(Value::Null),
        "monthlyVisits": visits_graph["Data"][domain]["Total"].get(0).cloned().unwrap_or(visits_graph["Data"].clone()),
        "trafficSources": traffic_sources["Data"]["Total"][domain],
        "topCountries": top_countries(&geo, 5),
        "similarSites": similar_sites,
    }))
}

/// An overview as `<run>/raw/sw/sw_<domain>.json` of a competitor report: `monthlyVisits` is the last
/// month of the window (the window's monthly average when the series is missing) and `trafficSources`
/// each channel's share of the window's visits.
pub fn site_snapshot(overview: &Value) -> Value {
    let months: Vec<Value> = overview["monthlyVisits"].as_array().into_iter().flatten()
        .filter_map(|m| Some(json!({"month": m["Key"], "visits": m["Value"].as_f64()?.round()}))).collect();
    let engagement = &overview["engagement"];
    let visits = months.last().map(|m| m["visits"].clone())
        .or_else(|| engagement["AvgMonthVisits"].as_f64().map(|v| json!(v.round()))).unwrap_or(Value::Null);
    let mix = &overview["trafficSources"];
    let total: f64 = CHANNELS.iter().filter_map(|(name, _)| mix[*name].as_f64()).sum();
    let sources: Map<String, Value> = CHANNELS.iter()
        .map(|(name, key)| ((*key).to_string(), mix[*name].as_f64().filter(|_| total > 0.0).map_or(Value::Null, |v| json!(v / total)))).collect();
    json!({
        "found": !visits.is_null(),
        "source": SOURCE,
        "data": {
            "domain": overview["domain"],
            "window": {"from": overview["from"], "to": overview["to"]},
            "monthlyVisits": visits,
            "visitsHistory": months,
            "bounceRate": engagement["BounceRate"],
            "pagesPerVisit": engagement["PagesPerVisit"],
            "avgVisitDurationSeconds": engagement["AvgVisitDuration"],
            "uniqueUsers": engagement["UniqueUsers"],
            "trafficSources": sources,
            "topCountries": overview["topCountries"],
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_keeps_the_last_month_channel_shares_and_country_codes() {
        // Shapes of the 2026-11-10 `attio.com` responses, values shortened.
        let geo = json!({
            "Data": [{"Country": 840, "Share": 0.24}, {"Country": 764, "Share": 0.16}],
            "Filters": {"country": [{"id": "764", "text": "Thailand", "icon": "flag flag-th"}, {"id": "840", "text": "United States", "icon": "flag flag-us"}]},
        });
        let overview = json!({
            "domain": "attio.com", "from": "2026-08-01", "to": "2026-10-31",
            "engagement": {"AvgMonthVisits": 1662797.5, "BounceRate": 0.38},
            "monthlyVisits": [{"Key": "2026-09-01", "Value": 1724792.7}, {"Key": "2026-10-01", "Value": 1653556.1}],
            "trafficSources": {"Direct": 600.0, "Search - Organic": 300.0, "Search - Paid": 100.0},
            "topCountries": top_countries(&geo, 5),
        });
        let snap = site_snapshot(&overview);
        assert_eq!(snap["source"], SOURCE);
        assert_eq!(snap["data"]["monthlyVisits"], 1653556.0);
        assert_eq!(snap["data"]["trafficSources"]["searchOrganic"], 0.3);
        assert_eq!(snap["data"]["trafficSources"]["searchPaid"], 0.1);
        assert!(snap["data"]["trafficSources"]["genAi"].is_null());
        assert_eq!(snap["data"]["topCountries"][0], json!({"countryCode": "US", "countryName": "United States", "share": 0.24}));
        assert_eq!(snap["data"]["topCountries"][1]["countryCode"], "TH");
    }

    #[test]
    fn snapshot_without_a_monthly_series_falls_back_to_the_window_average() {
        let snap = site_snapshot(&json!({"engagement": {"AvgMonthVisits": 1200.4}, "monthlyVisits": null, "trafficSources": null, "topCountries": []}));
        assert_eq!(snap["data"]["monthlyVisits"], 1200.0);
        assert_eq!(snap["found"], true);
        assert!(snap["data"]["trafficSources"]["direct"].is_null());
    }
}
