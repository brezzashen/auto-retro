//! Keyword judgment rules shared by every SEO tab prompt: the hourly retro's Trends labels, the
//! daily retro's re-judgment, the same-hour follow-up, the keyword room and the red-team advisor.
//! One text keeps the tabs from undoing each other's verdicts.

/// Mars focus areas a Trends riser is judged against.
pub const AREAS: &str = "关系与约会沟通、读懂短信、情感咨询与挽回、识别高价值男性、人脉 CRM、会议记录、AI 助手、语音记录";

/// Founder 2026-11-11, after the 21:00 hourly retro parked "relationship counselling singapore" as
/// `观察` for its human-service intent: counselling, win-back, local human counselling and spotting
/// high-value men are Mars's market.
pub const CORE_MARKET: &str = "核心市场判词规则（创始人 2026-11-11「情感咨询，关系挽回，本地真人辅导其实都是我们的市场…用户需求和我们的产品是重叠的。包括分析优质男性，如何识别高价值的男性这种也是我们的核心目标」）：情感咨询与心理、婚姻咨询（relationship / marriage / couples counselling、counseling、therapy、therapist、恋愛相談、연애 상담、情感咨询、感情諮詢）、关系挽回（get your ex back、復縁、挽回）、本地真人辅导（带城市名、near me、singapore 的咨询词）与识别高价值男性（high value man、qualities of a good man、signs he is serious、いい男 特徴、优质男人）在任何市场与语言里都是 Mars 的市场：只要 watch.config.json discovery.seeds 有对口种子页就判「机会」（找词里判「做」），page 按 discovery.seeds 当前内容填（情感与婚姻咨询：带 singapore 的词填 /relationship-counselling-singapore，其余咨询词（美国城市、near me、泛咨询词）填通用咨询页 /relationship-counselling，不再并入 /ai-relationship-coach（创始人 2026-11-12）；挽回填 /win-him-back；识别高价值男性填 /high-value-man），种子页已在 discovery.seeds 但还没建成也照填，找词轮次会建页；「搜索者想找真人咨询师或本地服务」「带城市名」不是判观察、噪声、无关或意图不对口的理由。仍然排除：名人八卦与艺人情侣新闻、电视节目、剧集、歌名与漫画名（如 couples therapy 节目季数、こうめい 復縁、몸에 좋은 남자）、AI 女友或男友陪伴类（girlfriend 整词与 ai boyfriend、virtual boyfriend、AI彼氏 等陪伴短语在 discovery.blocked；评估真实伴侣的 boyfriend / 彼氏 词如 good boyfriend qualities、良い彼氏の特徴 不屏蔽，按识别高价值男性填 /high-value-man，创始人 2026-11-11）。这类词只在下列情况判观察：该语言没有对口落地页（不填不存在的语言路径）、对口种子页还不在 discovery.seeds（note 写建议的新页）、热度已回落；同一服务的城市变体（如 marriage counseling raleigh nc 与 boise）只判一个代表词为机会，其余判观察并在 note 写并入该词。";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_names_every_core_theme_and_keeps_the_exclusions() {
        for theme in ["情感咨询与挽回", "识别高价值男性"] { assert!(AREAS.contains(theme), "{theme}"); }
        for part in ["真人咨询师或本地服务", "不是判观察", "/win-him-back", "/high-value-man", "/relationship-counselling，不再并入 /ai-relationship-coach", "名人八卦", "AI 女友", "boyfriend", "城市变体", "discovery.seeds"] {
            assert!(CORE_MARKET.contains(part), "{part}");
        }
    }

    #[test]
    fn win_back_words_fit_the_win_back_seed_in_the_live_config() {
        use super::super::{watch_config::read_config, watch_discover::Candidate, watch_judge::fit_page};
        let config = read_config(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/seo_competitor/watch.config.json")).unwrap();
        let seeds = &config.discovery.as_ref().expect("discovery").seeds;
        let page = |k: &str| fit_page(&Candidate { keyword: k.into(), ..Default::default() }, seeds).map(|s| s.page.clone()).unwrap_or_default();
        // 2026-11-11: the keyword room dropped "how to get your ex back" (US 5,400, KD 1) as off-topic.
        for word in ["how to get your ex back", "my ex wants me back", "signs your ex wants you back"] { assert_eq!(page(word), "/win-him-back", "{word}"); }
        for word in ["exact match keywords", "go back to my excel file"] { assert_ne!(page(word), "/win-him-back", "{word}"); }
        for word in ["relationship counselling singapore", "marriage counselling in singapore", "couples therapy singapore"] {
            assert_eq!(page(word), "/relationship-counselling-singapore", "{word}");
        }
        // Founder 2026-11-12: counselling words without Singapore go to the general page, never the coach page.
        for word in ["marriage counseling", "couples therapist", "relationship counseling nyc", "pre marriage counseling questions"] {
            assert_eq!(page(word), "/relationship-counselling", "{word}");
        }
        for word in ["high value man", "qualities of a good man"] { assert_eq!(page(word), "/high-value-man", "{word}"); }
    }

    #[test]
    fn the_live_config_marks_the_core_market_seeds() {
        use super::super::watch_config::read_config;
        let config = read_config(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/seo_competitor/watch.config.json")).unwrap();
        let core: Vec<String> = config.discovery.as_ref().expect("discovery").seeds.iter().filter(|s| s.core_market).map(|s| s.page.clone()).collect();
        assert_eq!(core, ["/relationship-counselling-singapore", "/relationship-counselling", "/win-him-back", "/high-value-man", "/marriage-counselor-christian"]);
    }
}
