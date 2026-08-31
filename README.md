# backbone-digest

The periodic KPI-email engine (a port of Odoo's `digest` addon). One
company-fenced config row per digest; recipients, cadence, and enabled
metrics hang off it; a daily plain-pull cron renders and enqueues one
mail per recipient; an RFC 8058 one-click unsubscribe closes the loop.

Schema: `digest`. Fence posture: strict on `digest_digests` (the only
company column in the family); tips shared; subscription/KPI/tip-user
rows fence-none, read through the fenced parent. The full posture map
and every port deviation: `docs/port-notes.md`.

## What's inside

| Piece | Where |
|---|---|
| Schema (source of truth) | `schema/models/*.yaml`, `schema/hooks/*.yaml` |
| Declarative KPI registry (the extension API) | `src/application/service/kpi_registry.rs` |
| Recipient-context port (fail-closed identity seam) | `src/application/service/engagement_port.rs` |
| Base KPIs (Connected Users, Messages Sent) | `src/application/service/base_kpis.rs` |
| Digest verbs (create, cadence change, KPI enable, subscribe) | `src/application/service/digest_write_service.rs` |
| Per-recipient render (3 windows × previous-period margins; the out-of-fence drop lives here) | `src/application/service/digest_render_service.rs` |
| Daily pull sweep + slowdown ladder + Send Now | `src/application/service/digest_cron_service.rs` |
| RFC 8058 one-click unsubscribe engine | `src/application/service/unsubscribe_service.rs` |
| Public routes (one-click POST, human GET leg) | `src/presentation/http/public_routes.rs` |
| Growth loop (`sapiens.user.created` → subscribe) | `src/application/service/user_created_handler.rs` |
| Outbound mail seam (backbone-mail v0.2.7) | `src/application/service/digest_mail_seam.rs` |

## Composing it (host)

```rust
let digest = DigestModule::builder()
    .with_database(pool.clone())
    .with_token_secret(secret)              // or DIGEST_TOKEN_SECRET
    .with_public_base_url("https://mail.example.test")
    .with_default_digest(default_digest_id) // arms the growth loop
    .build()?;
digest.set_recipient_port(Arc::new(SqlRecipientContext::new(pool.clone())));
host_bus.register_handler(digest.user_created_handler()).await;
app = app.merge(digest.digest_public_routes());
```

## Extending it (other modules)

Register KPIs — never write `digest_digest_kpis` rows by hand for this:

```rust
DigestModule::builder()
    .register_kpi(KpiDefinition {
        name: "kpi_my_metric".into(),       // must start with kpi_
        label: "My Metric".into(),
        fence: KpiFenceDeclaration::CompanyData,
        computer: Arc::new(MyMetricKpi),
    })
```

The fence declaration on the entry — not any row count — decides
whether the KPI renders for a recipient (out-of-fence KPIs silently
don't render; see `docs/port-notes.md` §1.2, the binding port
condition).

## The cron

`digest-emails` (02:41 UTC, posture `pull`): the due-date column IS
the queue (`state = 'activated' AND next_run_date <= today`), claimed
with `FOR UPDATE SKIP LOCKED`, commit per batch. Mail-delivery failure
= no advance (retry next day). The slowdown ladder (daily → weekly →
monthly → quarterly for recipients who never log in) runs ONLY on this
path — Send Now never degrades cadence.

## Ops dependencies

- `DIGEST_TOKEN_SECRET` — HMAC secret for unsubscribe tokens. Without
  it the module builds without the unsubscribe engine and every render
  refuses loudly.
- DKIM/SPF/DMARC posture on the sending domain — a precondition for
  the RFC 8058 one-click leg to be trusted by receivers.
- The app connects as a non-superuser role; the RLS policy scopes
  `digest_digests` via `set_config('app.company_id', <uuid>, true)`.

## Regeneration

Schema YAML is the source of truth. `metaphor schema generate --force`
regenerates the tree; hand-written files are declared `user_owned` in
`metaphor.codegen.yaml` and are never read, merged, or deleted by the
generator. Declarations inside generated files live in
`// <<< CUSTOM … // END CUSTOM` markers.
