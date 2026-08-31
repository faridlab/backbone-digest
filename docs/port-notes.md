# digest — port notes (Odoo `digest` → backbone-digest)

Source of truth for the port: `docs/odoo/marketing/digest/` in this
repository (README, business logic, schema models, hooks). This file
records what was ported as-is, what was deliberately changed, and why.
Everything here is plain-language and self-contained by design.

The module in one line: one company-fenced config row per periodic KPI
email; recipients, cadence, and enabled metrics hang off it; a daily
plain-pull cron renders and enqueues per recipient; an RFC 8058
one-click unsubscribe closes the loop.

---

## 1. Deliberate deviations from upstream

### 1.1 The KPI registry is declarative and name-keyed (replaces `_fields` scanning)

Upstream discovers KPIs at runtime by scanning `digest.digest`'s field
names for the `kpi_` prefix and computes values with the CSV
`available_fields`/`_compute_kpi_value` machinery. Neither ports:
runtime field scanning has no analogue in a compiled module, and CSV
field-name dispatch is exactly what a registry replaces.

Here a KPI is a **registry entry** — `KpiDefinition { name, label,
fence, computer }` keyed by its `kpi_<name>` name, registered through
`DigestModule::builder().register_kpi(...)` (or post-build through
`module.register_kpi(...)`). A `digest_digest_kpis` row only NAMES a
registry key; the registry entry carries the compute function and the
fence declaration. The registry is an API surface other modules extend
— the intended analog of upstream's "other modules add kpi_* fields".

`available_fields` is dropped entirely (no CSV, no field list
endpoint). A duplicate registration panics at build time: a metric
name is an identity, not a slot.

### 1.2 The out-of-fence drop reads the fence DECLARATION — never a row count

The binding port condition (spec, condition 16), documented here at
the API-contract level as required:

> The KPI out-of-fence drop is decided from the registry entry's fence
> declaration, NEVER from row count (under RLS an out-of-fence read is
> zero rows, indistinguishable from a genuine zero); documented at the
> API-contract level. DKIM/domain posture named as an ops dependency on
> the RFC 8058 leg.

Concretely: `KpiFenceDeclaration` (`SharedData`, `CompanyData`,
`PinnedCompanyData(Uuid)`) is carried ON the registry entry, and
`renders_for(recipient_company, digest_company)` is a pure decision
over declarations. The render engine asks the declaration — it never
counts rows, because a company-scoped query under row-level security
returns zero rows for both "out of fence" and "genuinely zero", and
the two must never be confused. Out-of-fence KPIs silently don't
render for that recipient (documented behavior, not an error).

**Ops dependency, named:** the RFC 8058 one-click leg only counts as
compliant when the sending domain has DKIM signing and a matching
`List-Unsubscribe`/`List-Unsubscribe-Post` header pair (see 1.5).
Without DKIM the one-click unsubscribe is not trusted by receivers;
deployments MUST treat domain posture (SPF/DKIM/DMARC) as a
precondition for enabling digest mail.

### 1.3 `unsubscribe_oneclik` — the typo does not survive

Upstream's action-server method is misspelled
(`action_unsubscribe_oneclik`). The port keeps the BEHAVIOR (one-click
unsubscribe) under the correctly spelled route/service names
(`POST /digest/unsubscribe`, `UnsubscribeService`). Recorded here so
nobody goes looking for the typo.

### 1.4 Time windows are UTC

Upstream computes windows in the user/company timezone. The backbone
organization stack has no timezone home for a company, so all three
windows (24h / 7d / 30d) and their previous-period comparison windows
are computed in UTC (`windows_at(now)`). Rendering a local-time
digest would require a company timezone field that does not exist;
when one lands, the render takes it as an input — the window math is
already isolated.

### 1.5 The unsubscribe link is a Tier A HMAC capability, not a session

Upstream's one-click unsubscribe authenticates loosely and relies on
the portal/session. The port uses the sanctioned HMAC action-link
class (ADR-0019, Tier A): token = HMAC-SHA256 over
`(digest_id, user_id, expiry)` with a mandatory expiry delta
(180 days), constant-time verification, verify-failure lockout
(5 failures → 60 s per (digest, user)), and a deliberately coarse
refusal (expired / forged / malformed are indistinguishable — no
oracle). Re-POST of a valid token on an already-unsubscribed row is a
NO-OP success (bare 200, no re-stamp, no repeat audit event) — the
tombstone subscription row is what makes the second POST decidable
from data. The secret comes from `DIGEST_TOKEN_SECRET` (builder arg
or env); without it the module builds without the unsubscribe engine
and every render refuses loudly — a digest mail must never go out
without its unsubscribe leg.

