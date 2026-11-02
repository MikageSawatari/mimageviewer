//! App-wide bookshelf display and latest-value order writer; viewer resources are independent.
use super::*;
use crate::book_order::{
    BookOrderPolicy, BookOrderReadState, BookOrderToken, BookOrderWriteError, BookShelfCompletion,
    BookShelfSnapshot,
};

#[derive(Clone, Debug)]
pub(crate) struct ShelfScope {
    pub root: PathBuf,
    pub epoch: u64,
    pub revision: u64,
}

#[derive(Default)]
pub(crate) struct BookShelfOwner {
    pub snapshot: Option<BookShelfSnapshot>,
    epoch: u64,
    revision: u64,
    phase: Phase,
    #[cfg(test)]
    test_writes: Option<mpsc::Sender<BookOrderWriteForTest>>,
}

#[cfg(test)]
pub(crate) struct BookOrderWriteForTest {
    pub policy: BookOrderPolicy,
    pub token: BookOrderToken,
    pub reply: mpsc::Sender<Result<crate::books::BookOpResult, String>>,
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Reading(ShelfScope),
    ReadFailed,
    Saving {
        scope: ShelfScope,
        sent: BookOrderPolicy,
        names: Vec<String>,
        _lease: Option<crate::remote_ipc::session::LocalModalLease>,
        close: Option<super::root_lifecycle::RootCloseIntent>,
    },
    Unsaved,
}

impl App {
    #[cfg(test)]
    pub(crate) fn book_order_writes_for_test(&mut self) -> mpsc::Receiver<BookOrderWriteForTest> {
        let (tx, rx) = mpsc::channel();
        self.book_shelf.test_writes = Some(tx);
        rx
    }
    pub(crate) fn shelf_scope(&self) -> ShelfScope {
        ShelfScope {
            root: self.book_root_path(),
            epoch: self.book_shelf.epoch,
            revision: self.book_shelf.revision,
        }
    }

    pub(crate) fn shelf_scope_current(&self, scope: &ShelfScope) -> bool {
        scope.epoch == self.book_shelf.epoch
            && crate::folder_tree::path_eq(&scope.root, &self.book_root_path())
    }

    pub(crate) fn pending_book_shelf_current(&self) -> bool {
        self.book_op_pending
            .as_ref()
            .and_then(|pending| pending.shelf_scope.as_ref())
            .is_none_or(|scope| self.shelf_scope_current(scope))
    }

    pub(crate) fn book_shelf_root_changed(&mut self) {
        debug_assert!(!self.book_order_saving());
        self.book_shelf.epoch = self.book_shelf.epoch.wrapping_add(1);
        self.book_shelf.revision = self.book_shelf.revision.wrapping_add(1);
        self.book_shelf.snapshot = None;
        self.book_shelf.phase = Phase::Idle;
        self.book_manager_rename_inputs.clear();
    }

    pub(crate) fn book_list_refresh_pending(&self) -> bool {
        self.book_op_pending.as_ref().is_some_and(|pending| {
            matches!(pending.intent, crate::books::BookOpIntent::List { .. })
        })
    }

    pub(crate) fn book_order_saving(&self) -> bool {
        matches!(self.book_shelf.phase, Phase::Saving { .. })
    }

