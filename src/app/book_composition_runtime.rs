//! G2 storage adoption only. G3 owns layout/materialization and presentation admission.
//! The accepted immutable record travels with its viewer, including parked and snapshot views.
use super::App;
use crate::book_composition::{
    CompositionPrepareReply, CompositionToken, PageDomain, StoredComposition,
};
use crate::book_scope::BookScopeKey;
use std::sync::{Arc, mpsc};

#[derive(Default)]
pub(crate) enum ViewerBookComposition {
    #[default]
    Automatic,
    Preparing {
        scope: BookScopeKey,
        receiver: mpsc::Receiver<CompositionPrepareReply>,
    },
    Accepted {
        scope: BookScopeKey,
        token: CompositionToken,
        record: Option<Arc<StoredComposition>>,
    },
    Blocked {
        scope: BookScopeKey,
        token: CompositionToken,
        error: String,
    },
}

impl Clone for ViewerBookComposition {
    fn clone(&self) -> Self {
        match self {
            Self::Automatic | Self::Preparing { .. } => Self::Automatic,
            Self::Accepted {
                scope,
                token,
                record,
            } => Self::Accepted {
                scope: scope.clone(),
                token: *token,
                record: record.clone(),
            },
            Self::Blocked {
                scope,
                token,
                error,
            } => Self::Blocked {
                scope: scope.clone(),
                token: *token,
                error: error.clone(),
            },
        }
    }
}
impl std::fmt::Debug for ViewerBookComposition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Automatic => f.write_str("Automatic"),
            Self::Preparing { scope, .. } => f.debug_tuple("Preparing").field(scope).finish(),
            Self::Accepted {
                scope,
                token,
                record,
            } => f
                .debug_tuple("Accepted")
                .field(scope)
                .field(token)
                .field(record)
                .finish(),
            Self::Blocked {
                scope,
                token,
                error,
            } => f
                .debug_tuple("Blocked")
                .field(scope)
                .field(token)
                .field(error)
                .finish(),
        }
    }
}
impl PartialEq for ViewerBookComposition {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Automatic, Self::Automatic) => true,
            (
                Self::Accepted {
                    scope: a,
                    token: b,
                    record: c,
                },
                Self::Accepted {
                    scope: x,
                    token: y,
                    record: z,
                },
            ) => (a, b, c) == (x, y, z),
            (
                Self::Blocked {
                    scope: a,
                    token: b,
                    error: c,
                },
                Self::Blocked {
                    scope: x,
                    token: y,
                    error: z,
                },
            ) => (a, b, c) == (x, y, z),
            _ => false,
        }
    }
}

impl App {
    pub(crate) fn install_book_composition_runtime(
        &mut self,
        service: crate::book_composition::BookCompositionService,
    ) {
        if let Some(handle) = self.remote_session_handle() {
            handle.install_book_composition_service(Some(service.clone()));
        }
        self.book_composition_service = Some(service);
    }

    fn composition_scope_domain(&self) -> Option<(BookScopeKey, PageDomain)> {
        let scope = self.current_spread_shift_scope()?;
        let domain = if self.zip_nav.is_some() {
            PageDomain::ArchiveLevel {
                prefix: scope.level_prefix.clone(),
            }
        } else if self
            .items
            .first()
            .is_some_and(|item| matches!(item, crate::grid_item::GridItem::PdfPage { .. }))
        {
            PageDomain::Document
        } else {
            // Image-folder persistence/backup and adoption are added together in G4F.
            return None;
        };
        Some((scope, domain))
    }

