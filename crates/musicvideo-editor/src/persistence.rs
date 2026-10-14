//! One host worker owns recent-list and diagnostic I/O. No inferred data directory.
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

pub(crate) const LOG_LIMIT: u64 = 4 * 1024 * 1024;
const RECENT_LIMIT: usize = 10;

pub(crate) enum Command {
    SetContext(egui::Context),
    Add(PathBuf),
    Remove(PathBuf),
    Log(String),
    Print(String),
    Panic(String),
    Stop,
}

pub(crate) enum Event {
    Recent(Vec<PathBuf>),
    Notice(String),
}

pub(crate) struct Persistence {
    pub tx: mpsc::Sender<Command>,
    pub rx: mpsc::Receiver<Event>,
}

pub(crate) struct WorkerGuard {
    tx: mpsc::Sender<Command>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        // Owned outside run_native: final drain happens only after the GUI exits.
        let _ = self.tx.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Persistence {
    pub fn start(data_dir: Option<PathBuf>) -> std::io::Result<(Self, WorkerGuard)> {
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("musicvideo-persistence".into())
            .spawn(move || run_worker(data_dir.as_deref(), commands, events))?;
        Ok((
            Self { tx: tx.clone(), rx },
            WorkerGuard {
                tx,
                worker: Some(worker),
            },
        ))
    }
}

/// Bootstrap and audio command senders share the host's writer. No synchronous
/// stderr fallback: a missing writer is already reported by the host.
pub(crate) fn log(tx: Option<&mpsc::Sender<Command>>, text: String) {
    if let Some(tx) = tx {
        let _ = tx.send(Command::Log(text));
    }
}

struct Diagnostics {
    log: Option<File>,
    panic_path: Option<PathBuf>,
}

impl Diagnostics {
    fn open(data_dir: Option<&Path>) -> std::io::Result<Self> {
        let Some(data_dir) = data_dir else {
            return Ok(Self {
                log: None,
                panic_path: None,
            });
        };
        let logs = data_dir.join("logs");
        fs::create_dir_all(&logs)?;
        let path = logs.join("musicvideo.log");
        if path.metadata().is_ok_and(|meta| meta.len() > LOG_LIMIT) {
            let backup = logs.join("musicvideo.log.1");
            match fs::remove_file(&backup) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            fs::rename(&path, backup)?;
        }
        Ok(Self {
            log: Some(OpenOptions::new().create(true).append(true).open(path)?),
            panic_path: Some(logs.join("musicvideo-panic.log")),
        })
    }

    fn write(&mut self, text: &str, panic: bool) -> std::io::Result<()> {
        eprintln!("{text}");
        if panic {
            if let Some(path) = &self.panic_path {
                writeln!(
                    OpenOptions::new().create(true).append(true).open(path)?,
                    "{text}"
                )?;
            }
        } else if let Some(log) = &mut self.log {
            writeln!(log, "{text}")?;
        }
        Ok(())
    }
}

pub(crate) fn panic_hook(
    tx: mpsc::Sender<Command>,
) -> impl Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static {
    move |info| {
        let _ = tx.send(Command::Panic(format!("{info}")));
    }
}

fn read_recent(path: &Path) -> Result<Vec<PathBuf>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("最近使ったプロジェクトを読めません: {error}")),
    };
    let paths: Vec<PathBuf> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("最近使ったプロジェクトの JSON が壊れています: {error}"))?;
    let mut recent = Vec::new();
    for path in paths {
        if !path.is_absolute() {
            return Err("最近使ったプロジェクトに絶対パス以外が含まれています".into());
        }
        if !recent.contains(&path) {
            recent.push(path);
        }
        if recent.len() == RECENT_LIMIT {
            break;
        }
    }
    Ok(recent)
}

fn write_recent(path: &Path, recent: &[PathBuf]) -> Result<(), String> {
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let parent = path.parent().unwrap();
        fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut temp, recent)?;
        writeln!(temp)?;
        temp.as_file().sync_all()?;
        // Close the writer before replacement: an open previous writer can leave
        // its destination delete-pending on Windows during concurrent replacements.
        temp.into_temp_path().persist(path)?;
        Ok(())
    })();
    result.map_err(|error| format!("最近使ったプロジェクトを保存できません: {error}"))
}

fn run_worker(
    data_dir: Option<&Path>,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
) {
    let mut diagnostics = match Diagnostics::open(data_dir) {
        Ok(diagnostics) => diagnostics,
        Err(error) => {
            let message = format!("音楽ビデオのログを開けません: {error}");
            eprintln!("{message}");
            let _ = events.send(Event::Notice(message));
            // No retries or recovery on a rare log failure.
            Diagnostics {
                log: None,
                panic_path: data_dir.map(|dir| dir.join("logs/musicvideo-panic.log")),
            }
        }
    };
    let path = data_dir.map(|dir| dir.join("musicvideo/recent-projects.json"));
    let mut recent = path
        .as_deref()
        .map_or_else(Vec::new, |path| match read_recent(path) {
            Ok(recent) => recent,
            Err(error) => {
                let _ = diagnostics.write(&error, false);
                Vec::new()
            }
        });
    if path.is_some() {
        let _ = events.send(Event::Recent(recent.clone()));
    }
    let mut context: Option<egui::Context> = None;
    while let Ok(command) = commands.recv() {
        let error = match command {
            Command::SetContext(ctx) => {
                context = Some(ctx);
                None
            }
            Command::Stop => break,
            Command::Log(text) => diagnostics
                .write(&text, false)
                .err()
                .map(|error| format!("音楽ビデオのログを書けません: {error}")),
            Command::Print(text) => {
                // Preview benchmark stdout is part of its CLI contract, but its
                // producer is a GUI callback and must not wait for the stream.
                println!("{text}");
                None
            }
            Command::Panic(text) => diagnostics
                .write(&text, true)
                .err()
                .map(|error| format!("音楽ビデオの panic ログを書けません: {error}")),
            Command::Add(input) => {
                let Some(path) = path.as_deref() else {
                    continue;
                };
                match crate::project_file::absolute_lexical(&input) {
                    Ok(input) => {
                        recent.retain(|entry| entry != &input);
                        recent.insert(0, input);
                        recent.truncate(RECENT_LIMIT);
                        let _ = events.send(Event::Recent(recent.clone()));
                        write_recent(path, &recent).err()
                    }
                    Err(error) => Some(error),
                }
            }
            Command::Remove(input) => {
                let Some(path) = path.as_deref() else {
                    continue;
                };
                recent.retain(|entry| entry != &input);
                let _ = events.send(Event::Recent(recent.clone()));
                write_recent(path, &recent).err()
            }
        };
        if let Some(error) = error {
            // Do not attempt to log a logger failure through itself.
            eprintln!("{error}");
            if error.starts_with("最近使った") {
                let _ = diagnostics.write(&error, false);
            }
            let _ = events.send(Event::Notice(error));
        }
        if let Some(ctx) = &context {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests;
