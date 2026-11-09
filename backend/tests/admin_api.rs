//! Admin workflow routes: triage, merge, brief link, automation runs, and the
//! changelog draft/publish path as the public board sees it.

use axum::{
    body::to_bytes,
    http::{Request, StatusCode},
};
use ryu_feedback_board::model::{CreateComment, CreateGuestRequest, RequestPatch};
use ryu_feedback_board::{routes, Ctx, Store};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

fn app() -> (axum::Router, Store) {
    let store = Store::open_memory().unwrap();
    (
        axum::Router::new().merge(routes(Arc::new(Ctx::new(store.clone()).unwrap()))),
        store,
    )
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            axum::body::Body::from(value.to_string())
        }
        None => axum::body::Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

fn public_request(store: &Store, title: &str) -> String {
    store
        .create_guest_request(
            "feedback",
            CreateGuestRequest {
                title: title.into(),
                body: "Admin workflow fixture.".into(),
                category: "Ideas".into(),
                ..Default::default()
            },
            "author",
        )
        .unwrap()
        .id
}

fn comment(store: &Store, request_id: &str, body: &str) {
    store
        .add_public_comment(
            "feedback",
            request_id,
            CreateComment {
                body: body.into(),
                ..Default::default()
            },
        )
        .unwrap();
}

fn ship(store: &Store, request_id: &str) {
    let current = store.request_by_id(request_id).unwrap().unwrap();
    store
        .patch_request(
            request_id,
            current.revision,
            RequestPatch {
                status: Some("shipped".into()),
                ..RequestPatch::default()
            },
        )
        .unwrap();
}

#[tokio::test]
async fn merge_moves_votes_and_comments_to_the_survivor_and_closes_the_duplicate() {
    let (app, store) = app();
    let survivor = public_request(&store, "CSV export");
    let duplicate = public_request(&store, "Download as spreadsheet");
    store.vote(&survivor, "voter-a").unwrap();
    store.vote(&survivor, "voter-shared").unwrap();
    store.vote(&duplicate, "voter-shared").unwrap();
    store.vote(&duplicate, "voter-b").unwrap();
    store.vote(&duplicate, "voter-c").unwrap();
    comment(&store, &duplicate, "I need this for finance reports.");

    let (status, body) = send(
        &app,
        "POST",
        &format!("/admin/requests/{survivor}/merge"),
        Some(json!({ "duplicate_request_id": duplicate })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["merge"]["merged_request_id"], duplicate.as_str());
    // a, shared, b, c: the shared voter is counted once.
    assert_eq!(body["merge"]["survivor"]["vote_count"], 4);
    assert_eq!(body["merge"]["survivor"]["comment_count"], 1);

    let comments = store.public_comments(&survivor).unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].body, "I need this for finance reports.");
    assert!(store.admin_comments(&duplicate).unwrap().is_empty());

    let closed = store.request_by_id(&duplicate).unwrap().unwrap();
    assert_eq!(closed.duplicate_of.as_deref(), Some(survivor.as_str()));
    assert_eq!(closed.moderation_state, "merged");
    assert!(!closed.public_visible);
    assert!(store
        .public_request("feedback", &duplicate)
        .unwrap()
        .is_none());

    // The duplicate is closed: a second merge is a conflict, not a double count.
    let (again, _) = send(
        &app,
        "POST",
        &format!("/admin/requests/{survivor}/merge"),
        Some(json!({ "duplicate_request_id": duplicate })),
    )
    .await;
    assert_eq!(again, StatusCode::CONFLICT);
    assert_eq!(
        store.request_by_id(&survivor).unwrap().unwrap().vote_count,
        4
    );
}

