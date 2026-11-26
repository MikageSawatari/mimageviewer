//! スクリプトの製品内の書き手はこの FIFO worker だけ。UI は enqueue だけ行う。

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock, mpsc};

use sha2::{Digest, Sha256};

use super::{CompiledStackScript, DEFAULT_SCRIPT, MAX_SCRIPT_BYTES, script_path};

#[derive(Debug)]
pub struct ScriptDocument {
    pub source: String,
    /// 本文全体の UTF-8 バイト列の SHA-256。欠損時は内蔵既定のもの。
    pub sha256: String,
    pub builtin: bool,
}

#[derive(Debug)]
pub struct ScriptWriteOutcome {
    pub sha256: String,
    pub before_lines: usize,
    pub after_lines: usize,
}

/// worker 専用。空の利用者ファイルもそのまま返す。欠損以外の読み取り失敗を隠さない。
pub fn read_script_document() -> Result<ScriptDocument, String> {
    read_document_at(&script_path())
}

/// agent worker 専用。検証後に enqueue し、自分の仕事の結果だけを待つ。
/// コンパイルのみ行い、group やトップレベルのコードは実行しない。
pub fn write_script(source: String, expected_sha256: String) -> Result<ScriptWriteOutcome, String> {
    write_script_with(source, expected_sha256, writer)
}

fn write_script_with<'a>(
    source: String,
    expected_sha256: String,
    get_writer: impl FnOnce() -> Result<&'a ScriptWriter, String>,
) -> Result<ScriptWriteOutcome, String> {
    validate_source(&source)?;
    get_writer()?.write_validated(source, expected_sha256)
}

/// 環境設定のボタン用。ファイル I/O や worker の完了を UI で待たない。
pub fn enqueue_reset_user_script() {
    enqueue_ui_job(ScriptJob::Reset);
}

/// 作成とエディタ起動も同じ FIFO に入れるため、直前の reset/write のあとに開く。
pub fn enqueue_open_user_script() {
    enqueue_ui_job(ScriptJob::Open);
}

fn enqueue_ui_job(job: ScriptJob) {
    if writer().and_then(|writer| writer.enqueue(job)).is_err() {
        crate::logger::log("stack script job could not be queued");
    }
}

static WRITER: LazyLock<Result<ScriptWriter, String>> =
    LazyLock::new(|| ScriptWriter::start(script_path, open_editor));

fn writer() -> Result<&'static ScriptWriter, String> {
    WRITER.as_ref().map_err(Clone::clone)
}

enum ScriptJob {
    Write {
        source: String,
        expected_sha256: String,
        reply: mpsc::Sender<Result<ScriptWriteOutcome, String>>,
    },
    Reset,
    Open,
}

struct ScriptWriter {
    sender: mpsc::Sender<ScriptJob>,
}

impl ScriptWriter {
    fn start(
        path: impl FnOnce() -> PathBuf + Send + 'static,
        open: impl Fn(&Path) -> Result<(), String> + Send + 'static,
    ) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("stack-script-writer".into())
            .spawn(move || {
                let path = path();
                while let Ok(job) = receiver.recv() {
                    match job {
                        ScriptJob::Write {
                            source,
                            expected_sha256,
                            reply,
                        } => {
                            let _ = reply.send(write_at(&path, &source, &expected_sha256));
                        }
                        ScriptJob::Reset => {
                            if replace_script(&path, DEFAULT_SCRIPT).is_err() {
                                crate::logger::log("stack script reset failed");
                            }
                        }
                        ScriptJob::Open => {
                            if ensure_exists(&path).and_then(|()| open(&path)).is_err() {
                                crate::logger::log("stack script open failed");
                            }
                        }
                    }
                }
            })
            .map_err(|_| "スタック用スクリプトの書き手を開始できません".to_owned())?;
        Ok(Self { sender })
    }

    fn enqueue(&self, job: ScriptJob) -> Result<(), String> {
        self.sender
            .send(job)
            .map_err(|_| "スタック用スクリプトの仕事を受け付けられません".to_owned())
    }

    fn write_validated(
        &self,
        source: String,
        expected_sha256: String,
    ) -> Result<ScriptWriteOutcome, String> {
        let (reply, result) = mpsc::channel();
        self.enqueue(ScriptJob::Write {
            source,
            expected_sha256,
            reply,
        })?;
        result
            .recv()
            .map_err(|_| "スタック用スクリプトの書き込み結果を受け取れません".to_owned())?
    }
}

fn validate_source(source: &str) -> Result<(), String> {
    if source.len() > mimageviewer_agent_ipc::MAX_STACK_SCRIPT_SOURCE_BYTES {
        return Err("AI が書くスタック用スクリプトは UTF-8 で 256 KiB 以下にしてください".into());
    }
    // 実際に使うコンパイラを通す。エラーは agent の応答だけに返し、ログや変更通知へ
    // 渡さないこと (コンパイルエラーにはソースの token/識別子が含まれることがある)。
    CompiledStackScript::compile(source, Arc::new(AtomicBool::new(false))).map(|_| ())
}

fn sha256(source: &str) -> String {
    format!("{:x}", Sha256::digest(source.as_bytes()))
}

