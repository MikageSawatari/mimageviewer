use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use rhai::{AST, Dynamic, Engine, Scope};

#[cfg(test)]
use super::DEFAULT_SCRIPT;
use super::file_worker::{FileJob, FileResult, FileWorker, FileWorkerOptions};
use super::{ScriptSlot, build_engine, parse_slots};

/// 操作スクリプト本文の上限。読む量自体もtakeで制限する (§7)。
pub(super) const MAX_SOURCE_BYTES: u64 = 256 * 1024;

/// App admits and enqueues in the same UI turn; only the FIFO reads or writes.
#[derive(Clone, Debug)]
pub(crate) struct AgentScriptRequest {
    pub source: Option<String>,
    pub expected_sha256: Option<String>,
    pub reply: Sender<Result<AgentScriptOutcome, String>>,
}

#[derive(Clone, Debug)]
pub(crate) enum AgentScriptOutcome {
    Read(ScriptDocument),
    Write(ScriptWriteOutcome),
}

#[derive(Clone, Debug)]
pub(crate) struct ScriptDocument {
    pub source: String,
    pub sha256: String,
    pub builtin: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ScriptWriteOutcome {
    pub sha256: String,
    pub before_lines: usize,
    pub after_lines: usize,
}

pub struct CompiledOperationScript {
    pub(super) engine: Engine,
    pub(super) ast: AST,
    pub slots: Vec<ScriptSlot>,
    pub has_on_event: bool,
    operations: Arc<AtomicU64>,
}

impl CompiledOperationScript {
    /// コンパイル済みのスロットを、その入力面で捕捉した状態だけで評価する。
    /// ファイル・DB・Appには触れない。
    pub fn evaluate_slot(
        &self,
        number: u8,
        snapshot: &super::StateSnapshot,
    ) -> Result<super::ScriptOutput, String> {
        self.operations.store(0, Ordering::Relaxed);
        let slot = self
            .slots
            .iter()
            .find(|slot| slot.number == number)
            .ok_or_else(|| format!("スロット {number} は未登録です"))?;
        self.evaluate_function(&slot.function, snapshot)
    }

    /// The event receiver uses the same bounded engine and output contract as a slot.
    /// Detection/admission and execution remain the caller's responsibility.
    pub fn evaluate_event(
        &self,
        snapshot: &super::StateSnapshot,
    ) -> Result<super::ScriptOutput, String> {
        self.operations.store(0, Ordering::Relaxed);
        if !self.has_on_event {
            return Err("イベントの受け口がありません".into());
        }
        self.evaluate_function("on_event", snapshot)
    }

    fn evaluate_function(
        &self,
        function: &str,
        snapshot: &super::StateSnapshot,
    ) -> Result<super::ScriptOutput, String> {
        let value = self
            .engine
            .call_fn::<Dynamic>(
                &mut Scope::new(),
                &self.ast,
                function,
                (snapshot.to_dynamic(),),
            )
            .map_err(|error| {
                if matches!(
                    error.unwrap_inner(),
                    rhai::EvalAltResult::ErrorTooManyOperations(_)
                        | rhai::EvalAltResult::ErrorStackOverflow(_)
                        | rhai::EvalAltResult::ErrorDataTooLarge(..)
                ) {
                    "時間がかかりすぎたため中止".to_owned()
                } else {
                    format!("スクリプトの実行に失敗: {error}")
                }
            })?;
        super::parse_output(value)
    }

    pub fn last_operation_count(&self) -> u64 {
        self.operations.load(Ordering::Relaxed)
    }

    /// 読み込みworker上でだけ呼ぶ。UIでのコンパイルし直しは行わない。
    pub fn compile(source: &str) -> Result<Self, String> {
        let mut engine = build_engine();
        let operations = Arc::new(AtomicU64::new(0));
        let progress = operations.clone();
        engine.on_progress(move |count| {
            progress.store(count, Ordering::Relaxed);
            None
        });
        let ast = super::syntax::compile_checked(&engine, source)?;
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
            operations,
        })
    }
}

pub fn script_path(data_dir: &Path) -> PathBuf {
    data_dir.join("operation_scripts.rhai")
}

pub(super) fn load_script(path: &Path) -> Result<CompiledOperationScript, String> {
    let file = File::open(path)
        .map_err(|e| format!("スクリプトの読み込みに失敗 ({}): {e}", path.display()))?;
    CompiledOperationScript::compile(&read_script_source(file, path)?)
}

pub(super) fn read_script_source(file: File, path: &Path) -> Result<String, String> {
    let mut source = Vec::new();
    file.take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut source)
        .map_err(|e| format!("スクリプトの読み込みに失敗 ({}): {e}", path.display()))?;
    if source.len() as u64 > MAX_SOURCE_BYTES {
        return Err("スクリプトの本文が256 KiBの上限を超えています".into());
    }
    // Check the byte limit before UTF-8 decoding: take() may cut a multibyte
    // character when the file is too large, which must still report the cap.
    String::from_utf8(source)
        .map_err(|e| format!("スクリプトの読み込みに失敗 ({}): {e}", path.display()))
}

pub(super) struct LoadResult {
    pub(super) generation: u64,
    pub(super) script: Result<CompiledOperationScript, String>,
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

pub struct OperationScriptLoad;

#[derive(Default)]
pub struct OperationScriptLoader {
    generation: u64,
    pub state: OperationScriptState,
    worker: Option<FileWorker>,
    results: Option<Receiver<FileResult>>,
    notifications: Vec<String>,
    #[cfg(test)]
    worker_options: Option<FileWorkerOptions>,
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
        self.state = OperationScriptState::Loading(OperationScriptLoad);
        if let Err(error) = self.enqueue(
            FileJob::Load {
                generation: self.generation,
            },
            data_dir,
            ctx,
        ) {
            crate::logger::log(format!("operation script: {error}"));
            self.state = OperationScriptState::Failed(error);
        }
    }

    /// UI は投入だけ。エディタ起動も読み込みと同じ FIFO で処理する。
    pub fn enqueue_open_user_script(&mut self, data_dir: &Path, ctx: &egui::Context) {
        if let Err(error) = self.enqueue(FileJob::Open, data_dir, ctx) {
            self.notifications
                .push(format!("スクリプトを開けません: {error}"));
        }
    }

    pub fn enqueue_reset_user_script(&mut self, data_dir: &Path, ctx: &egui::Context) {
        if let Err(error) = self.enqueue(FileJob::Reset, data_dir, ctx) {
            self.notifications
                .push(format!("スクリプトを既定に戻せません: {error}"));
        }
    }

