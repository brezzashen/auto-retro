# SEO Watchdog & Daily Keyword Room Playbook

This playbook documents how `seo-growth seo` automates hourly/daily SEO retrospectives, 6-dimensional anomaly detection, dual-model adversarial review, and daily keyword discovery.

## Commands

```bash
# Hourly SEO watchdog (probes sitemap, GSC, SERP ranks, AIsa signals, 6D anomalies, and dispatches retro)
seo-growth seo watch --config ./watch.config.json --state ~/.local/state/seo-watch

# Dual-model adversarial review of a retro or keyword-room draft
seo-growth seo advisor --run <run_dir> --mode daily|hourly|keywords --draft draft.md --out advisor.md

# Hourly digest & CPC keyword pool
seo-growth seo hourDigest --state ~/.local/state/seo-watch
seo-growth seo cpcPool --help

# Search-engine push (IndexNow / Bing / Baidu)
seo-growth seo push --config ./push.config.json

# Competitor & MRR analysis
seo-growth seo collect <config.json>
seo-growth seo analyze <run_dir>
seo-growth seo mrrEstimate <input.json>
seo-growth seo report <run_dir>
```

## Workflow Loop

1. **Collect & Probe**: Fetches sitemap status, Google Search Console hourly/daily rows, Bing/Baidu SERP ranks, Google Ads spend, PostHog traffic segments, and AI Overview/AI Mode citations.
2. **6D Anomaly Capture**: Detects Page-1 low CTR, keyword cannibalization, rank drops & SERP-vs-GSC divergence, canonical hijacks, new-visitor funnel leaks & bot batches, and Google-vs-Bing visibility gaps.
3. **Daily Keyword Room**: Scouts DataForSEO related keywords, Google Trends rising queries, and Similarweb anomaly keywords; scores intent/KD/SERP competition; gates new-page creation by 14-day GSC index rate.
4. **Dual-Model Adversarial Advisor**: Packs raw objective numbers alongside the lead agent's draft and invokes an independent model to red-team before shipping copy or page changes.