fn read_document_at(path: &Path) -> Result<ScriptDocument, String> {
    let (source, builtin) = match File::open(path) {
        Ok(file) => {
            // metadata だけの検査では、検査後にファイルが伸びると上限を越えて読めてしまう。
            let mut bytes = Vec::new();
            file.take(MAX_SCRIPT_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "スタック用スクリプトを読み取れません".to_owned())?;
            if bytes.len() as u64 > MAX_SCRIPT_BYTES {
                return Err("スタック用スクリプトは 1 MiB を超えているため読み取れません".into());
            }
            let source = String::from_utf8(bytes)
                .map_err(|_| "スタック用スクリプトが UTF-8 ではありません".to_owned())?;
            (source, false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (DEFAULT_SCRIPT.to_owned(), true)
        }
        Err(_) => return Err("スタック用スクリプトを開けません".into()),
    };
    Ok(ScriptDocument {
        sha256: sha256(&source),
        source,
        builtin,
    })
}

fn write_at(
    path: &Path,
    source: &str,
    expected_sha256: &str,
) -> Result<ScriptWriteOutcome, String> {
    // 必ず FIFO の仕事の中で照合する。enqueue 前に読むだけでは reset が割り込める。
    let previous = read_document_at(path)?;
    if previous.sha256 != expected_sha256 {
        return Err("読んだあとに変わっています。読み直してください".into());
    }
    replace_script(path, source)?;
    Ok(ScriptWriteOutcome {
        sha256: sha256(source),
        before_lines: previous.source.lines().count(),
        after_lines: source.lines().count(),
    })
}

fn ensure_exists(path: &Path) -> Result<(), String> {
    match fs::metadata(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            replace_script(path, DEFAULT_SCRIPT)
        }
        Err(_) => Err("スタック用スクリプトを確認できません".into()),
    }
}

/// 同じディレクトリの一時ファイルを完全に書いてから公開する。失敗時も旧本文を消さない。
fn replace_script(path: &Path, source: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("スタック用スクリプトの保存先を確認できません")?;
    fs::create_dir_all(parent)
        .map_err(|_| "スタック用スクリプトの保存先を作れません".to_owned())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| "スタック用スクリプトの一時ファイルを作れません".to_owned())?;
    temporary
        .write_all(source.as_bytes())
        .and_then(|()| temporary.flush())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| "スタック用スクリプトを書き込めません".to_owned())?;
    rename_script_temporary(temporary.as_file(), temporary.path(), path)
        .map_err(|_| "スタック用スクリプトを置き換えられません".to_owned())
}

/// POSIX replacement leaves sharing readers on the old contents. The ordinary
/// MoveFileExW helper cannot replace such an open target, even on NTFS.
#[cfg(windows)]
pub(crate) fn rename_script_temporary(
    temporary: &File,
    _temporary_path: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    use std::mem::{offset_of, size_of};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAGS_AND_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FileRenameInfoEx, ReOpenFile, SetFileInformationByHandle,
    };
    use windows::Win32::System::WindowsProgramming::{
        FILE_RENAME_FLAG_POSIX_SEMANTICS, FILE_RENAME_FLAG_REPLACE_IF_EXISTS,
    };

    let invalid_name = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid script destination",
        )
    };
    let absolute = std::path::absolute(destination)?;
    let mut name = absolute.as_os_str().encode_wide().collect::<Vec<_>>();
    if name.is_empty() || name.contains(&0) {
        return Err(invalid_name());
    }
    let name_bytes = name
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|bytes| u32::try_from(bytes).ok())
        .ok_or_else(invalid_name)?;
    name.push(0);
    let total = (offset_of!(FILE_RENAME_INFO, FileName) + name.len() * size_of::<u16>())
        .max(size_of::<FILE_RENAME_INFO>());
    let total_bytes = u32::try_from(total).map_err(|_| invalid_name())?;
    // u64 storage supplies alignment for FILE_RENAME_INFO and its variable tail.
    let mut storage = vec![0u64; total.div_ceil(size_of::<u64>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: the aligned allocation includes the full header and UTF-16 tail.
    // FileNameLength excludes NUL; the buffer passed to Windows includes it.
    unsafe {
        (*info).Anonymous.Flags =
            FILE_RENAME_FLAG_POSIX_SEMANTICS | FILE_RENAME_FLAG_REPLACE_IF_EXISTS;
        (*info).RootDirectory = HANDLE::default();
        (*info).FileNameLength = name_bytes;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            name.len(),
        );
    }
    // Reopen this exact temp object with DELETE access, without looking it up
    // by its pathname again. Keep all sharing modes compatible with std File.
    let handle = unsafe {
        ReOpenFile(
            HANDLE(temporary.as_raw_handle()),
            DELETE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAGS_AND_ATTRIBUTES(0),
        )
    }
    .map_err(|error| std::io::Error::other(error.to_string()))?;
    // SAFETY: ReOpenFile returned a new owned handle, closed once by this guard.
    let handle = unsafe { OwnedHandle::from_raw_handle(handle.0) };
    // SAFETY: the handle remains open and storage is valid for total_bytes.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(handle.as_raw_handle()),
            FileRenameInfoEx,
            storage.as_ptr().cast(),
            total_bytes,
        )
    }
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(not(windows))]
pub(crate) fn rename_script_temporary(
    _temporary: &File,
    temporary_path: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    fs::rename(temporary_path, destination)
}

#[cfg(windows)]
fn open_editor(path: &Path) -> Result<(), String> {
    crate::open_with::shell_execute_open(path, None, None, true, None)
}

#[cfg(not(windows))]
fn open_editor(path: &Path) -> Result<(), String> {
    opener::open(path).map_err(|_| "スタック用スクリプトをエディタで開けません".to_owned())
}

#[cfg(test)]
#[path = "writer_tests.rs"]
mod tests;