/// Sample requests carry counts with no voter rows behind them; a merge keeps
/// those counts instead of dropping them.
#[tokio::test]
async fn merge_keeps_vote_counts_that_have_no_voter_rows() {
    let (app, store) = app();
    store.bootstrap_sample().unwrap();
    let requests = store.list_admin_requests(&Default::default()).unwrap();
    let by_title = |title: &str| {
        requests
            .iter()
            .find(|request| request.title == title)
            .unwrap()
            .id
            .clone()
    };
    let survivor = by_title("Dark mode for the dashboard");
    let duplicate = by_title("Export requests as CSV");
    let (status, body) = send(
        &app,
        "POST",
        &format!("/admin/requests/{survivor}/merge"),
        Some(json!({ "duplicate_request_id": duplicate })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["merge"]["survivor"]["vote_count"], 42 + 28);
}

#[tokio::test]
async fn merge_refuses_self_merge_and_unknown_requests() {
    let (app, store) = app();
    let survivor = public_request(&store, "Self merge");
    let (self_merge, _) = send(
        &app,
        "POST",
        &format!("/admin/requests/{survivor}/merge"),
        Some(json!({ "duplicate_request_id": survivor })),
    )
    .await;
    assert_eq!(self_merge, StatusCode::CONFLICT);
    let (unknown, _) = send(
        &app,
        "POST",
        &format!("/admin/requests/{survivor}/merge"),
        Some(json!({ "duplicate_request_id": "req_missing" })),
    )
    .await;
    assert_eq!(unknown, StatusCode::NOT_FOUND);
    let stored = store.request_by_id(&survivor).unwrap().unwrap();
    assert_eq!(stored.moderation_state, "approved");
    assert!(stored.duplicate_of.is_none());
}

#[tokio::test]
async fn triage_applies_the_suggestion_and_clamps_scores() {
    let (app, store) = app();
    let request_id = public_request(&store, "Dark mode");
    let other = public_request(&store, "Night theme");
    let (status, body) = send(
        &app,
        "POST",
        &format!("/admin/requests/{request_id}/triage"),
        Some(json!({
            "suggestion": {
                "summary": "A request for a dark theme.",
                "tags": ["design", "theme"],
                "category": "Design",
                "impact_score": 250,
                "duplicate_request_ids": [other],
                "confidence": -5
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let stored = store.request_by_id(&request_id).unwrap().unwrap();
    assert_eq!(stored.category, "Design");
    assert_eq!(stored.tags, vec!["design".to_owned(), "theme".to_owned()]);
    assert_eq!(
        stored.ai_summary.as_deref(),
        Some("A request for a dark theme.")
    );
    assert_eq!(stored.impact_score, Some(100));
    assert_eq!(stored.duplicate_confidence, Some(0));
    assert_eq!(stored.duplicate_of.as_deref(), Some(other.as_str()));
    assert_eq!(stored.revision, 1);

    let (missing, _) = send(
        &app,
        "POST",
        "/admin/requests/req_missing/triage",
        Some(json!({
            "suggestion": {
                "summary": "x", "tags": [], "category": "Ideas",
                "impact_score": 1, "duplicate_request_ids": [], "confidence": 1
            }
        })),
    )
    .await;
    assert_eq!(missing, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn brief_link_persists_on_the_request() {
    let (app, store) = app();
    let request_id = public_request(&store, "Brief link");
    let (status, body) = send(
        &app,
        "POST",
        &format!("/admin/requests/{request_id}/brief"),
        Some(json!({ "space_doc_id": "doc_brief_1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["request"]["space_doc_id"], "doc_brief_1");
    let (_, detail) = send(&app, "GET", &format!("/admin/requests/{request_id}"), None).await;
    assert_eq!(detail["request"]["space_doc_id"], "doc_brief_1");
    // Public projections never carry the private brief link.
    let (_, public) = send(
        &app,
        "GET",
        &format!("/feedback/requests/{request_id}"),
        None,
    )
    .await;
    assert!(!public.to_string().contains("doc_brief_1"));

    let (missing, _) = send(
        &app,
        "POST",
        "/admin/requests/req_missing/brief",
        Some(json!({ "space_doc_id": "doc" })),
    )
    .await;
    assert_eq!(missing, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn automation_runs_start_resume_and_read_back() {
    let (app, store) = app();
    let request_id = public_request(&store, "Automate me");
    let (status, started) = send(
        &app,
        "POST",
        &format!("/admin/requests/{request_id}/automation/run"),
        Some(json!({ "mode": "build_review", "agent_id": "agent_1", "workflow_id": "wf_1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let run_id = started["run"]["id"].as_str().unwrap().to_owned();
    assert_eq!(started["run"]["status"], "queued");
    assert_eq!(started["run"]["mode"], "build_review");
    assert_eq!(started["run"]["request_id"], request_id.as_str());

    let (status, resumed) = send(
        &app,
        "POST",
        &format!("/admin/requests/{request_id}/automation/{run_id}/resume"),
        Some(json!({
            "status": "succeeded",
            "workflow_run_id": "wfr_9",
            "result_summary": "Opened a draft change."
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resumed}");

    let (status, read) = send(
        &app,
        "GET",
        &format!("/admin/automation/runs/{run_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["run"]["status"], "succeeded");
    assert_eq!(read["run"]["workflow_run_id"], "wfr_9");
    assert_eq!(read["run"]["workflow_id"], "wf_1");
    assert_eq!(read["run"]["result_summary"], "Opened a draft change.");

    let (_, listed) = send(
        &app,
        "GET",
        &format!("/admin/requests/{request_id}/automation"),
        None,
    )
    .await;
    let runs = listed["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["id"], run_id.as_str());
}

#[tokio::test]
async fn automation_errors_are_not_found_for_unknown_requests_and_runs() {
    let (app, store) = app();
    let request_id = public_request(&store, "Automation errors");
    let (start_missing, _) = send(
        &app,
        "POST",
        "/admin/requests/req_missing/automation/run",
        Some(json!({ "mode": "assist" })),
    )
    .await;
    assert_eq!(start_missing, StatusCode::NOT_FOUND);
    assert!(store
        .automation_runs_for_request("req_missing")
        .unwrap()
        .is_empty());

    let (resume_missing, _) = send(
        &app,
        "POST",
        &format!("/admin/requests/{request_id}/automation/run_missing/resume"),
        Some(json!({ "status": "failed", "error": "boom" })),
    )
    .await;
    assert_eq!(resume_missing, StatusCode::NOT_FOUND);

    let (read_missing, body) = send(&app, "GET", "/admin/automation/runs/run_missing", None).await;
    assert_eq!(read_missing, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn changelog_drafts_stay_private_until_published() {
    let (app, store) = app();
    let request_id = public_request(&store, "Shipped thing");
    ship(&store, &request_id);

    let (status, draft) = send(
        &app,
        "POST",
        "/admin/changelog/drafts",
        Some(json!({ "title": "October release", "body": "Draft notes.", "request_ids": [request_id] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{draft}");
    let release_id = draft["release"]["id"].as_str().unwrap().to_owned();
    assert_eq!(draft["release"]["status"], "draft");
    assert!(draft["release"]["published_at"].is_null());

    let (_, public_list) = send(&app, "GET", "/feedback/changelog", None).await;
    assert!(public_list["releases"].as_array().unwrap().is_empty());
    let (hidden, _) = send(
        &app,
        "GET",
        &format!("/feedback/changelog/{release_id}"),
        None,
    )
    .await;
    assert_eq!(hidden, StatusCode::NOT_FOUND);
    let (_, admin_list) = send(&app, "GET", "/admin/changelog", None).await;
    assert_eq!(admin_list["releases"].as_array().unwrap().len(), 1);

    let (status, published) = send(
        &app,
        "POST",
        &format!("/admin/changelog/{release_id}/publish"),
        Some(json!({ "body": "Final notes." })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{published}");
    assert_eq!(published["release"]["status"], "published");
    assert!(published["release"]["published_at"].is_i64());

    let (_, public_list) = send(&app, "GET", "/feedback/changelog", None).await;
    let releases = public_list["releases"].as_array().unwrap();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0]["body"], "Final notes.");
    assert_eq!(releases[0]["request_ids"], json!([request_id]));
    let (shown, detail) = send(
        &app,
        "GET",
        &format!("/feedback/changelog/{release_id}"),
        None,
    )
    .await;
    assert_eq!(shown, StatusCode::OK);
    assert_eq!(detail["release"]["title"], "October release");

    let (missing, _) = send(
        &app,
        "POST",
        "/admin/changelog/release_missing/publish",
        Some(json!({})),
    )
    .await;
    assert_eq!(missing, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn changelog_drafts_refuse_unshipped_or_empty_request_lists() {
    let (app, store) = app();
    let request_id = public_request(&store, "Not shipped yet");
    for request_ids in [json!([request_id]), json!([])] {
        let (status, _) = send(
            &app,
            "POST",
            "/admin/changelog/drafts",
            Some(json!({ "title": "Too early", "body": "Notes.", "request_ids": request_ids })),
        )
        .await;
        assert!(!status.is_success(), "draft accepted for {request_ids}");
    }
    assert!(store.admin_releases().unwrap().is_empty());
}

#[tokio::test]
async fn public_roadmap_groups_visible_requests_by_status() {
    let (app, store) = app();
    let shipped = public_request(&store, "Shipped column");
    let planned = public_request(&store, "Planned column");
    let hidden = public_request(&store, "Hidden planned request");
    ship(&store, &shipped);
    for (request_id, visible) in [(&planned, true), (&hidden, false)] {
        let current = store.request_by_id(request_id).unwrap().unwrap();
        store
            .patch_request(
                request_id,
                current.revision,
                RequestPatch {
                    status: Some("planned".into()),
                    public_visible: Some(visible),
                    ..RequestPatch::default()
                },
            )
            .unwrap();
    }

    let (status, body) = send(&app, "GET", "/feedback/roadmap", None).await;
    assert_eq!(status, StatusCode::OK);
    let columns = body["columns"].as_array().unwrap();
    let column = |code: &str| -> Vec<String> {
        columns
            .iter()
            .find(|column| column["code"] == code)
            .unwrap_or_else(|| panic!("missing column {code}"))["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|request| request["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(column("shipped"), vec![shipped.clone()]);
    assert_eq!(column("planned"), vec![planned.clone()]);
    let total: usize = columns
        .iter()
        .map(|column| column["requests"].as_array().unwrap().len())
        .sum();
    // The hidden request is still planned, but never reaches the public roadmap.
    assert_eq!(total, 2);
    assert!(!body.to_string().contains(hidden.as_str()));

    let (unknown, _) = send(&app, "GET", "/nope/roadmap", None).await;
    assert_eq!(unknown, StatusCode::NOT_FOUND);
}
