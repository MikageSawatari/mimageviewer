//! Process-global registration owner, separate from viewer contexts and preference drafts.
pub(crate) mod backend;
#[cfg(test)]
mod tests;
use crate::app::root_lifecycle::RootCloseIntent;
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    pub enabled: bool,
    pub output_dir: Option<PathBuf>,
}
impl Selection {
    pub fn from_settings(settings: &crate::settings::Settings) -> Self {
        Self {
            enabled: settings.browser_integration_enabled,
            output_dir: settings.browser_view_output_dir.clone(),
        }
    }
    pub fn apply(&self, settings: &mut crate::settings::Settings) {
        settings.browser_integration_enabled = self.enabled;
        settings.browser_view_output_dir = self.output_dir.clone();
    }
}
#[derive(Clone, Debug)]
pub(crate) enum Action {
    Apply(Selection),
    Remove,
    ChooseFolder,
    OpenFolder(Option<PathBuf>),
}
#[derive(Clone, Debug)]
pub(crate) enum JobKind {
    Action(Action),
    Persist,
}
#[derive(Clone, Debug)]
pub(crate) struct Prepared {
    pub env: backend::Environment,
    pub selection: Selection,
    pub registered: bool,
    pub message: Option<String>,
}
impl Prepared {
    pub fn effective_output(&self) -> &std::path::Path {
        self.selection
            .output_dir
            .as_deref()
            .unwrap_or(&self.env.default_output)
    }
}
pub(crate) struct Preparation {
    pub default_output: Option<PathBuf>,
    pub ready: Result<Prepared, String>,
}
#[derive(Debug)]
pub(crate) struct Completion {
    pub prepared: Prepared,
    pub selection: Option<Selection>,
}
pub(crate) enum Event {
    Changed(Selection, Option<String>),
    Failed(String),
}
pub(crate) trait Backend: Send + Sync {
    fn prepare(&self, selection: Selection) -> Preparation;
    fn perform(&self, prepared: Prepared, action: Action) -> Result<Completion, String>;
    fn persist(&self, selection: &Selection) -> Result<(), String>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparationEffect {
    ReadOnly,
    MayUpdateRegistration,
}
impl PreparationEffect {
    fn from_selection(selection: &Selection) -> Self {
        if selection.enabled {
            Self::MayUpdateRegistration
        } else {
            Self::ReadOnly
        }
    }
}
pub(crate) enum PreparationTask {
    NotStarted,
    Running {
        effect: PreparationEffect,
        rx: mpsc::Receiver<Preparation>,
    },
}
pub(crate) enum BrowserIntegrationState {
    Unavailable {
        reason: String,
    },
    Preparing(PreparationTask),
    Available(Prepared),
    Processing {
        prepared: Prepared,
        kind: JobKind,
        rx: mpsc::Receiver<Result<Completion, String>>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Unavailable(String),
    Preparing,
    Available,
    Processing,
}
#[derive(Clone, Debug)]
pub(crate) struct View {
    pub status: Status,
    pub default_output: Option<PathBuf>,
    pub selection: Selection,
    pub registered: bool,
    pub message: Option<String>,
}
impl Default for View {
    fn default() -> Self {
        Self {
            status: Status::Preparing,
            default_output: None,
            selection: Selection::default(),
            registered: false,
            message: None,
        }
    }
}
pub(crate) struct BrowserIntegration {
    state: BrowserIntegrationState,
    default_output: Option<PathBuf>,
    backend: Arc<dyn Backend>,
    // One consumed root-close continuation, owned here like the other lifecycle
    // participants. It is independent of the registration/persistence phase.
    close: Option<RootCloseIntent>,
}
impl Default for BrowserIntegration {
    fn default() -> Self {
        Self {
            #[cfg(not(test))]
            state: BrowserIntegrationState::Preparing(PreparationTask::NotStarted),
            // App fixtures must never inspect or mutate a personal registration.
            #[cfg(test)]
            state: BrowserIntegrationState::Unavailable {
                reason: "テストでは登録 backend を注入してください。".into(),
            },
            default_output: None,
            backend: Arc::new(backend::NativeBackend),
            close: None,
        }
    }
}
impl BrowserIntegration {
    fn has_pending_work(&self, unstarted_effect: PreparationEffect) -> bool {
        match &self.state {
            BrowserIntegrationState::Preparing(PreparationTask::NotStarted) => {
                unstarted_effect == PreparationEffect::MayUpdateRegistration
            }
            BrowserIntegrationState::Preparing(PreparationTask::Running { effect, .. }) => {
                *effect == PreparationEffect::MayUpdateRegistration
            }
            BrowserIntegrationState::Processing { .. } => true,
            _ => false,
        }
    }

