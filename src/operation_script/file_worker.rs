//! Loader ごとの単一 FIFO。ファイル操作と読み込みは必ず同じ列を通る。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use super::DEFAULT_SCRIPT;
use super::loader::{LoadResult, load_script};

pub(super) enum FileJob {
    Load { generation: u64 },
    Open,
    Reset,
}

pub(super) enum FileResult {
    Load(LoadResult),
    Open(Result<(), String>),
    Reset(Result<(), String>),
}

pub(super) struct FileWorkerOptions {
    path: PathBuf,
    open_editor: Box<dyn Fn(&Path) -> Result<(), String> + Send>,
    #[cfg(test)]
    before_job: Box<dyn Fn() + Send>,
}

impl FileWorkerOptions {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            open_editor: Box::new(open_editor),
            #[cfg(test)]
            before_job: Box::new(|| {}),
        }
    }

    #[cfg(test)]
    pub(super) fn for_test(
        path: PathBuf,
        editor: impl Fn(&Path) -> Result<(), String> + Send + 'static,
        before_job: impl Fn() + Send + 'static,
    ) -> Self {
        Self {
            path,
            open_editor: Box::new(editor),
            before_job: Box::new(before_job),
        }
    }
}

pub(super) struct FileWorker {
    sender: Sender<FileJob>,
    // Dropping the handle detaches: production never waits on the UI thread.
    // Tests explicitly close the sender and join before removing their data.
    _thread: JoinHandle<()>,
}

impl FileWorker {
    pub(super) fn start(
        options: FileWorkerOptions,
        ctx: egui::Context,
    ) -> Result<(Self, Receiver<FileResult>), String> {
        let (sender, jobs) = mpsc::channel();
        let (results, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("operation-script-files".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    #[cfg(test)]
                    (options.before_job)();
                    let result = match job {
                        FileJob::Load { generation } => FileResult::Load(LoadResult {
                            generation,
                            script: ensure_user_script(&options.path)
                                .and_then(|()| load_script(&options.path)),
                        }),
                        FileJob::Open => FileResult::Open(
                            ensure_user_script(&options.path)
                                .and_then(|()| (options.open_editor)(&options.path)),
                        ),
                        FileJob::Reset => FileResult::Reset(reset_user_script(&options.path)),
                    };
                    // Even after the loader drops its receiver, finish queued jobs.
                    if results.send(result).is_ok() {
                        ctx.request_repaint();
                    }
                }
            })
            .map_err(|error| format!("スクリプトのファイルworkerの起動に失敗: {error}"))?;
        Ok((
            Self {
                sender,
                _thread: thread,
            },
            receiver,
        ))
    }

    pub(super) fn enqueue(&self, job: FileJob) -> Result<(), String> {
        self.sender
            .send(job)
            .map_err(|_| "スクリプトのファイルworkerに要求を送れません".into())
    }

    #[cfg(test)]
    pub(super) fn shutdown(self) {
        drop(self.sender);
        self._thread
            .join()
            .expect("operation script file worker panicked");
    }
}

fn ensure_user_script(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("データフォルダの作成に失敗: {error}"))?;
    }
    // create_new is the ownership boundary: both a user file and an empty file
    // that another process has created must remain untouched.
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => file
            .write_all(DEFAULT_SCRIPT.as_bytes())
            .map_err(|error| format!("雛形の保存に失敗: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!("雛形の作成に失敗: {error}")),
    }
}

fn reset_user_script(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("スクリプトの保存先を確認できません")?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("データフォルダの作成に失敗: {error}"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("スクリプトの一時ファイルの作成に失敗: {error}"))?;
    temporary
        .write_all(DEFAULT_SCRIPT.as_bytes())
        .and_then(|()| temporary.flush())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| format!("既定のスクリプトの保存に失敗: {error}"))?;
    crate::filename_stack_script::rename_script_temporary(
        temporary.as_file(),
        temporary.path(),
        path,
    )
    .map_err(|error| format!("スクリプトの置き換えに失敗: {error}"))
}

#[cfg(windows)]
fn open_editor(path: &Path) -> Result<(), String> {
    crate::open_with::shell_execute_open(path, None, None, true, None)
}

#[cfg(not(windows))]
fn open_editor(path: &Path) -> Result<(), String> {
    opener::open(path).map_err(|error| format!("エディタで開けません: {error}"))
}
