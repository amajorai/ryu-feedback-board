use serde::{Deserialize, Serialize};

pub type Timestamp = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationMode {
	Manual,
	Assist,
	Prepare,
	BuildReview,
	Autopilot,
}

impl AutomationMode {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Manual => "manual",
			Self::Assist => "assist",
			Self::Prepare => "prepare",
			Self::BuildReview => "build_review",
			Self::Autopilot => "autopilot",
		}
	}
}

impl Default for AutomationMode {
	fn default() -> Self {
		Self::Assist
	}
}

impl TryFrom<&str> for AutomationMode {
	type Error = String;

	fn try_from(value: &str) -> Result<Self, Self::Error> {
		match value {
			"manual" => Ok(Self::Manual),
			"assist" => Ok(Self::Assist),
			"prepare" => Ok(Self::Prepare),
			"build_review" => Ok(Self::BuildReview),
			"autopilot" => Ok(Self::Autopilot),
			other => Err(format!("unknown automation mode '{other}'")),
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
	pub id: String,
	pub name: String,
	pub slug: String,
	pub description: String,
	pub primary_color: String,
	pub default_automation_mode: AutomationMode,
	pub default_agent_id: Option<String>,
	pub default_workflow_id: Option<String>,
	pub allow_guest_posts: bool,
	pub moderate_public_writes: bool,
	pub revision: i64,
	pub created_at: Timestamp,
	pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
	pub id: String,
	pub workspace_id: String,
	pub slug: String,
	pub name: String,
	pub description: String,
	pub automation_mode: Option<AutomationMode>,
	pub allow_guest_posts: bool,
	pub moderate_public_writes: bool,
	pub revision: i64,
	pub created_at: Timestamp,
	pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardStatus {
	pub code: String,
	pub label: String,
	pub tone: String,
	pub public: bool,
	pub terminal: bool,
	pub position: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicStatus {
	pub code: String,
	pub label: String,
	pub tone: String,
	pub terminal: bool,
}

impl From<BoardStatus> for PublicStatus {
	fn from(status: BoardStatus) -> Self {
		Self {
			code: status.code,
			label: status.label,
			tone: status.tone,
			terminal: status.terminal,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicBoard {
	pub workspace_name: String,
	pub workspace_description: String,
	pub workspace_primary_color: String,
	pub board_id: String,
	pub board_slug: String,
	pub board_name: String,
	pub board_description: String,
	pub statuses: Vec<PublicStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
	pub id: String,
	pub board_id: String,
	pub title: String,
	pub body: String,
	pub author_name: String,
	pub author_email: Option<String>,
	pub category: String,
	pub status: String,
	pub tags: Vec<String>,
	pub vote_count: i64,
	pub comment_count: i64,
	pub duplicate_of: Option<String>,
	pub duplicate_confidence: Option<i64>,
	pub impact_score: Option<i64>,
	pub priority: i64,
	pub internal_notes: String,
	pub ai_summary: Option<String>,
	pub moderation_state: String,
	pub public_visible: bool,
	pub space_doc_id: Option<String>,
	pub plan_id: Option<String>,
	pub workflow_run_id: Option<String>,
	pub automation_mode: Option<AutomationMode>,
	pub revision: i64,
	pub created_at: Timestamp,
	pub updated_at: Timestamp,
	pub shipped_at: Option<Timestamp>,
}

impl Request {
	pub fn fixture() -> Self {
		Self {
			id: "req_fixture".into(),
			board_id: "board_main".into(),
			title: "CSV export".into(),
			body: "Export rows from the request list.".into(),
			author_name: "Ada".into(),
			author_email: None,
			category: "Ideas".into(),
			status: "review".into(),
			tags: vec!["reporting".into()],
			vote_count: 3,
			comment_count: 0,
			duplicate_of: None,
			duplicate_confidence: None,
			impact_score: Some(70),
			priority: 0,
			internal_notes: "Private note".into(),
			ai_summary: Some("A request for structured export.".into()),
			moderation_state: "approved".into(),
			public_visible: true,
			space_doc_id: Some("doc_fixture".into()),
			plan_id: Some("feedback-board/req_fixture".into()),
			workflow_run_id: Some("run_fixture".into()),
			automation_mode: Some(AutomationMode::BuildReview),
			revision: 0,
			created_at: 1,
			updated_at: 1,
			shipped_at: None,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicRequest {
	pub id: String,
	pub title: String,
	pub body: String,
	pub author_name: String,
	pub category: String,
	pub status: PublicStatus,
	pub tags: Vec<String>,
	pub vote_count: i64,
	pub comment_count: i64,
	pub created_at: Timestamp,
	pub updated_at: Timestamp,
	pub shipped_at: Option<Timestamp>,
}

impl PublicRequest {
	pub fn from_request(request: Request, status: PublicStatus) -> Self {
		Self {
			id: request.id,
			title: request.title,
			body: request.body,
			author_name: request.author_name,
			category: request.category,
			status,
			tags: request.tags,
			vote_count: request.vote_count,
			comment_count: request.comment_count,
			created_at: request.created_at,
			updated_at: request.updated_at,
			shipped_at: request.shipped_at,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
	pub id: String,
	pub request_id: String,
	pub author_name: String,
	pub body: String,
	pub visibility: String,
	pub moderation_state: String,
	pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicComment {
	pub id: String,
	pub author_name: String,
	pub body: String,
	pub created_at: Timestamp,
}

impl From<Comment> for PublicComment {
	fn from(comment: Comment) -> Self {
		Self {
			id: comment.id,
			author_name: comment.author_name,
			body: comment.body,
			created_at: comment.created_at,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
	pub id: String,
	pub workspace_id: String,
	pub title: String,
	pub body: String,
	pub status: String,
	pub request_ids: Vec<String>,
	pub published_at: Option<Timestamp>,
	pub created_at: Timestamp,
	pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationSettings {
	pub mode: AutomationMode,
	pub agent_id: Option<String>,
	pub workflow_id: Option<String>,
	pub max_tokens: u32,
	pub wall_time_secs: u32,
	pub require_public_status_approval: bool,
	pub require_blueprint_approval: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationRun {
	pub id: String,
	pub request_id: String,
	pub mode: AutomationMode,
	pub agent_id: Option<String>,
	pub workflow_id: Option<String>,
	pub workflow_run_id: Option<String>,
	pub plan_id: Option<String>,
	pub status: String,
	pub error: Option<String>,
	pub result_summary: Option<String>,
	pub created_at: Timestamp,
	pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateGuestRequest {
	pub title: String,
	pub body: String,
	pub category: String,
	pub author_name: Option<String>,
	pub email: Option<String>,
	pub honeypot: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateComment {
	pub body: String,
	pub author_name: Option<String>,
	pub email: Option<String>,
	pub honeypot: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestPatch {
	pub title: Option<String>,
	pub body: Option<String>,
	pub category: Option<String>,
	pub status: Option<String>,
	pub tags: Option<Vec<String>>,
	pub priority: Option<i64>,
	pub internal_notes: Option<String>,
	pub public_visible: Option<bool>,
	pub duplicate_of: Option<Option<String>>,
	pub duplicate_confidence: Option<Option<i64>>,
	pub impact_score: Option<Option<i64>>,
	pub ai_summary: Option<Option<String>>,
	pub space_doc_id: Option<Option<String>>,
	pub plan_id: Option<Option<String>>,
	pub workflow_run_id: Option<Option<String>>,
	pub automation_mode: Option<Option<AutomationMode>>,
	pub moderation_state: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspacePatch {
	pub name: Option<String>,
	pub description: Option<String>,
	pub primary_color: Option<String>,
	pub default_automation_mode: Option<AutomationMode>,
	pub default_agent_id: Option<Option<String>>,
	pub default_workflow_id: Option<Option<String>>,
	pub allow_guest_posts: Option<bool>,
	pub moderate_public_writes: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoardPatch {
	pub name: Option<String>,
	pub description: Option<String>,
	pub automation_mode: Option<Option<AutomationMode>>,
	pub allow_guest_posts: Option<bool>,
	pub moderate_public_writes: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PublicQuery {
	pub query: Option<String>,
	pub sort: Option<String>,
	pub status: Option<String>,
	pub category: Option<String>,
	pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdminQuery {
	pub query: Option<String>,
	pub status: Option<String>,
	pub category: Option<String>,
	pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoteResult {
	pub voted: bool,
	pub vote_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResult {
	pub survivor: Request,
	pub merged_request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageSuggestion {
	pub summary: String,
	pub tags: Vec<String>,
	pub category: String,
	pub impact_score: i64,
	pub duplicate_request_ids: Vec<String>,
	pub confidence: i64,
}
