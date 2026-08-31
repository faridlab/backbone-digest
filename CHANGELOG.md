# Changelog

All notable changes to this module are documented here. The format
follows the family convention: one section per release, newest first.

## 0.1.0 — the initial port of Odoo's `digest` addon

The module's foundation: schema, generated tree, migrations, and the
hand-authored service surface.

### Schema and generated tree

- Five entities in the `digest` schema: `digest_digests` (the
  company-fenced config row: cadence, due-date spine, two-value
  hand-set state), `digest_subscriptions` (the recipient graph with
  the unsubscribed tombstone), `digest_digest_kpis` (per-digest
  enabled metrics, keyed by registry name), `digest_tips` (the shared
  carousel), `digest_tip_users` (per-user consumption markers).
- Three unqualified enum types in `public`: `digest_periodicity`,
  `digest_state`, `digest_subscription_state` (census-checked, no
  collisions).
- Two state machines: `digest_state` (activate/deactivate, both
  any→any) and `digest_subscription_state`
  (unsubscribe/resubscribe — the RFC 8058 edge and its reverse).
- Rules R-DG1 … R-DG13, R-DGT1 covering the declarative registry, the
  fence-declaration render drop, the slowdown ladder, the pull cron,
  the HMAC unsubscribe capability, the RFC 8058 header ride, the
  cadence whitelist, the growth loop, the family read-through-parent
  contract, and the tip sanitize-at-render pair.
- One scheduled job: `digest-emails` (02:41 UTC daily, posture
  `pull`, commit per batch, pickup lock) — the due-date column is the
  queue.
- Audit events: `digest_email_sent`, `digest_slowdown_degraded`,
  `digest_unsubscribed`, `digest_user_auto_subscribed`. Zero outbox
  events by design (the module emits mail, not domain events).
- Company fence: strict on `digest_digests` only (NOT NULL
  `company_id` + RLS policy); tips shared; the rest fence-none, read
  through the fenced parent.

### Hand-authored surface

- The declarative name-keyed KPI registry (`kpi_registry.rs`) — the
  extension API: `KpiDefinition` with `KpiFenceDeclaration`
  (`SharedData` / `CompanyData` / `PinnedCompanyData`) and the
  `KpiComputer` trait. The out-of-fence render drop reads the fence
  DECLARATION, never a row count.
- The fail-closed recipient-context port (`engagement_port.rs`) — the
  module's only window onto identity facts; denying default, swappable
  slot, SQL adapter installed by the host.
- The base KPI pair: Connected Users (company-scoped logins) and
  Messages Sent (shared message volume).
- The digest verbs, the per-recipient render engine (3 UTC windows ×
  previous-period margins, ammonia-sanitized tip at render, RFC 8058
  footer + headers, enqueue-then-consume), the daily pull sweep with
  the slowdown ladder and per-digest failure isolation, the manual
  Send Now (never degrades cadence).
- The RFC 8058 one-click unsubscribe: Tier A HMAC token with mandatory
  180-day expiry, constant-time verify, no-oracle refusal,
  verify-failure lockout, idempotent re-POST; the public route family
  (bare-200 POST leg + human GET leg, throttled 120/min).
- The growth-loop consumer: `sapiens.user.created` → subscribe the new
  internal user to the configured default digest (idempotent on the
  unique key).

### Dependencies

- backbone-framework crates at tag v2.7.11.
- backbone-mail at tag v0.2.7 (message_post + enqueue with per-mail
  headers — the RFC 8058 ride).
- No Cargo edge onto backbone-sapiens (identity facts flow through the
  recipient port) and none onto backbone-portal (public routes mount
  on the host's bare public-route class).

### Deviations from upstream

Recorded with rationale in `docs/port-notes.md`: the declarative
registry replaces `_fields` scanning; `available_fields` dropped; the
`unsubscribe_oneclik` typo corrected; UTC windows; NOT NULL
`company_id` (no `env.company` fallback); tip consumption after
enqueue; ammonia at render; `group_key` string instead of a `group_id`
m2m; slowdown ladder cron-only with no reverse rung; per-digest
failure isolation; the m2m pair materialized as through-rows.
