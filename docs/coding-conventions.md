# コーディング規約 (UI・IME・並行処理・UI 応答性・グリフ・エンコーディング)

> この文書は 2026-10-15 に CLAUDE.md から移した節をそのまま収めたもの。本文中に残る「本書」は移動前の CLAUDE.md を指す。移した節の行き先一覧は CLAUDE.md の「移した節の行き先」表を参照。

## Key Design Decisions

### UI / スクロール
- **Virtual scrolling**: `show_viewport` で全体高さだけ確保し、可視行のみ描画。
  スクロールオフセットは App が自前管理（egui の自動スクロールは使わない）。
- **Row snapping**: オフセットは常に `cell_size` の整数倍。最大オフセットも
  `ceil((total_h - viewport_h) / cell_size) * cell_size` で行境界に揃える。
  一覧の touch drag も `GridScrollState` のcanonical offset自体はanchorとして行境界に保ち、1 行未満の
  描画端数だけを viewport の一時データに分離する。端数を canonical offset へ混ぜない。
- **一覧の位置とgeometryの契約（5ch >>661）**: offset・一回要求・未消費RowAnchor・描画geometryは
  context所有の `GridScrollState` に集約する。詳細は実列数1／行高28、サムネイルは実列数／cell_hを記録する。
  現在body高と全content高から一度だけextentを解決し、確保高とegui opt-in APIへ同じ結果を渡す。
  viewport最小64 pt・margin 0を両者で揃える。復元／Edge／実移動／先行clamp・snapは `Set`、
  通常同期は `Sync`、App所有touchの描画位置は `AppGesture`。native readbackは候補値を行丸め・clampして
  採用し、明示要求を再発行しない。比率変更も描画proofに基づきgeometryとcanonicalを同時変換する。
  実装段と検証状況は [調査書](scroll-661-investigation.md#修正の設計) を正本とする。
- **一覧入力の寿命と再pass**: geometry／native慣性のStateはviewer context＋rendererで分け、
  pointer transport IDは実viewportの一覧に安定させる。pressの所属と処理済frameの所属は同じcapture ownerで
  区別し、順序付きイベントとeguiの実保持先で新pressを受理する。Responseの集約flagだけに依存しない。
  同ownerのSync再passは、Appが半行未満のreadbackを採らなくても最初のnative移動・rubber-bandを保持する。
  Set／AppGesture／session交代は明示位置を優先する。pointer・touchとも旧入力を宛先へ再所属させない。
  旧touch由来Moveと新mouse captureを混在させる既存経路は、基点比較後の設計担当判断で
  §1.487へ分離した。新契約では同frame描画にもその誤採用が出る差がある。範囲と完了状況は調査書を参照する。
- **Mouse wheel**: `ctx.input_mut` で MouseWheel イベントを消費し、1行分に変換。
- **Popup / menu wheel passthrough**: `menu_button` / `ComboBox` / popup 内に
  `ScrollArea` を置く場合、popup が開いている frame では `raw_scroll_delta` /
  `smooth_scroll_delta` / `Event::MouseWheel` を明示的に消費すること。これを忘れると
  popup 内をスクロールしたつもりの wheel が背面のサムネイル一覧にも通り、一覧が動く。
  さらに、グローバルな一覧ホイール処理 (`App::process_scroll`) はメニュー/ツールバー/
  アドレスバー/ファセットバーを描いた後、グリッド描画の直前で呼ぶこと。早く呼ぶと
  popup が wheel を処理する前に背面グリッドが先に動く。既存例: `src/ui_main.rs` の
  `suppress_menu_button_wheel_passthrough` とツールバー ComboBox の open guard。

### ダイアログ (egui::Window)
- **テーマと文字色の所有境界**: 通常 UI の配色は `os_theme::apply_resolved_with_contrast`
  が Light / Dark 両 Style をまとめて所有する。通常文字・補足文字・警告・エラーは
  `ui.visuals().text_color()` / `weak_text_color()` / `warn_fg_color` / `error_fg_color` を使い、
  画面ごとの固定 gray を追加しない。フルスクリーン等の暗色固定子 UI は
  `os_theme::apply_dark_ui`、ComboBox / popup は `dark_popup_style` / `dark_menu_popup`、
  タイトルバーを含む暗色 `egui::Window` は `with_dark_context_style` を使う。
  popup / Window のために `ctx.set_theme` / `ctx.set_visuals` を直接呼ばない。限定スコープが
  Light / Dark 両 Style と theme preference を復元することが、メインテーマへ漏らさない不変条件。
- **ドラッグ移動**: `anchor()` を使うとウィンドウが固定されドラッグできなくなる。
  必ず `default_pos()` を使う。定番の初期位置は `ctx.content_rect().min + egui::vec2(60.0, 40.0)`。
- **閉じるボタン**: `.open(&mut open)` でタイトルバーに × ボタンが付く。
  `open` が `false` になったら `show_*` フラグを落とす。
- **背面への wheel / key 伝播防止**: モーダル相当の表示状態は `App::common_modal_dialog_open`
  へ追加し、main / fullscreen で別々の一覧を持たない。`App::process_scroll` は描画済みの
  `Order::Middle` / `Foreground` layer がポインタ直下にある場合も背面グリッドの wheel を
  止めるため、モデルレス Window と将来の登録漏れにも安全側で動く。state が存在しても
  Window を描かない phase がある処理は、state の有無ではなく「現在表示されるか」を返す
  helper を登録すること。
- **メイン枠のポインター操作**: 各バーとフォルダーツリーは `gate_main_chrome` で停止する。
  グリッドの右クリックメニューだけは除外し、modal への遷移時に既存 popup と submenu 状態を閉じる。
  遷移判定は root の worker 採用・入力処理後、ウィンドウ描画前に1回だけ行い、
  その後 dialog 自身が開いた popup は閉じない。無効化時の枠全体の配色は維持する。
- 数十分以上続く取得・管理 UI (TensorRT パック等) はモデルレス tool window とし、
  `common_modal_dialog_open` へ含めて閲覧全体を停止しない。Window 上の入力だけは floating-layer
  guard で背面へ漏らさない。
- **ScrollArea の横幅**: ダイアログ右端に縦スクロールバーを置く一覧は
  `.auto_shrink([false, ...])` を指定し、利用可能幅を使い切る。既定の横 shrink のままだと
  内容幅まで縮み、スクロールバーがダイアログの途中に出る。
- **起動時ダイアログの小画面要件**: 初回設定・更新後の告知・設定の保護起動/復元案内など、
  起動時または初回設定直後に出る画面は **1093×614 logical points** でも確定/閉じる操作を
  完全に表示すること。`ctx.content_rect()` から枠・初期位置・見出し・操作行・画面余白を引いて
  本文の高さを制限し、見出しと操作ボタンは縦 `ScrollArea` の外へ置く。
  自動サイズの `Modal` ではスクロールの親領域も明示確保する。リサイズ可能な `Window` は
  `available_height()` の制約も尊重し、初期本文が不必要に狭くならない `default_height()` を指定する。
  本文の上限は実際の枠/見出し/折り返し操作行から計算し、余った高さを使う。一律の大きな控除や
  固定本文高は使わない。縦auto_shrinkで短文は自然高に縮め、長文の表示面積も回帰検査する。
  長いパス・エラー文・版またぎ告知もテストし、1093×614 / 1366×728 の snapshot と
  UI倍率100%/200%で、固定操作行の全ボタン（再接続等の別状態も含む）とタイトル×の
  矩形/クリック到達性の回帰検査を残す。起動時書庫変換の列挙/確認/変換/エラーも対象。
  初回設定の Enter 確定は IME helper を使い、
  選択肢/リンクにフォーカスがある場合はその widget 操作を優先する。Esc で初回設定を省略しない。
- **折り返し本文と floating scrollbar**: 長文を折り返すダイアログでは、floating scrollbar の
  `floating_allocated_width` を少なくとも `bar_inner_margin + bar_width` 確保する。共通 style は
  一覧の表示面積を優先して予約幅 0 のため、そのまま使うと右端の文字へバーが重なる。
- **パターン**: `ui_dialogs/` に 1 ファイル 1 メソッドで追加。
  `mod.rs` に `mod xxx;` を追加し、`app.rs` の `update()` 内で `self.show_xxx(ctx)` を呼ぶ。
  `App` 構造体に `show_xxx: bool` フィールドを追加し、`Default` impl で `false` 初期化。

### パネル (Area + Frame::popup)
- フルスクリーン上の固定パネルで `egui::Area::fixed_pos` + `Frame::popup` +
  `ScrollArea` を組み合わせる場合、`ScrollArea::max_height` だけに頼らない。
  egui は親 `Ui` の `available_rect_before_wrap()` を上限にするため (0.33 で確認)、自動サイズの
  `Area` / `Frame::popup` 内ではコンテンツ高に縮むことがある。
- 下端近くまで伸ばしたいパネルは、ScrollArea の前に
  `ui.allocate_ui_with_layout(egui::vec2(width, body_height), ...)` で親領域を明示確保し、
  その内側で `ScrollArea::vertical().max_height(body_height).auto_shrink([false, false])`
  を使う。
- タイトル / プレビュー / 閉じるボタンのヘッダは ScrollArea の外に固定する。
  ScrollArea 内へ戻すと、縦スクロールバーが × ボタンに重なる退行を起こす。
- パネルが下端近くまで伸びる場合、クリック吸収 sink とキャンバス入力抑制 rect も
  同じ高さから動的に作る。固定 1000px は 1440p / 4K / 縦長ウィンドウで不足する。
- 高さを行数などの計算で決めるなら、item_spacing・separator・padding・Frame の margin を
  計算に含めるか、該当する間隔を 0 にする。`show_rows` は行間を内部で足すので、行高に
  含めて二重に数えない。行内を `scope_builder` で描くと親カーソルが子の自然高へ戻るので、
  仮想行の高さと実際の行の位置がずれないか確かめる。
- 収まる画面ではスクロールバーが出ないこと、高さ不足では末尾まで届くことを
  `tests/support/scroll_layout.rs` の共通 helper で検査する。snapshot は見た目の記録であり、
  スクロールの要否の検査にはならない。監査と残件は
  [docs/calculated-panel-height-audit.md](calculated-panel-height-audit.md)。

### IME 対応 (日本語入力) ⚠️ 重要
設計・先行実機記録・移植検証は [IME 処理済みキーの計画](ime-process-keys-plan.md) を参照。
TextEdit を含むダイアログで Enter / Escape を拾うときは **必ず専用ヘルパーを使う**こと。
直接 `ctx.input(|i| i.key_pressed(Key::Enter/Escape))` を呼ぶと、**日本語 IME 変換中の Enter
(変換確定) や Escape (変換キャンセル) をダイアログが奪ってしまい、変換が破壊される**。

- **確定用**: `self.dialog_enter_pressed(ctx)` — IME 変換中は常に false
- **キャンセル用**: `self.dialog_escape_pressed(ctx)` — IME 変換中は常に false
- **判定ロジック**: 各 egui Context に登録した `ime_focus` の input plugin が
  `ViewportId` ごとの composition 状態を単一 ownership で保持する。
  `App::ime_input_active(ctx)` は今 pass の Ime 列を反映した後も composing のときだけ true。
  Commit / Disabled 後の通常 Enter / Escape / shortcut は同じ pass から直ちに通す。
  native presenter の pass 前判定は `ime_composing_with_pending_events` で最終状態を射影する。
  viewport 消滅・focus 喪失では、その viewport の composing を落とす。
  focus 喪失後も IME の終端待ち状態は保持し、後続の Disabled / 空 Commit による取消の空 preedit 補完を保つ。

上記はアプリ操作を抑止する責務で、TextEdit の focus 復帰とは別。日本語を入力できる新しい
single-line TextEdit は `crate::ime_focus` の helper 経由で描画すること。同 helper が IME の
キー処理由来の一時的な focus loss の復帰と矢印の focus lock を所有する。
Tab traversal はアプリ全体の Tab 方針が所有し、helper へ IME 専用 Tab lock は重ねない。
helper-managed field が直前の pass で持っていた focus は 1 pass の typed recovery claim として
keyboard owner にも渡し、TextEdit の復帰前に同じキーがアプリ shortcut へ漏れるのを防ぐ。
focus 復帰は `lost_focus()`、pointer 無しの key press、他 widget の focus 無し、かつ
「今 pass 反映後も composing、または Enter / Escape press 無し」のときだけ行う。
helper の retry は viewport / widget ごとに次 pass 限りで、`lost_focus()` 以外の同条件を満たし
欄が focus を持たない場合だけ使う。初回要求用の呼び出し側 latch には書き込まない。
pointer 入力や他 widget への正当な focus 移動を優先する。
raw TextEdit は理由付き allowlist を検査する unit test で禁止している。TSF を採用しない理由、
IMM32 の 2 経路と未対応範囲の正本は [src/ime_focus.rs](../src/ime_focus.rs) の helper doc comment を参照。

**ビューポート別のイベントキュー**:
egui の `show_viewport_immediate` は独立したイベントキューを持つ。メインビューポートと
フルスクリーンビューポートは別キュー。plugin は `memory.begin_pass` より前に各 RawInput を
viewport ごとに追跡する。`App::update_ime_state(ctx)` は helper-field 診断を処理前に始めるため
**各ビューポートの入り口**で呼ばれている:
- メイン: [src/app.rs](../src/app.rs) の `App::update` 先頭
- フルスクリーン: [src/ui_fullscreen.rs](../src/ui_fullscreen.rs) の `show_viewport_immediate` closure 先頭

新しいビューポートを追加した場合、closure 先頭で `self.update_ime_state(ctx)` を必ず呼ぶこと。
別 viewport を対象にする consumer は、その対象の既存 `ViewportId` を選んで
`ime_focus::ime_input_active_in_viewport` から共有 snapshot を読む。これは read-only の入口であり、
対象 viewport の event queue や composition state を更新しない。更新は必ず所有 viewport の pass に残す。

Windows の `allow_ime` が true の場合、vendored egui-winit は IME が処理した
`NamedKey::Process` の press（repeat を含む）を Key / Text / Cut / Copy / Paste にしない。
本家 0.34.0 #7967 の HashSet を、物理キーごとの Hidden（press 未配送）/ Forwarded（配送済み）の型付き状態へ置換する。
後続の Process repeat でも Forwarded を保持する。release は allow_ime にかかわらず状態を除去し Hidden のみ遮断する。
focus 喪失時は全状態を消す（egui も keys_down を消去する）。
それ以外の release と Ime event は残す。native presenter も
`VK_PROCESSKEY` に egui key を割り当てない。本体 Win32 queue と native 生キー公開も
同じく 0xE5 の press / repeat を捨て、release は残す。通常のキーを composition だけで
取り除かない。TextEdit の未確定文字選択を保ったまま、Windows が
空の `Preedit("")` を送らず `Disabled` だけを送る経路では、非空 Commit の無い composition の
終端直前に空 preedit を補う（focus 喪失後の終端待ちも含む）。composition が無い `Disabled` へは挿入せず、
非 composing Esc は通常入力として残す。

**借用の注意**:
`egui::Window::show(ctx, |ui| {...})` の closure 内で `self` 経由のメソッド呼び出しは
借用衝突になりやすい。`dialog_enter_pressed` / `dialog_escape_pressed` は closure の**前**で
ローカル変数にキャプチャしてから closure 内で参照する:
```rust
let enter_pressed = self.dialog_enter_pressed(ctx);
let escape_pressed = self.dialog_escape_pressed(ctx);
egui::Window::new("...").show(ctx, |ui| {
    if response.lost_focus() && enter_pressed { ... }
    if escape_pressed { cancel = true; }
});
```
深いネスト (例: `preferences.rs` の `draw_page` → `page_exif_display`) では一時構造体
(`PreferencesState::enter_pressed` 等) のフィールドに載せて伝搬する。

### サムネイルロード
- **Grid contents**: 実フォルダ / アーカイブ類 / 画像 / 動画・音声を、設定された 4 行へ
  カテゴリごとに配置する。同じ行は共通の `sort_order` で混在ソートし、空行は読み飛ばす。
  既定は「実フォルダ + アーカイブ類」先頭、「画像 + 動画・音声」後続の従来互換。
  ZIP/PDF ファイルは 1 枚目/1 ページ目のサムネイル＋種別バッジで表示。非対応ファイルは無視。
- **Cancellation**: `Arc<AtomicBool>` キャンセルトークン。`load_folder` 呼び出し時に
  旧トークンを `true` にして旧タスクを中断。
- **Per-load thread pool**: フォルダごとに新規 `rayon::ThreadPool` を作成。
  旧フォルダのプールと競合せず新タスクが即座に開始できる。
- **Priority loading**: Phase1（可視範囲）→ Phase2（残り）の2フェーズ並列処理。
- **Repaint loop**: `Pending` なサムネイルがある間は毎フレーム `ctx.request_repaint()`。
- **Page-based eviction**: 前後数ページ分のみ GPU メモリに保持、範囲外は Evicted。
- **Cache**: SQLite に WebP (q=75) で保存。Off / Auto / Always の 3 モード。

### フォルダ走査
- **Folder tree navigation (Ctrl+↑↓)**: 深さ優先前順トラバーサル。
  次 = 最初の子 → 次の兄弟 → 祖先の次の兄弟（再帰）。
  前 = 前の兄弟の最後の子孫 → 親。
- **BS key**: 親フォルダへ。
- **Path comparison**: `folder_tree::path_eq` は `path_key::eq_keep_drive` の再公開。ドライブを保持し、大文字小文字と `/`・`\` の表記差を無視する。受理する実パスは I/O なしの `path_key::normalize_input_path` で Windows 標準表記へ揃える。保存キーの規則は変えない。入口と例外は [docs/virtual-folders.md](virtual-folders.md) の「実パスの比較と入力表記」を参照。
- **AppleDouble 除外**: macOS/iPhone 由来の `._*` ファイルを自動除外。

### セキュリティ
- `image` クレート（純粋Rust、メモリ安全）で画像デコード。
- HEIC/AVIF/JXL/TIFF は WIC 経由（`unsafe` ブロックに局所化）。RAW は LibRaw へ先に振り分け、FFI は `crates/libraw-sys` に局所化。
- ONNX Runtime (ort crate) 経由の AI 推論は safe Rust API。DirectML EP で GPU アクセラレーション。

### 並行処理: try_lock + sleep は使わない ⚠️

「`Mutex::try_lock` に失敗したら sleep して再試行」というループは **飢餓 (starvation) を
起こす既知のアンチパターン**。2026-04 に PDF ワーカープールで Critical 要求が 10 秒
ブロックされる実害が発生した (詳細は [docs/async-architecture.md §5.5](async-architecture.md))。

- 複数スレッドが同じリソースを取り合う場合は **`Mutex + Condvar` で保護した優先度キュー +
  専用ディスパッチャースレッド** の構造にする
- リソース利用者は Job を enqueue して `mpsc::Receiver` で応答待ち、ディスパッチャーは
  `Condvar::wait` で起床してキューから pop する
- 実例: `src/pdf_loader.rs` の `PdfWorkerPool` / `JobQueue` / `run_dispatcher`
- `try_lock` 自体は「取れなければ今回は諦める」best-effort 用途のみ OK

### PDF レンダ pool の viewer-context epoch (2026-10-12、未リリース)

`PdfWorkerPool` は **3 段階優先度** (`Critical / HighNormal / Normal`) を維持し、
PDF render失効はviewerごとの非Clone `PdfRenderEpochOwner` が所有する。
App投影/bundleとwriterを移譲し、一覧採用で自身だけadvance、retire/dropで自身だけ終端する。
UIはatomic publish＋容量1の非blocking wakeだけ。pool内の一つのmaintenance workerが
HighNormal/Normalをpruneし、dispatcher popも各job ticketのstaleを検査する。
同じPDF/pageを読む兄弟contextのjobsを失効させない。

- `LoadRequest.pdf_render_validity` のViewer ticketを **UI enqueue時点で** 捕捉し、派生要求は
  immutable ticketをcloneする。workerはfresh化しない。
- fullscreen本体/background/Remote/Similar/enumerateは明示 `Unscoped`（旧epoch=0）。
- background 経路 (CatchupQueue / NeighborPrefetch / build_and_save_one_pdf / cache creator /
  `get_document_info` / `enumerate_pages_with_cancel`) は全てUnscoped。
- 詳細: [docs/pdf-render-epoch-ownership-plan.md](pdf-render-epoch-ownership-plan.md)。
  旧global整数epoch導入計画は履歴として残す。

### PDF レンダ pool の HarvestOnCancel (2026-05)

`CancelWaitPolicy` enum で `pool.execute` の cancel 時挙動を選択可能:

- `AbortOnCancel` (既定): cancel 検出と同時に `Err(Interrupted)` で抜ける。in-flight IPC
  結果は dispatcher が `reply.send` で silently 捨てる。
- `HarvestOnCancel` (thumbnail PDF render の cache-savable 経路のみ): cancel が立っても
  reply を待ち続け、in-flight IPC 結果を harvest。caller (= `load_one_cached`) が cache
  保存に進めて、PDFium が既に処理した結果を投資回収する。再エントリ時の再 render を防ぐ。

ポリシー選択は `process_load_request` で行う:
- `req.pdf_page.is_some() && !req.skip_cache && catalog.is_some()` → `HarvestOnCancel`
- それ以外 (enumerate / Critical / fullscreen / bulk / neighbor / background) → `AbortOnCancel`

perf イベント: `pool_cancel_harvest_wait` (待ち開始時) / `pdf_thumb_cache_saved_after_cancel`
(cache 保存成功時)。詳細: [docs/pdf-pool-harvest-on-cancel-plan.md](pdf-pool-harvest-on-cancel-plan.md)

### スクロール後の visible 昇格 + 計装 (2026-05)

スクロール中に prefetch (priority=False) で enqueue された PDF render ジョブは、
`reload_queue` は毎フレーム re-tag されるが、**既に pdf_pool.normal に積まれたものは
priority=False のまま居座る**。可視範囲到達後も Normal lane で処理されて 3 秒以上待たされる。

対策: `pdf_loader::promote_to_high_normal(visible_keys, viewer_target)` がowner/要求世代/perf_keyを
照合してNormal→HighNormalへ移す。dedup/retryはbundle-owned `PdfVisiblePromotionState` に集約する。
兄弟の同keyをfoundと数えず、遅着した対象要求は既存frame再評価で昇格する。
fullscreenの昇格targetはUnscoped固定で、Viewer seek thumbnailはその対象に含めない。

新 perf イベント:
- `ui/scroll_settle`: スクロール停止 (300ms) 検出。pool snapshot + target_count + already_loaded
- `ui/visible_thumb_first_ready` / `visible_thumb_all_ready`: settle 後の可視サムネ Loaded latency
- `pdf/pool_queue_snapshot`: 1 秒 tick で queue 状態 + in_flight age (max/p95/p50)
- `pdf/pool_promote_visible`: promote 件数の stats (promoted / already_high / not_found)

`scripts/analyze_perf.py scroll` で settle ごとの first/all_ready latency を集計可能。
詳細: [docs/scroll-visibility-priority-plan.md](scroll-visibility-priority-plan.md)

### スクロール中の prefetch 抑制 (2026-05)

`promote_to_high_normal` で promote しても、in-flight に既に居る prefetch render は
PDFium cancel 不可なので最低 1.5 秒待ち。これが settle 後の visible_thumb_first_ready
を最悪 9 秒以上ブロックする (実測)。

対策: **`reload_queue` / `heavy_io_queue` の prefetch (`req.priority=false`) を、スクロール中 /
visible 待ち中は enqueue しないし、queue に既に居る prefetch も毎フレ prune する**。
pool に流れる前に止めるので cancel 不可問題に影響されない。

判定は `decide_prefetch_allowed(now, last_prefetch_scroll_at, visible_pending)` 純関数。
- 最後の scroll 入力から **100ms 未満** → Block (`ScrollNotIdle`)
- visible 範囲に Pending/Evicted/Requested が **1 つでもあれば** → Block (`VisibleStillLoading`)
- 最後の scroll から **3 秒経過** → 無条件 Allow (`Backstop3s`、永久 stall 防止)

`last_prefetch_scroll_at` は **`emit_scroll_settle_event` で clear されない** 専用 timestamp
(`last_scroll_event_at` とは別)。`App::update` 冒頭の `detect_scroll_input_intent`
(ctx.input wheel + arrow keys + Page/Home/End + thumbnail grid の Touch Move / End) で即時更新する。
これで `update_keep_range_and_requests` の gate 判定時に同フレーム入力が反映済み。
scrollbar drag は `update_scroll_settle_state` の offset 変化 fallback で 1 フレ遅れて拾う。
一覧の touch drag は early Touch Move intent で同フレームの prefetch を止め、端数だけ動く
フレームでも settle / idle-upgrade を止めるため、`ui_main.rs` の touch command 適用境界と
release 後の snap 補間中から各 timestamp を明示更新する。snap 補間は行高の 20% 以上を
130ms ease-out で端数だけ動かし、anchor は完了時に 1 回だけ更新する。

新 perf イベント:
- `ui/prefetch_suppressed`: `transition`(start/continue/end) + `allow_reason`(unblock 時) +
  `backstop_hit` + 抑制件数 (suppressed_regular/heavy + pruned_regular/heavy)

`scripts/analyze_perf.py scroll` が settle 直前の suppression end を併記する。
詳細: [docs/prefetch-suppression-during-scroll-plan.md](prefetch-suppression-during-scroll-plan.md)

### UI スレッドでの同期 I/O は即 worker 化する ⚠️

`App::update` から (呼び出し先を含めて) 同期実行される処理で以下を行うと、
Ctrl+↑↓ 連打や Ctrl+F 検索で UI が引っかかる。**新機能追加時は必ず
[docs/ui-responsiveness.md](ui-responsiveness.md) §4 のチェックリストを通す**。

禁止 / worker 化必須な処理:

- **ファイル全体を読む** (`std::fs::read`, `File::open` + `read_to_end`): 画像デコード・
  XMP/EXIF/PNG メタ抽出など。worker thread + mpsc + cancel トークンに移す。
  実例: `start_fs_load`, `start_metadata_load`, `execute_search`。
- **`ctx.load_texture` を 1 フレームに複数回**: 20MP RGBA で 26-58ms/枚。`fs_upload_backlog`
  パターンで 1 フレーム 1 枚、現在ページのみ即時にする。
- **`std::fs::read_dir` ループで `Path::is_dir()` / `is_file()`**: Windows で per-entry
  `GetFileAttributes` syscall が走り、数百ファイルで 500-1000ms ブロック。
  **必ず `entry.file_type()` を使う** (`FindFirstFile` のキャッシュ再利用)。
- **SQLite の `CatalogDb::open` 相当の cold open**: warm で 5ms 以下だが cold で
  150ms 超のことあり。毎ステップ走る箇所は避ける。
- **DFS フォルダ走査**: `navigate_folder_with_skip` は HDD で 1 秒超の事例あり。
  `spawn_folder_nav` のように別スレッドで走らせる。

worker 化の定型パターン (`XxxPending { cancel, rx }` + `start_xxx` / `poll_xxx` + 3 箇所
cancel) は [docs/ui-responsiveness.md §2](ui-responsiveness.md) に実装テンプレを
載せている。

### レビュー時: UI 応答性の性能観点

レビューでは、差分に次の呼び出しが出たら **UI スレッドから同期到達しないか** を必ず確認する。
`App::update`、描画関数、入力ハンドラ、`open_fullscreen`、`load_folder`、`execute_*`、
`ensure_*` / `start_*` の呼び出し元まで追うこと。

- `std::fs::read`, `File::open`, `read_to_end`, `std::fs::read_dir`, `metadata`,
  `Path::is_dir` / `Path::is_file`
- `xmp_reader::read_tweet_info`, `png_metadata::extract_*`, `exif_reader::read_*`,
  `zip_loader::read_entry_bytes`
- `ctx.load_texture`
- `rusqlite` / `search_index_db` / catalog DB の open・全件 load・search

UI 同期なら、worker 化・キャンセル・結果適用時の世代/idx 整合・1 フレーム予算・perf 計装が
揃っていることを確認する。特に `decide_partial` のような lazy 化でも、cheap miss や
除外トークンだけの検索など「追加情報が必要になる最悪ケース」で UI を止めないかを見る。

測定は `--perf-log` + `python scripts/analyze_perf.py <path> {startup,nav,hitches}` で。
追加した同期処理の区間には perf::event を必ず差し込む (悪化を検知できるように)。

**ユーザーが perf-log を取って解析を依頼してきた場合**は、まず以下の場所を確認する:

```
%APPDATA%\mimageviewer\logs\
  ├ perf_events.jsonl       # 直近の perf-log (--perf-log の既定出力先)
  ├ perf_*.jsonl            # 任意名で取った過去ログ
  ├ mimageviewer.log        # 通常の logger 出力
  ├ panic.log               # Rust panic・UI hang
  └ native-exception.log    # Windows first-chance 例外 (記録は best-effort)
```

(Windows の絶対パス: `C:\Users\<user>\AppData\Roaming\mimageviewer\logs\`)。
ユーザーが「perf-log 取りました」とだけ伝えてきたらこのディレクトリの最新
`perf_*.jsonl` をそのまま `analyze_perf.py` に食わせる。パスを毎回聞かない。

## Markdown / テキストファイルのエンコーディング (BOM 必須ケース)

Claude Code の `Write` tool は **UTF-8 (BOM なし)** で書き出す。Linux / macOS や
モダンエディタはこれで問題ないが、以下のツール群は BOM 無し UTF-8 を
**Windows ANSI (= CP932 in JP)** として誤読し、日本語が mojibake になる:

- **Codex GUI** (= ローカル Codex デスクトップツール) — ユーザー報告 2026-04
  「ブリーフが文字化けしている」
- **Windows メモ帳 (旧バージョン)** — Win11 22H2 以降は UTF-8 既定だが、社用 PC で
  古い設定が残っているケースあり
- **PowerShell 5.1** の `Get-Content` (デフォルト) — `-Encoding utf8` 明示しない限り
  CP932 で読む

このため、外部ツールに渡す **Markdown / テキストブリーフ** には UTF-8 BOM
(`EF BB BF`) を **明示的に付与**する。

### 付与方法

Claude Code 経由で書き出した直後に、以下のヘルパースクリプトで BOM を付ける:

```bash
python scripts/write_utf8_bom.py docs/foo-brief.md
```

冪等 (= 既に BOM があれば no-op)。複数ファイル指定可。

### 検証

```bash
file docs/foo-brief.md
# Expected: "Unicode text, UTF-8 (with BOM) text"
head -c 3 docs/foo-brief.md | xxd
# Expected: "00000000: efbb bf"
```

### 過去の経緯

- 2026-04: PowerShell スクリプト `upload-trt-pack.ps1` の日本語文字列が CP932
  読みで mojibake → 該当スクリプトは ASCII オンリーに置換 (BOM の代わり)
  (commit f81fec7)。ASCII で書ける用途は ASCII 化が単純で堅い。
- 2026-04: Codex GUI に渡す Markdown ブリーフが mojibake → BOM 付与で回避
  (本ポリシー策定)。Markdown / 日本語文章は ASCII 化できないので BOM を使う。

### 使い分け

| ファイル種別                              | 推奨方式            |
|-------------------------------------------|---------------------|
| `*.rs` / `*.cpp` / `*.h`                  | UTF-8 BOM **無し**  |
| `*.md` / `*.txt` (リポジトリ内、開発用)   | UTF-8 BOM **無し**  |
| `*.md` / `*.txt` (**外部ツールに渡す用**) | UTF-8 BOM **あり**  |
| `*.ps1` (PowerShell スクリプト)           | ASCII オンリー推奨  |

リポジトリ内の通常 Markdown ドキュメント (`docs/*.md`、`README.md` 等) は
BOM 不要 (= Linux / macOS / git diff の互換性のため)。

## UI 文字列の Unicode グリフ選定ルール

mIV は既定のプライマリ proportional フォントに **Yu Gothic Medium** (Windows 標準) を
使い、v2.7.0 から `Settings.ui_font` で Windows のシステムフォントまたはユーザー追加
TTF/OTF/TTC/OTC の日本語 upright face を選べる。日本語なし / Italic / Oblique は候補外
([src/ui_fonts.rs](../src/ui_fonts.rs),
[src/ui_font_catalog.rs](../src/ui_font_catalog.rs))。通常 UI と native overlay は同じ
`ui_fonts::configure_fonts_with_settings()` を通り、動画メタデータなどユーザー由来テキスト向けに
`miv-user-text` family を別途登録する。この family は text-presentation 記号、
数学英字、絵文字の fallback を明示し、ttf-parser で選択 face / Yu Gothic の代表 glyph と
Segoe UI Emoji の代表 glyph の中心を読んで
baseline 補正を計算する。計測は egui 0.33 の `ab_glyph` outline 描画に合わせて
outline bbox を優先し、raster image bounds は bbox が取れない場合の fallback とする。
egui 0.37 (Skrifa + vello_cpu) への更新後もこの計測を使い、文字の見え方は 2026-10-17 に
利用者が旧版と比較して採択した ([egui 0.37 更新計画 §5](egui-0.37-update-plan.md))。動画・音声 HUD の固定サイズ control label は選択 face ではなく `miv-hud-text`
(無補正の既定日本語フォント) を使い、VST 等の固定 glyph はベクター描画する。
複数サンプルは中央値で扱う。ツールバーは既定 Yu Gothic の実測補正値に
選択 face との差分を加え、環境設定の ±4pt 微調整を最後に加える。`✉` / `⋈` のような text-presentation 記号は
Cambria Math や Segoe UI Emoji より前の Meiryo fallback で拾わせ、ブラウザに近い
文字表示へ寄せる。数学英字は Cambria Math、色付き絵文字は Segoe UI Emoji へ回す。
Cambria Math の数学英字も代表 glyph の中心から `FontTweak` 補正を導出し、`…` など
主フォント側の句読点と極端に上下ずれしないようにする。
Segoe UI Historic / Segoe UI Symbol も
縦位置補正付きで使う。通常 UI の proportional family では選択 face を先頭、既定日本語
font を次順位に置き、各 fallback は egui 既定 font の後ろに置く。選択 face が持たない
日本語 glyph は既定日本語 font へ回る。Yu Gothic は日本語 + 基本 Latin
+ Latin-1 Supplement までは網羅するが、**Misc Symbols** や **絵文字**は欠落
していることが多い。fallback はユーザー由来テキストの文字化けを軽減するためのもので、
UI の固定文言・ボタン・状態表示へ絵文字や環境依存記号を新規採用する理由にはしない。
過去に複数の文字化けバグが繰り返されている (2026-04 までに 🎚 / ✕ で発生)。

### 安全な代替表

| 危険 (フォント依存) | 安全 (Latin-1 / ASCII)         |
|---------------------|---------------------------------|
| ✕ (U+2715)          | × (U+00D7 multiplication sign)  |
| ✖ (U+2716)          | × (U+00D7)                      |
| 🎚 (U+1F39A)         | "VST" 等のテキスト              |
| 🟢⚫🔴 (status emoji)  | "[ON]" "[OFF]" 等              |

**矢印** ↑ U+2191 / ↓ U+2193 は Yu Gothic に含まれており OK。
**チェック** ✓ U+2713 / ✗ U+2717 は環境依存 (= 一部 Yu Gothic バリアントには
ある)。新しい場所で使う前に lint と実機で確認すること。

### コミット前 lint

```bash
python scripts/check_ui_glyphs.py
```

`src/` 配下の .rs ファイルから「実際に tofu 化したと確認された Unicode 文字」
を含む行を列挙する。0 件で exit 0、見つかれば exit 1。
新たに tofu 報告があった文字は同スクリプトの `DANGEROUS` dict に追加する。
誤検出の場合は該当行に `// glyph-lint:skip` を付ける。

CI には現状未統合だが、UI 文字列を新規追加・変更したコミット前に手動で
1 度走らせる慣行にする。
