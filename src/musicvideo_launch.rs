//! Launch the separate music-video editor without sharing viewer state or IPC.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

struct LaunchRequest {
    audio: PathBuf,
    data_dir: PathBuf,
}

#[derive(Debug)]
struct LaunchCommand {
    executable: PathBuf,
    audio: PathBuf,
    data_dir: PathBuf,
}

impl LaunchCommand {
    fn resolve(core: &Path, request: LaunchRequest) -> Result<Self, String> {
        let absolute = |path: &Path| {
            std::path::absolute(path).map_err(|error| {
                format!(
                    "音楽ビデオエディタのパスを解決できません: {} ({error})",
                    path.display()
                )
            })
        };
        Ok(Self {
            executable: core.with_file_name("mimageviewer-musicvideo.exe"),
            audio: absolute(&request.audio)?,
            data_dir: absolute(&request.data_dir)?,
        })
    }

    fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.executable);
        command
            .arg("--audio")
            .arg(&self.audio)
            .arg("--data-dir")
            .arg(&self.data_dir);
        command
    }
}

fn launch_with(
    command: LaunchCommand,
    cancel: &AtomicBool,
    spawn: impl FnOnce(&mut std::process::Command) -> std::io::Result<()>,
) -> Result<(), String> {
    if !command.executable.is_file() {
        return Err(format!(
            "音楽ビデオエディタが見つかりません: {}",
            command.executable.display()
        ));
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    spawn(&mut command.command()).map_err(|error| {
        format!(
            "音楽ビデオエディタを起動できません: {} ({error})",
            command.executable.display()
        )
    })
}

pub(crate) struct MusicVideoLaunchPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<(), String>>,
}

