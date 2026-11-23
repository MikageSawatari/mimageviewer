//! Favorite registration/index edits share the dialog's commit and UI admission.
use super::*;
use crate::agent_ipc::changes::{
    ChangeCounts, ChangeRecord, ChangeRecordId, ChangeRow, ChangeRowOutcome, ChangeRowResult,
};
use crate::agent_ipc::favorite::PreparedFavorite;
use crate::settings::{FavoriteAddError, FavoriteEntry};
use crate::ui_dialogs::FavoriteAddFlags;
use mimageviewer_agent_ipc::{
    AgentError, ErrorCode, FavoriteIndexState, FavoriteWriteResult, MAX_PAGE_CHARS,
    mcp_content_chars,
};

const ADDED_MESSAGE: &str = "新しく登録したお気に入りは AI から削除できません。削除するときは mIV の「お気に入り → 編集」を使ってください";

fn state(favorite: &FavoriteEntry) -> FavoriteIndexState {
    FavoriteIndexState {
        item_index: favorite.auto_index_metadata,
        container_index: favorite.auto_index_structure,
    }
}

fn add_error(error: FavoriteAddError) -> AgentError {
    AgentError::new(
        ErrorCode::BadRequest,
        match error {
            FavoriteAddError::Duplicate => {
                "このフォルダはすでにお気に入りに登録されています。".to_owned()
            }
            FavoriteAddError::LimitReached { max } => format!(
                "お気に入りは最大 {max} 件まで登録できます。不要な項目を削除してから追加してください。"
            ),
        },
    )
}

fn flag_label(flags: FavoriteIndexState) -> String {
    format!(
        "アイテム索引: {}、コンテナ索引: {}",
        if flags.item_index { "オン" } else { "オフ" },
        if flags.container_index {
            "オン"
        } else {
            "オフ"
        }
    )
}

impl App {
    pub(super) fn agent_set_favorite(
        &mut self,
        request: &PreparedFavorite,
    ) -> Result<FavoriteWriteResult, AgentError> {
        self.admit_agent_settings_write()?;
        let path = crate::path_key::normalize_input_path(&request.path);
        let existing = self.settings.favorites.iter().find(|favorite| {
            favorite.path == path || crate::folder_tree::path_eq(&favorite.path, &path)
        });
        let (id, name, response_path, before) = if let Some(favorite) = existing {
            (
                Some(favorite.id),
                favorite.name.clone(),
                favorite.path.clone(),
                Some(state(favorite)),
            )
        } else {
            request.folder_check.clone()?;
            // Match the dialog's physical target range. Book-library locations
            // have their own owner; this check is lexical and performs no I/O.
            let books_root = self
                .settings
                .book_root
                .as_deref()
                .unwrap_or(&request.default_books_root);
            if crate::books::path_is_under_or_equal(&path, books_root) {
                return Err(AgentError::new(
                    ErrorCode::BadRequest,
                    "本棚の場所はお気に入りへ追加できません",
                ));
            }
            let name = request.name.clone().unwrap_or_else(|| {
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            });
            let name = name.trim().to_owned();
            if name.is_empty() {
                return Err(AgentError::new(
                    ErrorCode::BadRequest,
                    "お気に入りの表示名を指定してください",
                ));
            }
            // Reuse the same limit/duplicate policy before response preflight.
            self.settings
                .clone()
                .try_add_favorite(name.clone(), path.clone())
                .map_err(add_error)?;
            (None, name, path.clone(), None)
        };
        let after = FavoriteIndexState {
            item_index: request
                .item_index
                .unwrap_or(before.is_some_and(|state| state.item_index)),
            container_index: request
                .container_index
                .unwrap_or(before.is_some_and(|state| state.container_index)),
        };
        let mut result = FavoriteWriteResult {
            path: response_path.to_string_lossy().into_owned(),
            name: name.clone(),
            added: id.is_none(),
            before,
            after,
            message: id.is_none().then(|| ADDED_MESSAGE.into()),
        };
        if mcp_content_chars(&result)
            .map_err(|_| AgentError::new(ErrorCode::Internal, "応答を作成できませんでした"))?
            > MAX_PAGE_CHARS
        {
            return Err(AgentError::new(
                ErrorCode::BadRequest,
                "お気に入りの名前またはパスが長すぎます",
            ));
        }
        if result.before == Some(result.after) {
            return Ok(result);
        }
        let controller = self
            .agent_changes
            .as_ref()
            .map(|ui| ui.controller.clone())
            .ok_or_else(|| AgentError::new(ErrorCode::NotReady, "mIV の起動処理中です"))?;
        let saved_generation = crate::settings::save_generation();
        let id = if let Some(id) = id {
            self.commit_favorite_index_flags(id, request.item_index, request.container_index);
            id
        } else {
            self.commit_favorite_add(
                name,
                path,
                FavoriteAddFlags {
                    item_index: after.item_index,
                    container_index: after.container_index,
                    thumbs: false,
                    similar: false,
                },
            )
            .map_err(add_error)?
        };
        let favorite = self
            .settings
            .favorites
            .iter()
            .find(|favorite| favorite.id == id)
            .ok_or_else(|| {
                AgentError::new(
                    ErrorCode::Internal,
                    "お気に入りの登録結果を確認できませんでした",
                )
            })?;
        result.after = state(favorite);
        debug_assert!(mcp_content_chars(&result).is_ok_and(|size| size <= MAX_PAGE_CHARS));
        let before_text = result
            .before
            .map(flag_label)
            .unwrap_or_else(|| "未登録".into());
        let mut after_text = flag_label(result.after);
        if let Some(message) = &result.message {
            after_text.push('\n');
            after_text.push_str(message);
        }
        let item = format!("お気に入り: {} — {}", result.name, result.path);
        let mut notice = format!(
            "AI アプリがお気に入りを{}しました: {item}\n{before_text} → {after_text}",
            if result.added { "登録" } else { "変更" }
        );
        if crate::settings::save_generation() == saved_generation {
            crate::logger::log("agent_ipc: set_favorite: favorite settings could not be saved");
            self.show_feedback_warning("AI によるお気に入り変更を保存できませんでした".into());
            notice.push_str("\nお気に入り変更を保存できませんでした");
        }
        crate::logger::log(format!(
            "agent_ipc: tool=set_favorite result={}",
            serde_json::to_string(&result).expect("favorite result serializes")
        ));
        controller.publish(ChangeRecord {
            id: ChangeRecordId::default(),
            at: std::time::SystemTime::now(),
            client_name: request.client_name.clone(),
            tool: "set_favorite".into(),
            collection: None,
            counts: ChangeCounts {
                changed: 1,
                ..Default::default()
            },
            summary: if result.added {
                "お気に入りを登録"
            } else {
                "お気に入りの索引を変更"
            }
            .into(),
            rows: vec![ChangeRow {
                item,
                before: before_text,
                after: after_text,
                result: ChangeRowResult::Done(ChangeRowOutcome::Changed),
            }],
            notice,
        });
        Ok(result)
    }
}

#[cfg(test)]
#[path = "agent_favorite_tests.rs"]
mod tests;
