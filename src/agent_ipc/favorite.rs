//! Physical-folder validation stays on the existing agent write worker.
use std::path::PathBuf;

use mimageviewer_agent_ipc::{AgentError, ErrorCode};

#[derive(Clone, Debug)]
pub(crate) struct PreparedFavorite {
    pub(crate) path: PathBuf,
    pub(crate) name: Option<String>,
    pub(crate) item_index: Option<bool>,
    pub(crate) container_index: Option<bool>,
    pub(crate) default_books_root: PathBuf,
    pub(crate) client_name: String,
}

pub(super) fn prepare(
    path: String,
    name: Option<String>,
    item_index: Option<bool>,
    container_index: Option<bool>,
    client_name: String,
    ctx: &super::ReadContext,
) -> Result<PreparedFavorite, AgentError> {
    ctx.checkpoint()?;
    let path = super::input_path::validate(&path)?;
    let metadata = std::fs::metadata(path)
        .map_err(|_| super::error(ErrorCode::BadRequest, "指定されたフォルダを確認できません"))?;
    if !metadata.is_dir() {
        return Err(super::error(
            ErrorCode::BadRequest,
            "お気に入りに追加できるのは実フォルダのみです",
        ));
    }
    ctx.checkpoint()?;
    let default_books_root = crate::books::default_books_root();
    ctx.checkpoint()?;
    Ok(PreparedFavorite {
        path: crate::path_key::normalize_input_path(path),
        name,
        item_index,
        container_index,
        default_books_root,
        client_name,
    })
}
