//! Local words of the ja, ko and th keyword pages (G11; Gefei, WeChat 2026-09-14): a translated word is
//! checked against the target country's first page and one shared Trends chart, a word with no curve stays
//! pending, and a confirmed word must survive every English copy rewrite.
//! `mars-skill-tools seo-competitor localeWords verify --locale <ja|ko|th> --page </path> --words "a|b" [--meanings "x|y"]`
//! writes the ledger `references/locale-words.json`; `localeWords check` lists confirmed words the locale copy lost.

use super::files::{at, fail, items, read_json, stringify, text, write_text};
use super::monid::{MonidCall, first, run_all};
use super::watch_analyze::Finding;
use super::watch_rank_serp::RankMarket;
use super::watch_trends_market::file_key;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const LEDGER: &str = ".claude/skills/seo-competitor/references/locale-words.json";
pub const COPY_DIR: &str = "Mars_CRM/web/src/screens/marketing/keywords";
const CONFIG: &str = ".claude/skills/seo-competitor/references/watch.config.json";
/// Both words of a comparison share one region and this range (Gefei compares over the past 12 months).
const RANGE: &str = "past_12_months";
/// Google Trends compares at most five search terms in one chart.
const MAX_WORDS: usize = 5;
/// Share of the chart's weeks that must carry searches for a word to have a curve (2026-11-10, Korea:
/// "카톡 답장" filled 44 of 52 weeks, four sibling phrasings 1 to 7). A chart of the weak words alone showed
/// the same weeks (7, 3, 5, 1), so one shared chart is enough.
const CURVE_WEEKS: f64 = 0.25;
pub const CONFIRMED: &str = "confirmed";
pub const PENDING: &str = "pending";
pub const RULE: &str = "ja/ko/th 页面的词先跑 localeWords verify（目标国 SERP 加同一张 Trends 图）核对当地说法，没有曲线的词记为待验证、不删；已确认的本地词不被英文改稿覆盖，改完跑 localeWords check";

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Word {
    pub locale: String,
    pub page: String,
    pub word: String,
    pub meaning: String,
    /// `confirmed` (it has a Trends curve in the target country) or `pending` (no curve yet).
    pub status: String,
    /// Mean Trends interest over the best word of the same comparison (0-1).
    pub share: f64,
    /// Share of the range's weeks with searches (0-1), in the shared chart or the word's own.
    #[serde(default)]
    pub weeks: f64,
    /// First-page results in the target country whose title holds the word.
    pub title_hits: usize,
    /// The strongest word of its comparison: the one for the title and H1.
    pub primary: bool,
    /// The page copy held the word at its last check; only an adopted word is guarded against a rewrite.
    #[serde(default)]
    pub adopted: bool,
    pub location_code: u32,
    pub range: String,
    pub checked_on: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Ledger { words: Vec<Word> }

/// What one comparison measured for a word.
pub struct Evidence {
    pub word: String,
    /// Mean interest and the share of weeks with searches in the shared chart.
    pub mean: f64,
    pub coverage: f64,
    pub title_hits: usize,
}

/// `(mean interest, share of weeks with searches)` of keyword #idx in one chart. Every complete week counts:
/// a week with no value for any word is a week without searches here (29 of 52 in the Thai chart), unlike
/// the pull gaps `watch_birth::curve` skips.
pub fn measure(chart: &Value, idx: usize) -> (f64, f64) {
    let rows: Vec<f64> = items(at(first(chart), &["items"])).iter().filter(|i| text(at(i, &["type"])) == "google_trends_graph")
        .flat_map(|i| items(at(i, &["data"])).iter()).filter(|p| !p["missing_data"].as_bool().unwrap_or(false))
        .map(|p| p["values"][idx].as_f64().unwrap_or(0.0)).collect();
    let n = rows.len().max(1) as f64;
    (rows.iter().sum::<f64>() / n, rows.iter().filter(|v| **v > 0.0).count() as f64 / n)
}

pub fn read(path: &str) -> Vec<Word> {
    read_json(path).ok().and_then(|v| serde_json::from_value::<Ledger>(v).ok()).unwrap_or_default().words
}

/// One Trends chart for every candidate as a search term, in the target country over `RANGE`.
pub fn trends_call(words: &[String], location: u32, out: String) -> MonidCall {
    MonidCall { provider: "dataforseo".into(), endpoint: "/keywords/google-trends-explore".into(), out,
        body: json!({"keywords": words, "location_code": location, "time_range": RANGE, "item_types": ["google_trends_graph"]}) }
}

/// The first page of one word with the target country as region and its language as result filter.
pub fn serp_call(word: &str, market: &RankMarket, out: String) -> MonidCall {
    MonidCall { provider: "dataforseo".into(), endpoint: "/serp/google-organic".into(), out,
        body: json!({"keyword": word, "location_code": market.location_code, "language_code": market.language_code, "depth": 10}) }
}

fn squeeze(s: &str) -> String { s.to_lowercase().split_whitespace().collect() }

/// Organic results of a first page whose title holds `word`, spaces ignored.
pub fn title_hits(serp: &Value, word: &str) -> usize {
    items(at(first(serp), &["items"])).iter().filter(|r| text(at(r, &["type"])) == "organic")
        .filter(|r| squeeze(text(at(r, &["title"]))).contains(&squeeze(word))).count()
}

pub fn has_curve(e: &Evidence) -> bool { e.coverage >= CURVE_WEEKS }

/// `(status, share, primary)` per word: a curve confirms it, no curve leaves it pending, never dropped.
/// The primary word is the confirmed one with the highest mean in the shared chart.
pub fn judge(evidence: &[Evidence]) -> Vec<(&'static str, f64, bool)> {
    let best = evidence.iter().map(|e| e.mean).fold(0.0, f64::max);
    let top = evidence.iter().position(|e| best > 0.0 && e.mean == best && has_curve(e));
    evidence.iter().enumerate().map(|(i, e)| {
        let share = if best > 0.0 { (e.mean / best * 100.0).round() / 100.0 } else { 0.0 };
        (if has_curve(e) { CONFIRMED } else { PENDING }, share, top == Some(i))
    }).collect()
}

/// Stores one checked word. A confirmed word is never set back to pending by a later empty curve, and
/// keeps its meaning when the new check names none; an adopted word stays adopted.
pub fn upsert(ledger: &mut Vec<Word>, new: Word) {
    let Some(old) = ledger.iter_mut().find(|w| w.locale == new.locale && w.page == new.page && squeeze(&w.word) == squeeze(&new.word)) else { ledger.push(new); return };
    if old.status == CONFIRMED && new.status == PENDING { return; }
    let meaning = if new.meaning.is_empty() { old.meaning.clone() } else { new.meaning.clone() };
    *old = Word { meaning, adopted: old.adopted || new.adopted, ..new };
}

/// The copy of one page inside a `keywordPages.<locale>.ts` table: from its `'<slug>': {` line to the next page.
pub fn page_block<'a>(source: &'a str, page: &str) -> Option<&'a str> {
    let start = source.find(&format!("\n  '{}': {{", page.trim_start_matches('/')))? + 1;
    let rest = &source[start..];
    let end = [rest[1..].find("\n  '"), rest.find("\n}")].into_iter().flatten().min().map_or(rest.len(), |e| e + 1);
    Some(&rest[..end])
}