    pub(crate) fn book_order_mode_label(&self) -> &'static str {
        if self
            .book_shelf
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| matches!(snapshot.policy, BookOrderPolicy::Manual(_)))
        {
            "手動順"
        } else {
            "名前順"
        }
    }

    pub(crate) fn book_order_status(&self) -> Option<&'static str> {
        match self.book_shelf.phase {
            Phase::Saving { .. } => Some("保存中…"),
            Phase::Unsaved => Some("未保存"),
            Phase::Reading(_) => Some("読込中…"),
            Phase::ReadFailed => Some("本棚一覧を読み込めません"),
            Phase::Idle => self.book_shelf.snapshot.as_ref().and_then(|snapshot| {
                matches!(snapshot.read, BookOrderReadState::ReadOnly(_))
                    .then_some("本の順序を読み込めないため名前順で表示しています")
            }),
        }
    }

    pub(crate) fn book_order_editable(&self) -> bool {
        if matches!(self.book_shelf.phase, Phase::ReadFailed) {
            return false;
        }
        self.book_shelf
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.token().is_some())
            && (self.book_order_saving()
                || (self.book_op_pending.is_none() && self.book_structural_op.is_none()))
    }

    pub(crate) fn begin_book_shelf_read(&mut self) -> ShelfScope {
        self.book_shelf.revision = self.book_shelf.revision.wrapping_add(1);
        let scope = self.shelf_scope();
        self.book_shelf.phase = Phase::Reading(scope.clone());
        scope
    }

    pub(crate) fn book_shelf_read_current(&self, scope: &ShelfScope) -> bool {
        self.shelf_scope_current(scope)
            && matches!(&self.book_shelf.phase, Phase::Reading(request) if request.revision == scope.revision)
    }

    pub(crate) fn fail_book_shelf_read(&mut self) {
        if matches!(self.book_shelf.phase, Phase::Reading(_)) {
            self.book_shelf.phase = Phase::ReadFailed;
        }
    }

    pub(crate) fn ensure_book_list_loaded(&mut self) {
        if self.book_shelf.snapshot.is_none() && !matches!(self.book_shelf.phase, Phase::ReadFailed)
        {
            self.request_book_list_refresh();
        }
    }

    pub(crate) fn book_list_read_failed(&self) -> bool {
        matches!(self.book_shelf.phase, Phase::ReadFailed)
    }

    pub(crate) fn adopt_book_shelf_completion(
        &mut self,
        completion: BookShelfCompletion,
        scope: Option<&ShelfScope>,
    ) {
        if scope.is_some_and(|scope| !self.shelf_scope_current(scope)) {
            return;
        }
        if let Some(warning) = completion.warning {
            self.show_feedback_warning(warning);
        }
        match completion.snapshot {
            Ok(mut snapshot) => {
                snapshot.epoch = self.book_shelf.epoch;
                snapshot.revision = self.book_shelf.revision;
                if let BookOrderReadState::ReadOnly(error) = &snapshot.read {
                    crate::logger::log(format!("[book-order] read: {error}"));
                    self.show_feedback_warning(
                        "本の順序を読み込めないため名前順で表示しています".into(),
                    );
                }
                for row in &snapshot.rows {
                    self.book_manager_rename_inputs
                        .entry(row.name.clone())
                        .or_insert_with(|| row.name.clone());
                }
                self.book_shelf.snapshot = Some(snapshot);
                self.book_shelf.phase = Phase::Idle;
            }
            Err(error) => {
                crate::logger::log(format!("[book-order] list: {error}"));
                self.show_feedback_warning(format!("本棚一覧を読み込めません: {error}"));
                self.book_shelf.phase = Phase::ReadFailed;
            }
        }
    }

    pub(crate) fn finish_book_shelf_read(
        &mut self,
        snapshot: BookShelfSnapshot,
        scope: Option<&ShelfScope>,
    ) {
        if let Some(scope) = scope {
            if !self.shelf_scope_current(scope)
                || !matches!(&self.book_shelf.phase, Phase::Reading(request) if request.revision == scope.revision)
            {
                return;
            }
        }
        self.adopt_book_shelf_completion(
            BookShelfCompletion {
                snapshot: Ok(snapshot),
                warning: None,
            },
            scope,
        );
    }

    fn admit_book_order_edit(&mut self) -> bool {
        if !self.book_order_editable() {
            return false;
        }
        if self.book_order_saving() {
            return true;
        }
        if self.book_structural_open_pending()
            || self.rename_pending.is_some()
            || self.delete_pending.is_some()
            || self.new_folder_pending.is_some()
            || matches!(
                self.file_organize_request,
                crate::ui_dialogs::file_organize::FileOrganizeRequest::Running(_)
            )
            || !self.drop_copy_pending.is_empty()
        {
            self.show_feedback_warning("処理中です。完了後に本の順序を変更してください".into());
            return false;
        }
        let lease = if let Some(handle) = self.remote_session_handle() {
            let Some(lease) = handle.try_local_modal_lease() else {
                self.show_feedback_warning("mIV Remote の使用中は本の順序を変更できません".into());
                return false;
            };
            Some(lease)
        } else {
            None
        };
        self.book_shelf.phase = Phase::Saving {
            scope: self.shelf_scope(),
            sent: BookOrderPolicy::Name,
            names: Vec::new(),
            _lease: lease,
            close: None,
        };
        true
    }

    pub(crate) fn move_book_order(&mut self, name: &str, delta: isize, ctx: &egui::Context) {
        let Some(snapshot) = self.book_shelf.snapshot.as_ref() else {
            return;
        };
        let Some(index) = snapshot.rows.iter().position(|row| row.name == name) else {
            return;
        };
        let Some(target) = index
            .checked_add_signed(delta)
            .filter(|index| *index < snapshot.rows.len())
        else {
            return;
        };
        if target == index || !self.admit_book_order_edit() {
            return;
        }
        let snapshot = self.book_shelf.snapshot.as_mut().unwrap();
        snapshot.rows.swap(index, target);
        snapshot.policy =
            BookOrderPolicy::Manual(snapshot.rows.iter().map(|row| row.name.clone()).collect());
        self.book_shelf.revision = self.book_shelf.revision.wrapping_add(1);
        snapshot.revision = self.book_shelf.revision;
        self.send_book_order_if_idle(ctx);
    }

    pub(crate) fn fix_book_order(&mut self, ctx: &egui::Context) {
        if self
            .book_shelf
            .snapshot
            .as_ref()
            .is_none_or(|snapshot| !matches!(snapshot.policy, BookOrderPolicy::Name))
            || !self.admit_book_order_edit()
        {
            return;
        }
        let snapshot = self.book_shelf.snapshot.as_mut().unwrap();
        snapshot.policy =
            BookOrderPolicy::Manual(snapshot.rows.iter().map(|row| row.name.clone()).collect());
        self.book_shelf.revision = self.book_shelf.revision.wrapping_add(1);
        snapshot.revision = self.book_shelf.revision;
        self.send_book_order_if_idle(ctx);
    }

    pub(crate) fn reset_book_order(&mut self, ctx: &egui::Context) {
        if self
            .book_shelf
            .snapshot
            .as_ref()
            .is_none_or(|snapshot| matches!(snapshot.policy, BookOrderPolicy::Name))
            || !self.admit_book_order_edit()
        {
            return;
        }
        let snapshot = self.book_shelf.snapshot.as_mut().unwrap();
        snapshot
            .rows
            .sort_by(|a, b| crate::book_order::compare_names(&a.name, &b.name));
        snapshot.policy = BookOrderPolicy::Name;
        self.book_shelf.revision = self.book_shelf.revision.wrapping_add(1);
        snapshot.revision = self.book_shelf.revision;
        self.send_book_order_if_idle(ctx);
    }

    fn send_book_order_if_idle(&mut self, ctx: &egui::Context) {
        if self.book_op_pending.is_some() {
            ctx.request_repaint();
            return;
        }
        let snapshot = self
            .book_shelf
            .snapshot
            .as_ref()
            .expect("admitted snapshot");
        let token = snapshot
            .token()
            .cloned()
            .expect("admitted writable snapshot");
        let root = snapshot.root.clone();
        let policy = snapshot.policy.clone();
        let scope = self.shelf_scope();
        if let Phase::Saving {
            scope: sent_scope,
            sent,
            names,
            ..
        } = &mut self.book_shelf.phase
        {
            *sent_scope = scope.clone();
            *sent = policy.clone();
            *names = snapshot.rows.iter().map(|row| row.name.clone()).collect();
        }
        let (tx, rx) = mpsc::channel();
        #[cfg(test)]
        if let Some(transport) = &self.book_shelf.test_writes {
            transport
                .send(BookOrderWriteForTest {
                    policy,
                    token,
                    reply: tx,
                })
                .expect("test receiver");
            self.book_op_pending = Some(crate::books::BookOpPending {
                rx,
                intent: crate::books::BookOpIntent::ShelfOrder,
                shelf_scope: Some(scope),
            });
            return;
        }
        let wake = ctx.clone();
        match std::thread::Builder::new()
            .name("book-order".into())
            .spawn(move || {
                let result = crate::book_order::write_order(&root, &token, &policy);
                let _ = tx.send(Ok(crate::books::BookOpResult::ShelfOrderSaved(result)));
                wake.request_repaint();
            }) {
            Ok(_) => {
                self.book_op_pending = Some(crate::books::BookOpPending {
                    rx,
                    intent: crate::books::BookOpIntent::ShelfOrder,
                    shelf_scope: Some(scope),
                })
            }
            Err(error) => self
                .finish_book_order_write(ctx, Err(BookOrderWriteError::Failed(error.to_string()))),
        }
        ctx.request_repaint();
    }

    pub(crate) fn finish_book_order_write(
        &mut self,
        ctx: &egui::Context,
        result: Result<BookOrderToken, BookOrderWriteError>,
    ) {
        let phase = std::mem::take(&mut self.book_shelf.phase);
        let Phase::Saving {
            scope,
            sent,
            names,
            _lease,
            close,
        } = phase
        else {
            return;
        };
        if !self.shelf_scope_current(&scope) {
            return;
        }
        match result {
            Ok(token) => {
                let snapshot = self.book_shelf.snapshot.as_mut().expect("saving snapshot");
                snapshot.root_presence = token.root_presence;
                snapshot.read = BookOrderReadState::Writable(token);
                let same = snapshot.policy == sent
                    && snapshot.rows.iter().map(|row| &row.name).eq(names.iter());
                if !same {
                    self.book_shelf.phase = Phase::Saving {
                        scope,
                        sent,
                        names,
                        _lease,
                        close,
                    };
                    self.send_book_order_if_idle(ctx);
                    return;
                }
            }
            Err(BookOrderWriteError::Conflict) => {
                self.show_feedback_warning("本棚が変更されました。最新の内容を読み直します".into());
                self.request_book_list_refresh();
            }
            Err(BookOrderWriteError::Failed(error)) => {
                crate::logger::log(format!("[book-order] save: {error}"));
                self.show_feedback_warning(format!("本の順序を保存できませんでした: {error}"));
                self.book_shelf.phase = Phase::Unsaved;
            }
        }
        drop(_lease);
        if let Some(intent) = close {
            self.resume_root_lifecycle_close(ctx, intent);
        }
        ctx.request_repaint();
    }

    pub(crate) fn accept_book_order_root_close(
        &mut self,
        ctx: &egui::Context,
        intent: super::root_lifecycle::RootCloseIntent,
    ) -> bool {
        let Phase::Saving { close, .. } = &mut self.book_shelf.phase else {
            return false;
        };
        if *close != Some(super::root_lifecycle::RootCloseIntent::ApplicationQuit) {
            *close = Some(intent);
        }
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::CancelClose);
        true
    }
}