    fn defer_root_close(
        &mut self,
        intent: RootCloseIntent,
        unstarted_effect: PreparationEffect,
    ) -> bool {
        if !self.has_pending_work(unstarted_effect) {
            return false;
        }
        if self.close != Some(RootCloseIntent::ApplicationQuit) {
            self.close = Some(intent);
        }
        true
    }

    fn take_ready_close(&mut self) -> Option<RootCloseIntent> {
        // A close retained before preparation starts was admitted only for an
        // enabled startup. Once started, the captured effect is authoritative.
        if self.has_pending_work(PreparationEffect::MayUpdateRegistration) {
            return None;
        }
        self.close.take()
    }

    /// Last process-exit fence, before any final Settings save. Read-only
    /// preparation is discarded. A registration-changing worker reaches this
    /// only if exit bypassed normal asynchronous admission. Receive its result;
    /// never redo its filesystem or registry work on the UI thread. ChooseFolder
    /// is also received here, so a
    /// bypassed admission can wait for its picker; that rare exit is deliberately
    /// left to the coarse failure policy, without another cancellation protocol.
    pub(crate) fn finish_for_exit(&mut self, settings: &mut crate::settings::Settings) {
        self.close = None;
        let state = std::mem::replace(
            &mut self.state,
            BrowserIntegrationState::Unavailable {
                reason: "終了しています。".into(),
            },
        );
        match state {
            BrowserIntegrationState::Preparing(PreparationTask::Running {
                effect: PreparationEffect::MayUpdateRegistration,
                rx,
            }) => match rx.recv() {
                Ok(preparation) => match preparation.ready {
                    Ok(prepared) => {
                        if let Some(message) = prepared.message {
                            crate::logger::log(format!("browser_registration: {message}"));
                        }
                    }
                    Err(error) => crate::logger::log(format!("browser_registration: {error}")),
                },
                Err(error) => crate::logger::log(format!("browser_registration: {error}")),
            },
            BrowserIntegrationState::Preparing(PreparationTask::Running {
                effect: PreparationEffect::ReadOnly,
                ..
            }) => {} // No registration effect to fence; drop the receiver without waiting.
            BrowserIntegrationState::Processing { rx, .. } => match rx.recv() {
                Ok(Ok(completion)) => {
                    if let Some(selection) = completion.selection {
                        selection.apply(settings);
                    }
                    // An already-running partial save has also completed here.
                    // Do not start another one: the caller's final full save
                    // includes the adopted selection and all other live values.
                }
                Ok(Err(error)) => crate::logger::log(format!("browser_registration: {error}")),
                Err(error) => crate::logger::log(format!("browser_registration: {error}")),
            },
            _ => {}
        }
    }

