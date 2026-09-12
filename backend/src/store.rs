use anyhow::{Context, Result};
use rusqlite::{params, types::Value, Connection, OptionalExtension, Row};
use std::path::Path;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use uuid::Uuid;

use crate::model::{
    AdminQuery, AutomationMode, AutomationRun, Board, BoardPatch, BoardStatus, Comment,
    CreateComment, CreateGuestRequest, MergeResult, PublicBoard, PublicComment, PublicQuery,
    PublicRequest, PublicStatus, Release, Request, RequestPatch, Timestamp, VoteResult, Workspace,
    WorkspacePatch,
};
use crate::validation;

pub const DB_FILE_NAME: &str = "feedback-board.db";
const DEFAULT_WORKSPACE_ID: &str = "workspace_default";
const DEFAULT_BOARD_ID: &str = "board_feedback";

#[derive(Debug, Clone, Error)]
pub enum StoreError {
    #[error("feedback board record not found")]
    NotFound,
    #[error("guest posting is disabled for this board")]
    GuestPostsDisabled,
    #[error("public request is not available")]
    PublicRequestUnavailable,
    #[error("the request changed while it was open; refresh and try again")]
    StaleRevision,
    #[error("the request cannot be merged into itself")]
    SelfMerge,
    #[error("the request is already merged")]
    AlreadyMerged,
    #[error("the public write was rejected")]
    Spam,
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        Self::from_connection(
            Connection::open(path).with_context(|| format!("opening {}", path.display()))?,
        )
    }

    pub fn open_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
			 PRAGMA foreign_keys=ON;
			 CREATE TABLE IF NOT EXISTS workspace (
			     id TEXT PRIMARY KEY,
			     name TEXT NOT NULL,
			     slug TEXT NOT NULL UNIQUE,
			     description TEXT NOT NULL,
			     primary_color TEXT NOT NULL,
			     default_automation_mode TEXT NOT NULL,
			     default_agent_id TEXT,
			     default_workflow_id TEXT,
			     allow_guest_posts INTEGER NOT NULL,
			     moderate_public_writes INTEGER NOT NULL,
			     revision INTEGER NOT NULL,
			     created_at INTEGER NOT NULL,
			     updated_at INTEGER NOT NULL
			 );
			 CREATE TABLE IF NOT EXISTS board (
			     id TEXT PRIMARY KEY,
			     workspace_id TEXT NOT NULL REFERENCES workspace(id) ON DELETE CASCADE,
			     slug TEXT NOT NULL,
			     name TEXT NOT NULL,
			     description TEXT NOT NULL,
			     automation_mode TEXT,
			     allow_guest_posts INTEGER NOT NULL,
			     moderate_public_writes INTEGER NOT NULL,
			     revision INTEGER NOT NULL,
			     created_at INTEGER NOT NULL,
			     updated_at INTEGER NOT NULL,
			     UNIQUE(workspace_id, slug)
			 );
			 CREATE TABLE IF NOT EXISTS board_status (
			     board_id TEXT NOT NULL REFERENCES board(id) ON DELETE CASCADE,
			     code TEXT NOT NULL,
			     label TEXT NOT NULL,
			     tone TEXT NOT NULL,
			     public INTEGER NOT NULL,
			     terminal INTEGER NOT NULL,
			     position INTEGER NOT NULL,
			     PRIMARY KEY(board_id, code)
			 );
			 CREATE TABLE IF NOT EXISTS request (
			     id TEXT PRIMARY KEY,
			     board_id TEXT NOT NULL REFERENCES board(id) ON DELETE CASCADE,
			     title TEXT NOT NULL,
			     body TEXT NOT NULL,
			     author_name TEXT NOT NULL,
			     author_email TEXT,
			     category TEXT NOT NULL,
			     status TEXT NOT NULL,
			     tags_json TEXT NOT NULL,
			     vote_count INTEGER NOT NULL,
			     comment_count INTEGER NOT NULL,
			     duplicate_of TEXT REFERENCES request(id),
			     duplicate_confidence INTEGER,
			     impact_score INTEGER,
			     priority INTEGER NOT NULL,
			     internal_notes TEXT NOT NULL,
			     ai_summary TEXT,
			     moderation_state TEXT NOT NULL,
			     public_visible INTEGER NOT NULL,
			     space_doc_id TEXT,
			     plan_id TEXT,
			     workflow_run_id TEXT,
			     automation_mode TEXT,
			     revision INTEGER NOT NULL,
			     created_at INTEGER NOT NULL,
			     updated_at INTEGER NOT NULL,
			     shipped_at INTEGER
			 );
			 CREATE INDEX IF NOT EXISTS idx_request_board_updated
			     ON request(board_id, updated_at DESC);
			 CREATE INDEX IF NOT EXISTS idx_request_status
			     ON request(board_id, status, public_visible);
			 CREATE TABLE IF NOT EXISTS request_vote (
			     request_id TEXT NOT NULL REFERENCES request(id) ON DELETE CASCADE,
			     voter_hash TEXT NOT NULL,
			     created_at INTEGER NOT NULL,
			     PRIMARY KEY(request_id, voter_hash)
			 );
			 CREATE TABLE IF NOT EXISTS comment (
			     id TEXT PRIMARY KEY,
			     request_id TEXT NOT NULL REFERENCES request(id) ON DELETE CASCADE,
			     author_name TEXT NOT NULL,
			     author_email TEXT,
			     body TEXT NOT NULL,
			     visibility TEXT NOT NULL,
			     moderation_state TEXT NOT NULL,
			     created_at INTEGER NOT NULL
			 );
			 CREATE TABLE IF NOT EXISTS release (
			     id TEXT PRIMARY KEY,
			     workspace_id TEXT NOT NULL REFERENCES workspace(id) ON DELETE CASCADE,
			     title TEXT NOT NULL,
			     body TEXT NOT NULL,
			     status TEXT NOT NULL,
			     published_at INTEGER,
			     created_at INTEGER NOT NULL,
			     updated_at INTEGER NOT NULL
			 );
			 CREATE TABLE IF NOT EXISTS release_request (
			     release_id TEXT NOT NULL REFERENCES release(id) ON DELETE CASCADE,
			     request_id TEXT NOT NULL REFERENCES request(id) ON DELETE CASCADE,
			     PRIMARY KEY(release_id, request_id)
			 );
			 CREATE TABLE IF NOT EXISTS automation_run (
			     id TEXT PRIMARY KEY,
			     request_id TEXT NOT NULL REFERENCES request(id) ON DELETE CASCADE,
			     mode TEXT NOT NULL,
			     agent_id TEXT,
			     workflow_id TEXT,
			     workflow_run_id TEXT,
			     plan_id TEXT,
			     status TEXT NOT NULL,
			     error TEXT,
			     result_summary TEXT,
			     created_at INTEGER NOT NULL,
			     updated_at INTEGER NOT NULL
			 );",
        )
        .context("creating Feedback Board schema")?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.ensure_default_workspace()?;
        Ok(store)
    }

    pub fn request_count(&self) -> Result<usize> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let count: i64 = guard.query_row("SELECT COUNT(*) FROM request", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    pub fn bootstrap_sample(&self) -> Result<Board> {
        let board = self
            .board_by_id(DEFAULT_BOARD_ID)?
            .ok_or(StoreError::NotFound)?;
        if self.request_count()? > 0 {
            return Ok(board);
        }
        let now = now_ms();
        let requests = [
            (
                "Dark mode for the dashboard",
                "A focused dark theme would make evening work easier to scan.",
                "Ideas",
                "review",
                vec!["design".to_owned(), "dashboard".to_owned()],
                42,
            ),
            (
                "Export requests as CSV",
                "Give teams a portable export of their request history.",
                "Ideas",
                "planned",
                vec!["reporting".to_owned()],
                28,
            ),
            (
                "Download the request list",
                "A download button would make the request list easier to share.",
                "Ideas",
                "new",
                vec!["reporting".to_owned()],
                5,
            ),
        ];
        let ids: Vec<String> = requests
            .iter()
            .map(|_| format!("req_{}", Uuid::new_v4().simple()))
            .collect();
        for ((title, body, category, status, tags, votes), id) in requests.iter().zip(&ids) {
            self.insert_request(
                id,
                &board.id,
                title,
                body,
                "Ryu community",
                None,
                category,
                status,
                tags,
                *votes,
                "approved",
                true,
                now,
            )?;
        }
        self.patch_request(
            &ids[2],
            0,
            RequestPatch {
                duplicate_of: Some(Some(ids[1].clone())),
                duplicate_confidence: Some(Some(92)),
                ..RequestPatch::default()
            },
        )?;
        Ok(board)
    }

    pub fn workspace(&self) -> Result<Workspace> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        guard
            .query_row(
                "SELECT id, name, slug, description, primary_color, default_automation_mode,
				        default_agent_id, default_workflow_id, allow_guest_posts,
				        moderate_public_writes, revision, created_at, updated_at
				 FROM workspace WHERE id = ?1",
                params![DEFAULT_WORKSPACE_ID],
                read_workspace,
            )
            .map_err(Into::into)
    }

    pub fn board_by_slug(&self, slug: &str) -> Result<Option<Board>> {
        let slug = validation::slug(slug)?;
        let guard = self.conn.lock().expect("feedback board store poisoned");
        Ok(guard
            .query_row(
                "SELECT id, workspace_id, slug, name, description, automation_mode,
				        allow_guest_posts, moderate_public_writes, revision, created_at, updated_at
				 FROM board WHERE workspace_id = ?1 AND slug = ?2",
                params![DEFAULT_WORKSPACE_ID, slug],
                read_board,
            )
            .optional()?)
    }

    pub fn board_by_id(&self, id: &str) -> Result<Option<Board>> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        Ok(guard
            .query_row(
                "SELECT id, workspace_id, slug, name, description, automation_mode,
				        allow_guest_posts, moderate_public_writes, revision, created_at, updated_at
				 FROM board WHERE id = ?1",
                params![id],
                read_board,
            )
            .optional()?)
    }

    pub fn list_boards(&self) -> Result<Vec<Board>> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let mut statement = guard.prepare(
            "SELECT id, workspace_id, slug, name, description, automation_mode,
			        allow_guest_posts, moderate_public_writes, revision, created_at, updated_at
			 FROM board WHERE workspace_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = statement.query_map(params![DEFAULT_WORKSPACE_ID], read_board)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn create_board(
        &self,
        raw_slug: &str,
        raw_name: &str,
        raw_description: &str,
    ) -> Result<Board> {
        let slug = validation::slug(raw_slug)?;
        let name = validation::title(raw_name)?;
        let description = raw_description.trim().to_owned();
        let id = format!("board_{}", Uuid::new_v4().simple());
        let now = now_ms();
        let guard = self.conn.lock().expect("feedback board store poisoned");
        guard.execute(
            "INSERT INTO board
			 (id, workspace_id, slug, name, description, allow_guest_posts,
			  moderate_public_writes, revision, created_at, updated_at)
			 VALUES (?1, ?2, ?3, ?4, ?5, 1, 0, 0, ?6, ?6)",
            params![id, DEFAULT_WORKSPACE_ID, slug, name, description, now],
        )?;
        let statuses = [
            ("new", "New", "blue", 0, 0),
            ("review", "Under review", "amber", 1, 0),
            ("planned", "Planned", "violet", 2, 0),
            ("in_progress", "In progress", "lime", 3, 0),
            ("shipped", "Shipped", "green", 4, 1),
            ("closed", "Closed", "neutral", 5, 1),
        ];
        for (code, label, tone, position, terminal) in statuses {
            guard.execute(
                "INSERT INTO board_status (board_id, code, label, tone, public, terminal, position)
				 VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
                params![id, code, label, tone, terminal, position],
            )?;
        }
        drop(guard);
        self.board_by_id(&id)?
            .ok_or_else(|| StoreError::NotFound.into())
    }

    pub fn public_board(&self, slug: &str) -> Result<Option<PublicBoard>> {
        let Some(board) = self.board_by_slug(slug)? else {
            return Ok(None);
        };
        let statuses = self
            .statuses(&board.id)?
            .into_iter()
            .filter(|status| status.public)
            .map(PublicStatus::from)
            .collect();
        let workspace = self.workspace()?;
        Ok(Some(PublicBoard {
            workspace_name: workspace.name,
            workspace_description: workspace.description,
            workspace_primary_color: workspace.primary_color,
            board_id: board.id,
            board_slug: board.slug,
            board_name: board.name,
            board_description: board.description,
            statuses,
        }))
    }

    pub fn statuses(&self, board_id: &str) -> Result<Vec<BoardStatus>> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let mut statement = guard.prepare(
            "SELECT code, label, tone, public, terminal, position
			 FROM board_status WHERE board_id = ?1 ORDER BY position",
        )?;
        let rows = statement.query_map(params![board_id], read_status)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn list_public_requests(
        &self,
        slug: &str,
        query: &PublicQuery,
    ) -> Result<Vec<PublicRequest>> {
        let Some(board) = self.board_by_slug(slug)? else {
            return Ok(Vec::new());
        };
        let statuses = self.statuses(&board.id)?;
        let status_map = statuses
            .iter()
            .map(|status| (status.code.clone(), PublicStatus::from(status.clone())))
            .collect::<std::collections::HashMap<_, _>>();
        let limit = query.limit.unwrap_or(50).clamp(1, 100) as i64;
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let mut sql = String::from(
            "SELECT id, board_id, title, body, author_name, author_email, category, status,
			        tags_json, vote_count, comment_count, duplicate_of, duplicate_confidence,
			        impact_score, priority, internal_notes, ai_summary, moderation_state,
			        public_visible, space_doc_id, plan_id, workflow_run_id, automation_mode,
			        revision, created_at, updated_at, shipped_at
			 FROM request WHERE board_id = ?1 AND public_visible = 1
			   AND moderation_state = 'approved'",
        );
        let mut values = vec![Value::Text(board.id.clone())];
        let mut next_parameter = 2;
        let search = query
            .query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(search) = search {
            sql.push_str(&format!(" AND (lower(title) LIKE '%' || lower(?{next_parameter}) || '%' OR lower(body) LIKE '%' || lower(?{next_parameter}) || '%' OR lower(category) LIKE '%' || lower(?{next_parameter}) || '%')"));
            values.push(Value::Text(search.to_owned()));
            next_parameter += 1;
        }
        if let Some(status) = query.status.as_deref() {
            sql.push_str(&format!(" AND status = ?{next_parameter}"));
            values.push(Value::Text(status.to_owned()));
            next_parameter += 1;
        }
        if let Some(category) = query.category.as_deref() {
            sql.push_str(&format!(" AND category = ?{next_parameter}"));
            values.push(Value::Text(category.to_owned()));
            next_parameter += 1;
        }
        sql.push_str(match query.sort.as_deref() {
            Some("top") => " ORDER BY vote_count DESC, updated_at DESC",
            Some("trending") => " ORDER BY updated_at DESC, vote_count DESC",
            Some("shipped") => " ORDER BY shipped_at DESC, updated_at DESC",
            _ => " ORDER BY created_at DESC",
        });
        sql.push_str(&format!(" LIMIT ?{next_parameter}"));
        values.push(Value::Integer(limit));
        let mut statement = guard.prepare(&sql)?;
        let mut rows = statement.query(rusqlite::params_from_iter(values.iter()))?;
        let mut output = Vec::new();
        while let Some(row) = rows.next()? {
            let request = read_request(row)?;
            if let Some(status) = status_map.get(&request.status) {
                output.push(PublicRequest::from_request(request, status.clone()));
            }
        }
        Ok(output)
    }

    pub fn public_request(&self, slug: &str, request_id: &str) -> Result<Option<PublicRequest>> {
        let Some(board) = self.board_by_slug(slug)? else {
            return Ok(None);
        };
        let Some(request) = self.request_by_id(request_id)? else {
            return Ok(None);
        };
        if request.board_id != board.id
            || !request.public_visible
            || request.moderation_state != "approved"
        {
            return Ok(None);
        }
        let status = self
            .statuses(&board.id)?
            .into_iter()
            .find(|status| status.code == request.status && status.public);
        Ok(status.map(|status| PublicRequest::from_request(request, status.into())))
    }

    pub fn public_comments(&self, request_id: &str) -> Result<Vec<PublicComment>> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let mut statement = guard.prepare(
            "SELECT id, request_id, author_name, body, visibility, moderation_state, created_at
			 FROM comment WHERE request_id = ?1 AND visibility = 'public'
			   AND moderation_state = 'approved' ORDER BY created_at ASC",
        )?;
        let rows = statement.query_map(params![request_id], read_comment)?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub fn admin_comments(&self, request_id: &str) -> Result<Vec<Comment>> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let mut statement = guard.prepare(
            "SELECT id, request_id, author_name, body, visibility, moderation_state, created_at
			 FROM comment WHERE request_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = statement.query_map(params![request_id], read_comment)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn public_releases(&self) -> Result<Vec<Release>> {
        self.releases(Some("published"))
    }

    pub fn admin_releases(&self) -> Result<Vec<Release>> {
        self.releases(None)
    }

    fn releases(&self, status: Option<&str>) -> Result<Vec<Release>> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let mut sql = String::from(
            "SELECT id, workspace_id, title, body, status, published_at, created_at, updated_at
			 FROM release WHERE workspace_id = ?1",
        );
        if status.is_some() {
            sql.push_str(" AND status = ?2");
        }
        sql.push_str(" ORDER BY COALESCE(published_at, created_at) DESC");
        let mut statement = guard.prepare(&sql)?;
        let mut rows = if let Some(status) = status {
            statement.query(params![DEFAULT_WORKSPACE_ID, status])?
        } else {
            statement.query(params![DEFAULT_WORKSPACE_ID])?
        };
        let mut releases = Vec::new();
        while let Some(row) = rows.next()? {
            let mut release = read_release(row)?;
            release.request_ids = release_request_ids(&guard, &release.id)?;
            releases.push(release);
        }
        Ok(releases)
    }

    pub fn public_release(&self, id: &str) -> Result<Option<Release>> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let Some(mut release) = guard
            .query_row(
                "SELECT id, workspace_id, title, body, status, published_at, created_at, updated_at
				 FROM release WHERE id = ?1 AND workspace_id = ?2 AND status = 'published'",
                params![id, DEFAULT_WORKSPACE_ID],
                read_release,
            )
            .optional()?
        else {
            return Ok(None);
        };
        release.request_ids = release_request_ids(&guard, &release.id)?;
        Ok(Some(release))
    }

    pub fn create_release(
        &self,
        title: &str,
        body: &str,
        request_ids: &[String],
    ) -> Result<Release> {
        let title = validation::title(title)?;
        let body = validation::body(body)?;
        if request_ids.is_empty() {
            anyhow::bail!("a changelog draft needs at least one request");
        }
        for request_id in request_ids {
            let Some(request) = self.request_by_id(request_id)? else {
                return Err(StoreError::NotFound.into());
            };
            if request.status != "shipped" {
                anyhow::bail!("only shipped requests can be linked to a changelog draft");
            }
        }
        let id = format!("release_{}", Uuid::new_v4().simple());
        let now = now_ms();
        let guard = self.conn.lock().expect("feedback board store poisoned");
        guard.execute(
            "INSERT INTO release (id, workspace_id, title, body, status, created_at, updated_at)
			 VALUES (?1, ?2, ?3, ?4, 'draft', ?5, ?5)",
            params![id, DEFAULT_WORKSPACE_ID, title, body, now],
        )?;
        for request_id in request_ids {
            guard.execute(
                "INSERT INTO release_request (release_id, request_id) VALUES (?1, ?2)",
                params![id, request_id],
            )?;
        }
        drop(guard);
        self.admin_releases()?
            .into_iter()
            .find(|release| release.id == id)
            .ok_or_else(|| StoreError::NotFound.into())
    }

    pub fn publish_release(&self, id: &str, body: Option<&str>) -> Result<Release> {
        let Some(current) = self
            .admin_releases()?
            .into_iter()
            .find(|release| release.id == id)
        else {
            return Err(StoreError::NotFound.into());
        };
        for request_id in &current.request_ids {
            let Some(request) = self.request_by_id(request_id)? else {
                return Err(StoreError::NotFound.into());
            };
            if request.status != "shipped" {
                anyhow::bail!("only shipped requests can be published");
            }
        }
        let guard = self.conn.lock().expect("feedback board store poisoned");
        guard.execute(
			"UPDATE release SET body = COALESCE(?2, body), status = 'published', published_at = ?3, updated_at = ?3
			 WHERE id = ?1",
			params![id, body, now_ms()],
		)?;
        drop(guard);
        self.public_release(id)?
            .ok_or_else(|| StoreError::NotFound.into())
    }

    pub fn create_guest_request(
        &self,
        slug: &str,
        input: CreateGuestRequest,
        _now_voter_hash: &str,
    ) -> Result<PublicRequest> {
        let Some(board) = self.board_by_slug(slug)? else {
            return Err(StoreError::NotFound.into());
        };
        if !board.allow_guest_posts {
            return Err(StoreError::GuestPostsDisabled.into());
        }
        if input
            .honeypot
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
            .not()
        {
            return Err(StoreError::Spam.into());
        }
        let title = validation::title(&input.title)?;
        let body = validation::body(&input.body)?;
        let category = validation::category(&input.category)?;
        let now = now_ms();
        let id = format!("req_{}", Uuid::new_v4().simple());
        self.insert_request(
            &id,
            &board.id,
            &title,
            &body,
            input.author_name.as_deref().unwrap_or("Anonymous").trim(),
            input.email.as_deref(),
            &category,
            "new",
            &[],
            0,
            if board.moderate_public_writes {
                "pending"
            } else {
                "approved"
            },
            !board.moderate_public_writes,
            now,
        )?;
        self.public_request(slug, &id)?
            .ok_or_else(|| StoreError::PublicRequestUnavailable.into())
    }

    pub fn vote(&self, request_id: &str, voter_hash: &str) -> Result<VoteResult> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let tx = guard.unchecked_transaction()?;
        let request_exists: Option<String> = tx
			.query_row(
				"SELECT id FROM request WHERE id = ?1 AND public_visible = 1 AND moderation_state = 'approved'",
				params![request_id],
				|row| row.get(0),
			)
			.optional()?;
        if request_exists.is_none() {
            return Err(StoreError::PublicRequestUnavailable.into());
        }
        let inserted = tx.execute(
			"INSERT OR IGNORE INTO request_vote (request_id, voter_hash, created_at) VALUES (?1, ?2, ?3)",
			params![request_id, voter_hash, now_ms()],
		)?;
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM request_vote WHERE request_id = ?1",
            params![request_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "UPDATE request SET vote_count = ?2, updated_at = ?3 WHERE id = ?1",
            params![request_id, count, now_ms()],
        )?;
        tx.commit()?;
        Ok(VoteResult {
            voted: inserted > 0,
            vote_count: count,
        })
    }

    pub fn add_public_comment(
        &self,
        slug: &str,
        request_id: &str,
        input: CreateComment,
    ) -> Result<PublicComment> {
        let Some(board) = self.board_by_slug(slug)? else {
            return Err(StoreError::NotFound.into());
        };
        if self.public_request(slug, request_id)?.is_none() {
            return Err(StoreError::PublicRequestUnavailable.into());
        }
        if input
            .honeypot
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
            .not()
        {
            return Err(StoreError::Spam.into());
        }
        let id = format!("comment_{}", Uuid::new_v4().simple());
        let moderation_state = if board.moderate_public_writes {
            "pending"
        } else {
            "approved"
        };
        let comment = Comment {
            id,
            request_id: request_id.to_owned(),
            author_name: input.author_name.unwrap_or_else(|| "Anonymous".into()),
            body: validation::comment(&input.body)?,
            visibility: "public".into(),
            moderation_state: moderation_state.into(),
            created_at: now_ms(),
        };
        let guard = self.conn.lock().expect("feedback board store poisoned");
        guard.execute(
			"INSERT INTO comment (id, request_id, author_name, author_email, body, visibility, moderation_state, created_at)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
			params![comment.id, comment.request_id, comment.author_name, input.email, comment.body, comment.visibility, comment.moderation_state, comment.created_at],
		)?;
        if moderation_state == "approved" {
            guard.execute(
				"UPDATE request SET comment_count = comment_count + 1, updated_at = ?2 WHERE id = ?1",
				params![request_id, now_ms()],
			)?;
        }
        Ok(comment.into())
    }

    pub fn request_by_id(&self, id: &str) -> Result<Option<Request>> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let sql = format!("{REQUEST_SELECT} WHERE id = ?1");
        Ok(guard
            .query_row(&sql, params![id], read_request)
            .optional()?)
    }

    pub fn list_admin_requests(&self, query: &AdminQuery) -> Result<Vec<Request>> {
        let limit = query.limit.unwrap_or(100).clamp(1, 500) as i64;
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let mut sql = format!("{REQUEST_SELECT} WHERE board_id = ?1");
        let mut values = vec![Value::Text(DEFAULT_BOARD_ID.to_owned())];
        let mut next_parameter = 2;
        let search = query
            .query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(search) = search {
            sql.push_str(&format!(" AND (lower(title) LIKE '%' || lower(?{next_parameter}) || '%' OR lower(body) LIKE '%' || lower(?{next_parameter}) || '%' OR lower(category) LIKE '%' || lower(?{next_parameter}) || '%')"));
            values.push(Value::Text(search.to_owned()));
            next_parameter += 1;
        }
        if let Some(status) = query.status.as_deref() {
            sql.push_str(&format!(" AND status = ?{next_parameter}"));
            values.push(Value::Text(status.to_owned()));
            next_parameter += 1;
        }
        if let Some(category) = query.category.as_deref() {
            sql.push_str(&format!(" AND category = ?{next_parameter}"));
            values.push(Value::Text(category.to_owned()));
            next_parameter += 1;
        }
        sql.push_str(&format!(
            " ORDER BY updated_at DESC LIMIT ?{next_parameter}"
        ));
        values.push(Value::Integer(limit));
        let mut statement = guard.prepare(&sql)?;
        let mut rows = statement.query(rusqlite::params_from_iter(values.iter()))?;
        let mut output = Vec::new();
        while let Some(row) = rows.next()? {
            output.push(read_request(row)?);
        }
        Ok(output)
    }

    pub fn patch_request(
        &self,
        id: &str,
        expected_revision: i64,
        patch: RequestPatch,
    ) -> Result<Request> {
        let Some(current) = self.request_by_id(id)? else {
            return Err(StoreError::NotFound.into());
        };
        if current.revision != expected_revision {
            return Err(StoreError::StaleRevision.into());
        }
        let mut next = current.clone();
        if let Some(value) = patch.title {
            next.title = validation::title(&value)?;
        }
        if let Some(value) = patch.body {
            next.body = validation::body(&value)?;
        }
        if let Some(value) = patch.category {
            next.category = validation::category(&value)?;
        }
        if let Some(value) = patch.tags {
            next.tags = validation::tags(&value)?;
        }
        if let Some(value) = patch.status {
            if !self
                .statuses(&next.board_id)?
                .iter()
                .any(|status| status.code == value)
            {
                anyhow::bail!("unknown request status");
            }
            next.status = value;
            if next.status == "shipped" && next.shipped_at.is_none() {
                next.shipped_at = Some(now_ms());
            }
        }
        if let Some(value) = patch.priority {
            next.priority = value.clamp(-100, 100);
        }
        if let Some(value) = patch.internal_notes {
            next.internal_notes = value;
        }
        if let Some(value) = patch.public_visible {
            next.public_visible = value;
        }
        if let Some(value) = patch.duplicate_of {
            next.duplicate_of = value;
        }
        if let Some(value) = patch.duplicate_confidence {
            next.duplicate_confidence = value;
        }
        if let Some(value) = patch.impact_score {
            next.impact_score = value.map(|score| score.clamp(0, 100));
        }
        if let Some(value) = patch.ai_summary {
            next.ai_summary = value;
        }
        if let Some(value) = patch.space_doc_id {
            next.space_doc_id = value;
        }
        if let Some(value) = patch.plan_id {
            next.plan_id = value;
        }
        if let Some(value) = patch.workflow_run_id {
            next.workflow_run_id = value;
        }
        if let Some(value) = patch.automation_mode {
            next.automation_mode = value;
        }
        if let Some(value) = patch.moderation_state {
            next.moderation_state = value;
        }
        next.revision += 1;
        next.updated_at = now_ms();
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let updated = guard.execute(
            "UPDATE request SET title = ?2, body = ?3, category = ?4, status = ?5, tags_json = ?6,
			        priority = ?7, internal_notes = ?8, public_visible = ?9, duplicate_of = ?10,
			        duplicate_confidence = ?11, impact_score = ?12, ai_summary = ?13, space_doc_id = ?14,
			        plan_id = ?15, workflow_run_id = ?16, automation_mode = ?17, moderation_state = ?18,
			        revision = ?19, updated_at = ?20, shipped_at = ?21
			 WHERE id = ?1 AND revision = ?22",
            params![
                id,
                next.title,
                next.body,
                next.category,
                next.status,
                serde_json::to_string(&next.tags)?,
                next.priority,
                next.internal_notes,
                next.public_visible,
                next.duplicate_of,
                next.duplicate_confidence,
                next.impact_score,
                next.ai_summary,
                next.space_doc_id,
                next.plan_id,
                next.workflow_run_id,
                next.automation_mode.map(AutomationMode::as_str),
                next.moderation_state,
                next.revision,
                next.updated_at,
                next.shipped_at,
                expected_revision,
            ],
        )?;
        if updated == 0 {
            return Err(StoreError::StaleRevision.into());
        }
        drop(guard);
        self.request_by_id(id)?
            .ok_or_else(|| StoreError::NotFound.into())
    }

    pub fn merge_requests(&self, survivor_id: &str, duplicate_id: &str) -> Result<MergeResult> {
        if survivor_id == duplicate_id {
            return Err(StoreError::SelfMerge.into());
        }
        let survivor = self
            .request_by_id(survivor_id)?
            .ok_or(StoreError::NotFound)?;
        let duplicate = self
            .request_by_id(duplicate_id)?
            .ok_or(StoreError::NotFound)?;
        if duplicate.duplicate_of.is_some() {
            return Err(StoreError::AlreadyMerged.into());
        }
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let tx = guard.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO request_vote (request_id, voter_hash, created_at)
			 SELECT ?1, voter_hash, created_at FROM request_vote WHERE request_id = ?2",
            params![survivor_id, duplicate_id],
        )?;
        tx.execute(
            "UPDATE comment SET request_id = ?1 WHERE request_id = ?2",
            params![survivor_id, duplicate_id],
        )?;
        tx.execute(
            "UPDATE release_request SET request_id = ?1 WHERE request_id = ?2",
            params![survivor_id, duplicate_id],
        )?;
        let votes: i64 = tx.query_row(
            "SELECT COUNT(*) FROM request_vote WHERE request_id = ?1",
            params![survivor_id],
            |row| row.get(0),
        )?;
        let comments: i64 = tx.query_row(
			"SELECT COUNT(*) FROM comment WHERE request_id = ?1 AND visibility = 'public' AND moderation_state = 'approved'",
			params![survivor_id],
			|row| row.get(0),
		)?;
        tx.execute(
			"UPDATE request SET vote_count = ?2, comment_count = ?3, updated_at = ?4, revision = revision + 1 WHERE id = ?1",
			params![survivor_id, votes.max(survivor.vote_count + duplicate.vote_count), comments, now_ms()],
		)?;
        tx.execute(
			"UPDATE request SET duplicate_of = ?2, public_visible = 0, moderation_state = 'merged', updated_at = ?3, revision = revision + 1 WHERE id = ?1",
			params![duplicate_id, survivor_id, now_ms()],
		)?;
        tx.commit()?;
        drop(guard);
        Ok(MergeResult {
            survivor: self
                .request_by_id(survivor_id)?
                .ok_or(StoreError::NotFound)?,
            merged_request_id: duplicate_id.to_owned(),
        })
    }

    pub fn create_automation_run(
        &self,
        request_id: &str,
        mode: AutomationMode,
        agent_id: Option<String>,
        workflow_id: Option<String>,
        workflow_run_id: Option<String>,
        plan_id: Option<String>,
        status: &str,
    ) -> Result<AutomationRun> {
        if self.request_by_id(request_id)?.is_none() {
            return Err(StoreError::NotFound.into());
        }
        let id = format!("run_{}", Uuid::new_v4().simple());
        let now = now_ms();
        let guard = self.conn.lock().expect("feedback board store poisoned");
        guard.execute(
			"INSERT INTO automation_run
			 (id, request_id, mode, agent_id, workflow_id, workflow_run_id, plan_id, status, created_at, updated_at)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
			params![id, request_id, mode.as_str(), agent_id, workflow_id, workflow_run_id, plan_id, status, now],
		)?;
        drop(guard);
        self.automation_run(&id)?
            .ok_or_else(|| StoreError::NotFound.into())
    }

    pub fn automation_run(&self, id: &str) -> Result<Option<AutomationRun>> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        Ok(guard
			.query_row(
				"SELECT id, request_id, mode, agent_id, workflow_id, workflow_run_id, plan_id, status, error, result_summary, created_at, updated_at
				 FROM automation_run WHERE id = ?1",
				params![id],
				read_automation_run,
			)
			.optional()?)
    }

    pub fn automation_runs_for_request(&self, request_id: &str) -> Result<Vec<AutomationRun>> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let mut statement = guard.prepare(
			"SELECT id, request_id, mode, agent_id, workflow_id, workflow_run_id, plan_id, status, error, result_summary, created_at, updated_at
			 FROM automation_run WHERE request_id = ?1 ORDER BY created_at DESC",
		)?;
        let rows = statement.query_map(params![request_id], read_automation_run)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn update_automation_run(
        &self,
        id: &str,
        status: &str,
        workflow_run_id: Option<String>,
        plan_id: Option<String>,
        result_summary: Option<String>,
        error: Option<String>,
    ) -> Result<AutomationRun> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let updated = guard.execute(
			"UPDATE automation_run SET status = ?2, workflow_run_id = COALESCE(?3, workflow_run_id),
			        plan_id = COALESCE(?4, plan_id), result_summary = ?5, error = ?6, updated_at = ?7
			 WHERE id = ?1",
			params![id, status, workflow_run_id, plan_id, result_summary, error, now_ms()],
		)?;
        if updated == 0 {
            return Err(StoreError::NotFound.into());
        }
        drop(guard);
        self.automation_run(id)?
            .ok_or_else(|| StoreError::NotFound.into())
    }

    pub fn patch_workspace(
        &self,
        expected_revision: i64,
        patch: WorkspacePatch,
    ) -> Result<Workspace> {
        let current = self.workspace()?;
        if current.revision != expected_revision {
            return Err(StoreError::StaleRevision.into());
        }
        let next_name = patch
            .name
            .map(|value| value.trim().to_owned())
            .unwrap_or(current.name);
        let next_description = patch
            .description
            .map(|value| value.trim().to_owned())
            .unwrap_or(current.description);
        let next_color = patch
            .primary_color
            .map(|value| value.trim().to_owned())
            .unwrap_or(current.primary_color);
        let next_mode = patch
            .default_automation_mode
            .unwrap_or(current.default_automation_mode);
        let next_agent = patch.default_agent_id.unwrap_or(current.default_agent_id);
        let next_workflow = patch
            .default_workflow_id
            .unwrap_or(current.default_workflow_id);
        let next_guest = patch.allow_guest_posts.unwrap_or(current.allow_guest_posts);
        let next_moderate = patch
            .moderate_public_writes
            .unwrap_or(current.moderate_public_writes);
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let updated = guard.execute(
            "UPDATE workspace SET name = ?2, description = ?3, primary_color = ?4,
			        default_automation_mode = ?5, default_agent_id = ?6, default_workflow_id = ?7,
			        allow_guest_posts = ?8, moderate_public_writes = ?9, revision = ?10, updated_at = ?11
			 WHERE id = ?1 AND revision = ?12",
            params![
                DEFAULT_WORKSPACE_ID,
                next_name,
                next_description,
                next_color,
                next_mode.as_str(),
                next_agent,
                next_workflow,
                next_guest,
                next_moderate,
                current.revision + 1,
                now_ms(),
                expected_revision
            ],
        )?;
        if updated == 0 {
            return Err(StoreError::StaleRevision.into());
        }
        drop(guard);
        self.workspace()
    }

    pub fn patch_board(
        &self,
        id: &str,
        expected_revision: i64,
        patch: BoardPatch,
    ) -> Result<Board> {
        let current = self.board_by_id(id)?.ok_or(StoreError::NotFound)?;
        if current.revision != expected_revision {
            return Err(StoreError::StaleRevision.into());
        }
        let next_name = patch
            .name
            .map(|value| value.trim().to_owned())
            .unwrap_or(current.name);
        let next_description = patch
            .description
            .map(|value| value.trim().to_owned())
            .unwrap_or(current.description);
        let next_mode = patch.automation_mode.unwrap_or(current.automation_mode);
        let next_guest = patch.allow_guest_posts.unwrap_or(current.allow_guest_posts);
        let next_moderate = patch
            .moderate_public_writes
            .unwrap_or(current.moderate_public_writes);
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let updated = guard.execute(
            "UPDATE board SET name = ?2, description = ?3, automation_mode = ?4,
			        allow_guest_posts = ?5, moderate_public_writes = ?6, revision = ?7, updated_at = ?8
			 WHERE id = ?1 AND revision = ?9",
            params![
                id,
                next_name,
                next_description,
                next_mode.map(AutomationMode::as_str),
                next_guest,
                next_moderate,
                current.revision + 1,
                now_ms(),
                expected_revision
            ],
        )?;
        if updated == 0 {
            return Err(StoreError::StaleRevision.into());
        }
        drop(guard);
        self.board_by_id(id)?
            .ok_or_else(|| StoreError::NotFound.into())
    }

    fn ensure_default_workspace(&self) -> Result<()> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        let now = now_ms();
        guard.execute(
			"INSERT OR IGNORE INTO workspace
			 (id, name, slug, description, primary_color, default_automation_mode,
			  allow_guest_posts, moderate_public_writes, revision, created_at, updated_at)
			 VALUES (?1, 'Your product', 'feedback', 'A public space to shape what comes next.', '#0099ff', 'assist', 1, 0, 0, ?2, ?2)",
			params![DEFAULT_WORKSPACE_ID, now],
		)?;
        guard.execute(
			"INSERT OR IGNORE INTO board
			 (id, workspace_id, slug, name, description, allow_guest_posts,
			  moderate_public_writes, revision, created_at, updated_at)
			 VALUES (?1, ?2, 'feedback', 'Feedback', 'Tell us what would make the product better.', 1, 0, 0, ?3, ?3)",
			params![DEFAULT_BOARD_ID, DEFAULT_WORKSPACE_ID, now],
		)?;
        let statuses = [
            ("new", "New", "blue", 0, 0),
            ("review", "Under review", "amber", 1, 0),
            ("planned", "Planned", "violet", 2, 0),
            ("in_progress", "In progress", "lime", 3, 0),
            ("shipped", "Shipped", "green", 4, 1),
            ("closed", "Closed", "neutral", 5, 1),
        ];
        for (code, label, tone, position, terminal) in statuses {
            guard.execute(
				"INSERT OR IGNORE INTO board_status (board_id, code, label, tone, public, terminal, position)
				 VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
				params![DEFAULT_BOARD_ID, code, label, tone, terminal, position],
			)?;
        }
        Ok(())
    }

    fn insert_request(
        &self,
        id: &str,
        board_id: &str,
        title: &str,
        body: &str,
        author_name: &str,
        author_email: Option<&str>,
        category: &str,
        status: &str,
        tags: &[String],
        vote_count: i64,
        moderation_state: &str,
        public_visible: bool,
        now: Timestamp,
    ) -> Result<()> {
        let guard = self.conn.lock().expect("feedback board store poisoned");
        guard.execute(
            "INSERT INTO request
			 (id, board_id, title, body, author_name, author_email, category, status, tags_json,
			  vote_count, comment_count, priority, internal_notes, moderation_state, public_visible,
			  revision, created_at, updated_at)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, 0, '', ?11, ?12, 0, ?13, ?13)",
            params![
                id,
                board_id,
                title,
                body,
                author_name,
                author_email,
                category,
                status,
                serde_json::to_string(tags)?,
                vote_count,
                moderation_state,
                public_visible,
                now
            ],
        )?;
        Ok(())
    }
}

