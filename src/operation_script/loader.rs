use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use rhai::{AST, Dynamic, Engine, Scope};

use super::{DEFAULT_SCRIPT, ScriptSlot, build_engine, parse_slots};

/// ファイル名スタックと同じ本文の上限。読む量自体もtakeで制限する。
const MAX_SOURCE_BYTES: u64 = 1024 * 1024;

pub struct CompiledOperationScript {
    pub(super) engine: Engine,
    pub(super) ast: AST,
    pub slots: Vec<ScriptSlot>,
    pub has_on_event: bool,
}

impl CompiledOperationScript {
    /// 読み込みworker上でだけ呼ぶ。UIでのコンパイルし直しは行わない。
    pub fn compile(source: &str) -> Result<Self, String> {
        let engine = build_engine();
        let ast = engine
            .compile(source)
            .map_err(|e| format!("コンパイルに失敗: {e}"))?;
        let has_on_event = ast
            .iter_functions()
            .any(|f| f.name == "on_event" && f.params.len() == 1);
        let slots = engine
            .call_fn::<Dynamic>(&mut Scope::new(), &ast, "slots", ())
            .map_err(|e| format!("slots() の評価に失敗: {e}"))?;
        let slots = parse_slots(slots, &ast).map_err(|e| format!("slots() の指定に失敗: {e}"))?;
        Ok(Self {
            engine,
            ast,
            slots,
            has_on_event,
        })
    }
}

pub fn script_path(data_dir: &Path) -> PathBuf {
    data_dir.join("operation_scripts.rhai")
}

fn load_script(path: &Path) -> Result<CompiledOperationScript, String> {
    let mut source = String::new();
    File::open(path)
        .and_then(|file| file.take(MAX_SOURCE_BYTES + 1).read_to_string(&mut source))
        .map_err(|e| format!("スクリプトの読み込みに失敗 ({}): {e}", path.display()))?;
    if source.len() as u64 > MAX_SOURCE_BYTES {
        return Err("スクリプトの本文が1 MiBの上限を超えています".into());
    }
    CompiledOperationScript::compile(&source)
}

struct LoadResult {
    generation: u64,
    script: Result<CompiledOperationScript, String>,
}

/// 無効・読み込み中・使える・失敗を一つの値で所有する。
/// App共通で保持し、viewer contextのbundleには含めない。
#[derive(Default)]
pub enum OperationScriptState {
    #[default]
    Disabled,
    Loading(OperationScriptLoad),
    Ready(Box<CompiledOperationScript>),
    Failed(String),
}

pub struct OperationScriptLoad {
    rx: Receiver<LoadResult>,
}

#[derive(Default)]
pub struct OperationScriptLoader {
    generation: u64,
    pub state: OperationScriptState,
}

impl OperationScriptLoader {
    /// 起動時・設定ON時はDisabledから一度だけ開始する。I/Oはworkerに渡す。
    pub fn sync_enabled(&mut self, enabled: bool, data_dir: &Path, ctx: &egui::Context) {
        if !enabled {
            if !matches!(self.state, OperationScriptState::Disabled) {
                self.generation = self.generation.wrapping_add(1);
                self.state = OperationScriptState::Disabled;
            }
        } else if matches!(self.state, OperationScriptState::Disabled) {
            self.reload(data_dir, ctx);
        }
    }