    fn pending_selection(&self) -> Option<Selection> {
        match &self.state {
            BrowserIntegrationState::Processing {
                kind: JobKind::Action(Action::Apply(selection)),
                ..
            } => Some(selection.clone()),
            BrowserIntegrationState::Processing {
                prepared,
                kind: JobKind::Action(Action::Remove),
                ..
            } => Some(Selection {
                enabled: false,
                output_dir: prepared.selection.output_dir.clone(),
            }),
            _ => None,
        }
    }
    pub fn view(&self, settings: &crate::settings::Settings) -> View {
        let (status, prepared) = match &self.state {
            BrowserIntegrationState::Unavailable { reason } => {
                (Status::Unavailable(reason.clone()), None)
            }
            BrowserIntegrationState::Preparing(_) => (Status::Preparing, None),
            BrowserIntegrationState::Available(prepared) => (Status::Available, Some(prepared)),
            BrowserIntegrationState::Processing { prepared, .. } => {
                (Status::Processing, Some(prepared))
            }
        };
        View {
            status,
            default_output: self.default_output.clone(),
            selection: self
                .pending_selection()
                .unwrap_or_else(|| Selection::from_settings(settings)),
            registered: prepared.is_some_and(|prepared| prepared.registered),
            message: prepared.and_then(|prepared| prepared.message.clone()),
        }
    }
    pub fn default_output(&self) -> Option<PathBuf> {
        self.default_output.clone()
    }
    pub fn poll(
        &mut self,
        settings: &mut crate::settings::Settings,
        ctx: &egui::Context,
    ) -> Option<Event> {
        if matches!(
            self.state,
            BrowserIntegrationState::Preparing(PreparationTask::NotStarted)
        ) {
            let selection = Selection::from_settings(settings);
            let effect = PreparationEffect::from_selection(&selection);
            let backend = self.backend.clone();
            let repaint = ctx.clone();
            let (tx, rx) = mpsc::channel();
            match std::thread::Builder::new()
                .name("browser-registration-prepare".into())
                .spawn(move || {
                    let result = backend.prepare(selection);
                    let _ = tx.send(result);
                    repaint.request_repaint_of(egui::ViewportId::ROOT);
                }) {
                Ok(_) => {
                    self.state =
                        BrowserIntegrationState::Preparing(PreparationTask::Running { effect, rx })
                }
                Err(error) => {
                    let reason = format!("連携の準備を開始できませんでした: {error}");
                    crate::logger::log(format!("browser_registration: {reason}"));
                    self.state = BrowserIntegrationState::Unavailable {
                        reason: reason.clone(),
                    };
                    return Some(Event::Failed(reason));
                }
            }
        }
        if let BrowserIntegrationState::Preparing(PreparationTask::Running { rx, .. }) = &self.state
        {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Preparation {
                    default_output: None,
                    ready: Err("連携の準備に失敗しました。".into()),
                }),
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                self.default_output = result.default_output;
                let mut startup_error = None;
                self.state = match result.ready {
                    Ok(prepared) => {
                        if let Some(message) = &prepared.message {
                            crate::logger::log(format!("browser_registration: {message}"));
                            startup_error = Some(message.clone());
                        }
                        BrowserIntegrationState::Available(prepared)
                    }
                    Err(reason) => {
                        crate::logger::log(format!("browser_registration: unavailable: {reason}"));
                        BrowserIntegrationState::Unavailable { reason }
                    }
                };
                if let Some(error) = startup_error {
                    return Some(Event::Failed(error));
                }
            }
        }
        if let BrowserIntegrationState::Processing { prepared, rx, .. } = &self.state {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("連携の処理が終了しませんでした。".into()))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                match result {
                    Ok(completion) => {
                        if let Some(selection) = completion.selection {
                            selection.apply(settings);
                            let warning = self
                                .start_persist(completion.prepared, selection.clone(), ctx)
                                .err();
                            return Some(Event::Changed(selection, warning));
                        }
                        self.state = BrowserIntegrationState::Available(completion.prepared);
                    }
                    Err(error) => {
                        crate::logger::log(format!("browser_registration: {error}"));
                        let mut previous = prepared.clone();
                        previous.message =
                            Some(format!("ブラウザ連携の処理に失敗しました: {error}"));
                        self.state = BrowserIntegrationState::Available(previous);
                        return Some(Event::Failed(error));
                    }
                }
            }
        }
        None
    }
    fn start_persist(
        &mut self,
        prepared: Prepared,
        selection: Selection,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let previous = prepared.clone();
        let backend = self.backend.clone();
        let repaint = ctx.clone();
        let (tx, rx) = mpsc::channel();
        match std::thread::Builder::new()
            .name("browser-registration-save".into())
            .spawn(move || {
                let result = backend
                    .persist(&selection)
                    .map(|()| Completion {
                        prepared,
                        selection: None,
                    })
                    .map_err(|error| {
                        format!("登録は変更済みですが、設定を保存できませんでした: {error}")
                    });
                let _ = tx.send(result);
                repaint.request_repaint_of(egui::ViewportId::ROOT);
            }) {
            Ok(_) => {
                self.state = BrowserIntegrationState::Processing {
                    prepared: previous,
                    kind: JobKind::Persist,
                    rx,
                };
                Ok(())
            }
            Err(error) => {
                let message =
                    format!("登録は変更済みですが、設定の保存を開始できませんでした: {error}");
                let mut previous = previous;
                previous.message = Some(message.clone());
                self.state = BrowserIntegrationState::Available(previous);
                Err(message)
            }
        }
    }
    pub fn submit(&mut self, action: Action, ctx: &egui::Context) -> bool {
        if self.close.is_some() {
            return false;
        }
        let BrowserIntegrationState::Available(prepared) = &self.state else {
            return false;
        };
        let previous = prepared.clone();
        let prepared = prepared.clone();
        let backend = self.backend.clone();
        let repaint = ctx.clone();
        let kind = JobKind::Action(action.clone());
        let (tx, rx) = mpsc::channel();
        match std::thread::Builder::new()
            .name("browser-registration".into())
            .spawn(move || {
                let result = backend.perform(prepared, action);
                let _ = tx.send(result);
                repaint.request_repaint_of(egui::ViewportId::ROOT);
            }) {
            Ok(_) => {
                self.state = BrowserIntegrationState::Processing {
                    prepared: previous,
                    kind,
                    rx,
                }
            }
            Err(error) => {
                crate::logger::log(format!(
                    "browser_registration: worker start failed: {error}"
                ));
                let mut previous = previous;
                previous.message = Some(format!("連携の処理を開始できませんでした: {error}"));
                self.state = BrowserIntegrationState::Available(previous);
                return false;
            }
        }
        true
    }
    fn owns_selection(&self, requested: &Selection) -> bool {
        match &self.state {
            BrowserIntegrationState::Processing {
                kind: JobKind::Action(Action::Apply(pending)),
                ..
            } => pending == requested,
            BrowserIntegrationState::Processing {
                prepared,
                kind: JobKind::Action(Action::Remove),
                ..
            } => !requested.enabled && requested.output_dir == prepared.selection.output_dir,
            _ => false,
        }
    }
}