const REQUEST_SELECT: &str = "SELECT id, board_id, title, body, author_name, author_email, category, status,
    tags_json, vote_count, comment_count, duplicate_of, duplicate_confidence, impact_score, priority,
    internal_notes, ai_summary, moderation_state, public_visible, space_doc_id, plan_id, workflow_run_id,
    automation_mode, revision, created_at, updated_at, shipped_at FROM request";

fn now_ms() -> Timestamp {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn read_workspace(row: &Row<'_>) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        id: row.get(0)?,
        name: row.get(1)?,
        slug: row.get(2)?,
        description: row.get(3)?,
        primary_color: row.get(4)?,
        default_automation_mode: AutomationMode::try_from(row.get::<_, String>(5)?.as_str())
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::other(error)),
                )
            })?,
        default_agent_id: row.get(6)?,
        default_workflow_id: row.get(7)?,
        allow_guest_posts: row.get::<_, i64>(8)? != 0,
        moderate_public_writes: row.get::<_, i64>(9)? != 0,
        revision: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn read_board(row: &Row<'_>) -> rusqlite::Result<Board> {
    let mode: Option<String> = row.get(5)?;
    Ok(Board {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        slug: row.get(2)?,
        name: row.get(3)?,
        description: row.get(4)?,
        automation_mode: mode
            .as_deref()
            .map(AutomationMode::try_from)
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::other(error)),
                )
            })?,
        allow_guest_posts: row.get::<_, i64>(6)? != 0,
        moderate_public_writes: row.get::<_, i64>(7)? != 0,
        revision: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn read_status(row: &Row<'_>) -> rusqlite::Result<BoardStatus> {
    Ok(BoardStatus {
        code: row.get(0)?,
        label: row.get(1)?,
        tone: row.get(2)?,
        public: row.get::<_, i64>(3)? != 0,
        terminal: row.get::<_, i64>(4)? != 0,
        position: row.get(5)?,
    })
}