The mail's `List-Unsubscribe` + `List-Unsubscribe-Post: One-Click`
headers ride backbone-mail v0.2.7's per-mail `headers` parameter on
`enqueue` — no mail-stack change was needed.

### 1.6 `company_id` is NOT NULL (no `env.company` fallback)

Upstream leaves the digest's company nullable and fills it from the
request environment at send time. The port closes that: the fence
ruling requires every digest to carry an owning company, so
`digest_digests.company_id` is `UUID NOT NULL`, and the table carries
the module's only company RLS policy (`digest_digests_company_isolation`,
`USING current_setting('app.company_id', true)`).

### 1.7 Tip consumption happens AFTER enqueue

Upstream marks a tip consumed for the user when the digest email is
COMPOSED — before the mail queue has accepted it; a delivery failure
burns the tip. The port consumes only after the per-recipient mail row
is successfully enqueued (enqueue = this module's notion of done; the
mail queue owns delivery).

### 1.8 Tip HTML is stored as authored, sanitized at render

`digest_tips.tip_description` is stored un-sanitized (upstream
`sanitize=False` — the stored-unsanitized-HTML pattern) and is
html-sanitized AT RENDER TIME, never before. The recorded sanitizer
choice is **ammonia** (already a mail-stack neighbor); sanitizing at
rest would corrupt authored markup the sanitizer cannot round-trip.
Render-time sanitizing is the load-bearing half of the pair.

### 1.9 `group_id` → `group_key` (stable key string, no identity edge)

Upstream gates tip visibility on a `res.groups` m2m. The port has no
identity-module Cargo edge by design, so the gate is a stable key
string (`digest_tips.group_key`, e.g. an uppercase role name) resolved
through the fail-closed recipient-context port
(`engagement_port::group_keys`); NULL = visible to everyone. The
module never interprets the key's syntax.

### 1.10 The slowdown ladder degrades the DIGEST, never a subscription

Upstream's `_check_slowdown` walks digest periodicity daily → weekly
→ monthly → quarterly for recipients who never log in. Ported with
three deltas: (a) the inactivity signal is `sapiens users.last_login`
(read through the recipient port — no direct identity edge); (b) the
ladder only ever runs on the CRON path — a manual "Send Now" sends at
the digest's configured cadence and never degrades it; (c) there is
no reverse rung — logging in again does not re-tighten cadence (the
operator resets it).

### 1.11 The cron is a plain daily pull; the due-date column is the queue

`digest-emails` (02:41 UTC daily) scans
`state = 'activated' AND next_run_date <= today`, claims rows with
`FOR UPDATE SKIP LOCKED`, and advances `next_run_date` by the
periodicity only on successful enqueue of the batch. Upstream dies
mid-loop on the first failure; the port isolates per digest (one
digest's mail failure records a typed error and does not block the
others — a decided delta), and a mail-delivery failure means NO
advance (the digest retries next day at the same cadence).

### 1.12 Dropped upstream surface (and where it went)

| Upstream | Here |
|---|---|
| `available_fields` CSV compute | dropped — the registry IS the field list (1.1) |
| `currency_id` editable related | dropped as a column — monetary units ride the registry entry; render currency resolves from the digest's company |
| `is_subscribed` non-stored compute | read service over the subscription row state; never persisted |
| `res.users.log` (the activity log table) | not a table here — the slowdown signal is `users.last_login` |
| `res.config.settings` digest fields | plain config keys (`digest.default_digest_id`, `digest.default_digest_emails`) — no table |
| `res.users` create interception (auto-subscribe) | the `sapiens.user.created` integration event on the host bus drives the subscribe verb (fail-closed internal-user predicate via the recipient port) — no identity edge, no monkey-patch analogue |
| portal/website unsubscribe page | the public route family mounts on the host's bare public-route class; there is deliberately NO Cargo or route edge onto backbone-portal (dep-edge substitution ruling) |

### 1.13 m2m fields became through-rows

`digest.digest.user_ids` and `digest.tip.user_ids` are materialized as
first-class junction entities (`digest_subscriptions`,
`digest_tip_users`) with unique keys, because the subscription needs a
STATE (the RFC 8058 tombstone) and the consumption marker needs to be
queryable as a set. `ON DELETE CASCADE` from the parent is preserved.

