//! What the reviews read from the keyword pool: the `cpcPool` block of `analysis.json`, the watch page section and
//! two Google Ads Editor CSVs that `scripts/ads-bulk.js` can preview and apply (`proposals/<date>-add.csv` for new
//! exact words, `proposals/<date>-pause.csv` for dead or wasting ones). Nothing here changes the account.

use super::cpc_pool::{Entry, Pool, PoolConfig};
use super::cpc_pool_judge::{DEAD, GOLDEN, CHEAP_CONVERSION, PLANNER_CHEAP, TERM_CONVERTS, TERM_GOLDEN, TERM_NEGATIVE, WASTE};
use super::files::write_text;
use serde_json::{Value, json};

/// Untested cheap candidates proposed per day, best volume first; tested words are always proposed.
const MAX_PLANNER_PROPOSALS: usize = 10;

fn volume(e: &Entry) -> f64 { e.markets.values().map(|m| m.volume).sum() }
fn bid_high(e: &Entry) -> f64 { e.markets.values().map(|m| m.bid_high).fold(0.0, f64::max) }
fn cjk(word: &str) -> bool { word.chars().any(|c| ('\u{2e80}'..='\u{9fff}').contains(&c)) }
fn csv(s: &str) -> String { format!("\"{}\"", s.replace('"', "\"\"")) }

fn row(word: &str, e: &Entry) -> Value {
    let w = &e.window;
    json!({"word": word, "status": e.status, "theme": e.theme, "source": e.source, "flags": e.flags, "since": e.flagged_since,
        "campaign": e.slots.first().map(|s| s.campaign.clone()), "adGroup": e.slots.first().map(|s| s.ad_group.clone()),
        "impressions": w.impressions, "clicks": w.clicks, "cost": (w.cost * 100.0).round() / 100.0, "conversions": w.conversions,
        "cpc": w.cpc().map(|c| (c * 100.0).round() / 100.0), "ctr": w.ctr().map(|c| (c * 1000.0).round() / 1000.0),
        "plannerVolume": volume(e), "plannerBidHigh": bid_high(e)})
}

/// Exact-match rows for words worth adding, placed by theme and script (`en` / `zh`) through `targets`.
pub fn add_rows(pool: &Pool, config: &PoolConfig) -> Vec<(String, String, String, String)> {
    let tested = |e: &Entry| e.flags.iter().any(|f| f == TERM_GOLDEN || f == TERM_CONVERTS);
    let mut cheap: Vec<(&String, &Entry)> = pool.words.iter().filter(|(_, e)| e.flags.iter().any(|f| f == PLANNER_CHEAP)).collect();
    cheap.sort_by(|a, b| volume(b.1).total_cmp(&volume(a.1)));
    let picked = pool.words.iter().filter(|(_, e)| tested(e)).chain(cheap.into_iter().take(MAX_PLANNER_PROPOSALS));
    picked.filter_map(|(word, e)| {
        let lang = if cjk(word) { "zh" } else { "en" };
        let (campaign, group, url) = config.targets.get(&e.theme)?.get(lang)?;
        Some((campaign.clone(), group.clone(), word.clone(), url.clone()))
    }).collect()
}

/// Pause rows for every slot of a live word that is dead or wasting money.
pub fn pause_rows(pool: &Pool) -> Vec<(String, String, String, String)> {
    pool.words.iter().filter(|(_, e)| e.status == "live" && e.flags.iter().any(|f| f == DEAD || f == WASTE))
        .flat_map(|(word, e)| e.slots.iter().map(move |s| (s.campaign.clone(), s.ad_group.clone(), word.clone(), s.match_type.replace(" match", ""))))
        .collect()
}

/// Writes one proposal CSV named `<day>-<kind>-<hash of its rows>.csv`: the Ads uploads list finds a file by name, so
/// each distinct set of rows gets its own name and is previewed once (`cpc_pool_preview`).
fn proposal(dir: &str, day: &str, kind: &str, header: &str, rows: &[String]) -> Result<Option<String>, String> {
    if rows.is_empty() { return Ok(None); }
    let path = format!("{dir}/{day}-{kind}-{}.csv", super::cpc_pool_expand::tag(rows));
    write_text(&path, &format!("{header}\n{}\n", rows.join("\n")))?;
    Ok(Some(path))
}

fn write_proposals(state_dir: &str, day: &str, pool: &Pool, config: &PoolConfig) -> Result<(Option<String>, Option<String>), String> {
    let dir = format!("{state_dir}/cpc-pool/proposals");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let add: Vec<String> = add_rows(pool, config).iter().map(|(c, g, k, u)| [csv(c), csv(g), csv(k), csv("Exact"), csv("Enabled"), csv(u)].join(",")).collect();
    let pause: Vec<String> = pause_rows(pool).iter().map(|(c, g, k, m)| [csv(c), csv(g), csv(k), csv(m), csv("Paused")].join(",")).collect();
    Ok((proposal(&dir, day, "add", "Campaign,Ad Group,Keyword,Criterion Type,Status,Final URL", &add)?,
        proposal(&dir, day, "pause", "Campaign,Ad Group,Keyword,Criterion Type,Status", &pause)?))
}