impl crate::app::App {
    pub(crate) fn accept_browser_integration_root_close(
        &mut self,
        ctx: &egui::Context,
        intent: RootCloseIntent,
    ) -> bool {
        if matches!(intent, RootCloseIntent::TrayHide)
            || (matches!(intent, RootCloseIntent::WindowClose)
                && self.settings.minimize_to_tray_on_close
                && self.tray_controller.is_some())
        {
            return false;
        }
        let effect = PreparationEffect::from_selection(&Selection::from_settings(&self.settings));
        let deferred = self.browser_integration.defer_root_close(intent, effect);
        if deferred {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::CancelClose);
            // The receipt may already be queued and its worker's repaint may
            // have been consumed. Schedule adoption, not a timed/busy wait.
            ctx.request_repaint();
        }
        deferred
    }

    pub(crate) fn poll_browser_integration(&mut self, ctx: &egui::Context) {
        let pending_selection = self.browser_integration.pending_selection();
        let draft_before = match (&self.browser_integration.state, self.pref_state.as_ref()) {
            (
                BrowserIntegrationState::Processing {
                    prepared,
                    kind: JobKind::Action(_),
                    ..
                },
                Some(state),
            ) => Some((
                prepared.selection.clone(),
                Selection::from_settings(&state.settings),
            )),
            _ => None,
        };
        let old_output_dir = self.settings.browser_view_output_dir.clone();
        let event = self.browser_integration.poll(&mut self.settings, ctx);
        if old_output_dir != self.settings.browser_view_output_dir {
            let new_output_dir = self.settings.browser_view_output_dir.clone();
            self.invalidate_final_cover_after_shared_input_change(
                ctx,
                crate::app::FinalCoverSharedInputChange::BrowserRoot {
                    old: old_output_dir.as_deref(),
                    new: new_output_dir.as_deref(),
                },
            );
        }
        if let Some(event) = event {
            match event {
                Event::Changed(selection, warning) => {
                    if let Some(preferences) = self.pref_state.as_mut() {
                        selection.apply(&mut preferences.settings);
                        preferences.browser_output_input = selection
                            .output_dir
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_default();
                    }
                    if draft_before
                        .is_some_and(|(before, draft)| draft != before && draft != selection)
                    {
                        self.show_feedback_warning("処理中に取り込んだブラウザ連携の値は適用しませんでした。他の環境設定はそのまま編集できます。".into());
                    }
                    if let Some(error) = warning {
                        crate::logger::log(format!("browser_registration: {error}"));
                        self.show_feedback_error(error);
                    }
                }
                Event::Failed(error) => {
                    if let Some(selection) = pending_selection
                        && let Some(preferences) = self.pref_state.as_mut()
                        && Selection::from_settings(&preferences.settings)
                            == Selection::from_settings(&self.settings)
                    {
                        selection.apply(&mut preferences.settings);
                    }
                    self.show_feedback_error(format!("ブラウザ連携の処理に失敗しました: {error}"));
                }
            }
        }
        if let Some(preferences) = self.pref_state.as_mut() {
            preferences.browser_integration_view = self.browser_integration.view(&self.settings);
        }
        if let Some(intent) = self.browser_integration.take_ready_close() {
            self.resume_root_lifecycle_close(ctx, intent);
        }
    }
    pub(crate) fn submit_browser_integration_preferences(&mut self, ctx: &egui::Context) {
        let action = self
            .pref_state
            .as_mut()
            .and_then(|state| state.browser_integration_action.take());
        if let Some(action) = action {
            let changes_selection = matches!(action, Action::Apply(_) | Action::Remove);
            if self.browser_integration.submit(action, ctx) {
                // The accepted request owns these values until completion. Do not
                // also leave them in the ordinary unsaved-preferences draft.
                if changes_selection && let Some(state) = self.pref_state.as_mut() {
                    Selection::from_settings(&self.settings).apply(&mut state.settings);
                }
            } else {
                self.show_feedback_warning(
                    "ブラウザ連携の処理を開始できません。状態を確認してください。".into(),
                );
            }
        }
        if let Some(state) = self.pref_state.as_mut() {
            state.browser_integration_view = self.browser_integration.view(&self.settings);
        }
    }
    /// Settings-file import still applies ordinary preferences immediately; only
    /// the new registration fields must pass through the same worker owner.
    pub(crate) fn admit_browser_integration_settings_change(&mut self, ctx: &egui::Context) {
        let current = Selection::from_settings(&self.settings);
        let requested = self
            .pref_state
            .as_ref()
            .map(|state| Selection::from_settings(&state.settings));
        if let Some(requested) = requested
            && requested != current
        {
            if let Some(state) = self.pref_state.as_mut() {
                current.apply(&mut state.settings);
            }
            if !self.browser_integration.owns_selection(&requested)
                && !self
                    .browser_integration
                    .submit(Action::Apply(requested), ctx)
            {
                self.show_feedback_warning("ブラウザ連携の値は適用しませんでした。処理の状態や、この起動で利用できるかを確認してください。".into());
            }
        }
    }
}
