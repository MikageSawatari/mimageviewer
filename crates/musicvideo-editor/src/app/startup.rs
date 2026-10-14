use super::*;
use crate::persistence::{Command, Event, Persistence};
use crate::startup::{OpenOnStartup, RuntimeAdmission, StartupArgs};

pub(super) struct Host {
    pub runtime: Arc<RuntimeAdmission>,
    pub persistence: Option<Persistence>,
    pub recent: Option<Vec<PathBuf>>,
    pub notices: Vec<String>,
    last_logged_status: String,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            runtime: Arc::new(RuntimeAdmission::Ready { _lease: None }),
            persistence: None,
            recent: None,
            notices: Vec::new(),
            last_logged_status: String::new(),
        }
    }
}

impl MusicVideoLabApp {
    pub(crate) fn configure_startup(
        &mut self,
        args: StartupArgs,
        runtime: Arc<RuntimeAdmission>,
        persistence: Option<Persistence>,
        service_error: Option<String>,
        ctx: &egui::Context,
    ) {
        self.host.runtime = runtime;
        self.host.recent = args.data_dir.as_ref().map(|_| Vec::new());
        self.host.persistence = persistence;
        if let Some(error) = service_error {
            self.host.notices.push(error);
        }
        if let Some(persistence) = &self.host.persistence {
            let _ = persistence.tx.send(Command::SetContext(ctx.clone()));
        }
        if let RuntimeAdmission::Blocked(error) = &*self.host.runtime {
            self.status = error.clone();
            self.log_status();
            return;
        }
        if let Some(error) = args.error {
            self.status = error.clone();
            self.host.notices.push(error);
        } else {
            match args.open {
                // No user input exists yet. Start the existing H1 reader directly;
                // its normal generation/Ready admission protects edits after startup.
                Some(OpenOnStartup::Project(path)) => self.start_project_load(path, ctx),
                Some(OpenOnStartup::Audio(path)) => self.start_audio_load(path, ctx),
                None => {}
            }
        }
        self.log_status();
    }

    pub(super) fn editing_admitted(&self) -> bool {
        matches!(&*self.host.runtime, RuntimeAdmission::Ready { .. })
    }

    pub(super) fn remember_project(&mut self, path: PathBuf) {
        if !path.as_os_str().is_empty() && self.host.recent.is_some() {
            self.send_host(Command::Add(path));
        }
    }

    pub(super) fn send_host(&mut self, command: Command) {
        if let Some(persistence) = &self.host.persistence
            && persistence.tx.send(command).is_err()
        {
            self.host
                .notices
                .push("診断・最近一覧ワーカーが終了しました".into());
            self.host.persistence = None;
        }
    }

    pub(super) fn poll_host(&mut self) {
        loop {
            let Some(persistence) = &self.host.persistence else {
                break;
            };
            match persistence.rx.try_recv() {
                Ok(Event::Recent(recent)) => self.host.recent = Some(recent),
                Ok(Event::Notice(error)) => self.host.notices.push(error),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.host
                        .notices
                        .push("診断・最近一覧ワーカーが終了しました".into());
                    self.host.persistence = None;
                    break;
                }
            }
        }
        self.host.notices.truncate(10);
    }

    pub(super) fn log_status(&mut self) {
        if self.status != self.host.last_logged_status {
            self.host.last_logged_status = self.status.clone();
            self.send_host(Command::Log(self.status.clone()));
        }
    }

    pub(super) fn draw_host_notices(&mut self, ctx: &egui::Context) {
        if self.host.notices.is_empty() {
            return;
        }
        egui::TopBottomPanel::top("musicvideo-startup-notices").show(ctx, |ui| {
            let mut dismiss = false;
            ui.horizontal(|ui| {
                ui.label("音楽ビデオからのお知らせ");
                dismiss = ui.button("閉じる").clicked();
            });
            egui::ScrollArea::vertical()
                .max_height(120.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for notice in &self.host.notices {
                        ui.colored_label(ui.visuals().error_fg_color, notice);
                    }
                });
            if dismiss {
                self.host.notices.clear();
            }
        });
    }
}

impl Host {
    pub(super) fn draw_recent(&self, ui: &mut egui::Ui, actions: &mut Vec<ChromeAction>) {
        ui.add_enabled_ui(self.recent.is_some() && self.persistence.is_some(), |ui| {
            ui.menu_button("最近使ったプロジェクト", |ui| {
                ui.set_max_width(500.0);
                if let Some(recent) = &self.recent {
                    if recent.is_empty() {
                        ui.label("最近使ったプロジェクトはありません");
                    }
                    for path in recent {
                        ui.horizontal(|ui| {
                            if ui
                                .add(egui::Button::new(path.display().to_string()).truncate())
                                .clicked()
                            {
                                actions.push(ChromeAction::OpenRecent(path.clone()));
                                ui.close();
                            }
                            if ui.button("一覧から外す").clicked() {
                                actions.push(ChromeAction::RemoveRecent(path.clone()));
                            }
                        });
                    }
                }
            });
        });
    }
}