    /// Capture adoption eligibility without retiring a usable script or changing
    /// generation. Rejected jobs never publish a loader result.
    pub(crate) fn enqueue_agent_script(
        &mut self,
        request: AgentScriptRequest,
        data_dir: &Path,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let generation =
            (!matches!(self.state, OperationScriptState::Disabled)).then_some(self.generation);
        self.enqueue(
            FileJob::Agent {
                request,
                generation,
            },
            data_dir,
            ctx,
        )
    }

    fn enqueue(
        &mut self,
        job: FileJob,
        data_dir: &Path,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        if self.worker.is_none() {
            #[cfg(test)]
            let options = self
                .worker_options
                .take()
                .unwrap_or_else(|| FileWorkerOptions::new(script_path(data_dir)));
            #[cfg(not(test))]
            let options = FileWorkerOptions::new(script_path(data_dir));
            let (worker, results) = FileWorker::start(options, ctx.clone())?;
            self.worker = Some(worker);
            self.results = Some(results);
        }
        self.worker.as_ref().expect("worker started").enqueue(job)
    }

    /// 状態を問わず全結果を非blockingで回収する。通知とログは App が発行する。
    /// OFF でも受け口は捨てず、古い Load の後のファイル操作の失敗まで読み切る。
    pub fn poll(&mut self) -> Vec<String> {
        let mut notifications = std::mem::take(&mut self.notifications);
        let Some(results) = &self.results else {
            return notifications;
        };
        loop {
            match results.try_recv() {
                Ok(FileResult::Load(result)) => {
                    if result.generation != self.generation
                        || !matches!(self.state, OperationScriptState::Loading(_))
                    {
                        continue;
                    }
                    self.state = match result.script {
                        Ok(script) => OperationScriptState::Ready(Box::new(script)),
                        Err(error) => {
                            // The result is consumed once. Share the Open/Reset
                            // notification path; App logs and shows the error.
                            notifications.push(format!("スクリプトを読み込めません: {error}"));
                            OperationScriptState::Failed(error)
                        }
                    };
                }
                Ok(FileResult::AiWrite { generation, script }) => {
                    if generation == Some(self.generation)
                        && matches!(
                            self.state,
                            OperationScriptState::Ready(_) | OperationScriptState::Failed(_)
                        )
                    {
                        self.state = OperationScriptState::Ready(Box::new(script));
                    }
                }
                Ok(FileResult::Open(Err(error))) => {
                    notifications.push(format!("スクリプトを開けません: {error}"));
                }
                Ok(FileResult::Reset(Err(error))) => {
                    notifications.push(format!("スクリプトを既定に戻せません: {error}"));
                }
                Ok(FileResult::Open(Ok(())) | FileResult::Reset(Ok(()))) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if matches!(self.state, OperationScriptState::Loading(_)) {
                        let error = "読み込みworkerが結果を返さず終了しました".to_owned();
                        crate::logger::log(format!("operation script: {error}"));
                        self.state = OperationScriptState::Failed(error);
                    }
                    break;
                }
            }
        }
        notifications
    }

    #[cfg(test)]
    pub(crate) fn with_file_worker(
        path: PathBuf,
        editor: impl Fn(&Path) -> Result<(), String> + Send + 'static,
        before_job: impl Fn() + Send + 'static,
    ) -> Self {
        Self {
            worker_options: Some(FileWorkerOptions::for_test(path, editor, before_job)),
            ..Self::default()
        }
    }

    /// テストだけの終了待ち。先に before_job の停止を解除し、tempdir より先に呼ぶ。
    /// 受け口は残すため、join 後も poll で最後の結果を採用できる。
    #[cfg(test)]
    pub(crate) fn shutdown_file_worker_for_test(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn evaluate_event_uses_bounded_engine_and_delivers_every_event_in_order() {
        use crate::operation_script::{ScriptContext, ScriptEvent, StateSnapshot};
        let script = super::CompiledOperationScript::compile(
            r#"fn slots() { [] }
            fn on_event(s) {
                if s.events.len() == 4 && s.events[0] == "open" && s.events[1] == "view"
                    && s.events[2] == "page" && s.events[3] == "select" {
                    #{run:["FsBgCycle"],notice:s.context}
                } else { [] }
            }"#,
        )
        .unwrap();
        let mut snapshot = StateSnapshot::new(ScriptContext::FsImage);
        snapshot.events = vec![
            ScriptEvent::Open,
            ScriptEvent::View,
            ScriptEvent::Page,
            ScriptEvent::Select,
        ];
        let output = script.evaluate_event(&snapshot).unwrap();
        assert_eq!(output.actions, ["FsBgCycle"]);
        assert_eq!(output.notice.as_deref(), Some("fs_image"));
        assert!(script.last_operation_count() > 0);
        assert!(script.last_operation_count() <= crate::operation_script::MAX_OPERATIONS);

        let absent = super::CompiledOperationScript::compile("fn slots() { [] }").unwrap();
        assert!(
            absent
                .evaluate_event(&snapshot)
                .unwrap_err()
                .contains("受け口")
        );
        assert_eq!(absent.last_operation_count(), 0);
        let infinite =
            super::CompiledOperationScript::compile("fn slots() { [] } fn on_event(s) { loop {} }")
                .unwrap();
        assert!(
            infinite
                .evaluate_event(&snapshot)
                .unwrap_err()
                .contains("時間がかかりすぎた")
        );
    }

    #[test]
    fn evaluate_loaded_slot_records_operations_and_reports_budget_limit() {
        use super::*;
        let script = CompiledOperationScript::compile(
            r#"
            fn slots() { [#{slot:1,label:"test",run:"entry"}, #{slot:2,label:"limit",run:"limit"}] }
            fn entry(s) { #{run:[],notice:s.context} }
            fn limit(s) { loop {} }
        "#,
        )
        .unwrap();
        let snapshot = crate::operation_script::StateSnapshot::new(
            crate::operation_script::ScriptContext::Grid,
        );
        assert_eq!(
            script
                .evaluate_slot(1, &snapshot)
                .unwrap()
                .notice
                .as_deref(),
            Some("grid")
        );
        assert!(script.last_operation_count() > 0);
        assert!(script.last_operation_count() <= crate::operation_script::MAX_OPERATIONS);
        assert!(
            script
                .evaluate_slot(2, &snapshot)
                .unwrap_err()
                .contains("時間がかかりすぎた")
        );
        assert!(script.evaluate_slot(20, &snapshot).is_err());
        assert_eq!(script.last_operation_count(), 0);
    }

    use super::*;
    use crate::operation_script::{ScriptContext, ScriptEvent, StateSnapshot, parse_output};

    const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    /// Always release an injected block and join, including during assertion unwinding.
    /// Declare this after the tempdir so cleanup cannot race queued file operations.
    struct TestLoader {
        loader: OperationScriptLoader,
        release: Option<std::sync::mpsc::Sender<()>>,
    }

    impl TestLoader {
        fn new(data_dir: &Path) -> Self {
            Self::with_editor(data_dir, |_| Ok(()))
        }

        fn with_editor(
            data_dir: &Path,
            editor: impl Fn(&Path) -> Result<(), String> + Send + 'static,
        ) -> Self {
            Self {
                loader: OperationScriptLoader::with_file_worker(
                    script_path(data_dir),
                    editor,
                    || {},
                ),
                release: None,
            }
        }

        fn blocked(
            data_dir: &Path,
            editor: impl Fn(&Path) -> Result<(), String> + Send + 'static,
        ) -> (Self, std::sync::mpsc::Receiver<()>) {
            let (entered, entered_rx) = std::sync::mpsc::channel();
            let (release, release_rx) = std::sync::mpsc::channel();
            let first = std::cell::Cell::new(true);
            let loader =
                OperationScriptLoader::with_file_worker(script_path(data_dir), editor, move || {
                    if first.replace(false) {
                        let _ = entered.send(());
                        let _ = release_rx.recv();
                    }
                });
            (
                Self {
                    loader,
                    release: Some(release),
                },
                entered_rx,
            )
        }

        fn receive_load(&self) -> LoadResult {
            match self
                .loader
                .results
                .as_ref()
                .unwrap()
                .recv_timeout(TEST_TIMEOUT)
                .unwrap()
            {
                FileResult::Load(result) => result,
                _ => panic!("expected load result"),
            }
        }

        fn agent(
            &mut self,
            data_dir: &Path,
            source: Option<&str>,
            expected: Option<&str>,
        ) -> std::sync::mpsc::Receiver<Result<AgentScriptOutcome, String>> {
            let (reply, receiver) = std::sync::mpsc::channel();
            self.enqueue_agent_script(
                AgentScriptRequest {
                    source: source.map(str::to_owned),
                    expected_sha256: expected.map(str::to_owned),
                    reply,
                },
                data_dir,
                &egui::Context::default(),
            )
            .unwrap();
            receiver
        }

        fn finish(&mut self) {
            if let Some(release) = self.release.take() {
                let _ = release.send(());
            }
            self.loader.shutdown_file_worker_for_test();
        }
    }

    impl std::ops::Deref for TestLoader {
        type Target = OperationScriptLoader;
        fn deref(&self) -> &Self::Target {
            &self.loader
        }
    }

    impl std::ops::DerefMut for TestLoader {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.loader
        }
    }

    impl Drop for TestLoader {
        fn drop(&mut self) {
            self.finish();
        }
    }

    #[test]
    fn default_script_evaluates_slots_and_every_example() {
        let script = CompiledOperationScript::compile(DEFAULT_SCRIPT).unwrap();
        assert_eq!(script.slots.len(), 2);
        assert!(script.has_on_event);
        let mut snapshot = StateSnapshot::new(ScriptContext::FsImage);
        snapshot.page.is_last = Some(false);
        snapshot.item.pinned_tags = Some([true; 20]);
        snapshot.events = vec![
            ScriptEvent::Open,
            ScriptEvent::View,
            ScriptEvent::Page,
            ScriptEvent::Select,
        ];
        let call = |name: &str, state: &StateSnapshot| {
            let value = state.to_dynamic();
            script
                .engine
                .ensure_data_size_within_limits(&value)
                .unwrap();
            let result = script
                .engine
                .call_fn::<Dynamic>(&mut Scope::new(), &script.ast, name, (value,))
                .unwrap();
            parse_output(result).unwrap().actions
        };
        assert_eq!(call("next_or_next_book", &snapshot), ["FsPageNext"]);
        snapshot.page.is_last = Some(true);
        assert_eq!(call("next_or_next_book", &snapshot), ["FsSiblingNext"]);
        assert_eq!(call("rate3_next", &snapshot), ["RatingItem3", "FsPageNext"]);
        snapshot.book.path = Some(r"C:\manga\book".into());
        snapshot.view.direction = Some("ltr".into());
        assert_eq!(call("on_event", &snapshot), ["FsReadingDirectionToggle"]);
        let output = script.evaluate_event(&snapshot).unwrap();
        assert!(
            crate::operation_script::validate_output(
                output,
                crate::operation_script::ValidationContext {
                    context: ScriptContext::FsImage,
                    invocation: crate::operation_script::ScriptInvocation::Event(&snapshot.events),
                    navigation_already_accepted: false,
                }
            )
            .is_ok(),
            "the shipped example must pass event admission, not just evaluate"
        );
        snapshot.view.direction = Some("rtl".into());
        assert!(call("on_event", &snapshot).is_empty());
        snapshot.view.direction = Some("ltr".into());
        snapshot.events = vec![ScriptEvent::Page];
        assert!(call("on_event", &snapshot).is_empty());
        snapshot.events = vec![ScriptEvent::View];
        assert!(call("on_event", &snapshot).is_empty());
        snapshot.events = vec![ScriptEvent::Open];
        snapshot.book.path = Some(r"C:\other\book".into());
        assert!(call("on_event", &snapshot).is_empty());
        for context in [ScriptContext::Grid, ScriptContext::FsVideo] {
            snapshot.context = context;
            for slot in &script.slots {
                assert!(call(&slot.function, &snapshot).is_empty());
            }
            assert!(call("on_event", &snapshot).is_empty());
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
        std::fs::write(&path, "fn slots() { bad_slots_evaluation(); }").unwrap();
        let reason = load_script(&path).err().unwrap();
        assert!(reason.contains("slots() の評価に失敗"), "{reason}");
        assert!(reason.contains("bad_slots_evaluation"), "{reason}");
        std::fs::write(&path, DEFAULT_SCRIPT).unwrap();
        let mut loader = TestLoader::new(temp.path());
        loader.reload(temp.path(), &egui::Context::default());
        let result = loader.receive_load();
        assert_eq!(result.generation, loader.generation);
        assert_eq!(result.script.unwrap().slots.len(), 2);
    }

    #[test]
    fn script_file_size_limit_is_256_kib_and_worker_reports_oversize() {
        assert_eq!(MAX_SOURCE_BYTES, 256 * 1024);
        let temp = tempfile::tempdir().unwrap();
        let path = script_path(temp.path());
        let mut source = b"fn slots() { [] }\n".to_vec();
        source.resize(MAX_SOURCE_BYTES as usize, b' ');
        std::fs::write(&path, &source).unwrap();
        assert!(load_script(&path).is_ok());

        source.push(b' ');
        std::fs::write(&path, &source).unwrap();
        assert!(load_script(&path).err().unwrap().contains("256 KiB"));

        // The bounded read cuts this final character in the middle. The
        // failure must still be the file-size limit rather than invalid UTF-8.
        source.truncate(MAX_SOURCE_BYTES as usize - 1);
        source.extend_from_slice("界".as_bytes());
        std::fs::write(&path, &source).unwrap();
        let mut loader = TestLoader::new(temp.path());
        loader.reload(temp.path(), &egui::Context::default());
        let result = loader.receive_load();
        assert!(result.script.err().unwrap().contains("256 KiB"));
    }

    #[test]
    fn worker_returns_assignment_compile_failure_with_line_and_column() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            script_path(temp.path()),
            "fn slots() { slots_should_not_execute(); }\nfn unused(s) {\n    s[0] += 1;\n}",
        )
        .unwrap();
        let mut loader = TestLoader::new(temp.path());
        loader.reload(temp.path(), &egui::Context::default());
        let result = loader.receive_load();
        let reason = result.script.err().unwrap();
        assert!(reason.contains("3行5桁"), "{reason}");
        assert!(
            reason.contains("添字・プロパティへの代入は使えません"),
            "{reason}"
        );
        assert!(!reason.contains("slots_should_not_execute"), "{reason}");
    }

    #[test]
    fn worker_rejects_exception_statements_while_loading() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        for (source, keyword) in [
            (
                "try { missing_function(); } catch(e) {} fn slots() { [] }",
                "try",
            ),
            ("fn slots() { try { [] } catch(e) { [] } }", "try"),
            (
                "fn slots() { [] } fn unused() { try { missing_function(); } catch(e) {} }",
                "try",
            ),
            ("fn slots() { throw [0]; }", "throw"),
            ("fn slots() { [] } fn unused(s) { throw s; }", "throw"),
        ] {
            std::fs::write(script_path(temp.path()), source).unwrap();
            let mut loader = TestLoader::new(temp.path());
            loader.reload(temp.path(), &ctx);
            let result = loader.receive_load();
            let reason = result.script.err().unwrap();
            assert!(reason.contains("コンパイルに失敗"), "{reason}");
            assert!(reason.contains(keyword), "{reason}");
            assert!(!reason.contains("slots() の評価に失敗"), "{reason}");
        }
    }

    #[test]
    fn loader_discards_stale_generation_and_accepts_latest() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut loader = OperationScriptLoader {
            generation: 2,
            state: OperationScriptState::Loading(OperationScriptLoad),
            results: Some(rx),
            ..OperationScriptLoader::default()
        };
        tx.send(FileResult::Load(LoadResult {
            generation: 1,
            script: Err("old".into()),
        }))
        .unwrap_or_else(|_| panic!("result channel closed"));
        tx.send(FileResult::Load(LoadResult {
            generation: 2,
            script: Err("latest reason".into()),
        }))
        .unwrap_or_else(|_| panic!("result channel closed"));
        assert!(
            tx.send(FileResult::Open(Err("after latest load".into())))
                .is_ok()
        );
        let notifications = loader.poll();
        assert_eq!(notifications.len(), 2);
        assert_eq!(
            notifications[0],
            "スクリプトを読み込めません: latest reason"
        );
        assert!(notifications[1].contains("after latest load"));
        assert!(
            matches!(&loader.state, OperationScriptState::Failed(reason) if reason == "latest reason")
        );
        assert!(loader.poll().is_empty());
    }

    #[test]
    fn loader_publishes_compiled_ast_and_slots_as_one_ready_value() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut loader = OperationScriptLoader {
            generation: 3,
            state: OperationScriptState::Loading(OperationScriptLoad),
            results: Some(rx),
            ..OperationScriptLoader::default()
        };
        assert!(
            tx.send(FileResult::Load(LoadResult {
                generation: 3,
                script: CompiledOperationScript::compile(DEFAULT_SCRIPT),
            }))
            .is_ok()
        );
        assert!(loader.poll().is_empty());
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
        assert!(loader.poll().is_empty());
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
        let temp = tempfile::tempdir().unwrap();
        let (mut loader, entered) = TestLoader::blocked(temp.path(), |_| Ok(()));
        let ctx = egui::Context::default();
        loader.sync_enabled(true, temp.path(), &ctx);
        entered.recv_timeout(TEST_TIMEOUT).unwrap();
        let first_generation = loader.generation;
        loader.sync_enabled(true, temp.path(), &ctx);
        assert_eq!(loader.generation, first_generation);
        loader.sync_enabled(false, temp.path(), &ctx);
        loader.finish();
        assert!(loader.poll().is_empty());
        assert!(matches!(loader.state, OperationScriptState::Disabled));
    }

    #[test]
    fn create_default_worker_preserves_existing_user_script() {
        for content in [None, Some("user content"), Some("")] {
            let temp = tempfile::tempdir().unwrap();
            let path = script_path(temp.path());
            if let Some(content) = content {
                std::fs::write(&path, content).unwrap();
            }
            let (opened, received) = std::sync::mpsc::channel();
            let mut loader = TestLoader::with_editor(temp.path(), move |path| {
                opened.send(std::fs::read_to_string(path).unwrap()).unwrap();
                Ok(())
            });
            loader.enqueue_open_user_script(temp.path(), &egui::Context::default());
            loader.finish();
            let expected = content.unwrap_or(DEFAULT_SCRIPT);
            assert_eq!(received.recv_timeout(TEST_TIMEOUT).unwrap(), expected);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
            assert!(loader.poll().is_empty());
            assert!(matches!(loader.state, OperationScriptState::Disabled));
        }
    }

    #[test]
    fn file_worker_starts_lazily_and_missing_enable_creates_ready_default() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("nested-data");
        let mut loader = TestLoader::new(&data_dir);
        let ctx = egui::Context::default();
        assert!(loader.worker.is_none());
        assert!(loader.results.is_none());
        loader.sync_enabled(false, &data_dir, &ctx);
        assert!(loader.worker.is_none());
        loader.sync_enabled(true, &data_dir, &ctx);
        assert!(loader.worker.is_some());
        loader.finish();
        assert!(loader.poll().is_empty());
        assert!(
            matches!(&loader.state, OperationScriptState::Ready(script) if script.slots.len() == 2 && script.has_on_event)
        );
        assert_eq!(
            std::fs::read_to_string(script_path(&data_dir)).unwrap(),
            DEFAULT_SCRIPT
        );
    }

    #[test]
    fn file_worker_reset_then_reload_reads_replacement_in_fifo_order() {
        let temp = tempfile::tempdir().unwrap();
        let path = script_path(temp.path());
        let old = "fn slots() { [] }";
        std::fs::write(&path, old).unwrap();
        let (mut loader, entered) = TestLoader::blocked(temp.path(), |_| Ok(()));
        let ctx = egui::Context::default();
        loader.enqueue_reset_user_script(temp.path(), &ctx);
        entered.recv_timeout(TEST_TIMEOUT).unwrap();
        loader.reload(temp.path(), &ctx);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), old);
        assert!(matches!(loader.state, OperationScriptState::Loading(_)));
        loader.finish();
        assert!(loader.poll().is_empty());
        assert!(
            matches!(&loader.state, OperationScriptState::Ready(script) if script.slots.len() == 2)
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), DEFAULT_SCRIPT);
    }

    #[test]
    fn file_worker_open_then_enable_loads_created_default_in_fifo_order() {
        let temp = tempfile::tempdir().unwrap();
        let path = script_path(temp.path());
        let (opened, received) = std::sync::mpsc::channel();
        let (mut loader, entered) = TestLoader::blocked(temp.path(), move |path| {
            opened.send(std::fs::read_to_string(path).unwrap()).unwrap();
            Ok(())
        });
        let ctx = egui::Context::default();
        loader.enqueue_open_user_script(temp.path(), &ctx);
        entered.recv_timeout(TEST_TIMEOUT).unwrap();
        loader.sync_enabled(true, temp.path(), &ctx);
        assert!(!path.exists());
        assert!(matches!(loader.state, OperationScriptState::Loading(_)));
        loader.finish();
        assert!(loader.poll().is_empty());
        assert!(
            matches!(&loader.state, OperationScriptState::Ready(script) if script.slots.len() == 2)
        );
        assert_eq!(received.recv_timeout(TEST_TIMEOUT).unwrap(), DEFAULT_SCRIPT);
    }

    #[test]
    fn file_worker_reset_does_not_reload_ready_script_automatically() {
        let temp = tempfile::tempdir().unwrap();
        let old = "fn slots() { [] }";
        let mut loader = TestLoader::new(temp.path());
        loader.state =
            OperationScriptState::Ready(Box::new(CompiledOperationScript::compile(old).unwrap()));
        loader.enqueue_reset_user_script(temp.path(), &egui::Context::default());
        loader.finish();
        assert!(loader.poll().is_empty());
        assert!(
            matches!(&loader.state, OperationScriptState::Ready(script) if script.slots.is_empty())
        );
        assert_eq!(
            std::fs::read_to_string(script_path(temp.path())).unwrap(),
            DEFAULT_SCRIPT
        );
    }

    #[test]
    fn file_worker_reload_recreates_deleted_file_and_preserves_empty_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = script_path(temp.path());
        std::fs::write(&path, "fn slots() { [] }").unwrap();
        let mut loader = TestLoader::new(temp.path());
        let ctx = egui::Context::default();
        loader.reload(temp.path(), &ctx);
        assert!(loader.receive_load().script.unwrap().slots.is_empty());
        let previous_generation = loader.generation;
        std::fs::remove_file(&path).unwrap();
        loader.reload(temp.path(), &ctx);
        assert_ne!(loader.generation, previous_generation);
        loader.finish();
        assert!(loader.poll().is_empty());
        assert!(
            matches!(&loader.state, OperationScriptState::Ready(script) if script.slots.len() == 2)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_SCRIPT);

        // An empty file is user content too: fail compilation rather than replace it.
        std::fs::write(&path, "").unwrap();
        let mut empty_loader = TestLoader::new(temp.path());
        empty_loader.sync_enabled(true, temp.path(), &ctx);
        empty_loader.finish();
        let notifications = empty_loader.poll();
        assert_eq!(notifications.len(), 1);
        assert!(notifications[0].starts_with("スクリプトを読み込めません:"));
        assert!(matches!(
            empty_loader.state,
            OperationScriptState::Failed(_)
        ));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "");
        assert!(empty_loader.poll().is_empty());
    }

    #[test]
    fn loader_drains_file_errors_in_every_state_without_adopting_stale_load() {
        let ready = || {
            OperationScriptState::Ready(Box::new(
                CompiledOperationScript::compile(DEFAULT_SCRIPT).unwrap(),
            ))
        };
        for state in [
            OperationScriptState::Disabled,
            OperationScriptState::Loading(OperationScriptLoad),
            ready(),
            OperationScriptState::Failed("existing failure".into()),
        ] {
            let expected_state = std::mem::discriminant(&state);
            let (tx, rx) = std::sync::mpsc::channel();
            let mut loader = OperationScriptLoader {
                generation: 5,
                state,
                results: Some(rx),
                ..OperationScriptLoader::default()
            };
            assert!(
                tx.send(FileResult::Load(LoadResult {
                    generation: 4,
                    script: Err("stale failure".into()),
                }))
                .is_ok()
            );
            assert!(tx.send(FileResult::Open(Err("open error".into()))).is_ok());
            assert!(
                tx.send(FileResult::Reset(Err("reset error".into())))
                    .is_ok()
            );
            let notifications = loader.poll();
            assert_eq!(notifications.len(), 2);
            assert!(notifications[0].contains("open error"));
            assert!(notifications[1].contains("reset error"));
            assert_eq!(std::mem::discriminant(&loader.state), expected_state);
            assert!(loader.poll().is_empty());
        }
    }

    #[test]
    fn loader_disabled_rejects_even_matching_load_and_keeps_file_notifications() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut loader = OperationScriptLoader {
            generation: 3,
            results: Some(rx),
            ..OperationScriptLoader::default()
        };
        assert!(
            tx.send(FileResult::Load(LoadResult {
                generation: 3,
                script: CompiledOperationScript::compile(DEFAULT_SCRIPT),
            }))
            .is_ok()
        );
        assert!(
            tx.send(FileResult::Open(Err("still notify".into())))
                .is_ok()
        );
        assert_eq!(loader.poll().len(), 1);
        assert!(matches!(loader.state, OperationScriptState::Disabled));
    }

    #[test]
    fn file_worker_off_open_and_reset_errors_are_reported_and_destination_survives() {
        let temp = tempfile::tempdir().unwrap();
        let path = script_path(temp.path());
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("user-content"), "keep me").unwrap();
        let mut loader =
            TestLoader::with_editor(temp.path(), |_| Err("fake editor failure".into()));
        let ctx = egui::Context::default();
        loader.enqueue_open_user_script(temp.path(), &ctx);
        loader.enqueue_reset_user_script(temp.path(), &ctx);
        loader.finish();
        let notifications = loader.poll();
        assert_eq!(notifications.len(), 2);
        // Windows may reject CREATE_NEW on a directory before calling the editor.
        assert!(notifications[0].starts_with("スクリプトを開けません:"));
        assert!(notifications[1].contains("置き換えに失敗"));
        assert!(matches!(loader.state, OperationScriptState::Disabled));
        assert_eq!(
            std::fs::read_to_string(path.join("user-content")).unwrap(),
            "keep me"
        );
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
        assert!(loader.poll().is_empty());
    }

    #[test]
    fn file_worker_reset_atomically_replaces_file_with_sharing_reader_and_cleans_temp() {
        let temp = tempfile::tempdir().unwrap();
        let path = script_path(temp.path());
        std::fs::write(&path, "old user script").unwrap();
        let mut old = File::open(&path).unwrap();
        let mut loader = TestLoader::new(temp.path());
        loader.enqueue_reset_user_script(temp.path(), &egui::Context::default());
        loader.finish();
        assert!(loader.poll().is_empty());
        let mut old_content = String::new();
        old.read_to_string(&mut old_content).unwrap();
        assert_eq!(old_content, "old user script");
        assert_eq!(std::fs::read_to_string(path).unwrap(), DEFAULT_SCRIPT);
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    fn agent_hash(source: &str) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(source.as_bytes()))
    }

    const AI_SOURCE: &str = r#"fn slots() { [#{slot:1,label:"AI",run:"entry"}] }
