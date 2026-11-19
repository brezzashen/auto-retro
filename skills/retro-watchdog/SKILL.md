# Retro Watchdog Playbook

`retro-watchdog` is an autonomous retrospective and telemetry analysis engine that monitors AI coding agent sessions, search quality, vector recall, release pipelines, and production service turns, opening ack-gated retrospective sessions whenever a threshold or anomaly triggers.

## Built-in Retro Routes

| Trigger | Route | Purpose |
|---|---|---|
| `conversation_count_reached` | `conversation-retro` | Review developer agent conversations for waste, loops, and rule friction |
| `model_runtime_reached` | `runtime-retro` | Break down where model wall-time and tool calls go |
| `search_calls_reached` | `search-retro` | Audit local-search precision, speed, and fallback compliance |
| `recall_calls_reached` | `recall-retro` | Evaluate vector recall quality and index hygiene |
| `agent_wait_reached` | `wait-retro` | Analyze queue and wait-time bottlenecks across concurrent agents |
| `release_count_reached` | `release-retro` | Review release failures, retries, and CI bottlenecks |
| `canary_release_count_reached` | `canary-retro` | Track canary deployment health and error trends |
| `memory_changes_reached` / `memory_dream_due` | `memory-dream` | Consolidate, verify, and prune shared agent memory notes |
| `prompt_review_due` | `prompt-review` | Audit prompt-surface bloat and semantic duplicates |
| `hook_defects_found` | `hook-retro` | Diagnose agent hook failures and latency regressions |
| `vector_db_growth_reached` | `vector-db-retro` | Monitor vector index growth and stale embeddings |
| `crm_turn_count_reached` | `crm-turn-retro` | Analyze wasted steps, failed tools, and cache misses in production turns |
| `crm_costly_turn_found` | `crm-costly-turn` | Deep-dive single high-cost production turns |
| `risk_warning_found` | `risk-retro` | Audit security/risk anomalies |

## Lifecycle

1. **Trigger & Incident**: Threshold or anomaly writes an incident JSON under `~/.local/state/cpu-watchdog/incidents/<id>.json`.
2. **Evidence & Recall**: `retro-watchdog retro-recall <id>` retrieves prior history; `retro-watchdog retro-followup <id>` scores previously shipped fixes before adding new findings.
3. **Ack Gate**: `retro-watchdog monitor-ack <id>` validates required receipts and closes the incident so the next window can open.
4. **Overdue Escalation**: Unacknowledged incidents past 24 hours are surfaced by `retro-watchdog retro-overdue` and escalated to a takeover session.