impl Drop for MusicVideoLaunchPending {
    fn drop(&mut self) {
        // Match external-tool shutdown: cancel unstarted work, never wait for
        // a worker or stop a child that has already been spawned.
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn start_worker_with(
    request: LaunchRequest,
    ctx: egui::Context,
    launch: impl FnOnce(LaunchRequest, &AtomicBool) -> Result<(), String> + Send + 'static,
) -> Result<MusicVideoLaunchPending, String> {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    std::thread::Builder::new()
        .name("musicvideo-launch".into())
        .spawn(move || {
            if worker_cancel.load(Ordering::Relaxed) {
                return;
            }
            let result = launch(request, &worker_cancel);
            let _ = tx.send(result);
            ctx.request_repaint();
        })
        .map_err(|error| format!("音楽ビデオエディタ起動 worker を開始できません: {error}"))?;
    Ok(MusicVideoLaunchPending { cancel, rx })
}

impl crate::app::App {
    pub(crate) fn start_musicvideo_editor(&mut self, ctx: &egui::Context, audio: PathBuf) {
        self.start_musicvideo_editor_with(ctx, audio, |request, cancel| {
            let core = std::env::current_exe()
                .map_err(|error| format!("音楽ビデオエディタの場所を解決できません: {error}"))?;
            let command = LaunchCommand::resolve(&core, request)?;
            // Dropping Child detaches; the GUI subsystem needs no console flags.
            launch_with(command, cancel, |command| command.spawn().map(|_| ()))
        });
    }

    fn start_musicvideo_editor_with(
        &mut self,
        ctx: &egui::Context,
        audio: PathBuf,
        launch: impl FnOnce(LaunchRequest, &AtomicBool) -> Result<(), String> + Send + 'static,
    ) {
        let request = LaunchRequest {
            audio,
            data_dir: crate::data_dir::get(),
        };
        let result = start_worker_with(request, ctx.clone(), launch);
        match result {
            Ok(pending) => self.musicvideo_launches.push(pending),
            Err(error) => {
                crate::logger::log(&error);
                self.show_feedback_toast(error);
            }
        }
        ctx.request_repaint();
    }

    pub(crate) fn poll_musicvideo_launches(&mut self, ctx: &egui::Context) {
        let mut index = 0;
        while index < self.musicvideo_launches.len() {
            let result = match self.musicvideo_launches[index].rx.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => {
                    index += 1;
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err("音楽ビデオエディタ起動 worker が切断されました".into())
                }
            };
            self.musicvideo_launches.remove(index);
            if let Err(error) = result {
                crate::logger::log(&error);
                self.show_feedback_toast(error);
            }
        }
        if !self.musicvideo_launches.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> LaunchRequest {
        LaunchRequest {
            audio: PathBuf::from("media/日本語 song.mp3"),
            data_dir: PathBuf::from("relative data"),
        }
    }

    #[test]
    fn context_menu_musicvideo_arguments_are_absolute_and_keep_os_paths() {
        let core = std::path::absolute("runtime/mimageviewer-core.exe").unwrap();
        for data_dir in [
            PathBuf::from("relative data"),
            std::path::absolute("absolute data").unwrap(),
        ] {
            let request = LaunchRequest {
                data_dir: data_dir.clone(),
                ..request()
            };
            let resolved = LaunchCommand::resolve(&core, request).unwrap();
            let command = resolved.command();
            assert_eq!(
                command.get_program(),
                core.with_file_name("mimageviewer-musicvideo.exe")
            );
            let args: Vec<_> = command.get_args().collect();
            assert_eq!(args.len(), 4);
            assert_eq!(args[0], "--audio");
            assert_eq!(
                args[1],
                std::path::absolute("media/日本語 song.mp3").unwrap()
            );
            assert_eq!(args[2], "--data-dir");
            assert_eq!(args[3], std::path::absolute(data_dir).unwrap());
            assert!(Path::new(args[1]).is_absolute());
            assert!(Path::new(args[3]).is_absolute());
        }
    }

    #[test]
    fn context_menu_musicvideo_worker_is_nonblocking_and_allows_multiple_launches() {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let ui_thread = std::thread::current().id();
        let (entered_tx, entered_rx) = mpsc::channel();
        let mut releases = Vec::new();
        for _ in 0..2 {
            let (release_tx, release_rx) = mpsc::channel();
            releases.push(release_tx);
            let entered_tx = entered_tx.clone();
            app.start_musicvideo_editor_with(&ctx, request().audio, move |request, _| {
                entered_tx
                    .send((std::thread::current().id(), request.audio, request.data_dir))
                    .unwrap();
                release_rx
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
                Ok(())
            });
        }
        for _ in 0..2 {
            let (thread, audio, data_dir) = entered_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            assert_ne!(thread, ui_thread);
            assert_eq!(audio, request().audio);
            assert_eq!(data_dir, crate::data_dir::get());
        }
        app.poll_musicvideo_launches(&ctx);
        assert_eq!(app.musicvideo_launches.len(), 2);
        for release in releases {
            release.send(()).unwrap();
        }
        for pending in &app.musicvideo_launches {
            assert!(
                pending
                    .rx
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap()
                    .is_ok()
            );
        }
        // Consumed completions disconnect; use separately completed channels to
        // verify successful polling is silent, without a real editor process.
        app.musicvideo_launches.clear();
        for _ in 0..2 {
            let (tx, rx) = mpsc::channel();
            tx.send(Ok(())).unwrap();
            app.musicvideo_launches.push(MusicVideoLaunchPending {
                rx,
                cancel: Arc::new(AtomicBool::new(false)),
            });
        }
        app.poll_musicvideo_launches(&ctx);
        assert!(app.musicvideo_launches.is_empty());
        assert!(app.fs_feedback_toast.is_none());
        assert!(matches!(
            app.external_tool_operation,
            crate::external_tool::ExternalOperation::Idle
        ));
    }

    #[test]
    fn context_menu_musicvideo_missing_exe_and_spawn_failure_notify_with_reason() {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("mimageviewer-musicvideo.exe");
        for exists in [false, true] {
            if exists {
                std::fs::write(&exe, b"dummy, never executed").unwrap();
            }
            let command =
                LaunchCommand::resolve(&tmp.path().join("mimageviewer-core.exe"), request())
                    .unwrap();
            let result = launch_with(command, &AtomicBool::new(false), |_| {
                assert!(exists, "missing editor must never reach spawn");
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected spawn denial",
                ))
            });
            let (tx, rx) = mpsc::channel();
            tx.send(result).unwrap();
            app.musicvideo_launches.push(MusicVideoLaunchPending {
                rx,
                cancel: Arc::new(AtomicBool::new(false)),
            });
            app.poll_musicvideo_launches(&ctx);
            assert!(app.musicvideo_launches.is_empty());
            let message = &app.fs_feedback_toast.as_ref().unwrap().0;
            assert!(message.contains(&exe.display().to_string()));
            assert!(message.contains(if exists {
                "起動できません"
            } else {
                "見つかりません"
            }));
            if exists {
                assert!(message.contains("injected spawn denial"));
            }
        }
    }

    #[test]
    fn context_menu_musicvideo_disconnected_worker_notifies_and_finishes() {
        let mut app = crate::app::setup_app_for_test();
        let (tx, rx) = mpsc::channel();
        drop(tx);
        app.musicvideo_launches.push(MusicVideoLaunchPending {
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        app.poll_musicvideo_launches(&egui::Context::default());
        assert!(app.musicvideo_launches.is_empty());
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .unwrap()
                .0
                .contains("worker が切断")
        );
    }

    #[test]
    fn context_menu_musicvideo_drop_cancels_without_waiting_or_spawning() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("mimageviewer-musicvideo.exe"), b"dummy").unwrap();
        let command = LaunchCommand::resolve(&tmp.path().join("core.exe"), request()).unwrap();
        let ctx = egui::Context::default();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let pending = start_worker_with(request(), ctx, move |_, cancel| {
            entered_tx.send(()).unwrap();
            release_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            assert!(cancel.load(Ordering::Relaxed));
            let result = launch_with(command, cancel, |_| {
                panic!("cancelled launch must not spawn")
            });
            done_tx.send(result.is_ok()).unwrap();
            result
        })
        .unwrap();
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let cancel = Arc::clone(&pending.cancel);
        drop(pending);
        assert!(cancel.load(Ordering::Relaxed));
        release_tx.send(()).unwrap();
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap()
        );
    }
}
