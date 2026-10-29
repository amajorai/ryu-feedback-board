use axum::{
    body::to_bytes,
    http::{Request, StatusCode},
};
use ryu_feedback_board::model::BoardPatch;
use ryu_feedback_board::{routes, Ctx, Store};
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

fn app() -> (axum::Router, Store) {
    let store = Store::open_memory().unwrap();
    (
        axum::Router::new().merge(routes(Arc::new(Ctx::new(store.clone()).unwrap()))),
        store,
    )
}

async fn json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn public_request_detail_never_contains_private_fields() {
    let (app, _) = app();
    let response = app
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
    let (app, _) = app();
    let response = app
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
    let request_id_header = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .expect("API errors include a request id header")
        .to_owned();
    let body = json(response).await;
    assert!(!request_id_header.is_empty());
    assert_eq!(body["error"]["requestId"], request_id_header);
}

#[tokio::test]
async fn workspace_moderation_updates_all_boards_in_one_revisioned_change() {
    let (app, store) = app();
    let first_board = store.board_by_slug("feedback").unwrap().unwrap();
    let second_board = store
        .create_board("roadmap", "Roadmap", "Planned product work.")
        .unwrap();
    let workspace = store.workspace().unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/admin/workspace")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({
                        "revision": workspace.revision,
                        "moderate_public_writes": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["workspace"]["moderate_public_writes"], true);

    for (board_id, expected_revision) in [
        (&first_board.id, first_board.revision + 1),
        (&second_board.id, second_board.revision + 1),
    ] {
        let board = store.board_by_id(board_id).unwrap().unwrap();
        assert!(board.moderate_public_writes);
        assert_eq!(board.revision, expected_revision);
    }
}

#[tokio::test]
async fn moderated_guest_submission_returns_a_review_receipt_without_publishing_it() {
    let (app, store) = app();
    let board = store.board_by_slug("feedback").unwrap().unwrap();
    store
        .patch_board(
            &board.id,
            board.revision,
            BoardPatch {
                moderate_public_writes: Some(true),
                ..BoardPatch::default()
            },
        )
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/feedback/requests")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({
                        "title": "Moderated request",
                        "body": "Please review before publishing.",
                        "category": "Ideas",
                        "author_name": "Grace"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["pendingReview"], true);
    assert_eq!(body["request"]["title"], "Moderated request");
    assert!(body["request"].get("internal_notes").is_none());

    let request_id = body["request"]["id"].as_str().unwrap();
    let stored = store.request_by_id(request_id).unwrap().unwrap();
    assert_eq!(stored.moderation_state, "pending");
    assert!(!stored.public_visible);
    assert!(store
        .public_request("feedback", request_id)
        .unwrap()
        .is_none());
    assert_eq!(store.request_count().unwrap(), 1);

    let public_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/feedback/requests")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let public_body = json(public_response).await;
    assert!(public_body["requests"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn moderated_comments_remain_hidden_until_admin_approval_and_rejected_comments_stay_hidden() {
    let (app, store) = app();
    let request = store
        .create_guest_request(
            "feedback",
            ryu_feedback_board::model::CreateGuestRequest {
                title: "Comment moderation".into(),
                body: "Review comments before publishing.".into(),
                category: "Ideas".into(),
                ..Default::default()
            },
            "voter-a",
        )
        .unwrap();
    let board = store.board_by_slug("feedback").unwrap().unwrap();
    store
        .patch_board(
            &board.id,
            board.revision,
            BoardPatch {
                moderate_public_writes: Some(true),
                ..BoardPatch::default()
            },
        )
        .unwrap();

    let pending = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/feedback/requests/{}/comments", request.id))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({
                        "body": "This comment should wait for review.",
                        "author_name": "Grace"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pending.status(), StatusCode::OK);
    let pending_body = json(pending).await;
    assert_eq!(pending_body["pendingReview"], true);
    let pending_id = pending_body["comment"]["id"].as_str().unwrap();
    assert!(store.public_comments(&request.id).unwrap().is_empty());

    let approved = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/admin/requests/{}/comments/{pending_id}",
                    request.id
                ))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({ "revision": 0, "approved": true }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    let approved_body = json(approved).await;
    assert_eq!(approved_body["comment"]["moderation_state"], "approved");
    assert_eq!(approved_body["request"]["comment_count"], 1);
    let public_comments = store.public_comments(&request.id).unwrap();
    assert_eq!(public_comments.len(), 1);
    assert_eq!(
        public_comments[0].body,
        "This comment should wait for review."
    );

    let pending_again = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/feedback/requests/{}/comments", request.id))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({
                        "body": "This comment should be rejected.",
                        "author_name": "Lin"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let rejected_id = json(pending_again).await["comment"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let rejected = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/admin/requests/{}/comments/{rejected_id}",
                    request.id
                ))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({ "revision": 1, "approved": false }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::OK);
    let rejected_body = json(rejected).await;
    assert_eq!(rejected_body["comment"]["moderation_state"], "rejected");
    assert_eq!(rejected_body["request"]["comment_count"], 1);
    assert_eq!(store.public_comments(&request.id).unwrap().len(), 1);
    assert_eq!(
        store.admin_comments(&request.id).unwrap()[1].moderation_state,
        "rejected"
    );
}

