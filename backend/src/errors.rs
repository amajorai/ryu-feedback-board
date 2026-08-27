use axum::{
	http::StatusCode,
	response::{IntoResponse, Response},
	Json,
};
use serde_json::json;

use crate::store::StoreError;

pub struct ApiError {
	pub status: StatusCode,
	pub code: &'static str,
	pub message: String,
}

impl ApiError {
	pub fn bad_request(message: impl Into<String>) -> Self {
		Self {
			status: StatusCode::BAD_REQUEST,
			code: "invalid_request",
			message: message.into(),
		}
	}

	pub fn not_found(message: impl Into<String>) -> Self {
		Self {
			status: StatusCode::NOT_FOUND,
			code: "not_found",
			message: message.into(),
		}
	}

	pub fn conflict(message: impl Into<String>) -> Self {
		Self {
			status: StatusCode::CONFLICT,
			code: "conflict",
			message: message.into(),
		}
	}

	pub fn too_many_requests(message: impl Into<String>) -> Self {
		Self {
			status: StatusCode::TOO_MANY_REQUESTS,
			code: "rate_limited",
			message: message.into(),
		}
	}
}

impl IntoResponse for ApiError {
	fn into_response(self) -> Response {
		(
			self.status,
			Json(json!({
				"error": { "code": self.code, "message": self.message }
			})),
		)
			.into_response()
	}
}

impl From<StoreError> for ApiError {
	fn from(error: StoreError) -> Self {
		match error {
			StoreError::NotFound | StoreError::PublicRequestUnavailable => {
				Self::not_found(error.to_string())
			}
			StoreError::StaleRevision => Self::conflict(error.to_string()),
			StoreError::SelfMerge | StoreError::AlreadyMerged => {
				Self::conflict(error.to_string())
			}
			StoreError::GuestPostsDisabled => Self {
				status: StatusCode::FORBIDDEN,
				code: "guest_posts_disabled",
				message: error.to_string(),
			},
			StoreError::Spam => Self::too_many_requests(error.to_string()),
		}
	}
}

impl From<anyhow::Error> for ApiError {
	fn from(error: anyhow::Error) -> Self {
		if let Some(store_error) = error.downcast_ref::<StoreError>() {
			return store_error.to_owned().into();
		}
		tracing::warn!("feedback-board API error: {error:#}");
		Self {
			status: StatusCode::INTERNAL_SERVER_ERROR,
			code: "server_error",
			message: "Feedback Board could not complete that request.".into(),
		}
	}
}

pub type ApiResult<T> = Result<T, ApiError>;
