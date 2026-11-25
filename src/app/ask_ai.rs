//! Help submenu: registration is sampled only while the submenu is drawn.
use crate::agent_registration::{AppTarget, Detection, RegistrationState};
use crate::external_links::{AiAppLink, AiWebSite, OpenTarget};
use crate::ui_dialogs::agent_integration::{
    RegistrationDetectionReceiver, poll_registration_detection, spawn_registration_detection,
};
use std::time::{Duration, Instant};

const REGISTRATION_INTERVAL: Duration = Duration::from_secs(5);
// U1: keep the Web and getting-started sections even with registered apps.
fn show_web_and_guide(_has_registered_app: bool) -> bool {
    true
}

pub(crate) const APP_PROMPT: &str = "mImageViewer の MCP ツール (search_help・get_help) で、この版のマニュアルを調べてから答えてください。質問: ";

pub(crate) fn web_prompt() -> String {
    format!(
        "mImageViewer (mIV、Windows 用の画像・動画・本のビューア) v{} の使い方の質問です。公式マニュアル https://www.mikage.to/mimageviewer/manual/ の内容をもとに答えてください。質問: ",
        env!("CARGO_PKG_VERSION")
    )
}

#[derive(Default)]
pub(crate) struct AskAiRegistration {
    pending: Option<RegistrationDetectionReceiver>,
    latest: Option<[bool; 2]>,
    last_started: Option<Instant>,
    #[cfg(test)]
    mock_backend: Option<crate::ui_dialogs::agent_integration::MockBackend>,
}

fn registered_apps(detection: &Detection) -> [bool; 2] {
    [AppTarget::ClaudeDesktop, AppTarget::ChatGpt].map(|target| {
        detection
            .apps
            .iter()
            .any(|app| app.target == target && app.state == RegistrationState::Registered)
    })
}

impl AskAiRegistration {
    fn refresh(&mut self, ctx: &egui::Context, now: Instant) {
        if let Some(result) = self.pending.as_ref().and_then(poll_registration_detection) {
            self.latest = Some(match result {
                Ok((_, detection)) => registered_apps(&detection),
                Err(_) => {
                    crate::logger::log("ask_ai registration detection failed");
                    [false; 2]
                }
            });
            self.pending = None;
        }
        if self.pending.is_none()
            && self
                .last_started
                .is_none_or(|last| now.saturating_duration_since(last) >= REGISTRATION_INTERVAL)
        {
            self.pending = Some(spawn_registration_detection(
                ctx,
                #[cfg(test)]
                self.mock_backend.as_ref(),
            ));
            self.last_started = Some(now);
        }
        if self.pending.is_none()
            && let Some(last) = self.last_started
        {
            ctx.request_repaint_after_for(
                REGISTRATION_INTERVAL.saturating_sub(now.saturating_duration_since(last)),
                egui::ViewportId::ROOT,
            );
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Destination {
    ClaudeApp,
    ChatGptApp,
    Google,
    ChatGptWeb,
    ClaudeWeb,
    Guide,
}

impl Destination {
    fn label(self) -> &'static str {
        match self {
            Self::ClaudeApp => "Claude で調べる (Claude Desktop)",
            Self::ChatGptApp => "ChatGPT で調べる (Codex)",
            Self::Google => "Google の AI モードで調べる",
            Self::ChatGptWeb => "ChatGPT (Web) で調べる",
            Self::ClaudeWeb => "Claude (Web) で調べる",
            Self::Guide => "AI 連携の始め方 (MCP の設定)",
        }
    }

    fn target(self) -> OpenTarget {
        match self {
            Self::ClaudeApp => OpenTarget::AiApp(AiAppLink::ClaudeNew {
                prompt: APP_PROMPT.into(),
            }),
            Self::ChatGptApp => OpenTarget::AiApp(AiAppLink::CodexNew {
                prompt: APP_PROMPT.into(),
            }),
            Self::Guide => OpenTarget::Url(crate::ui_helpers::manual_url(
                "tut-ai-integration.html",
                None,
            )),
            web => OpenTarget::AiWeb {
                site: match web {
                    Self::Google => AiWebSite::Google,
                    Self::ChatGptWeb => AiWebSite::ChatGpt,
                    Self::ClaudeWeb => AiWebSite::Claude,
                    _ => unreachable!(),
                },
                text: web_prompt(),
                clipboard_seq: crate::ui_dialogs::context_menu::request_edited_clipboard_write(),
            },
        }
    }
}

fn fixed_scope<R>(
    ui: &mut egui::Ui,
    key: impl std::hash::Hash,
    draw: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.scope_builder(
        egui::UiBuilder::new().scope_id(egui::Id::new(("help_ask_ai", key))),
        draw,
    )
    .inner
}

fn draw_contents(
    ui: &mut egui::Ui,
    registered: [bool; 2],
    checking: bool,
    mut observe: impl FnMut(Destination, &egui::Response),
) -> Option<Destination> {
    let mut selected = None;
    let mut button = |ui: &mut egui::Ui, destination: Destination| {
        let response = fixed_scope(ui, destination, |ui| ui.button(destination.label()));
        observe(destination, &response);
        if response.clicked() {
            selected = Some(destination);
        }
    };
    if checking {
        fixed_scope(ui, "checking", |ui| {
            ui.weak("AI アプリの登録を確認中…");
        });
    }
    for (enabled, destination) in registered
        .into_iter()
        .zip([Destination::ClaudeApp, Destination::ChatGptApp])
    {
        if enabled {
            button(ui, destination);
        }
    }
    let has_registered = registered.into_iter().any(|value| value);
    if show_web_and_guide(has_registered) {
        if has_registered || checking {
            fixed_scope(ui, "apps_separator", |ui| {
                ui.separator();
            });
        }
        for destination in [
            Destination::Google,
            Destination::ChatGptWeb,
            Destination::ClaudeWeb,
        ] {
            button(ui, destination);
        }
        fixed_scope(ui, "web_separator", |ui| {
            ui.separator();
        });
        button(ui, Destination::Guide);
        fixed_scope(ui, "description", |ui| {
            ui.weak(if has_registered {
                "Web の AI へは案内の文をコピーして開きます"
            } else {
                "AI アプリに mIV を登録すると、この版のマニュアルを引いて答えます"
            });
        });
    }
    selected
}

impl super::App {
    pub(crate) fn draw_ask_ai_menu(&mut self, ui: &mut egui::Ui) {
        self.ask_ai_registration.refresh(ui.ctx(), Instant::now());
        let registration = &self.ask_ai_registration;
        let selected = draw_contents(
            ui,
            registration.latest.unwrap_or([false; 2]),
            registration.latest.is_none() && registration.pending.is_some(),
            |_, _| {},
        );
        if let Some(destination) = selected {
            let _ = crate::external_links::spawn_open(
                destination.target(),
                Some(self.external_open_sink(ui.ctx())),
            );
            ui.close();
        }
    }
}

#[cfg(test)]
mod tests;
