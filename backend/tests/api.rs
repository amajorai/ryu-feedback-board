use axum::{
    body::to_bytes,
    http::{Request, StatusCode},
};
use ryu_feedback_board::{routes, Ctx, Store};
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

fn app() -> axum::Router {
    let store = Store::open_memory().unwrap();
    axum::Router::new().merge(routes(Arc::new(Ctx { store })))
}

async fn json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn public_request_detail_never_contains_private_fields() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/feedback/requests")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert!(body.get("board").is_some());
    assert!(body.get("requests").is_some());
}

#[tokio::test]
async fn public_unknown_board_is_not_disclosed() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/unknown-board/requests")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