    pub fn reload(&mut self, data_dir: &Path, ctx: &egui::Context) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let path = script_path(data_dir);
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        // 前のreceiverをここで捨てる。古いworkerはUIへ結果を適用できない。
        self.state = OperationScriptState::Loading(OperationScriptLoad { rx });
        if let Err(error) = std::thread::Builder::new()
            .name("operation-script-load".into())
            .spawn(move || {
                let script = load_script(&path);
                if tx.send(LoadResult { generation, script }).is_ok() {
                    ctx.request_repaint();
                }
            })
        {
            self.state =
                OperationScriptState::Failed(format!("読み込みworkerの起動に失敗: {error}"));
        }
    }

    /// 非blocking。結果を採用したときだけtrue。古い世代は採用しない。
    pub fn poll(&mut self) -> bool {
        let OperationScriptState::Loading(pending) = &self.state else {
            return false;
        };
        let result = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                self.state =
                    OperationScriptState::Failed("読み込みworkerが結果を返さず終了しました".into());
                return true;
            }
        };
        if result.generation != self.generation {
            return false;
        }
        self.state = match result.script {
            Ok(script) => OperationScriptState::Ready(Box::new(script)),
            Err(error) => {
                crate::logger::log(format!("operation script: {error}"));
                OperationScriptState::Failed(error)
            }
        };
        true
    }
}

/// 段5の「スクリプトを開く」用。呼び出しは投入だけで、存在確認・作成はworker。
/// 既存ファイルはcreate_newにより上書きしない。外部エディタの起動は呼び出し側が行う。
pub fn enqueue_ensure_user_script(
    data_dir: PathBuf,
    ctx: egui::Context,
) -> Receiver<Result<PathBuf, String>> {
    let (tx, rx) = mpsc::channel();
    let failed_tx = tx.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("operation-script-create".into())
        .spawn(move || {
            let path = script_path(&data_dir);
            let result = ensure_user_script(&path).map(|()| path);
            let _ = tx.send(result);
            ctx.request_repaint();
        })
    {
        let _ = failed_tx.send(Err(format!("スクリプト作成workerの起動に失敗: {error}")));
    }
    rx
}