/// The `cpcPool` block of `analysis.json`; also writes the day's proposal files.
pub fn summary(pool: &Pool, config: &PoolConfig, state_dir: &str, day: &str, expand_note: &str, errors: &mut Vec<String>) -> Value {
    let count = |status: &str| pool.words.values().filter(|e| e.status == status).count();
    let mut flagged: Vec<Value> = pool.words.iter().filter(|(_, e)| !e.flags.is_empty()).map(|(w, e)| row(w, e)).collect();
    let order = [GOLDEN, CHEAP_CONVERSION, TERM_CONVERTS, TERM_GOLDEN, WASTE, TERM_NEGATIVE, DEAD];
    let first = |v: &Value| v["flags"].as_array().and_then(|f| f.iter().filter_map(|f| order.iter().position(|o| f == o)).min()).unwrap_or(order.len());
    flagged.sort_by_key(first);
    let new: Vec<&Value> = flagged.iter().filter(|v| v["since"].as_object().is_some_and(|s| s.values().any(|d| d == day))).collect();
    let negatives: Vec<&String> = pool.words.iter().filter(|(_, e)| e.flags.iter().any(|f| f == TERM_NEGATIVE)).map(|(w, _)| w).collect();
    let (add, pause) = write_proposals(state_dir, day, pool, config).unwrap_or_else(|error| { errors.push(format!("cpc pool proposals: {error}")); (None, None) });
    json!({"pool": super::cpc_pool::path(state_dir), "counts": {"live": count("live"), "searchTerm": count("search-term"), "candidate": count("candidate"), "blocked": count("blocked")},
        "expand": expand_note, "lastExpandDay": pool.last_expand_day, "flagged": flagged.len(), "newFlags": new, "flaggedRows": flagged.iter().take(40).collect::<Vec<_>>(),
        "proposals": {"add": add, "pause": pause, "negatives": negatives,
            "addRows": add_rows(pool, config).iter().map(|(c, g, k, _)| json!({"campaign": c, "adGroup": g, "keyword": k})).collect::<Vec<_>>(),
            "pauseRows": pause_rows(pool).iter().map(|(c, g, k, m)| json!({"campaign": c, "adGroup": g, "keyword": k, "match": m})).collect::<Vec<_>>()}})
}

/// `5700.0` → `5700`, `0.5` → `0.5`, `-0.0` (an empty f64 sum) → `0`.
fn plain(v: &Value) -> String {
    v.as_f64().map_or_else(|| v.to_string(), |n| if n == 0.0 { "0".into() } else if n.fract() == 0.0 { format!("{n:.0}") } else { format!("{n}") })
}

/// The watch page section: counts, then each word whose flag appeared today.
pub fn section(summary: Option<&Value>) -> String {
    let Some(s) = summary else { return String::new() };
    let c = &s["counts"];
    let mut out = format!("## CPC 关键词池\n\n在投 {} 个词，真实搜索词 {} 个，候选 {} 个，排除 {} 个；带标记 {} 个。{}\n\n",
        c["live"], c["searchTerm"], c["candidate"], c["blocked"], s["flagged"], s["expand"].as_str().unwrap_or(""));
    let new = s["newFlags"].as_array().cloned().unwrap_or_default();
    if new.is_empty() { out.push_str("今天没有新出现的异常词。\n\n"); }
    else {
        out.push_str("| 词 | 标记 | 展示 | 点击 | 花费 | CPC | 转化 | 规划师月搜索量 |\n|---|---|---|---|---|---|---|---|\n");
        for v in new.iter().take(15) {
            let flags: Vec<&str> = v["flags"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            out.push_str(&format!("| {} | {} | {} | {} | ${} | {} | {} | {} |\n", v["word"].as_str().unwrap_or(""), flags.join("；"), plain(&v["impressions"]), plain(&v["clicks"]), plain(&v["cost"]),
                v["cpc"].as_f64().map_or("—".into(), |c| format!("${c}")), plain(&v["conversions"]), plain(&v["plannerVolume"])));
        }
        out.push('\n');
    }
    out.push_str(&super::cpc_pool_preview::section(&s["proposals"]));
    out
}

/// One watchdog run: seed, the run's Ads tables, flags, the daily planner refresh and ideas, flags again, persist.
pub fn run(config: Option<&PoolConfig>, ads: Option<&Value>, state_dir: &str, now: chrono::DateTime<chrono::FixedOffset>, dry_run: bool, errors: &mut Vec<String>) -> Option<Value> {
    let config = config?;
    let day = now.format("%Y-%m-%d").to_string();
    let mut pool = super::cpc_pool::read(state_dir);
    match super::files::read_json(&config.seed) {
        Ok(seed) => super::cpc_pool::merge_seed(&mut pool, config, &seed, &day),
        Err(error) => errors.push(format!("cpc pool seed: {error}")),
    }
    let tabs = ads.map(|a| a["tabs"].clone()).unwrap_or(Value::Null);
    if ads.is_some_and(|a| a["error"].is_null() && a["skipped"].is_null()) { errors.extend(super::cpc_pool::merge_ads(&mut pool, config, &tabs, &day)); }
    if super::cpc_pool::read_both(&tabs, "search_terms") { pool.last_terms_read = Some(now.to_rfc3339()); }
    super::cpc_pool_judge::judge(&mut pool, &tabs, &config.thresholds, now.date_naive());
    let note = super::cpc_pool_expand::daily(&mut pool, config, state_dir, &day, dry_run, errors);
    super::cpc_pool_judge::judge(&mut pool, &tabs, &config.thresholds, now.date_naive());
    if !dry_run && let Err(error) = super::cpc_pool::write(state_dir, &pool) { errors.push(format!("cpc pool: {error}")); }
    Some(summary(&pool, config, state_dir, &day, &note, errors))
}