fn read_request(row: &Row<'_>) -> rusqlite::Result<Request> {
    let mode: Option<String> = row.get(22)?;
    Ok(Request {
        id: row.get(0)?,
        board_id: row.get(1)?,
        title: row.get(2)?,
        body: row.get(3)?,
        author_name: row.get(4)?,
        author_email: row.get(5)?,
        category: row.get(6)?,
        status: row.get(7)?,
        tags: serde_json::from_str(&row.get::<_, String>(8)?).unwrap_or_default(),
        vote_count: row.get(9)?,
        comment_count: row.get(10)?,
        duplicate_of: row.get(11)?,
        duplicate_confidence: row.get(12)?,
        impact_score: row.get(13)?,
        priority: row.get(14)?,
        internal_notes: row.get(15)?,
        ai_summary: row.get(16)?,
        moderation_state: row.get(17)?,
        public_visible: row.get::<_, i64>(18)? != 0,
        space_doc_id: row.get(19)?,
        plan_id: row.get(20)?,
        workflow_run_id: row.get(21)?,
        automation_mode: mode
            .as_deref()
            .map(AutomationMode::try_from)
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    22,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::other(error)),
                )
            })?,
        revision: row.get(23)?,
        created_at: row.get(24)?,
        updated_at: row.get(25)?,
        shipped_at: row.get(26)?,
    })
}