    pub(super) fn poll_book_composition_service(&mut self, ctx: &egui::Context) {
        let Some(service) = self.book_composition_service.clone() else {
            return;
        };
        for error in service.drain_errors() {
            crate::logger::log(format!("book_composition: {error}"));
            self.show_feedback_toast(format!(
                "本の構成の保存データを処理できませんでした: {error}"
            ));
            ctx.request_repaint();
        }
        let changed = service.drain_changed_scope_keys();
        // Read-only visits never invalidate a sibling. G3 attaches Book reload to this exact set.
        #[cfg(windows)]
        {
            for id in self.book_composition_contexts_to_poll(&changed) {
                let _ = self.with_viewer_context(id, |app| {
                    if (matches!(
                        app.book_composition,
                        ViewerBookComposition::Preparing { .. }
                    ) || app
                        .book_composition
                        .scope()
                        .is_some_and(|scope| changed.contains(&scope.storage_key())))
                        && app.extracted_result_surface().is_none()
                    {
                        app.poll_book_composition_read(ctx);
                    }
                });
            }
        }
        self.poll_book_composition_read(ctx);
    }

    pub(super) fn poll_book_composition_read(&mut self, ctx: &egui::Context) {
        let Some((scope, domain)) = self.composition_scope_domain() else {
            self.book_composition = ViewerBookComposition::Automatic;
            return;
        };
        let Some(service) = self.book_composition_service.as_ref() else {
            return;
        };
        let token = service.snapshot().token_for(&scope);
        match &self.book_composition {
            ViewerBookComposition::Accepted {
                scope: accepted,
                token: old,
                ..
            }
            | ViewerBookComposition::Blocked {
                scope: accepted,
                token: old,
                ..
            } if accepted == &scope && old == &token => return,
            ViewerBookComposition::Preparing {
                scope: requested, ..
            } if requested == &scope => {}
            _ => {
                self.book_composition = ViewerBookComposition::Preparing {
                    scope: scope.clone(),
                    receiver: service.prepare(scope.clone(), domain.clone()),
                };
            }
        }
        let ViewerBookComposition::Preparing { receiver, .. } = &self.book_composition else {
            return;
        };
        match receiver.try_recv() {
            Ok(reply) if reply.scope == scope => {
                // Publication may have advanced after Prepare replied and its change
                // notification may already have been drained. Compare at adoption,
                // including absent/blocked records and at-rest viewers. Keep the new
                // read in Preparing so the owning context remains eligible for polling.
                if reply.token != service.snapshot().token_for(&scope) {
                    self.book_composition = ViewerBookComposition::Preparing {
                        receiver: service.prepare(scope.clone(), domain),
                        scope,
                    };
                } else {
                    self.book_composition = match reply.status {
                        Ok(record) => ViewerBookComposition::Accepted {
                            scope,
                            token: reply.token,
                            record: record.map(Arc::new),
                        },
                        Err(error) => {
                            // Store-wide errors are notified once by the service drain above.
                            // A scope/domain rejection is local to this read and still needs notice.
                            if service.snapshot().status.is_ok() {
                                crate::logger::log(format!(
                                    "book_composition: read failed: {error}"
                                ));
                                self.show_feedback_toast(format!(
                                    "本の構成を読み込めませんでした: {error}"
                                ));
                            }
                            ViewerBookComposition::Blocked {
                                scope,
                                token: reply.token,
                                error,
                            }
                        }
                    };
                    ctx.request_repaint();
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                let error = "構成読込workerが終了しました".to_owned();
                crate::logger::log(format!("book_composition: {error}"));
                self.show_feedback_toast(error.clone());
                self.book_composition = ViewerBookComposition::Blocked {
                    scope,
                    token,
                    error,
                };
                ctx.request_repaint();
            }
            Ok(_) => self.book_composition = ViewerBookComposition::Automatic,
        }
        // Match the existing worker-pending delayed repaint contract, outside the
        // grid tail. A dedicated/parked viewer must wake ROOT, which owns the
        // process service drain, even when its own viewport is idle or not drawn.
        if matches!(
            self.book_composition,
            ViewerBookComposition::Preparing { .. }
        ) {
            ctx.request_repaint_after_for(
                std::time::Duration::from_millis(50),
                egui::ViewportId::ROOT,
            );
        }
    }
}
impl ViewerBookComposition {
    pub(crate) fn scope(&self) -> Option<&BookScopeKey> {
        match self {
            Self::Automatic => None,
            Self::Preparing { scope, .. }
            | Self::Accepted { scope, .. }
            | Self::Blocked { scope, .. } => Some(scope),
        }
    }
}
