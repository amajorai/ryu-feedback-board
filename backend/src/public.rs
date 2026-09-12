use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Html,
    routing::{get, post},
    Json, Router,
};
use serde_json::json;

use crate::api::Ctx;
use crate::errors::{ApiError, ApiResult};
use crate::model::{CreateComment, CreateGuestRequest, PublicQuery};
use crate::public_html::PUBLIC_HTML;

pub fn routes(ctx: Arc<Ctx>) -> Router {
    Router::new()
        .route("/:boardSlug", get(public_shell))
        .route(
            "/:boardSlug/requests",
            get(list_requests).post(create_request),
        )
        .route("/:boardSlug/requests/:requestId", get(get_request))
        .route("/:boardSlug/requests/:requestId/vote", post(vote_request))
        .route(
            "/:boardSlug/requests/:requestId/comments",
            post(create_comment),
        )
        .route("/:boardSlug/roadmap", get(roadmap))
        .route("/:boardSlug/changelog", get(changelog))
        .route("/:boardSlug/changelog/:releaseId", get(changelog_detail))
        .with_state(ctx)
}

async fn public_shell(
    State(ctx): State<Arc<Ctx>>,
    Path(board_slug): Path<String>,
) -> ApiResult<Html<&'static str>> {
    if ctx.store.public_board(&board_slug)?.is_none() {
        return Err(ApiError::not_found("public board not found"));
    }
    Ok(Html(PUBLIC_HTML))
}

async fn list_requests(
    State(ctx): State<Arc<Ctx>>,
    Path(board_slug): Path<String>,
    Query(query): Query<PublicQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let board = ctx
        .store
        .public_board(&board_slug)?
        .ok_or_else(|| ApiError::not_found("public board not found"))?;
    let requests = ctx.store.list_public_requests(&board_slug, &query)?;
    Ok(Json(json!({ "board": board, "requests": requests })))
}

async fn get_request(
    State(ctx): State<Arc<Ctx>>,
    Path((board_slug, request_id)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let request = ctx
        .store
        .public_request(&board_slug, &request_id)?
        .ok_or_else(|| ApiError::not_found("public request not found"))?;
    let comments = ctx.store.public_comments(&request_id)?;
    Ok(Json(json!({ "request": request, "comments": comments })))
}

async fn create_request(
    State(ctx): State<Arc<Ctx>>,
    Path(board_slug): Path<String>,
    Json(input): Json<CreateGuestRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let request = ctx.store.create_guest_request(&board_slug, input, "")?;
    Ok(Json(json!({ "request": request })))
}

async fn vote_request(
    State(ctx): State<Arc<Ctx>>,
    Path((_board_slug, request_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let voter = voter_hash(&headers);
    let result = ctx.store.vote(&request_id, &voter)?;
    Ok(Json(json!(result)))
}

async fn create_comment(
    State(ctx): State<Arc<Ctx>>,
    Path((board_slug, request_id)): Path<(String, String)>,
    Json(input): Json<CreateComment>,
) -> ApiResult<Json<serde_json::Value>> {
    let comment = ctx
        .store
        .add_public_comment(&board_slug, &request_id, input)?;
    Ok(Json(json!({ "comment": comment })))
}

async fn roadmap(
    State(ctx): State<Arc<Ctx>>,
    Path(board_slug): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let board = ctx
        .store
        .public_board(&board_slug)?
        .ok_or_else(|| ApiError::not_found("public board not found"))?;
    let requests = ctx
        .store
        .list_public_requests(&board_slug, &PublicQuery::default())?;
    let columns = board
		.statuses
		.iter()
		.map(|status| {
			json!({
				"code": status.code,
				"label": status.label,
				"tone": status.tone,
				"requests": requests.iter().filter(|request| request.status.code == status.code).collect::<Vec<_>>(),
			})
		})
		.collect::<Vec<_>>();
    Ok(Json(json!({ "board": board, "columns": columns })))
}

async fn changelog(
    State(ctx): State<Arc<Ctx>>,
    Path(board_slug): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let board = ctx
        .store
        .public_board(&board_slug)?
        .ok_or_else(|| ApiError::not_found("public board not found"))?;
    let releases = ctx.store.public_releases()?;
    Ok(Json(json!({ "board": board, "releases": releases })))
}

async fn changelog_detail(
    State(ctx): State<Arc<Ctx>>,
    Path((board_slug, release_id)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    if ctx.store.public_board(&board_slug)?.is_none() {
        return Err(ApiError::not_found("public board not found"));
    }
    let release = ctx
        .store
        .public_release(&release_id)?
        .ok_or_else(|| ApiError::not_found("release not found"))?;
    Ok(Json(json!({ "release": release })))
}

fn voter_hash(headers: &HeaderMap) -> String {
    headers
        .get("x-feedback-voter")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .unwrap_or("anonymous")
        .to_owned()
}
