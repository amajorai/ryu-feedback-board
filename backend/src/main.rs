use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::{
    extract::Request,
    http::{header::AUTHORIZATION, StatusCode},
    middleware::{from_fn, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use ryu_feedback_board::{api, paths, routes, Ctx, Store};

const DEFAULT_PORT: u16 = 8076;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let port = std::env::var("RYU_FEEDBACK_BOARD_PORT")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_PORT);
    let token = std::env::var("RYU_EXT_TOKEN")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let store = Store::open(paths::ryu_dir().join(paths::DB_FILE_NAME))?;
    let ctx = Arc::new(Ctx {
        store: store.clone(),
    });
    let gated_token = token.clone();
    let protected = Router::new()
        .nest("/api/feedback-board", routes(ctx))
        .layer(from_fn(move |request: Request, next: Next| {
            let expected = gated_token.clone();
            async move { require_token(request, next, expected.as_deref()).await }
        }));

    let probe_store = store;
    let app = Router::new()
        .route(
            "/health",
            get(move || {
                let store = probe_store.clone();
                async move { api::health(store).await }
            }),
        )
        .merge(protected);

    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, "ryu-feedback-board sidecar listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn require_token(request: Request, next: Next, expected: Option<&str>) -> Response {
    let provided = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if bearer_matches(provided, expected) {
        next.run(request).await
    } else {
        (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
}

fn bearer_matches(provided: Option<&str>, expected: Option<&str>) -> bool {
    let Some(expected) = expected.filter(|value| !value.is_empty()) else {
        return false;
    };
    let provided = provided.unwrap_or("").as_bytes();
    let expected = expected.as_bytes();
    if provided.len() != expected.len() {
        return false;
    }
    provided
        .iter()
        .zip(expected)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

#[cfg(test)]
mod tests {
    use super::bearer_matches;

    #[test]
    fn token_is_fail_closed_when_not_injected() {
        assert!(!bearer_matches(Some("secret"), None));
        assert!(!bearer_matches(None, Some("secret")));
    }

    #[test]
    fn token_requires_an_exact_match() {
        assert!(bearer_matches(Some("secret"), Some("secret")));
        assert!(!bearer_matches(Some("secre"), Some("secret")));
        assert!(!bearer_matches(Some("secret2"), Some("secret")));
    }
}
