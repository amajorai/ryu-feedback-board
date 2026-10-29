use std::sync::Arc;

use axum::{
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;

use crate::admin;
use crate::public;
use crate::store::Store;
use crate::voter::VoterTokens;

pub struct Ctx {
    pub store: Store,
    /// Issues and verifies the signed voter cookie public votes are keyed on.
    pub voters: VoterTokens,
}

impl Ctx {
    /// Build the context, loading (or creating once) the voter signing key.
    pub fn new(store: Store) -> anyhow::Result<Self> {
        let voters = VoterTokens::load(&store)?;
        Ok(Self { store, voters })
    }
}

pub fn routes(ctx: Arc<Ctx>) -> Router {
    public::routes(ctx.clone()).merge(Router::new().nest("/admin", admin::routes(ctx)))
}

pub async fn health(store: Store) -> Response {
    match store.request_count() {
        Ok(request_count) => Json(json!({
            "ok": true,
            "service": "feedback-board",
            "request_count": request_count,
            "sealing": store.seal_health(),
        }))
        .into_response(),
        Err(error) => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "ok": false, "error": error.to_string() })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
fn health_route() -> Router<Arc<Ctx>> {
    Router::new().route("/health", get(|| async { "ok" }))
}
