//! The RFC 8058 one-click unsubscribe route family (hand-written;
//! user-owned) — the module's only PUBLIC surface.
//!
//! BARE capability mounts (the token is the auth — the ADR-0019 Tier A
//! action-link class), throttled 120/min per client, mounted on the
//! host's PUBLIC tree (never behind a portal edge — the dep-edge
//! substitution ruling).
//!
//! | Method | Path | Behavior |
//! |---|---|---|
//! | POST | /digest/unsubscribe | the MUA one-click leg: `t` from the
//! |       |  | QUERY STRING (the RFC 8058 wire shape — the MUA POSTs to
//! |       |  | the URL the List-Unsubscribe header carries, with the
//! |       |  | fixed body `List-Unsubscribe=One-Click` and no `t` field)
//! |       |  | or the form body, verify, apply. BARE 200, NO body —
//! |       |  | RFC 8058 §3 wants 2xx from the MUA path, always.
//! | GET  | /digest/unsubscribe?t=… | the human leg (the visible footer
//! |       |  | link): same verify+apply, tiny human page.
//!
//! No-oracle posture: a refused token (expired, forged, malformed — one
//! coarse shape) performs NO action and is indistinguishable from a
//! no-op re-POST on the wire: bare 200 both ways on the POST leg. The
//! GET leg answers one coarse refusal page for every refusal reason.

use std::sync::Arc;

use axum::extract::{Form, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;

use crate::application::service::unsubscribe_service::UnsubscribeService;
use crate::DigestModule;

/// The shared state: the whole module behind an Arc.
pub type ApiState = Arc<DigestModule>;

/// The token carrier for both legs.
#[derive(Debug, Deserialize, Default)]
pub struct UnsubscribeTokenParam {
    pub t: Option<String>,
}

/// The PUBLIC digest tree — BARE mount, throttled 120/min per client.
/// Mount at the host root (the token is the auth).
pub fn public_composer() -> Router<ApiState> {
    use axum::middleware as axum_mw;

    Router::<ApiState>::new()
        .route("/digest/unsubscribe", post(unsubscribe_one_click).get(unsubscribe_human))
        .route_layer(axum_mw::from_fn_with_state(
            backbone_rate_limit::middleware(120, 60),
            backbone_rate_limit::rate_limit_middleware,
        ))
}

/// The MUA one-click leg (RFC 8058): token from the QUERY string — the
/// wire shape is "POST to the URL the List-Unsubscribe header carries,
/// body is the fixed `List-Unsubscribe=One-Click` marker with no `t`
/// field" — with the urlencoded form body as a secondary carrier (manual
/// posts). BARE 200, NO body — expired, forged, malformed, replayed, and
/// successful are indistinguishable on this leg by design (no oracle,
/// no bounce).
async fn unsubscribe_one_click(
    State(module): State<ApiState>,
    Query(params): Query<UnsubscribeTokenParam>,
    Form(form): Form<UnsubscribeTokenParam>,
) -> Response {
    let Some(unsub) = module.unsubscribe_service() else {
        // No HMAC secret configured: no mail ever carried a link (the
        // render path refuses loudly first), so a POST here is noise.
        // Still the RFC 8058 shape: bare 200, no action.
        tracing::error!(
            "unsubscribe POST with no token secret configured; no action taken"
        );
        return StatusCode::OK.into_response();
    };
    let token = form.t.or(params.t).unwrap_or_default();
    if token.is_empty() {
        return StatusCode::OK.into_response();
    }
    // Verdict + idempotent apply; EVERY outcome is the bare 200.
    match unsub.unsubscribe_by_token(&token, chrono::Utc::now().timestamp()).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "unsubscribe apply failed; still the bare 200");
            StatusCode::OK.into_response()
        }
    }
}

/// The human leg: same verify + idempotent apply, human-facing page.
/// One coarse refusal page for every refusal reason.
async fn unsubscribe_human(
    State(module): State<ApiState>,
    Query(params): Query<UnsubscribeTokenParam>,
) -> Response {
    let Some(unsub) = module.unsubscribe_service() else {
        return Html(
            "<html><body><h2>Unsubscribe unavailable</h2>\
             <p>This digest deployment has no unsubscribe signing key configured.</p></body></html>",
        )
        .into_response();
    };
    let Some(token) = params.t.filter(|t| !t.is_empty()) else {
        return refused_page().into_response();
    };
    match unsub.unsubscribe_by_token(&token, chrono::Utc::now().timestamp()).await {
        // Ok(true) = performed; Ok(false) = already unsubscribed (the
        // idempotent no-op) — both are success for the human.
        Ok(_) => Html(
            "<html><body><h2>Unsubscribed</h2>\
             <p>You will no longer receive this digest.</p></body></html>",
        )
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "unsubscribe apply failed on the human leg");
            refused_page().into_response()
        }
    }
}

fn refused_page() -> Html<&'static str> {
    Html(
        "<html><body><h2>Link invalid or expired</h2>\
         <p>This unsubscribe link is not valid (it may have expired — links \
         live 180 days and regenerate with every digest mail).</p></body></html>",
    )
}
