//! The unsubscribe route family (hand-written; user-owned) — the module's
//! only PUBLIC surface.
//!
//! BARE capability mounts (the token is the auth — the ADR-0019 Tier A
//! action-link class), throttled 120/min per client, mounted on the
//! host's PUBLIC tree (never behind a portal edge — the dep-edge
//! substitution ruling).
//!
//! | Method | Path | Behavior |
//! |---|---|---|
//! | GET  | /digest/unsubscribe?t=… | the human leg (the visible footer
//! |       |  | link): checks the link and changes NOTHING — a valid link
//! |       |  | shows a confirm page whose button POSTs `confirm`; an
//! |       |  | expired or refused link says so. A mail prefetcher that
//! |       |  | follows the link unsubscribes nobody.
//! | POST | /digest/unsubscribe | acts. From the confirm page (`confirm`
//! |       |  | in the form) it answers a page saying what happened. From
//! |       |  | a mail client (RFC 8058: `t` in the QUERY string, the fixed
//! |       |  | body `List-Unsubscribe=One-Click`) it answers a BARE 200 with
//! |       |  | no body — RFC 8058 §3 wants 2xx from the MUA path, always.
//!
//! Only a link carrying a genuine MAC can read as expired, so telling a
//! person their link expired gives a forger nothing.

use std::sync::Arc;

use axum::extract::{Form, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;

use crate::application::service::unsubscribe_service::{TokenVerdict, UnsubscribeOutcome};
use crate::DigestModule;

/// The shared state: the whole module behind an Arc.
pub type ApiState = Arc<DigestModule>;

/// The token carrier for both legs; `confirm` marks the confirm page's own
/// POST, which answers a page instead of the mail client's bare 200.
#[derive(Debug, Deserialize, Default)]
pub struct UnsubscribeTokenParam {
    pub t: Option<String>,
    pub confirm: Option<String>,
}

/// The PUBLIC digest tree — BARE mount, throttled 120/min per client.
/// Mount at the host root (the token is the auth).
pub fn public_composer() -> Router<ApiState> {
    use axum::middleware as axum_mw;

    Router::<ApiState>::new()
        .route("/digest/unsubscribe", post(unsubscribe_post).get(unsubscribe_confirm_page))
        .route_layer(axum_mw::from_fn_with_state(
            backbone_rate_limit::middleware(120, 60),
            backbone_rate_limit::rate_limit_middleware,
        ))
}

/// The acting leg. A mail client's RFC 8058 one-click carries the token in
/// the query string and gets the bare 200 whatever happened; the confirm
/// page's form carries `confirm` and gets a page saying what happened.
async fn unsubscribe_post(
    State(module): State<ApiState>,
    Query(params): Query<UnsubscribeTokenParam>,
    Form(form): Form<UnsubscribeTokenParam>,
) -> Response {
    let from_confirm_page = form.confirm.is_some();
    let Some(unsub) = module.unsubscribe_service() else {
        // No HMAC secret configured: no mail ever carried a link (the
        // render path refuses loudly first), so a POST here is noise.
        tracing::error!("unsubscribe POST with no token secret configured; no action taken");
        return if from_confirm_page { unavailable_page().into_response() } else { StatusCode::OK.into_response() };
    };
    let token = form.t.or(params.t).unwrap_or_default();
    let outcome = if token.is_empty() {
        Ok(UnsubscribeOutcome::Refused)
    } else {
        unsub.unsubscribe_by_token(&token, chrono::Utc::now().timestamp()).await
    };
    if !from_confirm_page {
        if let Err(e) = &outcome {
            tracing::error!(error = %e, "unsubscribe apply failed; still the bare 200");
        }
        return StatusCode::OK.into_response();
    }
    match outcome {
        Ok(UnsubscribeOutcome::Unsubscribed) | Ok(UnsubscribeOutcome::AlreadyUnsubscribed) => page(
            "Unsubscribed",
            "You will no longer receive this digest.",
        )
        .into_response(),
        Ok(UnsubscribeOutcome::Expired) => expired_page().into_response(),
        Ok(UnsubscribeOutcome::Refused) => refused_page().into_response(),
        Err(e) => {
            tracing::error!(error = %e, "unsubscribe apply failed on the confirm page");
            page("Something went wrong", "Your unsubscribe was not recorded. Please try the link again later.")
                .into_response()
        }
    }
}

/// The human leg: checks the link and changes nothing. A valid link gets a
/// confirm page; an expired or refused one says so.
async fn unsubscribe_confirm_page(
    State(module): State<ApiState>,
    Query(params): Query<UnsubscribeTokenParam>,
) -> Response {
    let Some(unsub) = module.unsubscribe_service() else {
        return unavailable_page().into_response();
    };
    let Some(token) = params.t.filter(|t| !t.is_empty()) else {
        return refused_page().into_response();
    };
    match unsub.check_token(&token, chrono::Utc::now().timestamp()) {
        // Only a token that parsed reaches here: UUIDs, digits, hex and
        // dots, nothing the page could read as markup.
        TokenVerdict::Valid => Html(format!(
            "<html><body><h2>Unsubscribe from this digest?</h2>\
             <p>You will no longer receive this digest by mail.</p>\
             <form method=\"post\" action=\"/digest/unsubscribe\">\
             <input type=\"hidden\" name=\"t\" value=\"{token}\">\
             <input type=\"hidden\" name=\"confirm\" value=\"1\">\
             <button type=\"submit\">Unsubscribe</button></form></body></html>"
        ))
        .into_response(),
        TokenVerdict::Expired => expired_page().into_response(),
        TokenVerdict::Refused => refused_page().into_response(),
    }
}

fn page(title: &str, body: &str) -> Html<String> {
    Html(format!("<html><body><h2>{title}</h2><p>{body}</p></body></html>"))
}

fn unavailable_page() -> Html<String> {
    page("Unsubscribe unavailable", "This digest deployment has no unsubscribe signing key configured.")
}

fn expired_page() -> Html<String> {
    page(
        "This link has expired",
        "Unsubscribe links live 180 days. Use the link in your most recent digest mail.",
    )
}

fn refused_page() -> Html<String> {
    page("This link is not valid", "This unsubscribe link was not issued by this digest, or it is incomplete.")
}