fn public_request(store: &Store, title: &str) -> String {
    store
        .create_guest_request(
            "feedback",
            ryu_feedback_board::model::CreateGuestRequest {
                title: title.into(),
                body: "Count every backer exactly once.".into(),
                category: "Ideas".into(),
                ..Default::default()
            },
            "author",
        )
        .unwrap()
        .id
}

async fn vote(
    app: &axum::Router,
    request_id: &str,
    header_voter: &str,
    cookie: Option<&str>,
) -> axum::response::Response {
    vote_from(app, request_id, header_voter, cookie, None).await
}

/// A vote as Core forwards it: `rate_subject` is the `x-ryu-caller-rate-subject`
/// Core stamped for the client (`None` = Core could not tell clients apart).
async fn vote_from(
    app: &axum::Router,
    request_id: &str,
    header_voter: &str,
    cookie: Option<&str>,
    rate_subject: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!("/feedback/requests/{request_id}/vote"))
        .header("x-feedback-voter", header_voter);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    if let Some(subject) = rate_subject {
        builder = builder.header("x-ryu-caller-rate-subject", subject);
    }
    app.clone()
        .oneshot(builder.body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap()
}

fn issued_cookie(response: &axum::response::Response) -> String {
    let set_cookie = response
        .headers()
        .get("set-cookie")
        .expect("a first vote issues the signed voter cookie")
        .to_str()
        .unwrap();
    assert!(set_cookie.starts_with("ryu_feedback-board_voter="));
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("Path=/feedback"));
    set_cookie.split(';').next().unwrap().to_owned()
}

/// The audit exploit: rotating the caller-chosen `x-feedback-voter` header used to
/// add one counted vote per value. The header no longer selects the voter: one
/// signed cookie is one vote however the header changes.
#[tokio::test]
async fn rotating_the_voter_header_cannot_add_votes_for_one_signed_voter() {
    let (app, store) = app();
    let request_id = public_request(&store, "Signed votes");

    let first = vote(&app, &request_id, "stable", None).await;
    assert_eq!(first.status(), StatusCode::OK);
    let cookie = issued_cookie(&first);
    assert_eq!(json(first).await["vote_count"], 1);

    for header_voter in ["stable", "rotating-1", "rotating-2", "rotating-3"] {
        let repeat = vote(&app, &request_id, header_voter, Some(&cookie)).await;
        assert_eq!(repeat.status(), StatusCode::OK);
        assert!(repeat.headers().get("set-cookie").is_none());
        let body = json(repeat).await;
        assert_eq!(body["voted"], false);
        assert_eq!(body["vote_count"], 1);
    }
    let stored = store
        .public_request("feedback", &request_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.vote_count, 1);
}

/// A forged or unsigned cookie is not an identity: it is treated as no cookie,
/// so the caller is issued a real one (and counted once), never keyed on the
/// forged value.
#[tokio::test]
async fn an_unsigned_voter_cookie_is_not_trusted() {
    let (app, store) = app();
    let request_id = public_request(&store, "Forged cookie");
    let forged = vote(
        &app,
        &request_id,
        "stable",
        Some("ryu_feedback-board_voter=v1.00000000000000000000000000000000.deadbeef"),
    )
    .await;
    assert_eq!(forged.status(), StatusCode::OK);
    let issued = issued_cookie(&forged);
    assert!(!issued.contains("deadbeef"));
}