fn in_copy(w: &Word, copy: &impl Fn(&str) -> Option<String>) -> bool {
    let source = copy(&w.locale).unwrap_or_default();
    page_block(&source, &w.page).is_some_and(|b| b.to_lowercase().contains(&w.word.to_lowercase()))
}

/// Adopted words the locale copy no longer holds; `copy` returns the copy file of a locale. A confirmed
/// phrasing the page never used is a spare candidate, not a loss.
pub fn missing<'a>(ledger: &'a [Word], copy: impl Fn(&str) -> Option<String>) -> Vec<&'a Word> {
    ledger.iter().filter(|w| w.status == CONFIRMED && w.adopted && !in_copy(w, &copy)).collect()
}

/// Marks every confirmed word the page copy holds as adopted; returns how many were newly marked.
pub fn adopt(ledger: &mut [Word], copy: impl Fn(&str) -> Option<String>) -> usize {
    let fresh: Vec<usize> = (0..ledger.len()).filter(|&i| ledger[i].status == CONFIRMED && !ledger[i].adopted && in_copy(&ledger[i], &copy)).collect();
    for &i in &fresh { ledger[i].adopted = true; }
    fresh.len()
}

fn save(path: &str, mut words: Vec<Word>) -> Result<(), String> {
    words.sort_by(|a, b| (&a.locale, &a.page, !a.primary, &a.word).cmp(&(&b.locale, &b.page, !b.primary, &b.word)));
    write_text(path, &format!("{}\n", serde_json::to_string_pretty(&Ledger { words }).map_err(|e| e.to_string())?))
}

fn copy_file(dir: &str) -> impl Fn(&str) -> Option<String> + '_ {
    move |locale| std::fs::read_to_string(format!("{dir}/keywordPages.{locale}.ts")).ok()
}

/// The words of `page` a copy rewrite must keep, for the hand-over; empty when the ledger has none.
pub fn kept_note(ledger: &[Word], page: &str) -> String {
    let list = |status: &str, adopted: bool| ledger.iter().filter(|w| w.page == page && w.status == status && w.adopted == adopted)
        .map(|w| format!("{}「{}」", w.locale, w.word)).collect::<Vec<_>>().join("、");
    let (kept, spare, waiting) = (list(CONFIRMED, true), list(CONFIRMED, false), list(PENDING, false));
    let mut out = String::new();
    if !kept.is_empty() { out.push_str(&format!("；该页已确认的本地词（英文改稿不得覆盖）：{kept}")); }
    if !spare.is_empty() { out.push_str(&format!("；已确认但页面还没用的本地词：{spare}")); }
    if !waiting.is_empty() { out.push_str(&format!("；待验证的本地词（保留，不删）：{waiting}")); }
    out
}

