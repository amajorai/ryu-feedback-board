//! Shared Ryu sidecar data-directory seam.

use std::path::PathBuf;

pub fn ryu_dir() -> PathBuf {
    ryu_sidecar_runtime::ryu_dir()
}

pub const DB_FILE_NAME: &str = "feedback-board.db";
