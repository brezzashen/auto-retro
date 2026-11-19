# auto-retro

Autonomous retrospective, SEO keyword discovery, and telemetry analysis suite in Rust for AI coding agents and production web apps.

一套用于 AI Agent 与线上 Web 产品的 **自动复盘（含 SEO 自动复盘、自动找词）与自动分析工具集**（纯 Rust Workspace）。

## Architecture

```
auto-retro/
├── crates/
│   ├── retro-watchdog/   # Autonomous agent & runtime retro engine (15 retro routes + telemetry analyzers)
│   ├── seo-growth/       # SEO auto-retro, daily keyword room, 6D anomaly capture, adversarial advisor & PostHog conversion watchdog
│   └── prompt-review/    # Automated prompt-surface budget & semantic duplicate analyzer
├── skills/               # Agent playbooks & example watch configs
│   ├── retro-watchdog/
│   ├── seo-watchdog/
│   └── conversion-analytics/
└── shared/               # macOS fixed-identity app bundle helper for local-network daemons
```

## Components

### 1. `retro-watchdog` — Autonomous Retro & Telemetry Analysis Engine
Continuously monitors developer agent sessions, search quality, vector recall, release pipelines, and production chat turns, opening ack-gated retrospective sessions whenever a threshold or anomaly fires:

- **15 Built-in Retro Routes**: `conversation-retro`, `runtime-retro`, `search-retro`, `recall-retro`, `wait-retro`, `release-retro`, `canary-retro`, `memory-dream`, `prompt-review`, `hook-retro`, `vector-db-retro`, `crm-turn-retro`, `crm-costly-turn`, `risk-retro`, plus overdue incident escalation (`retro-overdue`).
- **Recursive Verification**: `retro-followup` scores previously shipped fixes against fresh windows before opening new findings; `retro-recall` pulls prior retro history; `retro-clarity` audits prompt clarity; `retro-series` aggregates multi-window trend pages.
- **Root-Cause & Telemetry Analyzers**: cause-tree attribution (`cause_tree`), crash repair (`crash_repair`), runtime wall-time breakdown, transcript call auditing, task-stall detection, and search compliance scoring.

### 2. `seo-growth` — SEO Auto-Retro, Daily Keyword Room & Conversion Analytics
A single Rust binary (`seo-growth`) covering three modules:

- **`seo` (SEO Watchdog, Auto-Retro & Daily Keyword Room)**:
  - **Hourly & Daily SEO Retro (`seo watch`, `seo hourDigest`)**: Probes sitemap health, Google Search Console hourly/daily data, Bing & Baidu SERP rankings, Google Ads spend & bid reviews, AIsa search signals (Google AI Overview / AI Mode citations, keyword intent & KD, new/lost backlinks), and GEO brand citation checks.
  - **6-Dimensional Zero-Extra-Cost Anomaly Capture**: Detects Page-1 low CTR, base-page keyword cannibalization, rank drops & SERP-vs-GSC divergence, canonical hijacks, new-visitor `CTA -> signup` funnel leaks & bot batches, and Google-vs-Bing visibility gaps.
  - **Dual-Model Adversarial Advisor (`seo advisor`)**: Packs raw objective numbers and anomalies alongside the lead model's draft and invokes an independent red-team model before edits ship.
  - **Daily Keyword Room (`seo watch`, `seo cpcPool`, `seo localeWords`)**: Scouts DataForSEO related keywords, Google Trends rising queries & surge tracking, and Similarweb anomaly keywords; scores SERP competition & commercial intent; gates new landing pages by 14-day GSC index rate; expands and judges CPC keyword pools; pushes updated URLs via IndexNow / Bing / Baidu (`seo push`).
  - **Competitor & MRR Analysis (`seo collect`, `seo analyze`, `seo mrrEstimate`, `seo report`)**.
- **`conversion` (Conversion & Retention Auto-Analysis)**:
  - Hourly PostHog funnel, activation, retention, traffic-source attribution, tracking-health probes (24h & 2h windows), upstream recovery analysis, and automated finding retro reconciliation (`conversion watch`, `conversion query`, `conversion api`).
- **`similarweb`**:
  - Browserless HTTP CLI for overview, organic/paid keywords, and 28-day anomaly extraction.

### 3. `prompt-review` — Prompt Surface & Semantic Duplicate Analyzer
Audits an AI agent's system prompt, tool descriptions/schemas, and bundled skill prompts:
- Tracks token/byte size deltas across runs (`history.ndjson`).
- Detects both literal and semantic duplicates using embeddings + reranking.

## Quick Start

### Prerequisites
- Rust 1.85+ (edition 2024 workspace)

### Build & Test

```bash
cargo build --release --workspace
cargo test --workspace
```

### Run CLIs

```bash
# SEO watchdog / keyword room / adversarial advisor / conversion analytics
./target/release/seo-growth --help
./target/release/seo-growth seo watch --config skills/seo-watchdog/watch.config.example.json --dry-run
./target/release/seo-growth conversion watch --config skills/conversion-analytics/watch.config.example.json --dry-run

# Autonomous retro watchdog
./target/release/retro-watchdog --help

# Prompt surface analyzer
./target/release/prompt-review --help
```

## Configuration & Credentials

All domains, properties, API keys, and state directories are configured via JSON config files or environment variables — no credentials or private endpoints are hardcoded:

- `AISA_API_KEY`: AIsa search signals API key
- `POSTHOG_API_KEY` / PostHog project config in `watch.config.json`
- Google Search Console OAuth / service credentials
- DataForSEO / SerpAPI / IndexNow / Baidu push tokens via config files

See [`skills/`](./skills/) for example config templates and agent playbooks.

## License

MIT