fn entry(s) { #{run:[],notice:"AI"} }"#;
    const OLD_SOURCE: &str = "fn slots() { [] }";

    fn assert_ready_slots(loader: &OperationScriptLoader, count: usize) {
        assert!(
            matches!(&loader.state, OperationScriptState::Ready(script) if script.slots.len() == count)
        );
    }

    fn agent_write_result(
        receiver: std::sync::mpsc::Receiver<Result<AgentScriptOutcome, String>>,
    ) -> ScriptWriteOutcome {
        match receiver.recv_timeout(TEST_TIMEOUT).unwrap().unwrap() {
            AgentScriptOutcome::Write(outcome) => outcome,
            AgentScriptOutcome::Read(_) => panic!("expected write outcome"),
        }
    }

    #[test]
    fn script_363_agent_read_returns_missing_default_without_creating_and_preserves_empty() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("not-created");
        let mut loader = TestLoader::new(&data_dir);
        let read = loader.agent(&data_dir, None, None);
        loader.finish();
        let AgentScriptOutcome::Read(document) = read.recv_timeout(TEST_TIMEOUT).unwrap().unwrap()
        else {
            panic!("expected read document");
        };
        assert_eq!(document.source, DEFAULT_SCRIPT);
        assert_eq!(document.sha256, agent_hash(DEFAULT_SCRIPT));
        assert!(document.builtin);
        assert!(!data_dir.exists());
        assert!(loader.poll().is_empty());
        assert!(matches!(loader.state, OperationScriptState::Disabled));

        std::fs::write(script_path(temp.path()), "").unwrap();
        let mut loader = TestLoader::new(temp.path());
        let read = loader.agent(temp.path(), None, None);
        loader.finish();
        let AgentScriptOutcome::Read(document) = read.recv_timeout(TEST_TIMEOUT).unwrap().unwrap()
        else {
            panic!("expected read document");
        };
        assert_eq!(document.source, "");
        assert_eq!(document.sha256, agent_hash(""));
        assert!(!document.builtin);
    }

    #[test]
    fn script_363_agent_read_reports_io_utf8_and_bounded_size_errors_without_default() {
        for case in 0..3 {
            let temp = tempfile::tempdir().unwrap();
            let path = script_path(temp.path());
            match case {
                0 => std::fs::create_dir(&path).unwrap(),
                1 => std::fs::write(&path, [0xff]).unwrap(),
                _ => std::fs::write(&path, vec![b'x'; MAX_SOURCE_BYTES as usize + 1]).unwrap(),
            }
            let mut loader = TestLoader::new(temp.path());
            let read = loader.agent(temp.path(), None, None);
            loader.finish();
            let reason = read.recv_timeout(TEST_TIMEOUT).unwrap().unwrap_err();
            assert!(
                reason.contains(if case == 2 {
                    "256 KiB"
                } else {
                    "読み込みに失敗"
                }),
                "{reason}"
            );
            assert!(loader.poll().is_empty());
        }
    }

    #[test]
    fn script_363_agent_acceptance_keeps_ready_generation_and_slots_then_adopts_written_ast() {
        let temp = tempfile::tempdir().unwrap();
        let path = script_path(temp.path());
        std::fs::write(&path, OLD_SOURCE).unwrap();
        let (mut loader, entered) = TestLoader::blocked(temp.path(), |_| Ok(()));
        loader.generation = 7;
        loader.state = OperationScriptState::Ready(Box::new(
            CompiledOperationScript::compile(DEFAULT_SCRIPT).unwrap(),
        ));
        let reply = loader.agent(
            temp.path(),
            Some(AI_SOURCE),
            Some(&agent_hash(OLD_SOURCE).to_uppercase()),
        );
        entered.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_eq!(loader.generation, 7);
        assert_ready_slots(&loader, 2);
        let OperationScriptState::Ready(script) = &loader.state else {
            unreachable!()
        };
        assert!(
            script
                .evaluate_slot(1, &StateSnapshot::new(ScriptContext::Grid))
                .is_ok()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), OLD_SOURCE);
        loader.finish();
        let outcome = agent_write_result(reply);
        assert_eq!(outcome.sha256, agent_hash(AI_SOURCE));
        assert_eq!(outcome.before_lines, OLD_SOURCE.lines().count());
        assert_eq!(outcome.after_lines, AI_SOURCE.lines().count());
        assert!(loader.poll().is_empty());
        assert_eq!(loader.generation, 7);
        assert_ready_slots(&loader, 1);
        assert_eq!(std::fs::read_to_string(path).unwrap(), AI_SOURCE);
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn script_363_agent_rejections_keep_file_ready_script_and_generation_unchanged() {
        let oversized = "x".repeat(MAX_SOURCE_BYTES as usize + 1);
        for (source, expected, reason) in [
            (
                AI_SOURCE,
                Some(agent_hash("different")),
                "読んだあとに変わっています",
            ),
            (
                "fn slots( {",
                Some(agent_hash("editor content")),
                "コンパイルに失敗",
            ),
            (
                "fn slots() { missing(); }",
                Some(agent_hash("editor content")),
                "slots() の評価に失敗",
            ),
            (
                "fn slots() { [42] }",
                Some(agent_hash("editor content")),
                "slots() の指定に失敗",
            ),
            (AI_SOURCE, None, "expected_sha256"),
            (
                oversized.as_str(),
                Some(agent_hash("editor content")),
                "256 KiB",
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = script_path(temp.path());
            std::fs::write(&path, "editor content").unwrap();
            let (mut loader, entered) = TestLoader::blocked(temp.path(), |_| Ok(()));
            loader.generation = 9;
            loader.state = OperationScriptState::Ready(Box::new(
                CompiledOperationScript::compile(DEFAULT_SCRIPT).unwrap(),
            ));
            let reply = loader.agent(temp.path(), Some(source), expected.as_deref());
            entered.recv_timeout(TEST_TIMEOUT).unwrap();
            assert_eq!(loader.generation, 9);
            assert_ready_slots(&loader, 2);
            loader.finish();
            let error = reply.recv_timeout(TEST_TIMEOUT).unwrap().unwrap_err();
            assert!(error.contains(reason), "expected {reason}, got {error}");
            assert!(loader.poll().is_empty());
            assert_eq!(loader.generation, 9);
            assert_ready_slots(&loader, 2);
            assert_eq!(std::fs::read_to_string(path).unwrap(), "editor content");
            assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn script_363_agent_fifo_reset_then_write_compares_replaced_default() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(script_path(temp.path()), OLD_SOURCE).unwrap();
        let (mut loader, entered) = TestLoader::blocked(temp.path(), |_| Ok(()));
        loader.state = OperationScriptState::Ready(Box::new(
            CompiledOperationScript::compile(OLD_SOURCE).unwrap(),
        ));
        loader.enqueue_reset_user_script(temp.path(), &egui::Context::default());
        entered.recv_timeout(TEST_TIMEOUT).unwrap();
        let reply = loader.agent(
            temp.path(),
            Some(AI_SOURCE),
            Some(&agent_hash(DEFAULT_SCRIPT)),
        );
        loader.finish();
        let outcome = agent_write_result(reply);
        assert_eq!(outcome.before_lines, DEFAULT_SCRIPT.lines().count());
        assert!(loader.poll().is_empty());
        assert_ready_slots(&loader, 1);
        assert_eq!(
            std::fs::read_to_string(script_path(temp.path())).unwrap(),
            AI_SOURCE
        );
    }

    #[test]
    fn script_363_agent_fifo_write_then_reset_adopts_ai_and_leaves_default_file() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(script_path(temp.path()), OLD_SOURCE).unwrap();
        let (mut loader, entered) = TestLoader::blocked(temp.path(), |_| Ok(()));
        loader.state = OperationScriptState::Ready(Box::new(
            CompiledOperationScript::compile(OLD_SOURCE).unwrap(),
        ));
        let reply = loader.agent(temp.path(), Some(AI_SOURCE), Some(&agent_hash(OLD_SOURCE)));
        entered.recv_timeout(TEST_TIMEOUT).unwrap();
        loader.enqueue_reset_user_script(temp.path(), &egui::Context::default());
        loader.finish();
        agent_write_result(reply);
        assert!(loader.poll().is_empty());
        assert_ready_slots(&loader, 1);
        assert_eq!(
            std::fs::read_to_string(script_path(temp.path())).unwrap(),
            DEFAULT_SCRIPT
        );
        assert!(loader.poll().is_empty());
        assert_ready_slots(&loader, 1);
    }

    #[test]
    fn script_363_agent_fifo_write_then_reload_or_reenable_retires_ai_result() {
        for reenable in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            std::fs::write(script_path(temp.path()), OLD_SOURCE).unwrap();
            let ctx = egui::Context::default();
            let (mut loader, entered) = TestLoader::blocked(temp.path(), |_| Ok(()));
            loader.generation = 5;
            loader.state = OperationScriptState::Ready(Box::new(
                CompiledOperationScript::compile(OLD_SOURCE).unwrap(),
            ));
            let reply = loader.agent(temp.path(), Some(AI_SOURCE), Some(&agent_hash(OLD_SOURCE)));
            entered.recv_timeout(TEST_TIMEOUT).unwrap();
            if reenable {
                loader.sync_enabled(false, temp.path(), &ctx);
                loader.sync_enabled(true, temp.path(), &ctx);
            } else {
                loader.reload(temp.path(), &ctx);
            }
            let generation = loader.generation;
            assert_ne!(generation, 5);
            loader.finish();
            agent_write_result(reply);
            assert!(loader.poll().is_empty());
            assert_eq!(loader.generation, generation);
            assert_ready_slots(&loader, 1);
            assert_eq!(
                std::fs::read_to_string(script_path(temp.path())).unwrap(),
                AI_SOURCE
            );
        }
    }

    #[test]
    fn script_363_agent_write_while_off_is_not_adopted_and_later_enable_reads_it() {
        for enable_after_enqueue in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let (mut loader, entered) = TestLoader::blocked(temp.path(), |_| Ok(()));
            let reply = loader.agent(
                temp.path(),
                Some(AI_SOURCE),
                Some(&agent_hash(DEFAULT_SCRIPT)),
            );
            entered.recv_timeout(TEST_TIMEOUT).unwrap();
            if enable_after_enqueue {
                loader.sync_enabled(true, temp.path(), &egui::Context::default());
            }
            loader.finish();
            agent_write_result(reply);
            assert!(loader.poll().is_empty());
            if enable_after_enqueue {
                assert_ready_slots(&loader, 1);
            } else {
                assert!(matches!(loader.state, OperationScriptState::Disabled));
            }
            assert_eq!(
                std::fs::read_to_string(script_path(temp.path())).unwrap(),
                AI_SOURCE
            );
        }
    }

    #[test]
    fn script_363_agent_write_in_flight_then_off_does_not_adopt() {
        let temp = tempfile::tempdir().unwrap();
        let (mut loader, entered) = TestLoader::blocked(temp.path(), |_| Ok(()));
        loader.state = OperationScriptState::Ready(Box::new(
            CompiledOperationScript::compile(DEFAULT_SCRIPT).unwrap(),
        ));
        let reply = loader.agent(
            temp.path(),
            Some(AI_SOURCE),
            Some(&agent_hash(DEFAULT_SCRIPT)),
        );
        entered.recv_timeout(TEST_TIMEOUT).unwrap();
        loader.sync_enabled(false, temp.path(), &egui::Context::default());
        loader.finish();
        agent_write_result(reply);
        assert!(loader.poll().is_empty());
        assert!(matches!(loader.state, OperationScriptState::Disabled));
        assert_eq!(
            std::fs::read_to_string(script_path(temp.path())).unwrap(),
            AI_SOURCE
        );
    }

    #[test]
    fn script_363_agent_success_recovers_failed_and_loading_states_after_prior_result() {
        for case in 0..3 {
            let loading = case != 0;
            let temp = tempfile::tempdir().unwrap();
            let path = script_path(temp.path());
            let before = if case == 2 {
                OLD_SOURCE
            } else {
                "invalid source"
            };
            std::fs::write(&path, before).unwrap();
            let (mut loader, entered) = TestLoader::blocked(temp.path(), |_| Ok(()));
            if loading {
                loader.reload(temp.path(), &egui::Context::default());
                entered.recv_timeout(TEST_TIMEOUT).unwrap();
            } else {
                loader.state = OperationScriptState::Failed("previous error".into());
            }
            let reply = loader.agent(temp.path(), Some(AI_SOURCE), Some(&agent_hash(before)));
            if !loading {
                entered.recv_timeout(TEST_TIMEOUT).unwrap();
                assert!(
                    matches!(&loader.state, OperationScriptState::Failed(reason) if reason == "previous error")
                );
            }
            loader.finish();
            agent_write_result(reply);
            let notifications = loader.poll();
            assert_eq!(notifications.len(), usize::from(case == 1));
            assert_ready_slots(&loader, 1);
            assert_eq!(std::fs::read_to_string(path).unwrap(), AI_SOURCE);
        }
    }

    #[test]
    fn script_363_agent_poll_adopts_only_matching_generation_ready_or_failed() {
        for generation in [None, Some(3), Some(4)] {
            for state in [
                OperationScriptState::Disabled,
                OperationScriptState::Loading(OperationScriptLoad),
                OperationScriptState::Ready(Box::new(
                    CompiledOperationScript::compile(DEFAULT_SCRIPT).unwrap(),
                )),
                OperationScriptState::Failed("keep failure".into()),
            ] {
                let adopts = generation == Some(4)
                    && matches!(
                        state,
                        OperationScriptState::Ready(_) | OperationScriptState::Failed(_)
                    );
                let original = std::mem::discriminant(&state);
                let (sender, receiver) = std::sync::mpsc::channel();
                let mut loader = OperationScriptLoader {
                    generation: 4,
                    state,
                    results: Some(receiver),
                    ..OperationScriptLoader::default()
                };
                assert!(
                    sender
                        .send(FileResult::AiWrite {
                            generation,
                            script: CompiledOperationScript::compile(AI_SOURCE).unwrap()
                        })
                        .is_ok()
                );
                assert!(loader.poll().is_empty());
                if adopts {
                    assert_ready_slots(&loader, 1);
                } else {
                    assert_eq!(std::mem::discriminant(&loader.state), original);
                }
            }
        }
    }

    #[test]
    fn script_363_agent_fifo_read_between_writes_sees_preceding_write() {
        let temp = tempfile::tempdir().unwrap();
        let (mut loader, entered) = TestLoader::blocked(temp.path(), |_| Ok(()));
        loader.state = OperationScriptState::Ready(Box::new(
            CompiledOperationScript::compile(DEFAULT_SCRIPT).unwrap(),
        ));
        let first = loader.agent(
            temp.path(),
            Some(AI_SOURCE),
            Some(&agent_hash(DEFAULT_SCRIPT)),
        );
        entered.recv_timeout(TEST_TIMEOUT).unwrap();
        let read = loader.agent(temp.path(), None, None);
        let last = loader.agent(temp.path(), Some(OLD_SOURCE), Some(&agent_hash(AI_SOURCE)));
        loader.finish();
        agent_write_result(first);
        let AgentScriptOutcome::Read(document) = read.recv_timeout(TEST_TIMEOUT).unwrap().unwrap()
        else {
            panic!("expected read document");
        };
        assert_eq!(document.source, AI_SOURCE);
        assert_eq!(document.sha256, agent_hash(AI_SOURCE));
        assert!(!document.builtin);
        let final_write = agent_write_result(last);
        assert_eq!(final_write.before_lines, AI_SOURCE.lines().count());
        assert_eq!(final_write.sha256, agent_hash(OLD_SOURCE));
        assert!(loader.poll().is_empty());
        assert_ready_slots(&loader, 0);
        assert_eq!(
            std::fs::read_to_string(script_path(temp.path())).unwrap(),
            OLD_SOURCE
        );
    }

    #[test]
    fn script_363_agent_write_accepts_256_kib_and_creates_missing_data_directory() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("new-data");
        let mut source = OLD_SOURCE.to_owned();
        source.extend(std::iter::repeat_n(
            ' ',
            MAX_SOURCE_BYTES as usize - source.len(),
        ));
        let mut loader = TestLoader::new(&data_dir);
        let reply = loader.agent(&data_dir, Some(&source), Some(&agent_hash(DEFAULT_SCRIPT)));
        loader.finish();
        let outcome = agent_write_result(reply);
        assert_eq!(outcome.sha256, agent_hash(&source));
        assert_eq!(outcome.before_lines, DEFAULT_SCRIPT.lines().count());
        assert_eq!(outcome.after_lines, 1);
        assert_eq!(
            std::fs::read_to_string(script_path(&data_dir)).unwrap(),
            source
        );
        assert_eq!(std::fs::read_dir(&data_dir).unwrap().count(), 1);
        assert!(loader.poll().is_empty());
        assert!(matches!(loader.state, OperationScriptState::Disabled));
    }

    #[cfg(windows)]
    #[test]
    fn script_363_agent_replace_failure_preserves_file_ready_state_and_cleans_temporary() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let path = script_path(temp.path());
        std::fs::write(&path, OLD_SOURCE).unwrap();
        // Permit the worker's read, but deny the rename's delete sharing.
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        let mut loader = TestLoader::new(temp.path());
        loader.generation = 8;
        loader.state = OperationScriptState::Ready(Box::new(
            CompiledOperationScript::compile(DEFAULT_SCRIPT).unwrap(),
        ));
        let reply = loader.agent(temp.path(), Some(AI_SOURCE), Some(&agent_hash(OLD_SOURCE)));
        loader.finish();
        let error = reply.recv_timeout(TEST_TIMEOUT).unwrap().unwrap_err();
        assert!(error.contains("置き換えに失敗"), "{error}");
        assert!(loader.poll().is_empty());
        assert_ready_slots(&loader, 2);
        assert_eq!(loader.generation, 8);
        drop(lock);
        assert_eq!(std::fs::read_to_string(path).unwrap(), OLD_SOURCE);
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn file_worker_failed_atomic_reset_preserves_locked_user_file_and_cleans_temp() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let path = script_path(temp.path());
        std::fs::write(&path, "old user script").unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let mut loader = TestLoader::new(temp.path());
        loader.enqueue_reset_user_script(temp.path(), &egui::Context::default());
        loader.finish();
        let notifications = loader.poll();
        assert_eq!(notifications.len(), 1);
        assert!(notifications[0].contains("置き換えに失敗"));
        drop(lock);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "old user script");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