/// Discarding the cookie to get a fresh identity is bounded server-side: past the
/// issuance bound the vote is refused with 429 and no row is inserted.
#[tokio::test]
async fn voter_rotation_is_bounded_and_refused_votes_insert_nothing() {
    let (app, store) = app();
    let request_id = public_request(&store, "Rotation bound");
    let bound = ryu_feedback_board::voter::MAX_NEW_VOTERS_PER_MINUTE;
    for index in 0..bound {
        let response = vote(&app, &request_id, &format!("rotating-{index}"), None).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let refused = vote(&app, &request_id, "rotating-over", None).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(refused.headers().get("set-cookie").is_none());
    let stored = store
        .public_request("feedback", &request_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.vote_count, bound);
}

/// A vote on a request this board does not publicly show is refused before an
/// identity is issued, so it cannot spend the issuance budget.
#[tokio::test]
async fn a_vote_on_an_unknown_request_issues_no_identity() {
    let (app, _) = app();
    let response = vote(&app, "missing-request", "stable", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(response.headers().get("set-cookie").is_none());
}

const PEER_A: &str = "peer:aaaaaaaaaaaaaaaaaaaaaaaa";
const PEER_B: &str = "peer:bbbbbbbbbbbbbbbbbbbbbbbb";

/// The verifier's gap: one anonymous client discarding its cookie used to add
/// up to the node-wide bound (60 a minute) in votes, and to lock every other
/// first-time voter out. Keyed on Core's per-client rate subject, one client gets
/// only a handful of identities, and a different client can still vote.
#[tokio::test]
async fn one_cookieless_client_is_held_to_a_small_per_client_bound() {
    let (app, store) = app();
    let request_id = public_request(&store, "Per-client bound");
    let bound = ryu_feedback_board::voter::MAX_NEW_VOTERS_PER_CLIENT_PER_HOUR;
    for index in 0..bound {
        let response = vote_from(
            &app,
            &request_id,
            &format!("rotating-{index}"),
            None,
            Some(PEER_A),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let refused = vote_from(&app, &request_id, "rotating-over", None, Some(PEER_A)).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(refused.headers().get("set-cookie").is_none());

    let other = vote_from(&app, &request_id, "someone-else", None, Some(PEER_B)).await;
    assert_eq!(other.status(), StatusCode::OK);
    let other_cookie = issued_cookie(&other);

    let stored = store
        .public_request("feedback", &request_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.vote_count, bound + 1);
    assert!(bound < ryu_feedback_board::voter::MAX_NEW_VOTERS_PER_MINUTE);

    // A returning voter with a cookie never needs a new identity, so the spent
    // per-client bound does not lock them out.
    let again = vote_from(&app, &request_id, "x", Some(&other_cookie), Some(PEER_A)).await;
    assert_eq!(again.status(), StatusCode::OK);
}

/// The issuance windows live in the app database: restarting the sidecar (a new
/// `Ctx` over the same file, as after an idle stop) does not hand a client a
/// fresh budget.
#[tokio::test]
async fn the_per_client_bound_survives_a_sidecar_restart() {
    let dir = std::env::temp_dir().join(format!(
        "ryu-feedback-board-restart-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("feedback-board.db");
    let bound = ryu_feedback_board::voter::MAX_NEW_VOTERS_PER_CLIENT_PER_HOUR;
    let request_id = {
        let store = Store::open(&path).unwrap();
        let app = axum::Router::new().merge(routes(Arc::new(Ctx::new(store.clone()).unwrap())));
        let request_id = public_request(&store, "Restart bound");
        for index in 0..bound {
            let response =
                vote_from(&app, &request_id, &format!("r-{index}"), None, Some(PEER_A)).await;
            assert_eq!(response.status(), StatusCode::OK);
        }
        request_id
    };
    let store = Store::open(&path).unwrap();
    let app = axum::Router::new().merge(routes(Arc::new(Ctx::new(store.clone()).unwrap())));
    let refused = vote_from(&app, &request_id, "after-restart", None, Some(PEER_A)).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        store
            .public_request("feedback", &request_id)
            .unwrap()
            .unwrap()
            .vote_count,
        bound
    );
    std::fs::remove_dir_all(dir).ok();
}

/// Core stamps `unknown` when it cannot tell visitors apart (the forwarding
/// tunnel, or a same-host or private-network reverse proxy it does not trust).
/// Those visitors must not share a 5/hour per-client bucket, where one of them
/// clearing cookies would lock out every other first-time voter; they share only
/// the node-wide backstop, which still refuses rotation past its bound with 429
/// and inserts nothing.
#[tokio::test]
async fn visitors_core_cannot_tell_apart_fall_back_to_the_node_wide_backstop() {
    let (app, store) = app();
    let request_id = public_request(&store, "Unknown subject");
    let per_client = ryu_feedback_board::voter::MAX_NEW_VOTERS_PER_CLIENT_PER_HOUR;
    let backstop = ryu_feedback_board::voter::MAX_NEW_VOTERS_PER_MINUTE;
    assert!(per_client < backstop);
    for index in 0..backstop {
        let response = vote_from(
            &app,
            &request_id,
            &format!("visitor-{index}"),
            None,
            Some("unknown"),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "first-time voter {index} behind an unidentifiable proxy"
        );
    }
    let refused = vote_from(&app, &request_id, "visitor-over", None, Some("unknown")).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(refused.headers().get("set-cookie").is_none());
    assert_eq!(
        store
            .public_request("feedback", &request_id)
            .unwrap()
            .unwrap()
            .vote_count,
        backstop
    );
}
