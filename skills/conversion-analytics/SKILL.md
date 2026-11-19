# Conversion Analytics Watchdog Playbook

This playbook documents how `seo-growth conversion` automates hourly PostHog funnel, activation, retention, and tracking-health analysis with finding-level retrospective tracking.

## Commands

```bash
# Hourly conversion & tracking-health watchdog
seo-growth conversion watch --config ./watch.config.json --state ~/.local/state/conversion-watch

# Ad-hoc HogQL / PostHog queries
seo-growth conversion query --help
seo-growth conversion api --help
```

## Capabilities

- **Tracking Health**: Verifies event ingestion over 24-hour and 2-hour windows and flags P0/P1 drops past cooldown.
- **Funnel & Retention**: Tracks landing -> signup -> activation -> paid conversion and isolates the largest drop-off step.
- **Traffic Source Attribution**: Breaks down visitors and signups by referrer, UTM, and landing page.
- **Finding Retro**: Reconciles previously dispatched findings across runs and marks stale or unresolved defects.
