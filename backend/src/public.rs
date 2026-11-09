use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{
        header::{ACCEPT_LANGUAGE, COOKIE, ORIGIN, SET_COOKIE},
        HeaderMap, HeaderValue,
    },
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use ryu_sidecar_runtime::branding;
use serde_json::json;

use crate::api::Ctx;
use crate::errors::{ApiError, ApiResult};
use crate::model::{CreateComment, CreateGuestRequest, PublicQuery};
use crate::public_html::PUBLIC_HTML;
use crate::public_locale::{self, PublicLocale};
use crate::voter::{rate_subject, RATE_SUBJECT_HEADER, VOTER_COOKIE, VOTER_COOKIE_MAX_AGE_SECS};

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
    headers: HeaderMap,
) -> ApiResult<Response> {
    if ctx.store.public_board(&board_slug)?.is_none() {
        return Err(ApiError::not_found("public board not found"));
    }
    let locale = public_locale::negotiate(
        headers
            .get(ACCEPT_LANGUAGE)
            .and_then(|value| value.to_str().ok()),
        public_locale::owner_locale_from_env().as_deref(),
    );
    let html = public_shell_html(branding::branding_removable(), &locale);
    // The shell differs by visitor language, so shared caches must key on it.
    Ok(([("vary", "Accept-Language")], Html(html)).into_response())
}

/// The public board shell in the visitor's language, carrying the "Powered by
/// Ryu" badge unless the owner's plan drops it.
fn public_shell_html(branding_removable: bool, locale: &PublicLocale) -> String {
    let html = public_locale::localize_shell(PUBLIC_HTML, locale);
    if branding_removable {
        html
    } else {
        let copy = branding::PoweredByRyuCopy {
            label: locale.tl(branding::POWERED_BY_RYU_LABEL),
            aria_label: locale.tl(branding::POWERED_BY_RYU_ARIA_LABEL),
            dir: locale.dir,
        };
        branding::inject_powered_by_ryu_with(&html, "feedback-board", &copy)
    }
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
    let board = ctx
        .store
        .board_by_slug(&board_slug)?
        .ok_or_else(|| ApiError::not_found("public board not found"))?;
    let pending_review = board.moderate_public_writes;
    let request = ctx.store.create_guest_request(&board_slug, input, "")?;
    Ok(Json(json!({
        "request": request,
        "pendingReview": pending_review,
    })))
}

/// A public vote is keyed on the server-issued, signed voter cookie (see
/// [`crate::voter`]). A caller-chosen header never selects the voter key. A caller
/// without a valid cookie is issued one, within the per-client issuance bound
/// keyed on Core's rate subject (and the node-wide backstop); past either bound
/// the vote is refused before any row is written.
async fn vote_request(
    State(ctx): State<Arc<Ctx>>,
    Path((board_slug, request_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    // Checked before a voter identity is issued, so a vote that cannot count does
    // not spend the issuance budget.
    if ctx
        .store
        .public_request(&board_slug, &request_id)?
        .is_none()
    {
        return Err(ApiError::not_found("public request not found"));
    }
    let verified = cookie_value(&headers, VOTER_COOKIE).and_then(|token| ctx.voters.verify(token));
    let (voter, issued_token) = match verified {
        Some(voter) => (voter, None),
        None => {
            let subject = rate_subject(
                headers
                    .get(RATE_SUBJECT_HEADER)
                    .and_then(|value| value.to_str().ok()),
            );
            let issued = ctx.voters.issue(&ctx.store, subject)?.ok_or_else(|| {
                ApiError::too_many_requests("too many new voters right now; try again later")
            })?;
            (issued.voter_key, Some(issued.token))
        }
    };
    let result = ctx.store.vote(&request_id, &voter)?;
    let mut response = Json(json!(result)).into_response();
    if let Some(token) = issued_token {
        response
            .headers_mut()
            .insert(SET_COOKIE, voter_cookie(&token, is_https_request(&headers)));
    }
    Ok(response)
}

async fn create_comment(
    State(ctx): State<Arc<Ctx>>,
    Path((board_slug, request_id)): Path<(String, String)>,
    Json(input): Json<CreateComment>,
) -> ApiResult<Json<serde_json::Value>> {
    let board = ctx
        .store
        .board_by_slug(&board_slug)?
        .ok_or_else(|| ApiError::not_found("public board not found"))?;
    let pending_review = board.moderate_public_writes;
    let comment = ctx
        .store
        .add_public_comment(&board_slug, &request_id, input)?;
    Ok(Json(json!({
        "comment": comment,
        "pendingReview": pending_review,
    })))
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

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .find_map(|cookie| {
            let (cookie_name, cookie_value) = cookie.trim().split_once('=')?;
            (cookie_name == name && !cookie_value.is_empty()).then_some(cookie_value)
        })
}

/// Scoped to the public mount so the browser sends it only to this board.
fn voter_cookie(token: &str, secure: bool) -> HeaderValue {
    let secure_suffix = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{VOTER_COOKIE}={token}; Path=/feedback; Max-Age={VOTER_COOKIE_MAX_AGE_SECS}; HttpOnly; SameSite=Lax{secure_suffix}"
    ))
    .expect("an issued voter token is a valid cookie value")
}

fn is_https_request(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("https"))
        || headers
            .get(ORIGIN)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.trim_start().starts_with("https://"))
}

#[cfg(test)]
mod tests {
    use super::public_shell_html;
    use crate::public_locale::negotiate;

    #[test]
    fn free_boards_carry_the_powered_by_badge() {
        let english = negotiate(None, None);
        assert!(
            public_shell_html(false, &english).contains("data-ryu-powered-by=\"feedback-board\"")
        );
        assert!(!public_shell_html(true, &english).contains("data-ryu-powered-by"));
    }

    #[test]
    fn the_badge_follows_the_visitor_language() {
        let html = public_shell_html(false, &negotiate(Some("ar"), None));
        let badge = &html[html.find("data-ryu-powered-by").unwrap()..];
        assert!(badge.contains("dir=\"rtl\""));
        assert!(badge.contains("inset-inline-end:12px"));
        assert!(badge.contains("مدعوم من <strong style=\"font-weight:700\">Ryu</strong>"));
        assert!(badge.contains("aria-label=\"مدعوم من Ryu — أنشئ الخاص بك\""));
        let english = public_shell_html(false, &negotiate(Some("en"), None));
        assert!(english.contains("aria-label=\"Powered by Ryu — make your own\""));
    }

    #[test]
    fn the_shell_follows_the_visitor_language() {
        let html = public_shell_html(true, &negotiate(Some("ar"), None));
        assert!(html.starts_with("<!doctype html>\n<html lang=\"ar\" dir=\"rtl\">"));
        assert!(html.contains("<script type=\"application/json\" id=\"feedback-board-i18n\">"));
        // The full catalog slice never pushes the charset declaration out of
        // the first 1024 bytes the browser prescans.
        assert!(html.find("<meta charset=\"UTF-8\">").unwrap() < 1024);
    }
}