/// One finding per confirmed word the locale copy lost, for the hourly watchdog.
pub fn findings(ledger: &str, copy_dir: &str) -> Vec<Finding> {
    let words = read(ledger);
    missing(&words, copy_file(copy_dir)).into_iter().map(|w| Finding { key: format!("locale-word:{}:{}:{}", w.locale, w.page, w.word), severity: "P1".into(),
        title: format!("{} 页 {} 丢了已确认的本地词「{}」（{}，{} 核对）", w.locale, w.page, w.word, w.meaning, w.checked_on),
        action: format!("在 keywordPages.{}.ts 的该页恢复这个词（keywords 与标题或 section），英文改稿不得覆盖；依据 {ledger}", w.locale) }).collect()
}

fn arg(argv: &[String], name: &str) -> Option<String> { argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).cloned() }

fn verify(argv: &[String]) -> Result<(), String> {
    let need = |name: &str| arg(argv, name).ok_or(format!("{name} is required"));
    let (locale, page) = (need("--locale")?, need("--page")?);
    let words: Vec<String> = need("--words")?.split('|').map(|w| w.trim().to_string()).filter(|w| !w.is_empty()).collect();
    if words.is_empty() || words.len() > MAX_WORDS { return Err(format!("--words takes 1 to {MAX_WORDS} words separated by |")); }
    let meanings: Vec<String> = arg(argv, "--meanings").unwrap_or_default().split('|').map(|m| m.trim().to_string()).collect();
    let config = super::watch_config::read_config(&arg(argv, "--config").unwrap_or_else(|| CONFIG.into()))?;
    let market = config.rank.as_ref().and_then(|r| r.markets.iter().find(|m| m.prefix == format!("/{locale}"))).ok_or(format!("rank.markets has no /{locale}"))?;
    let day = chrono::Utc::now().with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).expect("offset")).format("%Y-%m-%d").to_string();
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let dir = format!("{}/locale-words/{day}", arg(argv, "--state").unwrap_or_else(|| format!("{home}/.local/state/seo-watch")));
    let trends = format!("{dir}/trends_{locale}_{}.json", file_key(&words.join("|")));
    let serp = |w: &str| format!("{dir}/serp_{locale}_{}.json", file_key(w));
    let mut calls = vec![trends_call(&words, market.location_code, trends.clone())];
    calls.extend(words.iter().map(|w| serp_call(w, market, serp(w))));
    run_all(calls, 4)?;
    let chart = read_json(&trends)?;
    let evidence: Vec<Evidence> = words.iter().enumerate().map(|(i, w)| {
        let (mean, coverage) = measure(&chart, i);
        Evidence { word: w.clone(), mean, coverage, title_hits: read_json(&serp(w)).map_or(0, |s| title_hits(&s, w)) }
    }).collect();
    let path = arg(argv, "--ledger").unwrap_or_else(|| LEDGER.into());
    let mut ledger = read(&path);
    for (i, (e, (status, share, primary))) in evidence.iter().zip(judge(&evidence)).enumerate() {
        let word = Word { locale: locale.clone(), page: page.clone(), word: e.word.clone(), meaning: meanings.get(i).cloned().unwrap_or_default(), status: status.into(), share,
            weeks: (e.coverage * 100.0).round() / 100.0, title_hits: e.title_hits, primary, adopted: false, location_code: market.location_code, range: RANGE.into(), checked_on: day.clone() };
        println!("{}", stringify(&word)?);
        upsert(&mut ledger, word);
    }
    adopt(&mut ledger, copy_file(&arg(argv, "--copy").unwrap_or_else(|| COPY_DIR.into())));
    save(&path, ledger)
}

fn check(argv: &[String]) -> Result<(), String> {
    let ledger = arg(argv, "--ledger").unwrap_or_else(|| LEDGER.into());
    let copy = arg(argv, "--copy").unwrap_or_else(|| COPY_DIR.into());
    let lost = findings(&ledger, &copy);
    let mut words = read(&ledger);
    let newly = adopt(&mut words, copy_file(&copy));
    if newly > 0 { save(&ledger, words)?; }
    if lost.is_empty() { println!("locale words: every adopted word of {ledger} is still in its page copy ({newly} newly adopted)"); return Ok(()); }
    Err(lost.iter().map(|f| f.title.clone()).collect::<Vec<_>>().join("\n"))
}

pub fn main(argv: &[String]) -> i32 {
    let run = match argv.first().map(String::as_str) { Some("verify") => verify(argv), Some("check") => check(argv), _ => Err("usage: localeWords verify|check".into()) };
    match run { Ok(()) => 0, Err(error) => fail(&error) }
}

#[cfg(test)]
#[path = "watch_locale_words_tests.rs"]
mod tests;
