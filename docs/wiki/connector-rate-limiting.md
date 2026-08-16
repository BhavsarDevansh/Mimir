# Connector Rate Limiting & Retry

> **Phase:** 3 — Connectors
>
> **Landed in:** v0.72.0 (issue #189 / F12)

## What it is

When Mimir's connectors fetch data from your email, calendar, or photo library, they make network requests to external services. Those services have rate limits — some strict (the free OSM Nominatim geocoder allows at most one request per second), some gentler. Mimir now ships a single, shared rate limiter and retry mechanism for connectors to use. Connectors will route their outbound calls through it as their backends are implemented in later Phase 3 issues, so Mimir never accidentally hammers a service and always backs off politely when a service says "slow down".

## How it works

Each connector instance is configured with a small rate-limit policy:

- **Requests per second** — the sustained pace (fractional values allowed, so `0.5` means one request every two seconds).
- **Burst size** — how many requests can go out at once before the sustained pace kicks in.
- **Daily quota** — an optional cap on total requests per rolling 24 hours. When it's spent, the connector pauses for the rest of the day rather than blocking forever or getting banned. The limiter notices an exhausted quota instantly (it doesn't sit waiting first), and it can save and reload the quota window across Mimir restarts so a relaunch can't quietly reset your daily allowance and overspend it.
- **Backoff strategy** — how long to wait between retries if a request fails (exponential, linear, or fixed), plus a little random jitter so retries from different connectors don't all fire at the same instant.

Before every outbound API call, a connector asks the limiter for permission (`acquire`). If it's within the rate and quota, the call proceeds. If the service returns a "slow down" response (HTTP 429) or is temporarily unavailable (502/503/504), the request is retried automatically with increasing delays, honouring any `Retry-After` hint the service sends (capped so a large hint plus jitter can never wait longer than the configured ceiling).

## What it does *not* cover

Connector **LLM** calls (e.g. asking the model to extract a flight booking from an email) are exempt — those go through Mimir's shared LLM worker pool on a lower-priority queue, so they never compete with your chat and never violate the LLM provider's one-at-a-time concurrency rules. The rate limiter governs service API calls (HTTP, IMAP, CalDAV) only.

## Use cases

- **OSM Nominatim geocoder** — a built-in preset (`nominatim`) enforces the ≤ 1 req/s usage policy automatically.
- **IMAP email** — polite polling/IDLE without overwhelming the mail server.
- **CalDAV calendar** — spaced-out sync-token requests.
- **Photo library** — bounded EXIF/GPS processing when scanning large folders.

## Best practices

- Use the tightest policy the service allows; Mimir's defaults are conservative.
- Set a `daily_quota` for services with hard daily caps so a runaway sync can't burn your allowance.
- Keep `burst_size` at `1` for strict services (Nominatim, most free APIs); allow a small burst only for services that document it.
- Connectors are still responsible for sending an identifying `User-Agent` where a service's policy requires it (e.g. Nominatim).

## Status

This is a library component in `mimir-connectors`. The primitive is in place and tested; the first consumers (the geocoder and the Photos/Calendar/Email backends) wire it up in later Phase 3 issues. No user-facing CLI or config-file section is added yet — per-connector rate limits travel inside each connector's own `config_json`.