### 1.13 An empty public base URL refuses renders (no default host in production)

Upstream builds the unsubscribe URL from `web.base.url` (whatever it
holds). Here the mailed one-click link is minted from the configured
public base URL, and an EMPTY one is a loud per-digest render refusal
(`DigestError::PublicBaseUrlNotConfigured`) — symmetric with the
missing-HMAC-secret refusal. The dev posture keeps a localhost default
in the consuming host's BASE config overlay; the production overlay
declares the env var with an EMPTY default (no localhost fallback), so
an undeclared `DIGEST_PUBLIC_BASE_URL` surfaces as refused renders,
never as production mail carrying links to a guessed host. The one-click
exit is load-bearing enough that minting its link on a silent default
is treated as a configuration error, not a convenience.

---

## 2. The company fence (the port's central posture call)

Upstream ships ZERO ir.rules on digest: every internal user reads
every company's digest configs and tips. The port picks PER-COMPANY
DIGESTS (the spec's faithfulness note explicitly authorizes choosing a
real-RLS posture for the port):

| Entity | Fence | Why |
|---|---|---|
| `digest_digests` | strict (`company_id UUID NOT NULL`, RLS policy) | the config row is company-private; the cross-company probe ("operator A cannot enumerate company B's digests") is answered by a declaration |
| `digest_tips` | shared (no company column) | one global carousel; every company reads the same rows |
| `digest_subscriptions` | none (no company column) | the recipient graph — every supported read joins the strict-fenced parent |
| `digest_digest_kpis` | none (no company column) | enablement rows — read through the fenced parent |
| `digest_tip_users` | none (no company column) | per-user consumption markers, keyed by user not tenant |

The fence-none children carry no company column BY RULING (the spec's
C2 single-tenancy posture): their company axis is inherited — the
family-wide cross-company invisibility probe is answered by the
parent's RLS.

NOTE: ADR-0014's `none`-exemplar list still names digest; that entry
is stale — the posture above supersedes it.

---

## 3. Schema-DSL notes for future module authors

- **`transitions:` nests UNDER `states:`** in a hook file. At top
  level it parses, validates, and is then SILENTLY IGNORED — the
  generated `Transition` enum comes out empty and the crate does not
  compile (`match self {}` on `&EmptyEnum` is non-exhaustive). The
  per-entity machine files here carry a comment warning about it.
- `from: "*"` (any-state) is supported and emits wildcard transitions.
- The DSL has no `text` primitive; `type: string` still emits `TEXT`
  in DDL (`digest_tips.tip_description`).
- `final: true` on a state is the validator's lint floor (≥1 final
  state required), NOT irreversibility — transitions may leave a final
  state (`action_activate` re-enters `deactivated`; `resubscribe`
  re-enters `unsubscribed`). Both machines here are hand-set two-value
  Selections, the family's simplest shape.
- Enum types are created UNQUALIFIED in `public` with
  `IF NOT EXISTS`: `digest_periodicity`, `digest_state`,
  `digest_subscription_state`. Census-checked against every module
  migration in the tree — the nearest neighbor is sapiens'
  notification-frequency enum `digest_frequency` (a DIFFERENT concept:
  per-user notification cadence vs this module's send periodicity;
  both name and stems are distinct).

---

## 4. Migrations (generated set, canonical)

One migration family, emitted from the schema YAML (the source of
truth): `20260426220000_create_enums` through
`20260426220008_add_audit_triggers` plus `seeds/` — enums, the five
tables, FKs, unique constraints, the company RLS policy
(`digest_digests` only), audit triggers, seed rows. Regenerating is
byte-stable; nothing under `migrations/` is hand-authored, so nothing
there is listed in `metaphor.codegen.yaml`'s `user_owned`.

History note: an earlier bootstrap-overlay family
(`2026083112000*`, `CREATE TABLE IF NOT EXISTS` parity + extra
indexes) bridged the gap before the generated tree landed; it was
retired once the generated set became canonical, and its dangling
`user_owned` glob went with it. If a future seat wants index hardening
the generator does not emit (metadata GIN, expression indexes), that
is the shape to reintroduce — as a NEW versioned migration, never by
editing the generated files.

Run order note: the generated set's audit-trigger migration has no
`.down` (the family shape — see backbone-survey).

Cross-module order in a shared database: digest first, then
backbone-sapiens, then backbone-mail, then the messaging outbox —
verified: digest's DDL has no cross-module FK and needs no extension,
so the order is convention, not dependency (the probe harness applies
them in exactly this order).