fn read_comment(row: &Row<'_>) -> rusqlite::Result<Comment> {
    Ok(Comment {
        id: row.get(0)?,
        request_id: row.get(1)?,
        author_name: row.get(2)?,
        body: row.get(3)?,
        visibility: row.get(4)?,
        moderation_state: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn read_release(row: &Row<'_>) -> rusqlite::Result<Release> {
    Ok(Release {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        title: row.get(2)?,
        body: row.get(3)?,
        status: row.get(4)?,
        request_ids: Vec::new(),
        published_at: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn release_request_ids(connection: &Connection, release_id: &str) -> Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT request_id FROM release_request WHERE release_id = ?1 ORDER BY request_id",
    )?;
    let rows = statement.query_map(params![release_id], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn read_automation_run(row: &Row<'_>) -> rusqlite::Result<AutomationRun> {
    let mode = AutomationMode::try_from(row.get::<_, String>(2)?.as_str()).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::other(error)),
        )
    })?;
    Ok(AutomationRun {
        id: row.get(0)?,
        request_id: row.get(1)?,
        mode,
        agent_id: row.get(3)?,
        workflow_id: row.get(4)?,
        workflow_run_id: row.get(5)?,
        plan_id: row.get(6)?,
        status: row.get(7)?,
        error: row.get(8)?,
        result_summary: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

trait BoolNot {
    fn not(self) -> bool;
}

impl BoolNot for bool {
    fn not(self) -> bool {
        !self
    }
}