fn ensure_user_script(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("データフォルダの作成に失敗: {e}"))?;
    }
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => file
            .write_all(DEFAULT_SCRIPT.as_bytes())
            .map_err(|e| format!("雛形の保存に失敗: {e}")),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!("雛形の作成に失敗: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_script::{ScriptContext, ScriptEvent, StateSnapshot, parse_output};

    #[test]
    fn default_script_evaluates_slots_and_every_example() {
        let script = CompiledOperationScript::compile(DEFAULT_SCRIPT).unwrap();
        assert_eq!(script.slots.len(), 2);
        assert!(script.has_on_event);
        let mut snapshot = StateSnapshot::new(ScriptContext::FsImage);
        snapshot.page.is_last = Some(false);
        let call = |name: &str, state: &StateSnapshot| {
            let result = script
                .engine
                .call_fn::<Dynamic>(&mut Scope::new(), &script.ast, name, (state.to_dynamic(),))
                .unwrap();
            parse_output(result).unwrap().actions
        };
        assert_eq!(call("next_or_next_book", &snapshot), ["FsPageNext"]);
        snapshot.page.is_last = Some(true);
        assert_eq!(call("next_or_next_book", &snapshot), ["FsSiblingNext"]);
        assert_eq!(call("rate3_next", &snapshot), ["RatingItem3", "FsPageNext"]);
        snapshot.events = vec![ScriptEvent::View];
        snapshot.book.path = Some(r"C:\manga\book".into());
        snapshot.view.direction = Some("ltr".into());
        assert_eq!(call("on_event", &snapshot), ["FsReadingDirectionToggle"]);
        for slot in &script.slots {
            assert!(call(&slot.function, &StateSnapshot::new(ScriptContext::Grid)).is_empty());
        }
    }

    #[test]
    fn worker_load_success_and_failure_reasons() {
        let temp = tempfile::tempdir().unwrap();
        let path = script_path(temp.path());
        let error = load_script(&path).err().unwrap();
        assert!(error.contains("読み込みに失敗"));
        std::fs::write(&path, "fn slots( {").unwrap();
        assert!(load_script(&path).err().unwrap().contains("コンパイル"));
        std::fs::write(&path, "fn slots() { throw \"bad slots\"; }").unwrap();
        assert!(load_script(&path).err().unwrap().contains("bad slots"));
        std::fs::write(&path, DEFAULT_SCRIPT).unwrap();
        let mut loader = OperationScriptLoader::default();
        loader.reload(temp.path(), &egui::Context::default());
        let OperationScriptState::Loading(pending) = &loader.state else {
            panic!("not loading")
        };
        let result = pending
            .rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(result.generation, loader.generation);
        assert_eq!(result.script.unwrap().slots.len(), 2);
    }

    #[test]
    fn loader_discards_stale_generation_and_accepts_latest() {
        let (tx, rx) = mpsc::channel();
        let mut loader = OperationScriptLoader {
            generation: 2,
            state: OperationScriptState::Loading(OperationScriptLoad { rx }),
        };
        tx.send(LoadResult {
            generation: 1,
            script: Err("old".into()),
        })
        .unwrap();
        assert!(!loader.poll());
        assert!(matches!(loader.state, OperationScriptState::Loading(_)));
        tx.send(LoadResult {
            generation: 2,
            script: Err("latest reason".into()),
        })
        .unwrap();
        assert!(loader.poll());
        assert!(
            matches!(&loader.state, OperationScriptState::Failed(reason) if reason == "latest reason")
        );
    }

    #[test]
    fn loader_publishes_compiled_ast_and_slots_as_one_ready_value() {
        let (tx, rx) = mpsc::channel();
        let mut loader = OperationScriptLoader {
            generation: 3,
            state: OperationScriptState::Loading(OperationScriptLoad { rx }),
        };
        assert!(
            tx.send(LoadResult {
                generation: 3,
                script: CompiledOperationScript::compile(DEFAULT_SCRIPT),
            })
            .is_ok()
        );
        assert!(loader.poll());
        let OperationScriptState::Ready(script) = &loader.state else {
            panic!("not ready");
        };
        assert_eq!(script.slots.len(), 2);
        assert!(script.has_on_event);
        let result = script
            .engine
            .call_fn::<Dynamic>(
                &mut Scope::new(),
                &script.ast,
                "rate3_next",
                (StateSnapshot::new(ScriptContext::Grid).to_dynamic(),),
            )
            .unwrap();
        assert!(parse_output(result).unwrap().actions.is_empty());
        assert!(!loader.poll());
    }

    #[test]
    fn operation_script_settings_default_off_and_missing_fields_need_no_migration() {
        let settings = crate::settings::Settings::default();
        assert!(!settings.operation_script_enabled);
        assert!(!settings.operation_script_notice_confirmed);
        let mut json = serde_json::to_value(&settings).unwrap();
        let object = json.as_object_mut().unwrap();
        object.remove("operation_script_enabled");
        object.remove("operation_script_notice_confirmed");
        let restored: crate::settings::Settings = serde_json::from_value(json).unwrap();
        assert!(!restored.operation_script_enabled);
        assert!(!restored.operation_script_notice_confirmed);
    }

    #[test]
    fn loader_disable_retires_pending_result_and_on_starts_once() {
        let (tx, rx) = mpsc::channel();
        let mut loader = OperationScriptLoader {
            generation: 1,
            state: OperationScriptState::Loading(OperationScriptLoad { rx }),
        };
        let ctx = egui::Context::default();
        loader.sync_enabled(false, Path::new("unused"), &ctx);
        assert!(
            tx.send(LoadResult {
                generation: 1,
                script: Err("old".into())
            })
            .is_err()
        );
        assert!(matches!(loader.state, OperationScriptState::Disabled));
        let temp = tempfile::tempdir().unwrap();
        loader.sync_enabled(true, temp.path(), &ctx);
        let generation = loader.generation;
        loader.sync_enabled(true, temp.path(), &ctx);
        assert_eq!(loader.generation, generation);
    }

    #[test]
    fn create_default_worker_preserves_existing_user_script() {
        let temp = tempfile::tempdir().unwrap();
        let path = enqueue_ensure_user_script(temp.path().into(), egui::Context::default())
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_SCRIPT);
        std::fs::write(&path, "user content").unwrap();
        enqueue_ensure_user_script(temp.path().into(), egui::Context::default())
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "user content");
    }
}
