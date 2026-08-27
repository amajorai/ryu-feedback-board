use std::sync::Arc;

use axum::{
	extract::{Path, Query, State},
	routing::{get, post, put},
	Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::api::Ctx;
use crate::errors::{ApiError, ApiResult};
use crate::model::{
	AdminQuery, AutomationMode, BoardPatch, RequestPatch, TriageSuggestion, WorkspacePatch,
};

#[derive(Debug, Deserialize)]
struct RevisionPatch<T> {
	revision: i64,
	#[serde(flatten)]
	patch: T,
}

#[derive(Debug, Deserialize)]
struct CreateBoard {
	slug: String,
	name: String,
	description: String,
}

#[derive(Debug, Deserialize)]
struct TriageBody {
	suggestion: TriageSuggestion,
}

#[derive(Debug, Deserialize)]
struct MergeBody {
	duplicate_request_id: String,
}

#[derive(Debug, Deserialize)]
struct BriefBody {
	space_doc_id: String,
}

#[derive(Debug, Deserialize)]
struct AutomationRunBody {
	mode: AutomationMode,
	agent_id: Option<String>,
	workflow_id: Option<String>,
	workflow_run_id: Option<String>,
	plan_id: Option<String>,
	status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AutomationResumeBody {
	status: String,
	plan_id: Option<String>,
	workflow_run_id: Option<String>,
	result_summary: Option<String>,
	error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReleaseDraft {
	title: String,
	body: String,
	request_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PublishBody {
	body: Option<String>,
}

pub fn routes(ctx: Arc<Ctx>) -> Router {
	Router::new()
		.route("/workspace", get(get_workspace).put(patch_workspace))
		.route("/boards", get(list_boards).post(create_board))
		.route("/boards/:boardId", put(patch_board))
		.route("/requests", get(list_requests))
		.route(
			"/requests/:requestId",
			get(get_request).put(patch_request),
		)
		.route("/requests/:requestId/triage", post(apply_triage))
		.route("/requests/:requestId/merge", post(merge_request))
		.route("/requests/:requestId/brief", post(link_brief))
		.route(
			"/requests/:requestId/automation",
			get(get_request_automation),
		)
		.route(
			"/requests/:requestId/automation/run",
			post(start_automation),
		)
		.route(
			"/requests/:requestId/automation/:runId/resume",
			post(resume_automation),
		)
		.route("/automation/runs/:runId", get(get_automation_run))
		.route("/roadmap", get(get_roadmap))
		.route("/changelog", get(get_changelog))
		.route("/changelog/drafts", post(create_release_draft))
		.route("/changelog/:releaseId/publish", post(publish_release))
		.with_state(ctx)
}

async fn get_workspace(State(ctx): State<Arc<Ctx>>) -> ApiResult<Json<serde_json::Value>> {
	Ok(Json(json!({ "workspace": ctx.store.workspace()? })))
}

async fn patch_workspace(
	State(ctx): State<Arc<Ctx>>,
	Json(input): Json<RevisionPatch<WorkspacePatch>>,
) -> ApiResult<Json<serde_json::Value>> {
	let workspace = ctx.store.patch_workspace(input.revision, input.patch)?;
	Ok(Json(json!({ "workspace": workspace })))
}

async fn list_boards(State(ctx): State<Arc<Ctx>>) -> ApiResult<Json<serde_json::Value>> {
	Ok(Json(json!({ "boards": ctx.store.list_boards()? })))
}

async fn create_board(
	State(ctx): State<Arc<Ctx>>,
	Json(input): Json<CreateBoard>,
) -> ApiResult<Json<serde_json::Value>> {
	let board = ctx
		.store
		.create_board(&input.slug, &input.name, &input.description)?;
	Ok(Json(json!({ "board": board })))
}

async fn patch_board(
	State(ctx): State<Arc<Ctx>>,
	Path(board_id): Path<String>,
	Json(input): Json<RevisionPatch<BoardPatch>>,
) -> ApiResult<Json<serde_json::Value>> {
	let board = ctx
		.store
		.patch_board(&board_id, input.revision, input.patch)?;
	Ok(Json(json!({ "board": board })))
}

async fn list_requests(
	State(ctx): State<Arc<Ctx>>,
	Query(query): Query<AdminQuery>,
) -> ApiResult<Json<serde_json::Value>> {
	Ok(Json(json!({ "requests": ctx.store.list_admin_requests(&query)? })))
}

async fn get_request(
	State(ctx): State<Arc<Ctx>>,
	Path(request_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
	let request = ctx
		.store
		.request_by_id(&request_id)?
		.ok_or_else(|| ApiError::not_found("request not found"))?;
	let comments = ctx.store.admin_comments(&request_id)?;
	Ok(Json(json!({ "request": request, "comments": comments })))
}

async fn patch_request(
	State(ctx): State<Arc<Ctx>>,
	Path(request_id): Path<String>,
	Json(input): Json<RevisionPatch<RequestPatch>>,
) -> ApiResult<Json<serde_json::Value>> {
	let request = ctx
		.store
		.patch_request(&request_id, input.revision, input.patch)?;
	Ok(Json(json!({ "request": request })))
}

async fn apply_triage(
	State(ctx): State<Arc<Ctx>>,
	Path(request_id): Path<String>,
	Json(input): Json<TriageBody>,
) -> ApiResult<Json<serde_json::Value>> {
	let current = ctx
		.store
		.request_by_id(&request_id)?
		.ok_or_else(|| ApiError::not_found("request not found"))?;
	let duplicate = input.suggestion.duplicate_request_ids.first().cloned();
	let request = ctx.store.patch_request(
		&request_id,
		current.revision,
		RequestPatch {
			category: Some(input.suggestion.category.clone()),
			tags: Some(input.suggestion.tags.clone()),
			ai_summary: Some(Some(input.suggestion.summary.clone())),
			impact_score: Some(Some(input.suggestion.impact_score.clamp(0, 100))),
			duplicate_of: Some(duplicate),
			duplicate_confidence: Some(Some(input.suggestion.confidence.clamp(0, 100))),
			..RequestPatch::default()
		},
	)?;
	Ok(Json(json!({ "request": request, "suggestion": input.suggestion })))
}

async fn merge_request(
	State(ctx): State<Arc<Ctx>>,
	Path(request_id): Path<String>,
	Json(input): Json<MergeBody>,
) -> ApiResult<Json<serde_json::Value>> {
	let result = ctx.store.merge_requests(&request_id, &input.duplicate_request_id)?;
	Ok(Json(json!({ "merge": result })))
}

async fn link_brief(
	State(ctx): State<Arc<Ctx>>,
	Path(request_id): Path<String>,
	Json(input): Json<BriefBody>,
) -> ApiResult<Json<serde_json::Value>> {
	let current = ctx
		.store
		.request_by_id(&request_id)?
		.ok_or_else(|| ApiError::not_found("request not found"))?;
	let request = ctx.store.patch_request(
		&request_id,
		current.revision,
		RequestPatch {
			space_doc_id: Some(Some(input.space_doc_id)),
			..RequestPatch::default()
		},
	)?;
	Ok(Json(json!({ "request": request })))
}

async fn get_request_automation(
	State(ctx): State<Arc<Ctx>>,
	Path(request_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
	let request = ctx
		.store
		.request_by_id(&request_id)?
		.ok_or_else(|| ApiError::not_found("request not found"))?;
	let workspace = ctx.store.workspace()?;
	Ok(Json(json!({
		"request": request,
		"workspace": workspace,
		"runs": ctx.store.automation_runs_for_request(&request_id)?,
	})))
}

async fn start_automation(
	State(ctx): State<Arc<Ctx>>,
	Path(request_id): Path<String>,
	Json(input): Json<AutomationRunBody>,
) -> ApiResult<Json<serde_json::Value>> {
	let run = ctx.store.create_automation_run(
		&request_id,
		input.mode,
		input.agent_id,
		input.workflow_id,
		input.workflow_run_id,
		input.plan_id,
		input.status.as_deref().unwrap_or("queued"),
	)?;
	Ok(Json(json!({ "run": run })))
}

async fn resume_automation(
	State(ctx): State<Arc<Ctx>>,
	Path((_request_id, run_id)): Path<(String, String)>,
	Json(input): Json<AutomationResumeBody>,
) -> ApiResult<Json<serde_json::Value>> {
	let run = ctx.store.update_automation_run(
		&run_id,
		&input.status,
		input.workflow_run_id,
		input.plan_id,
		input.result_summary,
		input.error,
	)?;
	Ok(Json(json!({ "run": run })))
}

async fn get_automation_run(
	State(ctx): State<Arc<Ctx>>,
	Path(run_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
	let run = ctx
		.store
		.automation_run(&run_id)?
		.ok_or_else(|| ApiError::not_found("automation run not found"))?;
	Ok(Json(json!({ "run": run })))
}

async fn get_roadmap(State(ctx): State<Arc<Ctx>>) -> ApiResult<Json<serde_json::Value>> {
	let board = ctx
		.store
		.board_by_id("board_feedback")?
		.ok_or_else(|| ApiError::not_found("board not found"))?;
	Ok(Json(json!({
		"board": board,
		"statuses": ctx.store.statuses("board_feedback")?,
		"requests": ctx.store.list_admin_requests(&AdminQuery::default())?,
	})))
}

async fn get_changelog(State(ctx): State<Arc<Ctx>>) -> ApiResult<Json<serde_json::Value>> {
	Ok(Json(json!({ "releases": ctx.store.admin_releases()? })))
}

async fn create_release_draft(
	State(ctx): State<Arc<Ctx>>,
	Json(input): Json<ReleaseDraft>,
) -> ApiResult<Json<serde_json::Value>> {
	let release = ctx
		.store
		.create_release(&input.title, &input.body, &input.request_ids)?;
	Ok(Json(json!({ "release": release })))
}

async fn publish_release(
	State(ctx): State<Arc<Ctx>>,
	Path(release_id): Path<String>,
	Json(input): Json<PublishBody>,
) -> ApiResult<Json<serde_json::Value>> {
	let release = ctx
		.store
		.publish_release(&release_id, input.body.as_deref())?;
	Ok(Json(json!({ "release": release })))
}
