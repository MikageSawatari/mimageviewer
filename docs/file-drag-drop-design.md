# ファイル D&D (ドラッグでコピー送出) 実装設計

グリッドのサムネイルを掴んで、エクスプローラや他アプリへ **ドラッグ＆ドロップでファイルを
コピー** できるようにする機能の設計ドキュメント。

**状態**: 実装済み (2026-05-22)。`src/file_drag.rs` 新規 + `grid_item.rs` /
`app.rs` / `ui_main.rs` 改修 + ユニットテスト 8 件。`cargo check` / `cargo test`
通過。残るのは §8.2 の実機検証 (Q3 / Q5〜Q8)。

**改訂履歴**:

- 2026-07-24: 一覧のクリック選択方式 (チェック方式 / エクスプローラー方式) を追加。
  `handle_cell_interaction` の選択状態遷移だけを共通 helper 化し、
  `drag_started_by(Primary)` と `decide_drag_payload()` の D&D 経路は変更しない。
- 2026-05-23: ultrareview + Codex re-review (P2/P3) を反映して受け取り側 (§11) を
  再設計。`handle_external_file_drop` の検証ループを `file-drop-validate` worker
  thread へ移し、UI スレッドから `fs::canonicalize` × N (SMB で 10s+) を撤去
  (review #7)。コピー完了通知を `Vec<Receiver<()>>` から
  `drop_copy_pending: Vec<Receiver<CopyOutcome>>` に格上げし、PowerShell 側を
  `try/catch` + `::FAILED::N` / `::ERR::msg` マーカー出力に変更して失敗件数と
  エラー文を UI トーストへ伝えるようにした (review #15 / Codex P2-1)。
  spawn 失敗 / tmp 書き込み失敗 / `powershell` 実行失敗 / 非ゼロ終了 / マーカー欠落 /
  `recv()` Disconnected はすべて `CopyOutcome::all_failed(attempted, reason)` で
  全件失敗扱いに統一 (Codex P2-1: 旧実装は黙って `failed=0` の成功扱いに潰していた)。
  全件除外で実コピーに到達しなかったケースは `CopyOutcome::notice` フィールドで
  「ドロップ先と重なる N 件を除外した結果、コピー対象が 0 件」をトースト表示
  (Codex P2-2: 旧実装は無音だった)。
- 2026-05-22: Codex レビュー第 1 回を反映。主な修正点 — COM 初期化の寿命管理
  (§5.1 / §6.3)、混在選択 (実パス + 仮想アイテム) の仕様化 (§5.4)、`SearchContainer`
  のスコープ明記 (§2 / §5.2)、pointer reset を「要実機検証」に格下げ (§6.1)、工数見積もりの
  引き上げ (§9)。Q1 / Q2 は解決済 (§8)。
- 2026-05-22: Codex レビュー第 2 回を反映。主な修正点 — ポインタ固着対策に
  `ctx.stop_dragging()` を必須手順として追加 (§5.5 / §6.1。egui の interaction 層は
  `PointerState` と別管理のため、raw リセットだけでは `dragged_id` が残る)、
  `SHParseDisplayName` 失敗を黙ってスキップせず件数をトーストで明示する仕様に変更
  (§5.1 / §5.5b。戻り値を `DragOutcome` 構造体化)、`native_drag_just_finished` を
  `SHDoDragDrop` 到達時のみ立てるよう修正 (§5.5b)。
- 2026-05-22: Codex レビュー第 3 回を反映。主な修正点 — 混在選択トーストを
  ドラッグ完了後に表示する設計へ変更 (`pending_native_drag` を `PendingNativeDrag`
  構造体化。§5.3 / §5.4 / §5.5b。トースト文言も「コピー」→「ドラッグ対象に
  しました」へ)、ドラッグ開始を primary button 限定に
  (`drag_started_by(PointerButton::Primary)`。§5.4)、`SHParseDisplayName` /
  `BindToHandler` の `None` を `Option::<IBindCtx>::None` と型注釈明示 (§4.2 / §5.1)、
  `DragOutcome` に `effect` / `error` を追加し COM ステップ別失敗を区別 (§5.1)、
  `decide_drag_payload` の payload 順序を index 昇順に確定 (§5.4.2)。
- 2026-05-22: 実装完了。設計どおり `src/file_drag.rs` 新規 + `grid_item.rs`
  (`drag_source_path`) / `app.rs` (`PendingNativeDrag` / フレーム冒頭リセット /
  末尾実行 / `emit_drag_result_toasts`) / `ui_main.rs` (`DragDecision` /
  `decide_drag_payload` / `handle_cell_interaction` の D&D 検出) を実装。
  実装中の判断で `SHParseDisplayName` / `BindToHandler` の null `IBindCtx` は
  `None::<&IBindCtx>` (= `Option<&IBindCtx>`)、`SHDoDragDrop` の null `IDropSource`
  は `None::<&IDropSource>` で渡す形に確定 (windows-core 0.61 の `Param` は
  `Option<&T>` に実装されているため。本文 §4.2 / §5.1 の `Option::<IBindCtx>::None`
  表記は厳密には `&` 付きが正)。ユニットテスト 8 件追加、`cargo check` / `cargo
  test` / `cargo fmt` 通過。
- 2026-05-22: Codex レビュー第 4 回 (実装レビュー) を反映。P2 — COM 失敗 / 未開始時に
  混在選択の `post_drag_toast` が出て「成功したかのように」誤解させる問題を修正。
  `emit_drag_result_toasts` を `(outcome: Option<&DragOutcome>, post_drag_toast)`
  シグネチャに変更し、失敗・未開始を最優先で通知して `post_drag_toast` を抑止する
  (§5.5b)。P3 — `docs/README.md` 索引の「実装前のレビュー用」表記を実装済みに更新。
- 2026-05-22: 受け取り方向 (エクスプローラ → mIV へのドロップ → 現在フォルダへ
  コピー) を追加実装。当初 §2 で「対象外」としていたが、送出と対称の操作として
  ユーザー要望で対応。詳細は §11。`app.rs` (`handle_external_file_drop` + `update`
  での `dropped_files` 取り込み) と `context_menu.rs` (`copy_paths_into_folder`)。
- 2026-05-22: Codex レビュー第 5 回 (受け取り実装レビュー) を反映。P1 — 表示中
  フォルダがドロップ元の子孫だと `Copy-Item -Recurse` が自己再帰で無限増殖する
  バグを修正 (`file_drag::dir_copy_would_recurse` ガード + ユニットテスト 7 件)。
  P2 — Ctrl+G / Ctrl+S 検索結果ビュー表示中は `current_folder` が直前の実フォルダの
  ままで誤コピーするため、`items_are_global_search_view` / `favsearch.on_results_grid()`
  を明示チェックして拒否。P3 — `handle_external_file_drop` に紛れ込んでいた
  drop 完了 poll の doc comment を元に戻した。§11 を更新。
- 2026-05-22: Codex レビュー第 6 回 P1 を反映。再帰ガード `copy_target_inside_src` が
  ドライブルート / UNC 共有ルート (`Path::file_name()` が `None`) を素通りさせていた
  バグを修正 (`C:\A` 表示中の `C:\` ドロップ等)。ルートのときは `dest` 自体がルート
  配下かで判定。ユニットテスト 3 件追加 (合計 10 件)。
- 2026-05-22: 3 エージェント並列コードレビューの指摘を反映。`DragOutcome` の
  `effect` フィールドはどの呼び出し側も読まない死に状態だったため削除 (内部ログには
  残る)。`DragOutcome` の 6 箇所のリテラル構築を `not_started` / `failed_before_start`
  / `after_modal` コンストラクタに集約。`context_menu.rs` の PowerShell パスクォート
  重複 (4 箇所) を `ps_quote` ヘルパーに統合。`update()` のドロップ取り込みを「ドロップ
  無しなら空チェックのみ」に変更し毎フレームの `Vec` 確保を回避。

## 1. 背景・動機

- mIV は画像ビューアとして「見つけた画像を別フォルダへコピーする」ワークフローが頻出するが、
  現状は右クリック → 「コピー」→ 貼り付け先で Ctrl+V の 2 ステップが必要。
- エクスプローラや他の画像管理ソフトは当たり前にドラッグ送出ができるため、無いと不便。
- `docs/archive/release/feature-expansion-ideas.md` の「見送る機能」に **「ドラッグ送出 — 工数大、別議論」**
  として記録されていた項目。本ドキュメントはその「別議論」にあたる。
- 調査の結果、想定より低コストで実装できる見込みが立ったため、改めて設計に起こす。

## 2. スコープ

### 対象 (やる)

- **実ファイル**: 画像 / 動画 / ZIP 本体 / PDF 本体 / 変換前アーカイブ (RAR/7z/LZH 等)
- **実フォルダ**: グリッド上段のフォルダ
- ドロップ先: エクスプローラ、デスクトップ、他アプリ (`CF_HDROP` を受けるものすべて)
- 操作: **コピーのみ** (移動・リンクはしない。後述 §6.4)
- 複数選択ドラッグ: チェックボックスで複数選択した実ファイル / 実フォルダをまとめてドラッグ

### 対象外 (やらない)

- **仮想フォルダ内アイテム** (`GridItem::ZipImage` / `PdfPage`): ZIP/PDF 内の画像は
  ディスク上に独立した実ファイルとして存在しないため、D&D 送出の対象にしない。
  ユーザー合意済みの前提。
- **`GridItem::SearchContainer`**: Ctrl+G メタ検索の集約ビューに出るコンテナで、`path` は
  実フォルダまたは ZIP 本体を指す ([grid_item.rs:51](../src/grid_item.rs))。技術的には
  ドラッグ可能だが、検索結果の集約 UI からのドラッグはエッジケースであり、「通常の
  フォルダ閲覧でドラッグできる」という単純なメンタルモデルを保つため **初版では対象外**
  とする。`path` は保持しているので将来追加は容易 (`drag_source_path()` に 1 分岐
  足すだけ)。→ §5.2。
- 画像 **データ** (ビットマップ) のドラッグ送出: ファイルパス (`CF_HDROP`) のみ。
  ビットマップ D&D は将来課題。
- 移動 (MOVE) セマンティクス: §6.4 参照。

> **ドラッグ受け取り (他アプリ → mIV へのドロップ)** は当初「対象外」としていたが、
> 送出と対称の操作として後日実装した。詳細は §11。

## 3. 既存コードの足場 (調査結果)

| 既にあるもの | 場所 | 用途 |
| --- | --- | --- |
| メインウィンドウ HWND | `App::main_hwnd: Option<isize>` ([app.rs:2872](../src/app.rs)) | 初フレームで `frame.window_handle()` から取得済 ([app.rs:16853](../src/app.rs)) |
| `windows` crate 0.61 + 必要 feature | [Cargo.toml:37-70](../Cargo.toml) | `Win32_System_Ole` / `_Com` / `_Com_StructuredStorage` / `_UI_Shell` / `_UI_Shell_Common` / `_System_Memory` すべて有効 |
| ドラッグ対象パス判定 | `GridItem::drag_source_path()` ([grid_item.rs](../src/grid_item.rs)) | D&D 送出可能な実ファイル / 実フォルダのパス抽出 |
| 複数選択モデル | `App::checked: HashSet<usize>` | チェックボックス選択。`collect_checked_paths()` で実ファイル / 実フォルダのパス収集 |
| セルのクリック処理 | `App::handle_cell_interaction()` ([ui_main.rs:1474](../src/ui_main.rs)) | 現状 `egui::Sense::click()` |
| Shell `IDataObject` 生成 | `src/file_drag.rs` | D&D 送出用。クリップボード writer とは別に、同一プロセス内で Shell data object を作る |

### 3.1 クリップボード実装を流用しない理由

旧実装には PowerShell スクリプトで
`[System.Windows.Forms.Clipboard]::SetFileDropList()` を呼ぶファイルコピー helper があった。
これはクリップボードへの **非同期書き込み**には使えても、OLE ドラッグ＆ドロップでは
**同一プロセス内の `IDataObject` を `DoDragDrop` に渡す** 必要があるため、別プロセス
(PowerShell) の clipboard writer は流用できない。現在の Ctrl+C/X は Shell verb 経路へ
一本化済みだが、D&D は引き続き COM 経路で Shell `IDataObject` を組み立てる。

## 4. 技術アプローチ

### 4.1 OLE ドラッグ＆ドロップの基本構造

Windows の D&D ソース側は本来:

- `IDataObject` (メソッド 9 個) — ドラッグするデータの提供
- `IDropSource` (メソッド 2 個) — ドラッグ継続/中止の判定、カーソルフィードバック
- `IEnumFORMATETC` (メソッド 4 個) — `IDataObject` が対応形式を列挙するため

を実装し、`DoDragDrop(pDataObj, pDropSource, dwOKEffects, &effect)` を呼ぶ。
`IDataObject` の自前実装 (`DROPFILES` の `HGLOBAL` 構築、`STGMEDIUM` 所有権管理) が
「Rust で D&D は面倒」と言われる正体。

### 4.2 採用案: シェル提供の `IDataObject` を借りる

**データ形式・medium 生成を自前実装せず、シェルが用意済みのものを使う。** これで上記の面倒な部分が
丸ごと消える。

```
各 PathBuf
  → SHParseDisplayName(.., Option::<IBindCtx>::None, ..)        → *mut ITEMIDLIST (PIDL)
  → SHCreateShellItemArrayFromIDLists(&pidls)                   → IShellItemArray
  → array.BindToHandler(Option::<IBindCtx>::None, &BHID_DataObject) → IDataObject  ← 完成済み
  → SHDoDragDrop(hwnd, &dataobject, Option::<IDropSource>::None, DROPEFFECT_COPY)  ← ドラッグ開始
```

`None` を渡す引数 (`IBindCtx` / `IDropSource`) はいずれも型推論が効かないため、
`Option::<T>::None` の形で**型を明示**する (§5.1 / §4.3)。

この `IDataObject` は `CF_HDROP` に加え、シェル形式 (`CFSTR_SHELLIDLIST` 等) も保持するため
**エクスプローラへのドロップも他アプリへのドロップも正しく動く**。`SHDoDragDrop` は
既定のドラッグ画像・ドロップカーソルも自動で提供する。

Shell の形式・medium の生成を再実装しない。§12 の非同期対応では、送出専用の
`IDataObject` 委譲ラッパーと `IDataObjectAsyncCapability` を追加する。追加依存・feature は不要。

### 4.3 windows crate 0.61 での API 確認結果

`~/.cargo/registry/.../windows-0.61.3` を実地確認し、以下が**すべて存在**することを検証済:

| API | モジュール | 備考 |
| --- | --- | --- |
| `SHDoDragDrop` | `Win32::UI::Shell` | `(hwnd, pdata, pdsrc, dweffect) -> Result<DROPEFFECT>` |
| `SHCreateShellItemArrayFromIDLists` | `Win32::UI::Shell` | `(&[*const ITEMIDLIST]) -> Result<IShellItemArray>` |
| `SHParseDisplayName` | `Win32::UI::Shell` | パス文字列 → PIDL |
| `IShellItemArray::BindToHandler` | `Win32::UI::Shell` | `(pbc, bhid) -> Result<T>` |
| `BHID_DataObject` | `Win32::UI::Shell` | GUID 定数 |
| `DoDragDrop` / `IDropSource` | `Win32::System::Ole` | フォールバック経路用 (§4.4) |
| `DROPFILES` | `Win32::UI::Shell` | フォールバック経路用 |

`SHDoDragDrop` のラッパシグネチャ (実物):

```rust
pub unsafe fn SHDoDragDrop<P1, P2>(
    hwnd: Option<HWND>, pdata: P1, pdsrc: P2, dweffect: DROPEFFECT,
) -> windows_core::Result<DROPEFFECT>
where P1: Param<IDataObject>, P2: Param<IDropSource>;
```

`pdsrc` は `Param<IDropSource>` なので `None` を渡せばシェル既定の `IDropSource` が
使われる。**解決済** (Codex レビュー): Microsoft Learn の
[SHDoDragDrop ドキュメント](https://learn.microsoft.com/windows/win32/api/shlobj_core/nf-shlobj_core-shdodragdrop)
は Windows Vista 以降、`pdsrc` が NULL の場合シェルが `IDropSource` を生成すると明記して
いる。Rust 側は型推論を効かせるため `Option::<IDropSource>::None` を**明示**して渡す
(`None` 単独だと `P2` の型が決まらずコンパイルエラーになりうる)。

### 4.4 フォールバック経路 (案 4.2 が不調なら)

万一シェル `IDataObject` 経路が不調 (BindToHandler 失敗、ドロップ先が CF_HDROP を
取れない等) の場合でも、`DoDragDrop` を直接使う経路に切り替えられる。その場合でも
`IDataObject` は実装したくないので、選択肢は:

- (a) シェル `IDataObject` はそのまま使い、`DoDragDrop` に渡す。`IDropSource` だけ自前実装。
  `IDropSource` は 2 メソッドのみで簡単 — `QueryContinueDrag` (ボタン解放で
  `DRAGDROP_S_DROP`、Esc で `DRAGDROP_S_CANCEL`、それ以外 `S_OK`)、
  `GiveFeedback` (`DRAGDROP_S_USEDEFAULTCURSORS`)。約 15 行。
  ⚠ ただし `QueryContinueDrag` の引数 `grfKeyState: MODIFIERKEYS_FLAGS` 等を扱うため、
  現在の [Cargo.toml](../Cargo.toml) に**無い** `Win32_System_SystemServices` feature の
  追加が必要になる可能性がある (Codex 指摘)。フォールバックを採用する判断をした時点で
  feature 追加の要否を実コンパイルで確認する。
- (b) `IDataObject` も自前実装 (最終手段、§4.1 の面倒な経路)。本設計では採らない。

第一候補は §4.2、第二候補は (a)。(b) は採用しない方針。

**「追加 Cargo feature なし」は §4.2 の primary path 限定の記述**。フォールバック (a) を
採用する場合は上記の通り feature 追加がありうる。

## 5. 実装詳細

### 5.1 新規モジュール `src/file_drag.rs` (約 100〜150 行)

公開 API は 1 つ:

```rust
/// 指定パス群の OLE ドラッグ＆ドロップ (コピー) を開始する。
/// この関数は SHDoDragDrop が戻る (ドロップ完了 or キャンセル) までブロックする。
/// UI スレッドから、かつマウスボタンが押下中に呼ぶこと。
pub fn start_file_drag(hwnd: isize, paths: &[std::path::PathBuf]) -> DragOutcome;

/// `start_file_drag` の結果。呼び出し側 (§5.5b) はこれを見て
/// ポインタリセットの要否・失敗トーストの要否を判断する。
#[derive(Debug, Clone)]
pub struct DragOutcome {
    /// `SHDoDragDrop` を実際に呼んだか。**true のときだけ** §5.5(a) の
    /// ポインタリセットが要る (到達しなければ winit が通常どおり
    /// WM_LBUTTONUP を受ける)。`SHDoDragDrop` が HRESULT エラーを返した
    /// 場合でも「呼んだ = モーダルループに入った」ので true。
    pub started: bool,
    /// `SHParseDisplayName` に失敗したパス数 (0 が正常)。>0 ならトーストで明示する。
    pub failed_paths: usize,
    /// COM 各ステップで失敗した場合のエラー。正常時は `None`。
    pub error: Option<FileDragError>,
    /// effect != NONE による drop 受理。非同期コピー完了の意味ではない。
    pub dropped: bool,
}
// 構築は private コンストラクタ経由: not_started() / failed_before_start(failed,
// error) / after_modal(failed, error, dropped)。started を取り違えないようにするため。
// effect の数値はログ、dropped は mIV 自身への落下判定にだけ使う。

/// `start_file_drag` の COM ステップ別エラー。どこで失敗したかを呼び出し側が
/// 区別できるようにする (主にログの切り分け用)。
#[derive(Debug, Clone)]
pub enum FileDragError {
    /// `SHParseDisplayName` が全パスで失敗した (ドラッグ対象が 1 件も作れない)。
    AllPathsUnresolved,
    /// `SHCreateShellItemArrayFromIDLists` が失敗した。
    ShellArrayCreate(windows_core::HRESULT),
    /// `IShellItemArray::BindToHandler(BHID_DataObject)` が失敗した。
    BindToHandler(windows_core::HRESULT),
    /// async capability の設定 / 復帰時の寿命処理が失敗した。
    AsyncCapability(windows_core::HRESULT),
    /// `SHDoDragDrop` 自体が HRESULT エラーを返した (モーダルループには入っている)。
    DoDragDrop(windows_core::HRESULT),
}
```

`DragOutcome` の組み合わせ早見表 (呼び出し側 §5.5b の分岐根拠):

| 状況 | `started` | `error` |
| --- | --- | --- |
| `paths` 空 | false | None |
| `SHParseDisplayName` 全滅 | false | `AllPathsUnresolved` |
| 配列生成 / BindToHandler 失敗 | false | `ShellArrayCreate` / `BindToHandler` |
| ドラッグ → ドロップ成立 or キャンセル | true | None |
| `SHDoDragDrop` が HRESULT エラー | true | `DoDragDrop` |

処理手順:

1. `paths` が空なら `DragOutcome::not_started()` を返す。
2. **COM 初期化は行わない** (採用方針、§6.3 参照)。`start_file_drag` は UI スレッド
   (= winit のイベントループスレッド) からのみ呼ばれ、winit 0.30.13 がウィンドウ作成時に
   `OleInitialize` → `RegisterDragDrop` 済みなので、このスレッドは既に STA。
   `start_file_drag` 内で重ねて `OleInitialize` を呼ぶと、Microsoft Learn の
   [OleInitialize ドキュメント](https://learn.microsoft.com/windows/win32/api/ole2/nf-ole2-oleinitialize)
   が要求する「成功した呼び出し (`S_FALSE` 含む) ごとに `OleUninitialize` でバランスを
   取る」義務が生じる。これを取り違えると winit の COM 寿命を壊すため、**そもそも呼ばない**
   のが安全。万一どうしても自前初期化が要ると判明した場合は、RAII ガード
   (`Drop` で必ず対応する `OleUninitialize` を呼ぶ型) に必ず包む。
3. 各 `PathBuf` を UTF-16 (`\0` 終端) 化し、
   `SHParseDisplayName(pwstr, Option::<IBindCtx>::None, &mut pidl, 0, &mut attrs)` で
   `*mut ITEMIDLIST` を取得。第 2 引数の `IBindCtx` は不要なので渡さないが、`None` 単独だと
   型推論が効かないため **`Option::<IBindCtx>::None` と型を明示**する。
   **失敗を黙ってスキップしない** (§5.4.2 の「黙って一部だけ」回避方針と揃える。
   実ファイルが parse 失敗するのは「選択後ドラッグ前にファイルが消えた」等の異常系):
   失敗したパス数を数えて `DragOutcome.failed_paths` に載せる。成功した PIDL だけで
   ドラッグを続行し、呼び出し側 (§5.5b) が失敗件数をトーストで明示する。
   全パスが失敗したら以降の手順をスキップして
   `DragOutcome::failed_before_start(paths.len(), FileDragError::AllPathsUnresolved)`
   を返す。
4. `SHCreateShellItemArrayFromIDLists(&pidls)` で `IShellItemArray` を作る。失敗したら
   `DragOutcome::failed_before_start(failed, FileDragError::ShellArrayCreate(hr))` で
   return (PIDL 解放は手順 5 と同じく漏れなく行う)。
5. 取得した PIDL を `CoTaskMemFree` で順に解放 (配列側がコピー済みなので安全)。
   早期 return する経路でも漏れなく解放するよう、ガード型か明示的な解放順序で組む。
6. `array.BindToHandler(Option::<IBindCtx>::None, &BHID_DataObject)` で `IDataObject` を
   得る。第 1 引数の `IBindCtx` も手順 3 と同じく `Option::<IBindCtx>::None` と型を明示。
   失敗したら `DragOutcome::failed_before_start(failed, FileDragError::BindToHandler(hr))`。
7. §12 の送出専用 `DragDataObject` で inner を包み、async mode を有効にする。
   設定失敗は `AsyncCapability` の開始前失敗として通知し、黙って同期へ切り替えない。
   `SHDoDragDrop(Some(HWND(hwnd as *mut _)), &drag.data, None::<&IDropSource>,
   DROPEFFECT_COPY)` を呼ぶ。OLE モーダル区間はブロックする。
8. 復帰時は成否・effect に関係なく `finish_modal` で InOperation を確認する。
   未開始だけ解除し、Running は EndOperation まで維持する。所要時間と InOperation を
   ログへ残す。戻り値は `started: true` とし、Ok の `effect != 0` を dropped に使う。
   HRESULT 失敗は `DoDragDrop` として通知する。非同期では復帰をコピー完了と解釈しない。

`#[cfg(windows)]` でガードし、非 Windows では `DragOutcome::not_started()` を返す
空実装 (他のプラットフォーム分岐に揃える)。

### 5.2 `GridItem` にドラッグ対象パス抽出を追加 (約 15 行)

実装当初の `file_operation_path()` はフォルダを除外していたが、後に
Ctrl+X/C/V でもフォルダ整理を扱うようになったため、現在はフォルダも返す。
D&D でも同じ実体パスだけを送出するため、別アクセサ `drag_source_path()` を使う:

```rust
/// D&D で送出できる実ファイル / 実フォルダのパス。
/// file_operation_path() と同じく実パスを持つアイテムだけを返す。
/// 対象外:
///   - ZipImage / PdfPage  — 仮想フォルダ内 (ディスク上に実体がない)
///   - SearchContainer     — 検索集約 UI のコンテナ。path は実フォルダ/ZIP を指すが
///                           初版スコープ外 (§2)。将来含めるならここに 1 分岐足す。
pub fn drag_source_path(&self) -> Option<&Path> {
    match self {
        Self::Folder(p) | Self::Image(p) | Self::Video(p)
        | Self::ZipFile(p) | Self::PdfFile(p)
        | Self::ConvertibleArchive { path: p, .. } => Some(p),
        _ => None,
    }
}
```

`#[cfg(test)]` でユニットテストを追加 — Folder/Image/Video/ZipFile/PdfFile/
ConvertibleArchive が `Some`、ZipImage/PdfPage/**SearchContainer** が
`None` になる不変条件を検証する。`SearchContainer` を初版でスコープ外にする判断
(§2) をテストで固定し、将来含める変更時にこのテストが「意図的な仕様変更」の
明示ポイントになる。

### 5.3 `App` への状態追加 (約 10 行)

```rust
/// フレーム末尾で実行する native ドラッグの予約。
/// egui closure 内から SHDoDragDrop を直接呼べない (self 借用衝突・再入) ための受け渡し。
pub(crate) pending_native_drag: Option<PendingNativeDrag>,
/// 直前フレームで native ドラッグを実行した。次フレーム冒頭で egui の
/// ポインタ状態をリセットするためのフラグ (§6.1)。
pub(crate) native_drag_just_finished: bool,

/// `pending_native_drag` の中身。
pub(crate) struct PendingNativeDrag {
    /// ドラッグするファイル / フォルダの実パス群 (index 昇順、§5.4.2)。空ではない。
    pub paths: Vec<PathBuf>,
    /// 混在選択 (実パス + 仮想アイテム) で除外が発生したとき、
    /// **ドラッグ完了後**に出すトースト文言。除外なしなら None。
    /// drag_started() の時点でトーストを出すと、同じ update 末尾の SHDoDragDrop が
    /// 長時間ブロックする間にトーストの表示期限が切れてしまう (§5.4.3 / Codex 第 3 回)。
    pub post_drag_toast: Option<String>,
}
```

`Default` impl で `pending_native_drag` を `None`、`native_drag_just_finished` を
`false` に初期化。

### 5.4 `ui_main.rs` — ドラッグ検出 (約 70 行)

`handle_cell_interaction()` ([ui_main.rs:1481](../src/ui_main.rs)) の
`egui::Sense::click()` を `egui::Sense::click_and_drag()` に変更。
`clicked()` / `double_clicked()` / `secondary_clicked()` は `click_and_drag()` でも従来通り
発火するので、選択・フルスクリーン化・右クリックメニューは壊れない。

#### 5.4.1 ボタン / modifier の扱い (選択操作との非競合)

**primary button 限定** (Codex 第 3 回指摘): ドラッグ開始は
`response.drag_started_by(egui::PointerButton::Primary)` で判定する。素の
`drag_started()` は右ボタン / 中ボタンのドラッグでも true になりうる。右ドラッグは
コンテキストメニュー (`secondary_clicked()`)、中ドラッグはスクロール等と紛れるため、
**左ボタンのドラッグだけを native D&D の起点にする**。

複数選択は `if response.clicked()` 内の共通選択 helper で、設定したチェック方式 /
エクスプローラー方式に応じて Ctrl+クリック (トグル) / Shift+クリック (範囲) を
処理している。egui では
**`clicked()` と `drag_started()` は相互排他** — ポインタが閾値以上動けば `drag_started()`、
動かなければ `clicked()` のどちらか一方しか発火しない。したがって:

- **ドラッグ送出は modifier の有無に関わらず `drag_started_by(Primary)` で開始する。**
  mIV は COPY 限定 (§6.4) なので、Ctrl (コピー強制) / Shift (移動強制) を握っていても
  挙動は変わらない。OS 側がカーソル装飾を変える程度。
- Ctrl+クリック / Shift+クリックによる選択操作は `clicked()` 側のまま不変。drag とは
  co-fire しないので競合しない。
- 既知のトレードオフ: Ctrl+クリックのつもりでもポインタが egui のドラッグ閾値
  (約数 px) を超えて動くと `drag_started()` 側に倒れ、選択トグルでなくドラッグになる。
  これは `click_and_drag()` 採用の不可避な副作用。閾値は十分小さく実害は薄いが、
  手動テスト (§7) で確認する。

#### 5.4.2 ドラッグ対象の決定 — 混在選択の仕様

⚠ **Codex 指摘の最重要修正点**: `checked` には実パスを持つアイテムと仮想アイテム
(`ZipImage` / `PdfPage`、どちらも `is_checkable()` が true) が混在しうる。素朴に
`filter_map` で実ファイルだけ拾うと、ユーザーには **「選択したうち一部しかコピー
されなかった」** ように見え危険。明示仕様にする。

`drag_started_by(Primary)` 検出時、セル `idx` について:

**payload の順序** (Codex 第 3 回指摘): `App::checked` は `HashSet<usize>` で反復順が
不定。ドラッグ対象パス列は必ず **`checked` の index を昇順ソートしてから** 構築する。
index 昇順 = `items` 配列の並び順 = 通常フォルダでは現在のソート順なので、ユーザーから
見て自然な順になり、`decide_drag_payload()` のユニットテストも安定する。

**(A) `idx` が複数選択の一部 (`self.checked.contains(&idx)`) の場合**

`checked` の index を昇順ソートし、各 item を 2 群に分割する:

- `draggable`: `drag_source_path()` が `Some` のもの (実ファイル/実フォルダ)
- `virtual_excluded`: 仮想アイテム (`ZipImage` / `PdfPage`)

判定:

- `draggable` が空 → ドラッグ開始しない。トースト「ドラッグできる実ファイル / フォルダが
  選択されていません」を**即時**表示してよい (ドラッグが走らない = `SHDoDragDrop` の
  ブロックが無いため期限切れの心配がない)。
- `virtual_excluded` が非空 (= 混在選択) → **`draggable` をドラッグしつつ、除外を
  明示するトースト**を出す。Codex が挙げた 2 案 (「ドラッグ不可+トースト」/「除外を
  明示」) のうち **後者を採用** — 実ファイルは確実に送出でき、かつ除外をユーザーが
  認識できる。
  - ⚠ このトーストは `drag_started()` 時点では出さず、`PendingNativeDrag.post_drag_toast`
    に積んで **ドラッグ完了後** (`SHDoDragDrop` が戻った後) に出す。`drag_started()` で
    即時に出すと、同じ update 末尾の `SHDoDragDrop` が長時間ブロックする間にトーストの
    表示期限が切れ、ユーザーが見られない (Codex 第 3 回指摘)。
  - 文言は「`<N>` 件のフォルダ内画像は除外しました。実ファイル / フォルダ `<M>` 件をドラッグ
    対象にしました」程度にする。**「コピーしました」とは書かない** — ユーザーが
    ドロップ前にキャンセルした場合に嘘になるため (`SHDoDragDrop` の結果に関わらず
    出る文言なので、確定している「ドラッグ対象にした」事実だけを述べる)。
- 混在なし → `draggable` をそのままドラッグ。`post_drag_toast` は `None`。

**(B) `idx` が複数選択に含まれない場合**

エクスプローラ流: 掴んだ単体だけをドラッグする (選択集合は変えない)。

- `drag_source_path(idx)` が `Some` → その 1 件をドラッグ。
- `None` (= 掴んだのが仮想アイテム / セパレータ / SearchContainer) → ドラッグ開始
  しない。基本は無音 no-op (エクスプローラで掴めない物を掴んだのと同じ)。
  `ZipImage` / `PdfPage` を直接掴んだケースだけは、初回は気づきにくいので任意で
  トースト「ZIP/PDF 内の画像はドラッグでコピーできません」を出してもよい (任意)。

トースト基盤は v0.9.0 導入の `show_feedback_toast` 系を流用する。

#### 5.4.3 受け渡し

egui closure 内で `SHDoDragDrop` は呼べない (`self` の借用衝突・再入リスク) ので、
決定した内容を `self.pending_native_drag` に積むだけ。実行は §5.5(b)。

`decide_drag_payload` は §5.4.2 のロジックを純粋関数的にまとめた新規ヘルパで、
3 値の enum を返す:

```rust
enum DragDecision {
    /// ドラッグを開始する。paths は空でない (index 昇順、§5.4.2)。
    Start { paths: Vec<PathBuf>, post_drag_toast: Option<String> },
    /// ドラッグはしないが即時トーストを出す (全仮想選択など)。
    ImmediateToast(String),
    /// 何もしない (単体の仮想アイテム / セパレータ / SearchContainer を掴んだ no-op)。
    None,
}

// handle_cell_interaction 内:
if response.drag_started_by(egui::PointerButton::Primary) {
    match self.decide_drag_payload(idx) {
        DragDecision::Start { paths, post_drag_toast } => {
            self.pending_native_drag = Some(PendingNativeDrag { paths, post_drag_toast });
        }
        DragDecision::ImmediateToast(msg) => self.show_feedback_toast(msg),
        DragDecision::None => {}
    }
}
```

`decide_drag_payload` を独立メソッドにして `#[cfg(test)]` でユニットテスト可能にする
(混在選択 / 全仮想 / 単体実ファイル / 単体仮想 / 未チェック item / payload が index
昇順か、の各分岐)。混在選択の `post_drag_toast` 文言もこのヘルパが組み立てるので、
文言の検証もユニットテストで固定できる。

### 5.5 `app.rs` — フレーム末尾で実行 + ポインタリセット (約 50 行)

`App::update()` ([app.rs:16812](../src/app.rs)) で:

**(a) フレーム冒頭** — 前フレームで native ドラッグした直後ならポインタ状態をリセット:

```rust
if std::mem::take(&mut self.native_drag_just_finished) {
    // SHDoDragDrop がドラッグ終了の WM_LBUTTONUP を内部で消費し winit に
    // 届かないため、egui は左ボタンが押下中のままと誤認する。強制リセット。
    ctx.input_mut(|i| i.pointer = egui::PointerState::default());
    // PointerState は raw な入力状態のみ。egui の interaction 層
    // (interact_widgets.dragged / Memory::interaction().potential_drag_id) は
    // 別管理なので、これも明示的に止める。省くと「掴んでいた扱いのセル」が
    // 残り幽霊ドラッグの火種になる (§6.1、Codex レビュー第 2 回指摘)。
    ctx.stop_dragging();
}
```

**(b) フレーム末尾** — 全パネル描画後 (egui closure を全部抜けた後) に実行:

```rust
if let Some(PendingNativeDrag { paths, post_drag_toast }) = self.pending_native_drag.take() {
    match self.main_hwnd {
        Some(hwnd) => {
            let outcome = crate::file_drag::start_file_drag(hwnd, &paths); // モーダルブロック

            // SHDoDragDrop に到達したときだけポインタリセットが要る (§5.1 / §6.1)。
            // 未到達 (paths 全滅・COM 失敗) なら winit が通常どおり WM_LBUTTONUP を
            // 受け取るので、リセットすると §6.1 の副作用が無駄に出る。
            if outcome.started {
                self.native_drag_just_finished = true;
            }
            // COM ステップ別エラーはログに残す (DragOutcome.error で切り分け済み)。
            if let Some(err) = &outcome.error {
                crate::logger::log(&format!("file_drag: {err:?}"));
            }
            // 結果トーストは「ここで」= SHDoDragDrop が戻った後に出す。
            // drag_started() 時点で出すと SHDoDragDrop のブロック中に表示期限が
            // 切れる (§5.4.2 / Codex 第 3 回)。失敗・未開始時は emit_drag_result_toasts
            // 側が post_drag_toast を抑止する (Codex 第 4 回 P2)。
            self.emit_drag_result_toasts(Some(&outcome), post_drag_toast);
        }
        None => {
            // 初フレーム前など main_hwnd 未取得。ドラッグ自体が走らないので
            // ポインタリセットも不要。
            crate::logger::log("file_drag: main_hwnd unavailable, drag skipped");
            self.emit_drag_result_toasts(None, post_drag_toast);
        }
    }
    ctx.request_repaint();
}
```

`emit_drag_result_toasts(outcome: Option<&DragOutcome>, post_drag_toast: Option<String>)`
は小さなヘルパ。**失敗・未開始を最優先で通知する** (Codex 第 4 回 P2):

- `outcome` が `None` (main HWND 未取得) → 「ファイルのドラッグを開始できませんでした」。
- `outcome.error` が `Some`、または `outcome.started == false` → ドラッグが実際に
  始まっていない / COM 失敗。「ドラッグを開始できませんでした」(未開始) または
  「ドラッグ中にエラーが発生しました」(`started == true` で `SHDoDragDrop` がエラー) を
  出し、**`post_drag_toast` は抑止する**。抑止しないと「実ファイル / フォルダ N 件をドラッグ対象に
  しました」だけが出て、実際は始まっていないのに成功したかのように誤解させる。
- 正常時のみ → 除外トースト (`post_drag_toast`) と「N 件はドラッグできませんでした」
  (`failed_paths > 0`) を、2 連続 `show_feedback_toast` で上書きし合わないよう
  **1 本のメッセージに連結**して出す。

`SHDoDragDrop` 呼び出し時点でマウスボタンはまだ押下中 (このフレームで
`drag_started_by(Primary)` が出たばかり) なので、`DoDragDrop` の前提 (ボタン押下中に
呼ぶ) を満たす。

`native_drag_just_finished` を立てるのは `outcome.started == true` のときだけ
(Codex レビュー第 2 回指摘)。`main_hwnd` 未取得や `start_file_drag` がドラッグを
開始できなかった場合は `SHDoDragDrop` に到達しておらず、winit は WM_LBUTTONUP を
正常に受け取っているため、ポインタリセットを走らせると §6.1 の副作用 (hover /
tooltip の 1 フレーム乱れ) だけが無駄に出てしまう。

## 6. 注意点・落とし穴

### 6.1 ★最重要: ドラッグ後の egui ポインタ状態固着

`SHDoDragDrop` は内部で独自メッセージループを回し、ドラッグ終了の `WM_LBUTTONUP` を
**自身で消費**する (マウスを `SetCapture` し、ウィンドウプロシージャに up を渡さない)。
そのため winit はボタン解放を観測できず、egui は「左ボタンが押下中のまま」と誤認する。

放置すると、次にマウスを動かしたとき別セル上で `drag_started_by(Primary)` が誤発火し
**幽霊ドラッグ** が起きる (primary ボタンが押下中のままと誤認されるため)。

**対策は 2 段構えで、両方とも §5.5(a) の実装手順に含める** (Codex レビュー第 2 回指摘):

1. **raw ポインタ状態のリセット** —
   `ctx.input_mut(|i| i.pointer = egui::PointerState::default())`。
   `egui::PointerState` が `Default` を実装していることは確認済
   (egui 0.33.3 `input_state/mod.rs:1123`)。
2. **egui interaction 層のドラッグ停止** — `ctx.stop_dragging()`。
   `PointerState` は raw な入力状態しか持たず、egui が「どのウィジェットを掴んで
   いるか」(`Context::interact_widgets.dragged` / `Memory::interaction().potential_drag_id`)
   は**別管理**。`PointerState` だけ消しても interaction 層に `dragged` /
   `potential_drag_id` が残ると幽霊ドラッグの火種になる。`Context::stop_dragging()`
   は egui 0.33.3 に存在し (`context.rs:4054`)、この 2 つを直接クリアする。
   **(1) だけでは不十分** — raw 状態と interaction 層は独立しているため。

⚠ **要実機検証なのは (1) の副作用のみ**。`PointerState::default()` はボタン状態だけ
でなく `latest_pos` / `interact_pos` も消すため、リセット直後 1 フレームの hover /
tooltip / カーソル追従に副作用が出うる (実害は 1 フレームなので軽微の見込みだが未確認)。
(2) の `stop_dragging()` は副作用が小さく、無条件で入れてよい。

実装時は上記 2 段構えで進め、(1) の副作用が実機で問題になったら以下の代替に
切り替えられるよう設計に残す:

- **合成ポインタリリースイベントの注入**: 次フレームの raw input に
  `egui::Event::PointerButton { pressed: false, .. }` を 1 つ足す。`PointerState` 全消し
  より副作用が小さいが、eframe の `update` から raw input へ注入する口が限られるため
  実装はやや手間。
- **`drag_started` の 1 フレームガード**: native drag 直後の 1 フレームだけ
  §5.4 の `drag_started_by(Primary)` 判定を無視する。幽霊ドラッグ「再発火」だけは
  確実に防げる。
- **native drag 進行中/直後フラグでの抑止**: `native_drag_just_finished` が立っている
  間は §5.4 のドラッグ検出自体をスキップする (上記ガードの一般化)。

最低限 §5.4 のドラッグ検出に「直後フラグが立っていたらスキップ」を入れておけば、
ポインタ状態が固着しても幽霊ドラッグの連鎖だけは断てる (保険)。→ §8 Q3。

### 6.2 ドラッグ操作の OLE モーダル区間と非同期転送

ドラッグ操作の OLE モーダル区間は残る。非同期を選択して Drop から早期復帰する target では、
その後の転送完了を待たない。非対応 target の同期処理と開始前の Shell 解決コストも残る。

`SHDoDragDrop` の実行中は `App::update` が戻らず、通常の描画・入力処理が止まる。
意図的な例外は OLE モーダル区間であり、ドロップ後の長いコピー待ち全体を不可避とはしない。
送出 object の async 交渉・所有権は §12。復帰後は通常の winit message loop で STA 宛ての
COM 呼び出しを配送し、source 側に独自 pump・転送完了待ち・周期 repaint を足さない。
同じ制約を [ui-responsiveness.md §4.2](ui-responsiveness.md#42-意図的な例外-native-ファイル-dd-のモーダルブロック)
にも記載する。

### 6.3 COM アパートメント (寿命管理)

`SHDoDragDrop` は呼び出しスレッドが STA で初期化されていることを要求する。

**解決済** (Codex レビュー): winit 0.30.13 は Windows でウィンドウ作成時、`drag_and_drop`
既定値 true のもとで `OleInitialize` → `RegisterDragDrop` を呼ぶ。`start_file_drag` は
その同じ UI スレッド (winit イベントループスレッド) からのみ呼ばれるので、**スレッドは
既に STA 初期化済**。

⚠ 当初案の「`start_file_drag` 内で防御的に `OleInitialize` を呼び、`OleUninitialize` は
呼ばない」は**誤り**。Microsoft Learn の
[OleInitialize ドキュメント](https://learn.microsoft.com/windows/win32/api/ole2/nf-ole2-oleinitialize)
は「`S_FALSE` を含め、成功した `OleInitialize` 呼び出しごとに対応する `OleUninitialize`
が必要」と明記している。バランスを崩すと winit の COM 寿命を壊す。

**採用方針**: `start_file_drag` 内では COM 初期化を**一切行わない** — winit の初期化に
依存する (§5.1 手順 2)。これが最もシンプルで安全。仮に将来「UI スレッド以外から
呼びたい」等で自前初期化が必要になったら、`Drop` で必ず対応する `OleUninitialize` を
呼ぶ RAII ガード型に包む (アンバランスを型で防ぐ)。

### 6.4 コピー限定 (移動を許さない)

`dwOKEffects` を `DROPEFFECT_COPY` のみにする。`DROPEFFECT_MOVE` を含めると、
ドロップ先がデフォルトで MOVE を選んだ場合に **ユーザーの原本が移動してしまう** 事故に
なりうる。ユーザー要望も「コピー」なので COPY 限定とする。`DROPEFFECT_LINK`
(ショートカット作成) を加えるかは任意 — 当面は COPY のみ。

### 6.5 複数フォルダの同時ドラッグ

`App::checked` は `GridItem::is_checkable()` が true のアイテムのみを保持し、
**フォルダは `is_checkable()` から除外されている**。よって:

- **単体フォルダのドラッグ**: 動く (§5.4 の「このセル単体」経路、`drag_source_path` が
  Folder を許可するため)。
- **複数フォルダの同時ドラッグ**: 現状の `checked` 設計では不可。やるなら
  `is_checkable()` にフォルダを含める設計変更が別途必要。

初版スコープは「単体フォルダ + 複数ファイル」とし、複数フォルダ同時は将来課題
(`is_checkable()` にフォルダを含める設計変更は本機能のスコープ外)。

### 6.6 ScrollArea のドラッグスクロールとの競合

グリッドは `egui::ScrollArea::vertical()` 内に描画される ([ui_main.rs:1697](../src/ui_main.rs))。
セルが `Sense::click_and_drag()` でドラッグを消費するため、セル上で始まったドラッグが
ScrollArea のドラッグスクロールに食われることはないはず。念のため ScrollArea 側の
`drag_to_scroll` を無効化するか要検討 (デスクトップでは既定で大きな影響はないが、
レビューで確認したい)。→ §8 Q5。

タッチ対応 Phase 2 では、exact correlation 済みの touch-derived primary stream が存在する
frame だけ `drag_started_by(Primary)` から native D&D を開始しない。認識しきい値の競争にはせず、
touch start から release / cancel まで抑止する。mouse-only frame は従来の判定をそのまま通す。

### 6.7 eframe の present タイミング

§5.5(b) は「`update()` 末尾でブロックすると、このフレームの絵はまだ present されておらず
画面には前フレームが残る」想定。eframe 0.33 が `update()` の戻り後にテッセレーション →
present する順序であれば正しい。ドラッグ中に表示されるのは「ドラッグ開始 1 つ前の
グリッド」になる — 実用上問題ないが、正確な present 順序は要確認。→ §8 Q6。

## 7. テスト計画

OS モーダル操作のため `egui_kittest` スナップショットテストは**不可**。実機手動テスト必須。

### ユニットテスト (自動)

- `GridItem::drag_source_path()`: Folder/Image/Video/ZipFile/PdfFile/ConvertibleArchive が
  `Some`、ZipImage/PdfPage/SearchContainer が `None`。
- `decide_drag_payload()` (§5.4.3): 単体実パス / 単体仮想 / 複数実パス /
  混在選択 (実 + 仮想) / 全仮想 / 未チェック item の各分岐で、`DragDecision` の種別・
  ドラッグ対象パス・`post_drag_toast` 文言が仕様通りか。payload が **index 昇順**で
  並ぶことも検証する (§5.4.2)。

### 手動テスト (実機)

OS モーダル操作なので自動化不可。以下を実機で確認する。

**基本動作**

- グリッドの画像をエクスプローラの別フォルダへドラッグ → コピーされる。
- グリッドのフォルダをエクスプローラへドラッグ → フォルダごとコピーされる。
- ZIP 本体 / PDF 本体 / RAR / 7z をドラッグ → コピーされる。
- デスクトップ・他アプリ (ブラウザ、別画像ソフト等) へのドロップ。

**選択モデルとの組み合わせ**

- 複数チェック選択 (全部実ファイル / 実フォルダ) → どれか 1 つを掴んでドラッグ → 選択全部がコピー。
- **混在選択**: `checked` に実パスと ZipImage/PdfPage を混ぜる → ドラッグ →
  実パスだけコピーされ、除外トーストが出る (§5.4.2(A))。
- **`checked` がある状態で未チェック item を掴んでドラッグ** → エクスプローラ流に
  掴んだ単体だけがコピーされ、選択集合は変わらない (§5.4.2(B))。
- 全仮想アイテムだけを `checked` にしてドラッグ → ドラッグ開始せず、トースト表示。
- 仮想フォルダ (ZIP を開いた中) の画像を直接掴む → ドラッグが始まらない (no-op)。
- **Ctrl / Shift 操作との非競合**: Ctrl+クリック / Shift+クリックで選択を組んだ直後に
  ドラッグ / Ctrl・Shift を握ったままドラッグ → 選択操作とドラッグが破綻しない (§5.4.1)。

**パスのバリエーション**

- 日本語を含むパス / 非常に長いパス (260 文字近辺) / UNC・ネットワークパス
  (`\\server\share\...`) のドラッグ → 文字化け・失敗が起きない。

**回帰・固着 (§6.1)**

- ドラッグ正常完了後、グリッドのシングルクリック選択 / ダブルクリックでフルスクリーン /
  右クリックメニューが正常に動く。
- ドラッグを Esc / ウィンドウ外で離してキャンセル → 何も起きず、その後の
  クリック / ダブルクリック / 右クリックが固着しない。
- 連続ドラッグ (ドラッグ → すぐまたドラッグ) で幽霊ドラッグが起きない。

**環境**

- 複数モニタ / 高 DPI (150% / 200%) 環境でのドラッグ。

## 8. レビュー状況

### 8.1 解決済み (Codex レビュー第 1 回)

- **Q1 — `SHDoDragDrop` の `pdsrc` NULL 可否**: 解決。Vista 以降 NULL でシェルが
  `IDropSource` を生成する。`Option::<IDropSource>::None` を明示して渡す。→ §4.3。
- **Q2 — UI スレッドの STA 初期化**: 解決。winit 0.30.13 がウィンドウ作成時に
  `OleInitialize` + `RegisterDragDrop` 済み。あわせて「自前 `OleInitialize` の寿命管理」
  の誤りを修正 — `start_file_drag` 内では COM 初期化しない方針に確定。→ §5.1 / §6.3。
- **混在選択の仕様** (Codex 指摘 4): 解決。実パス + 仮想アイテム混在時は実パスを
  ドラッグしつつ除外トーストを出す。modifier 競合も非競合と整理。→ §5.4。
- **`SearchContainer` スコープ** (Codex 指摘 5): 解決。初版は対象外と明記。→ §2 / §5.2。

### 8.2 残る要検証点 (実装時に実機で確定)

- **Q3**: §6.1 のポインタ固着対策。対策は 2 段構え (`PointerState::default()` の raw
  リセット + `ctx.stop_dragging()` の interaction 層クリア) を**両方とも実装する**。
  実機検証が要るのは前者の副作用 (`interact_pos` クリアで hover / tooltip が 1 フレーム
  乱れないか) だけで、NG なら §6.1 の代替へ。保険として §5.4 のドラッグ検出に
  「native drag 直後フラグでのスキップ」を必ず入れる。
- **Q5**: §6.6 ScrollArea の `drag_to_scroll` を明示的に無効化すべきか。
- **Q6**: §6.7 eframe 0.33 の present タイミング。`update()` 末尾でブロックしたとき
  画面に残るのは前フレームで正しいか。
- **Q7**: `SHDoDragDrop` がシェル `IDataObject` に対しドラッグ画像 (サムネイル) を
  自動表示するか。`IDragSourceHelper` の明示連携が必要か。無くても実用上問題ないが、
  あった方が体験は良い。
- **Q8**: `SHDoDragDrop` のモーダルメッセージループが winit の wndproc を巻き込み、
  ドラッグ中に予期しない winit イベント (リサイズ、フォーカス変化等) が処理されて
  状態が壊れないか。

## 9. 工数見積もり

### コード規模

| 項目 | 規模 |
| --- | --- |
| 新規 `src/file_drag.rs` (COM 糊 + `DragOutcome` / `FileDragError`) | 約 120〜170 行 |
| `grid_item.rs` `drag_source_path()` + テスト | 約 30 行 |
| `app.rs` 状態追加 (`PendingNativeDrag`) + フレーム末尾実行 + `emit_drag_result_toasts` + ポインタリセット | 約 65 行 |
| `ui_main.rs` ドラッグ検出 + `DragDecision` / `decide_drag_payload()` + テスト | 約 80 行 |
| **合計** | **約 295〜345 行、新規モジュール 1 本** |

- 追加クレート: **なし**
- 追加 Cargo feature: **§4.2 の primary path なら不要**。§4.4 のフォールバック
  (自前 `IDropSource`) を採用する場合は `Win32_System_SystemServices` 追加がありうる。

### 所要時間 (Codex 指摘で上方修正)

当初「半日〜1 日」は楽観的すぎた。COM 糊コード自体は小さいが、**ポインタ状態・選択
モデル・エクスプローラ/デスクトップ/他アプリ/キャンセル/高 DPI/混在選択の手動検証が
重い**。

- **MVP (動く状態)**: 1.5〜3 日
- **堅めに仕上げる (回帰・固着・混在・パスバリエーションを潰し切る)**: 3〜5 日

最大の技術リスクは §6.1 のポインタ固着で、**設計段階で対策候補は用意したが「対策済」
ではなく実機検証が必要** (§8 Q3)。ここで詰まると検証時間がさらに伸びうる。

## 10. ドキュメント更新 (実装時)

CLAUDE.md「コード修正時のドキュメント同時更新」に従い、実装時に以下も更新する:

- `docs/keymap-spec.md` — マウス操作にドラッグ送出を追記
- `docs/spec.md` — 機能一覧・操作仕様に追記
- `docs/ui-responsiveness.md` — §6.2 の「native D&D モーダルブロックは意図的な例外」注記
- `docs/architecture-overview.md` — `file_drag.rs` をモジュールマップに追加
- `docs/README.md` — 本ドキュメントへのリンク (索引)
- `htdocs/mimageviewer/manual/` — ユーザー向けマニュアルに操作説明
- `htdocs/mimageviewer/index.html` — 製品ページの機能一覧

## 11. 受け取り方向 (エクスプローラ → mIV へのドロップ)

§1〜§10 は **送出** (mIV → 外部) の設計。ここでは後日追加した **受け取り** 方向を扱う。

### 11.1 仕様

> ⚠ **v1.1.0 で「フォルダのドロップ受け取り」は一旦無効化した** (同名衝突の無確認
> 上書き・再帰コピーのデータ破壊リスクのため、Explorer 相当の衝突解決と合わせて将来へ
> 延期)。現在はドロップされたディレクトリを `file_drag::partition_dropped_paths` で
> **全て skip** し、ファイルのみコピーする (skip 件数は notice/toast で通知)。以下の
> 自己再帰ガードや旧フローは、フォルダ再導入時に使う設計として残している。

- エクスプローラ等から mIV ウィンドウへファイルをドロップすると、**現在表示中の実
  フォルダへコピー** する (送出と対称の「ファイル整理」操作)。フォルダは上記のとおり skip。
- コピー先は `App::current_favorite_target()` — 実ディレクトリ表示中だけ `Some`。
  ZIP / PDF / 変換アーカイブ表示中は `None` で **トーストで拒否**。
- **検索結果ビュー (Ctrl+G 合成 / Ctrl+S favsearch) も拒否** する。これらは
  `current_folder` を直前の実フォルダのまま残すため、`current_favorite_target()` だけ
  だと「直前のフォルダ」へ誤コピーしてしまう。`items_are_global_search_view` /
  `favsearch.on_results_grid()` を明示的にチェックして拒否する (Codex 第 5 回 P2)。
- **自己再帰ガード**: ドロップされたディレクトリが表示中フォルダ (= コピー先) の
  祖先または自身だと、`Copy-Item -Recurse` が生成中のフォルダを再走査して無限増殖
  する (例: `C:\A\B` 表示中に `C:\A` をドロップ → `C:\A\B\A\B\A...`)。該当ディレクトリは
  コピー対象から除外する (Codex 第 5 回 P1)。ドライブルート / UNC 共有ルート
  (`C:\` や `\\server\share`、`Path::file_name()` が `None`) は basename が無く
  コピー先を一意化できないため、`dest` 自体がルート配下かで判定する (Codex 第 6 回 P1)。
- 操作は **コピーのみ** (送出と同じ方針)。現行の外部ドロップ受け取りは
  `Copy-Item -Force` 経路のため、同名既存ファイルは上書きされる。Ctrl+V の
  クリップボード貼り付けは Windows Shell paste 経路で、衝突確認と進捗 UI は Windows 側に委ねる。

### 11.2 実装

送出と違い COM ドラッグソースは不要。winit が `with_drag_and_drop` 既定 true で
mIV ウィンドウを OS のドロップターゲットに登録済みで、eframe が
`RawInput.dropped_files` にパスを届ける。よって実装は「それを読んで処理する」だけ。

- `App::update` で `ctx.input(|i| i.raw.dropped_files)` を読み、`path` のあるものを
  収集 → `handle_external_file_drop`。
- `handle_external_file_drop`:
  1. 検索結果ビュー (`items_are_global_search_view` / `favsearch.on_results_grid()`) なら
     拒否トースト。
  2. `current_favorite_target()` が `None` (ZIP/PDF 等) なら拒否トースト。
  3. UI スレッドからは「N 件の項目をコピーしています…」トーストを即出して、検証 +
     コピー起動 + 完了待ちを `file-drop-validate` worker thread に丸ごと投げる
     (review #7 対応、`fs::canonicalize` × N が SMB 越しで UI を秒オーダー止めるのを回避)。
- worker (`file-drop-validate`) の処理:
  1. `file_drag::partition_dropped_paths` で **ディレクトリを全て除外** (v1.1.0 で
     フォルダ drop 無効化)。`folder_skipped` 件数は後段の notice/toast で通知する。
     フォルダ再導入時はここで `dir_copy_would_recurse()` の自己再帰除外を併用する。
  2. 残り 0 件 (= ドロップが全てフォルダ) なら `CopyOutcome::notice` に「フォルダのドロップは
     現在無効です (N 件をスキップ)。ファイルのみコピーできます」をセットして送る (poll が
     無音化しないよう notice 経由で必ず通知する)。ファイル + フォルダ混在で `folder_skipped`
     が残った場合は、コピー完了後の `outcome.notice` に「フォルダ N 件はスキップしました」を補う。
  3. 残りがあれば `copy_paths_into_folder` を呼んで内部 worker から `CopyOutcome` を受け取り、
     UI へ転送。`recv()` の `Err` は `CopyOutcome::all_failed(attempted, reason)` で
     全件失敗扱いに格上げする (Codex P2-1 対応: Disconnected を成功扱いに潰さない)。
- `file_drag::dir_copy_would_recurse(src, dest)`: 両パスを `canonicalize` 正規化し、
  コピー先 `dest/basename(src)` が `src` 自身または配下かを小文字化 + コンポーネント
  単位の前方一致で判定する。純粋判定部 `copy_target_inside_src` はユニットテスト済み。
  **v1.1.0 現在は未使用** (フォルダ drop を全 skip しているため)、フォルダ再導入用に保持。
- コピーは `ui_dialogs::context_menu::copy_paths_into_folder` — **ディレクトリは skip し**
  ファイルのみ `Copy-Item -LiteralPath -Force` (`-Recurse` なし) を `try/catch` で囲って失敗
  カウントと先頭 5 件のエラーメッセージを stdout の `::FAILED::N` / `::ERR::msg` マーカーで
  返す。spawn 失敗 /
  tmp 書き込み失敗 / `powershell` 実行失敗 / 非ゼロ終了 / マーカー欠落はすべて
  `failed=attempted` で報告する (Codex P2-1 対応: 旧実装は `SilentlyContinue` で全部
  飲んでいた)。
- UI 側の受け取り経路:
  - 完了は `App::drop_copy_pending: Vec<Receiver<CopyOutcome>>` に積まれ、
    `poll_file_drop_pending` が `pending_reload` を立てつつ:
    - `failed > 0` のとき「成功 K / 失敗 N (例: ...)」トーストを出す
    - `notice = Some(_)` のとき (全件除外等) その文面をトーストで出す
    - それ以外 (`failed == 0 && notice == None`) は静かに reload のみ

### 11.3 送出との非干渉

`SHDoDragDrop` (送出) と winit の `RegisterDragDrop` (受け取り) は独立。送出実装は
受け取りを壊さない (送出追加前から受け取りの下地はあったが、それを読むコードが
無かっただけ)。

## 12. §1.288 ドロップ後のコピーの非同期化 — 段1 設計メモ (2026-10-19)

**状態: 段2実装・対象自動検証・確認用ビルド完了。実機未確認・未コミット。**
正本は [next-release-backlog.md §1.288](next-release-backlog.md)。
2026-10-19、利用者から独立設計レビュー (別 Codex / xhigh) の P1なし・条件付き合意と、
ClaudeCode の構造合意が報告され、段2の実装指示を受けた。条件は成功 HRESULT の保証範囲、
終了時の仕様、残る同期区間の文言、再入・effect 非依存の寿命試験。本節へ反映して実装する。

### 12.1 原因・維持する境界

利用者ログでは、2,606 件の送出で約108秒、6,220件で約324秒、UI スレッドが
`SHDoDragDrop` 内に留まった (いずれも `effect=1`)。同期 `IDropTarget::Drop` 内で
Explorer がコピー完了まで処理するため、`App::update` が戻らないことが原因である。
マウス押下中の OLE ドラッグループと、ドロップ後のコピー待ちは分けて扱う。

現行ソースで確認した境界:

- 実行箇所は `src/app.rs` から **`src/app/update_phases.rs::post_render_native_drag`**
  へ移動済み。フレーム末尾・perf 計測後に `file_drag::start_file_drag` を同期で呼ぶ。
- `src/file_drag.rs` は `SHParseDisplayName` → `SHCreateShellItemArrayFromIDLists` →
  `BindToHandler(BHID_DataObject)` で Shell の `IDataObject` を得る。
  この準備段階の PIDL 解決も現在は UI 上で同期実行されるが、本件のドロップ後のコピー待ちとは
  別の区間であり、今回 worker 化しない。開始前の大量パス・遅い共有の待ちは残る範囲外の課題。
- `dwOKEffects` は **`DROPEFFECT_COPY` のみ**。MOVE / LINK は現在許可していない (§6.4)。
- `effect` の数値はログに出すだけ。`effect != 0` を `DragOutcome.dropped` に変換し、
  呼び出し側は mIV 自身への落下判定に使う。元ファイル削除やコピー成功通知には使わない。
- `shell_data_object_for_paths` は Copy/Cut、Open With 等でも共用される。
  **非同期対応は `start_file_drag` が送出直前に包む部分に限定**し、共有 helper の戻り値や
  クリップボードの完了通知・所有権には変更を入れない。

不変条件は「既存の Shell データ形式・COPY 限定・ポインタ復旧・自己 drop 除外を保ち、
非同期対応 target へのドロップ後はコピー終了を UI が待たない」。全コピーをモーダルに
する簡素化は、本件の解消対象を残すので採らない。代わりに **1ドラッグ1個の自己完結した
COM owner** とし、App の一覧・選択・viewer context に非同期完了を戻さないことで、
フォルダ遷移との rollback / supersession / resume の組み合わせを作らない。

### 12.2 案2を採用する根拠と確認範囲

**採用: Shell 実体を保持し、`IDataObject` 9メソッドを委譲するラッパーに
`IDataObjectAsyncCapability` を実装する (依頼の案2)。** 案1の QI 成功だけでは不十分だった。

製品の起動・デスクトップ入力・実コピーを伴わない調査用 COM プローブを、この環境で実行した。
環境は Windows `10.0.26300.0`。Windows PowerShell の `Add-Type` による C# P/Invoke で
`OleInitialize` (終了時に対応する `OleUninitialize`) を呼び、製品と同じ上記の生成経路を通した。
入力は本 worktree の既存 `Cargo.toml` 1件、および `Cargo.toml` + `docs/README.md` 2件。
PIDL は配列作成後に解放、配列も Bind 後に解放し、保持した data object へ QI した。
プローブはコマンド内で実行し、リポジトリ内にプローブ用ファイルを残していない。
設定データは変更していない。

| 呼び出し | 1件 | 2件 |
| --- | --- | --- |
| QI (`IID_IDataObjectAsyncCapability`) | `S_OK` | `S_OK` |
| 初期 `GetAsyncMode` | `S_OK`, FALSE | `S_OK`, FALSE |
| `SetAsyncMode(1)` → `GetAsyncMode` | 両方 `S_OK`, **FALSE** | 両方 `S_OK`, **FALSE** |
| `SetAsyncMode(-1)` → `GetAsyncMode` | 両方 `S_OK`, **FALSE** | 両方 `S_OK`, **FALSE** |
| `InOperation` → `StartOperation(NULL)` → `InOperation` | FALSE → `S_OK` → TRUE | FALSE → `S_OK` → TRUE |
| `EndOperation(S_OK, NULL, COPY)` → `InOperation` | `S_OK` → FALSE | `S_OK` → FALSE |

最初に直接 vtable 呼び出し (`BOOL` は32 bitの整数)、次に `[ComImport]` の
`InterfaceIsIUnknown` 宣言経由で独立に呼び、`SetAsyncMode(1)` 後も FALSE の結果が一致した。
後者では TRUE 表記の差も除外するため `1` / `-1` 両方を確認した。どちらも失敗 HRESULT を
無視して FALSE と解釈した結果ではない。再調査は同じ生成経路で QI → Set → Get の
各 HRESULT / 出力値を記録すればよい。非同期コピーや別スレッド呼び出しの成功を示す試験ではない。

この Windows build・上記ファイルで **案1だけでは必要な TRUE を公開できない**ことは確認した。
Shell の内部理由、全 Windows 版・全 namespace での同一挙動、Explorer が必ず非同期を
選択することは未確認であり、断定しない。環境ごとの QI 成否で2経路に分岐するより、
送出専用ラッパーで TRUE と寿命を一貫して管理する方針とする。

利用できる API はローカル registry と `Cargo.lock` で確認済み:

- `windows 0.61.3` / `windows-core 0.61.2` / `windows-implement 0.60.2`。
  `Win32::UI::Shell::IDataObjectAsyncCapability` と対応 `_Impl` trait が存在する。
  5メソッドは Set / GetAsyncMode、Start / In / EndOperation。既存の
  `Win32_UI_Shell` / `Win32_System_Com` 等の feature で使え、依存更新は不要。
- `SetAsyncMode` の呼び出し側は `bool`、実装 trait の引数と Get / In の戻り値は
  `windows_core::BOOL`。End の引数は `HRESULT`, `Ref<IBindCtx>`, `u32`。
- `IDataObject_Impl` の9メソッドも存在する。既存
  `native_context_menu.rs::ClipboardDataObject` に委譲実装の先例があるが、
  クリップボード固有の追加形式・cut 完了処理は持ち込まない。
- **重要:** この版の `#[implement]` は既定で agile (`IAgileObject` と FTM の
  `IMarshal` を自動提供)。STA に属する Shell 実体を無断で別スレッドから呼ばないため、
  **`#[implement(IDataObject, IDataObjectAsyncCapability, Agile = false)]`** を使う。
  `windows-implement-0.60.2/src/lib.rs` / `gen.rs` で、この指定により両者の自動公開を
  抑止できることを確認した。`#[implement(...)]` だけで STA が保たれるとは考えない。

### 12.3 COM の identity・スレッドと委譲

ラッパーは UI スレッドの STA で生成し、Shell `IDataObject` を強参照で所有する。
`IDataObject` と async interface の QI は同じ外側の `IUnknown` identity に戻す。
未知の IID を無差別に inner の QI へ横流ししない (identity の分裂や async interface を
迂回した呼び出しを防ぐ)。独自の FTM / `IAgileObject` / `IMarshal` は実装せず、
Windows の標準 COM marshaling を利用する。`Send` / `Sync` の unsafe 追加もしない。

Explorer が worker から正しく marshal した interface を呼ぶと、外側への
`GetData` / `SetData` / `StartOperation` / `EndOperation` 等は **mIV の UI STA** で
実行され、同じ STA 上の inner へ委譲される。`InOperation` は通常 mIV が
`SHDoDragDrop` 復帰直後に問い合わせる。target が他スレッドから問い合わせる場合も同じ
marshaling 境界を通る。非同期というのは **target のコピー処理**を別スレッドへ移す契約であり、
source の `GetData` が worker 上で実行されるという意味ではない。

`winit 0.30.13` の `platform_impl/windows/window.rs::create_window_data` は
`OleInitialize` + `RegisterDragDrop` を実施する。復帰後の
`event_loop.rs::dispatch_peeked_messages` は HWND フィルターなしの
`PeekMessageW(..., 0, ..., PM_REMOVE)` → `DispatchMessageW`、待機は
`MsgWaitForMultipleObjectsEx`。従って COM の隠し STA window を配送できる構造である。
これはソースによる根拠であり、mIV + Explorer での復帰後 callback 到達はまだ未実測。
自前の message pump、追加 repaint の反復、UI 上の完了待ち、COM 初期化の重ね掛けは足さない。

委譲は既存の形式列挙、`CF_HDROP` / Shell IDList、失敗 HRESULT、
`STGMEDIUM` と `fRelease` の所有権を保つ。成功 HRESULT の保証範囲は下表に限定する。
windows 0.61.3 の生成 vtable は `Result<T>` 型の実装成功を `S_OK` にするため、
「すべての成功コードをそのまま保つ」とはしない。

| メソッド | 必要な成功コードと委譲契約 |
| --- | --- |
| QueryGetData / GetCanonicalFormatEtc | 生の HRESULT を保持。`S_FALSE` / `DATA_S_SAMEFORMATETC` も丸めず試験する |
| GetData / GetDataHere / SetData | 有効な出力・所有権を伴う成功を `S_OK` として返す。失敗コードは保持 |
| EnumFormatEtc / EnumDAdvise | 成功時は inner の列挙 interface をそのまま返し `S_OK`。列挙子自身の Next の `S_FALSE` はラップせず保持 |
| DAdvise / DUnadvise | 成功時の接続番号・解除を保ち `S_OK`。失敗コードは保持 |

データを事前コピーしたり、全ファイルの
内容をメモリへ読み込んだりしない。`SetData` が inner で未対応でも成功へ変換しない。
外側の COM メソッドは App / egui / viewer を参照せず、追加の I/O は行わない。
inner 呼び出しが STA を再入し得るため、状態の `RefCell` borrow を委譲や COM Release、
ログ出力をまたいで保持しない。

### 12.4 寿命と async 状態の単一 owner

| 資源 | 所有者・解放点 |
| --- | --- |
| `PendingNativeDrag.paths` | 呼び出し時の値。Shell object 作成後は借用し続けない。App の items / idx / checked を COM に持たせない |
| `SHParseDisplayName` が割り当てた PIDL | 現行どおり配列作成後に `CoTaskMemFree`。失敗経路でも解放。ラッパーへ raw pointer を保存しない |
| `IShellItemArray` | data object 取得までのローカル強参照。以降の Shell 項目・形式の保持は返された inner の責任。追加の App 所有配列は作らない |
| Shell `IDataObject` | 外側のフィールドが強参照で保持。外側の最終 Release で解放 |
| 外側 data object | ドラッグ呼び出し側のローカル強参照 + target / COM proxy の強参照 + 下記の一時的な自己保持参照 |
| 元ファイルの内容 | mIV は固定・ロック・複製しない。Shell が渡す識別子と実ファイルの存在期間は別 |

[SetAsyncMode の契約](https://learn.microsoft.com/en-us/windows/win32/api/shldisp/nf-shldisp-idataobjectasynccapability-setasyncmode)
にある AddRef と
[EndOperation の解放](https://learn.microsoft.com/en-us/windows/win32/api/shldisp/nf-shldisp-idataobjectasynccapability-endoperation)
を扱う。独立した bool / pending を積まず、外側だけに次の型付き状態を持つ (名称は仮):

| 状態 | 意味・保持 |
| --- | --- |
| `Disabled` | 初期 / 無効化済み。GetAsyncMode=FALSE、InOperation=FALSE、自己保持なし |
| `Armed { keep_alive }` | SetAsyncMode(TRUE) 済み。Get=TRUE、In=FALSE、自己への強参照を1個保持 |
| `Running { keep_alive }` | target が StartOperation 済み。Get=TRUE、In=TRUE、同じ保持参照を移譲 |
| `Finished` | EndOperation または検証済み Performed DropEffect で完了。Get=TRUE (設定値を維持)、In=FALSE、自己保持なし |

- 1ドラッグにつき1 object。`SetAsyncMode(TRUE)` は自己参照を1回だけ取得し、
  `Armed` にする。`StartOperation` は `Running` へ移し、追加 AddRef は不要。
- `EndOperation(hr, NULL, effects)` または `SetData(CFSTR_PERFORMEDDROPEFFECT)` は、
  Running の状態を `Finished` にしてから、保持参照を取り出して
  borrow の外で解放する。呼び出し中の object が先に破棄されないよう、呼び出し側の参照と
  ローカル強参照の順序を保証する。成功・コピー中止・失敗とも同じ寿命終端を使う。
  Performed 通知では SDK に従い S_OK と入力 DWORD を診断値にする。Logical 通知は完了に使わない。
- `SHDoDragDrop` が戻ったら、ローカル参照を手放す前に **InOperation** を確認する。
  TRUE なら `Running` の自己保持を残す。FALSE で `Armed` のままなら
  外側の専用後始末で `Disabled` へ解除し、自己保持を解放する。Esc / 無効 target / 非同期非対応の
  target は Start / End を呼ばないため、ここを省くと自己循環が漏れる。
  `Finished` なら End 済みなので二重解放しない。SHDoDragDrop の HRESULT が失敗でも
  同じ後始末を行い、**Running を一律リセットしない**。
- `SetAsyncMode(FALSE)` は実行前の解除に使う。Running の途中に解除して参照を切らない。
  重複 TRUE / Start / End は保持数を増減させず、Finished への End / Performed 重複は成功とする。
  未開始 Armed / Disabled への Performed は委譲だけ行い状態を変えず、その他の不適合な
  async 呼び出しはエラーとして診断する。HRESULT をユニットテストに固定する。再利用・複数並行 operation を
  同じ object に積む契約は作らない。
- 別のドラッグを始めても前の object を置換・キャンセルしない。各ドラッグが独立した
  Shell snapshot と寿命を持つ。App に「現在の非同期ドラッグ」slot やコピー待ち一覧は不要。
  target が異常終了して End を呼ばない場合の再試行・timeout 回収は設けない。

**利用者決定 (2026-10-19): コピー中に mIV を終了した場合、コピーが最後まで続くことは
保証しない。途中で止まり得るが、COPY 限定なので元ファイルは消えず、やり直せばよい。**
終了時の待機・確認・回復機構は足さない。COM 参照はプロセスを存続させない。
これは通常の終了でも到達する採択済み仕様境界であり、「まれな失敗」には分類しない。
実機ではコピー結果を問わず、mIV の終了が完了してハングせず、Explorer が固まらないことを確認する。

終了経路のソース確認: winit の `WM_DESTROY` の `RevokeDragDrop(window)` は、その HWND の
**受け取り用 IDropTarget の登録解除**であり、送出済み data object の切断や EndOperation 待ちではない。
eframe の `wgpu_integration::save_and_destroy` → `App::on_exit` / `on_exit_inner` / App Drop は
送出 object を保持・join しない。`src/lib.rs` の `run_native` 復帰後は通常の message pump が
止まり、Remote / collection / book composition / spread writer とログ等の終了処理を行う。
今回の object はこれらへの参照・完了チャネル・終了 hook を持たず、EndOperation を待たない。
従って外部 COM client の呼び出し完了を mIV 側の終了条件にする依存は追加しない。
終了処理中の外部 callback は配送されない可能性があり、Explorer の実挙動は上記実機確認に残す。

フォルダ移動・閲覧先切替は、App の items や選択の変更だけなら data object を失効させない。
一方、コピー中に元ファイルを mIV / 他アプリから移動・改名・削除・書き換えた場合は、
**Explorer が扱う通常のファイル競合・コピー失敗**とする。スナップショットはファイル内容を
凍結しないため、未読分が失敗したり、既に開いている handle の共有規則で元の操作が
失敗したりし得る。mIV の操作を追加で禁止せず、元に戻す・再コピーする仕組みも作らない。

### 12.5 既存入力・effect・完了通知

- **mIV 自身への drop:** winit の `drop_handler.rs` は `CF_HDROP` を同期取得し
  `DroppedFile` を送るだけで、async interface に QI / Start しない。従ってここは同期経路のまま。
  `outcome.started` によるポインタ復旧、`outcome.dropped && cursor_over_window(hwnd)` による
  `internal_drop_pending` の設定、`input_drop_and_activity` の `dropped_files` 到着時の消費を
  維持する。EndOperation を起点にこのフラグを変更しない。
- **1件:** 件数しきい値で経路を分けず同じラッパーを使う。target が非同期を選ばなくても正常。
- **Esc / 無効な場所で解放:** 通常は effect=NONE / In=FALSE。自己保持を解除し、従来の
  ポインタ復旧だけ行う。コピー完了待ちや新たな通知は作らない。
- **非同期非対応のアプリ:** GetAsyncMode / Start を使わなければ従来どおり同期。
  対応 target でも非同期を選ぶ義務はない。UI を強制的に先へ進める回避策は入れない。
- **effect:** 非同期時の `SHDoDragDrop` 復帰は drop の引き渡しであり、コピー完了ではない。
  `DragOutcome.dropped` は引き続き「drop 受理」の判定。`EndOperation` の結果から
  `dropped` / `internal_drop_pending` を後で書き換えない。現行トーストも対象除外・開始失敗の
  通知だけで、コピー成功を宣言していない。
- **EndOperation の結果:** hr / effects と operation の終端は診断ログに残すが、App の
  選択解除、reload、原本削除、DB 書き込み、成功トーストには利用しない。コピー途中の
  エラー・同名確認・キャンセル UI は実行主体の Explorer が担う。COM 外側の開始設定等の
  失敗は黙って同期へ切り替えず、既存のドラッグ失敗通知へ通す。
- **MOVE:** 現在は許可しないので、本修正でも COPY 限定を維持する。仮に将来 MOVE を
  許す場合、非同期 optimized move の移動・削除は受け取り側が行い、復帰時 effect を根拠に
  mIV が原本を削除してはいけない。非最適化 move / delete-on-paste は別の source 側責務が
  あり得るため、「非同期 MOVE は常に target が削除する」と全般化しない。最終結果は
  EndOperation / performed-effect 等の契約で扱う別設計が必要で、今回は追加しない。

まれな失敗は既定で割り切る (ログ + 通知 [+ 作り直し])。本件ではコピー側の失敗通知は
Explorer の責務。例外の多段回復・完了待ちを追加しない。

### 12.6 段2の自動テストと検証所有

段1のプローブは Shell 単体の局所観測であり、以下の合格証拠を代替しない。
段2の実装・自動テスト結果は §12.10 に記録する。

Windows COM ユニット / headless integration で確認する:

1. 同じ外側 identity から IDataObject / async interface へ往復 QI できること。
   `IAgileObject` / 独自 `IMarshal` を公開しないこと。既定 agile の取り忘れを検出する。
2. Set / Get / Start / In / End と、復帰直後の同期終了用後始末。Esc 相当、非対応 target 相当、
   Start 後に `SHDoDragDrop` が失敗する相当、End が復帰前に届く相当を状態遷移で検査する。
   mock inner の Drop カウンターで、ローカル参照解放後の Running 中は生存し、End / 未開始解除後の
   最終 Release で1回だけ破棄することを検証する。ドラッグ A の終了が B に影響しないことも確認する。
3. mock inner へ9メソッドの引数、列挙 interface、`STGMEDIUM` / `fRelease` が
   そのまま届くこと。HRESULT は §12.3 のメソッド別契約で確認する。mock inner から
   外側の InOperation / EndOperation へ再入できること、InOperation=TRUE なら復帰時の
   effect / HRESULT によらず寿命を維持することも検査する。
4. disposable STA と message pump、別 apartment の client を用意し、標準 COM marshaling 経由の
   GetData / EndOperation が **作成 STA の thread ID** で実行されること。client の完了待ちは UI STA を
   block しない。テスト用 pump は製品に追加しない。製品画面や実ユーザーのファイルは使わない。
5. 一時ファイルの実 Shell data object でも、生成元の Vec / PIDL / 配列の解放後に
   CF_HDROP / Shell IDList と形式列挙を取得できること。mock の成功だけで Shell の寿命を断定しない。
6. 既存の `decide_drag_payload`、`shell_parse_path_tests`、drop 受け取り、自己 drop、
   ドラッグ結果通知に関連するテストを狭い filter で再利用・必要に応じ補強する。
   見た目は変更しないため snapshot 更新は予定しない。

実装担当が targeted nextest / focused tests、`cargo fmt` と通常 core の check、
`scripts/build-dev.ps1` を担当する。本体 lib 全件は ClaudeCode の最終 gate で行い、
同じ source の有効な結果を再利用する。段1では文書だけを変更し、製品ビルドは行わなかった。

### 12.7 実機で確認する事項 (未実施)

| シナリオ | 期待・観測点 |
| --- | --- |
| disposable な数千件を Explorer / desktop へ drop | コピー継続中に SHDoDragDrop が復帰し、mIV のスクロール・選択・フォルダ閲覧が動く。コピー件数・内容も照合 |
| 同名ファイル確認 / Explorer でコピーをキャンセル | Explorer の標準 UI が動き、mIV が結果待ちにならず、completed の via / effect と object released を確認できる。marker の S_OK をコピー成功・中止の判定に使わない |
| コピー中に閲覧フォルダ変更、続けて別ドラッグ | 古い object の GetData / End が有効で、新しいドラッグや viewer の状態に干渉しない |
| disposable 原本の改名・移動・削除 | Explorer / ファイルシステムの通常の競合処理となり、mIV が別ファイルを代用したり復元したりしない |
| コピー中の mIV 終了 | mIV の終了が完了する (ハングしない)、Explorer が固まらない。コピー結果は問わない (§12.4 の利用者決定) |
| 1件・単体フォルダ・日本語 / UNC パス | 従来の送出結果・カーソル・drag image が維持される。非同期選択の有無は件数から決め付けない |
| mIV 自身へ drop / Esc / 無効 target | 跳ね返りコピーをしない、クリック等が固着しない、自己保持が漏れない |
| 非同期非対応の画像ソフト等 | 従来の CF_HDROP 受け取りが成立し、同期処理であっても破綻しない |

診断にはドラッグごとの識別子、生成 STA thread ID、Set / Start / End、
`SHDoDragDrop` 復帰時の effect / InOperation、破棄を相関できる記録を使う。
通常ログを全 GetData 呼び出しで埋めず、必要な thread 到達は自動テスト・限定診断で確認する。
Explorer の内部コピー手順は外部から保証できないので、TRUE 公開だけで修正完了とはしない。

製品バイナリはエージェントから起動しない。実装後は build-dev の起動コマンドと通常プロファイルへの
注意を利用者へ渡す。エージェント主導の UI 試験が必要なら、別途具体的な suite の明示承認後に
`prepare-portable-smoke.ps1` で作成する disposable portable だけを使う。

### 12.8 文書への反映内容

**§6.2 / `ui-responsiveness.md` §4.2 に反映する共通の表現**:

> ドラッグ操作の OLE モーダル区間は残る。非同期を選択して Drop から早期復帰する target では、
> その後の転送完了を待たない。非対応 target の同期処理と開始前の Shell 解決コストも残る。

段2で `src/file_drag.rs` 冒頭の「モーダルブロック不可避」コメント、§4.2 の
「自前 COM ゼロ」、§5.1 の復帰結果説明を更新した。§5.5 の歴史的コード例の最新の実行箇所・
復帰結果の扱いは §12.1 / §12.5 を参照する。マニュアルの D&D 説明も実装した範囲へ更新し、
非対応 target や非同期を選ばない target まで応答改善を保証する表現は使わない。

### 12.9 根拠・レビューへ渡す未確定点

- Microsoft: [Dragging and Dropping Shell Objects Asynchronously](https://learn.microsoft.com/en-us/windows/win32/shell/datascenarios#dragging-and-dropping-shell-objects-asynchronously)。
  source / target の交渉順序と、非同期時は復帰と転送完了が異なることの根拠。
- Microsoft: [IDataObjectAsyncCapability](https://learn.microsoft.com/en-us/windows/win32/api/shldisp/nn-shldisp-idataobjectasynccapability)、
  [StartOperation](https://learn.microsoft.com/en-us/windows/win32/api/shldisp/nf-shldisp-idataobjectasynccapability-startoperation)、
  [InOperation](https://learn.microsoft.com/en-us/windows/win32/api/shldisp/nf-shldisp-idataobjectasynccapability-inoperation)。
- Microsoft: [Single-Threaded Apartments](https://learn.microsoft.com/en-us/windows/win32/com/single-threaded-apartments)。
  STA の message loop・marshaling・再入の根拠。winit で配送可能という判断は上記ソースからの推論で、
  Explorer との実測結果ではない。

独立レビューでは、特に **Agile=false + 標準 marshaling**、SetAsyncMode の自己保持と
未開始経路の解除、元ファイル変更・通常終了の仕様境界、COPY 限定と identity の保持を確認する。
COM 跨ぎ自動試験の実績は §12.10、Explorer 実機での復帰後 callback / 応答改善は未実施。
重要な構造判断が変わったら本節を改訂し、合意前に実装へ進まない。

### 12.10 段2の実装・検証記録 (2026-10-20)

`src/file_drag/async_data_object.rs` に送出専用 owner を実装し、`start_file_drag` だけで
Shell object を包んだ。共有 `shell_data_object_for_paths` / clipboard / App の完了処理は
変更していない。復帰時に `finish_modal` が未開始の自己保持だけを解除する。
`Running` は modal の effect / HRESULT に関係なく End まで保持し、End 済みの
`Finished` は再解除しない。型付き状態以外の pending / App callback / 終了待機は追加しない。
通常ログに drag_id、async 有効化・Start / End の thread ID、復帰時 effect / InOperation、
SHDoDragDrop の elapsed_ms、最終 Release を記録する。

独立した Codex (`gpt-6.1-sol` / `xhigh`) の実装レビューは P1/P2なしで合意。
再入時の borrow 境界、自己保持の解放、ドラッグ間分離、終了経路の依存なしを確認した。
試験の borrowed callback は正規の owning COM 参照へ昇格してから呼ぶ形へ修正し、
DATA_S_SAMEFORMATETC の成功コードと、marshal 試験の valid HGLOBAL / 解放経路も確認した。
レビュー担当は Cargo / UI を再実行せず、以下の実装担当の結果を再利用する。

対象は本 worktree の未コミット差分。コード3ファイルの SHA-256 は
`target/dragcache-source-hashes.txt`。テスト後の全体 `cargo fmt` で同じ3ファイルに変更が
ないことも照合済み。新規ソースは `src/file_drag/async_data_object.rs` と
`src/file_drag/async_data_object/tests.rs` の2件。

| 自動検証 | 結果・証跡 |
| --- | --- |
| `cargo check -j 1 -p mimageviewer --bin mimageviewer-core` | exit 0。`target/dragcache-check-serial.log` |
| `cargo nextest run -j 1 -p mimageviewer --lib -E 'test(/file_drag::/) \| test(/decide_drag_payload/) \| test(/drag_source_path/) \| test(/native_grid_drag_guard/)'` | exit 0、35件成功 / 失敗0 / skipped 13,815。run ID `ff3152ad-84fb-44b2-81e1-17964916c759`、`target/dragcache-focused.log` |
| 追加 COM 試験 (上記35件中10件) | identity / Agile=false、状態・無効遷移・重複設定、未開始解除、End早着、Running自己保持とA/B分離、9委譲・HRESULT、medium/fRelease、inner→外側の再入、実Shell snapshot、STA↔MTA標準marshalを成功確認 |
| `cargo fmt` / `cargo fmt --check` / `git diff --check` | exit 0。source hash不変を確認 |
| `python scripts/check_ui_glyphs.py` | exit 0、危険グリフ0件。新規 UI 文字列・見た目の変更はなく、snapshot PNG の更新なし |
| `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | exit 0。通常feature set・非portable、core / remote / EPUB worker を生成。`target/dragcache-build-dev.log`。成果物は起動していない |
| 本体lib全件 / 実機 | 本体lib全件は依頼どおり ClaudeCode の最終gateへ残す。Explorer実機・製品起動は未実施 |

環境条件: `MSBUILDDISABLENODEREUSE=1`。最初の並列 native build は turbojpeg-sys の
MSBuild 段で exit 1 (Rust 製品コードの失敗ではない)。同じ出力先の CMake install を
`--parallel 1` で成功させ、上記 `cargo ... -j 1` を成功確認した。
確認用ビルドも `CARGO_BUILD_JOBS=1` を使う。通常の待機付き build-dev は残存 MSBuild node を
待ったため、その待機だけを中止し、`-PreserveRuntime -WaitForOtherBuildsMinutes 0` で実行した。
他 worktree のプロセスを停止・変更していない。ビルド設定や依存の製品変更は行わない。

§12.7 の実機シナリオ (大量drop中の操作、同名確認・Explorerキャンセル、フォルダ遷移・連続drag、
原本変更、コピー中の終了、1件等、自己drop・Esc・非同期非対応target) は実機で残る。
特に終了確認は「mIV の終了が完了し、Explorer が固まらない」、コピー結果は不問という
利用者決定で確認する。自動 COM 試験成功を Explorer の応答改善の実測結果とはしない。

### 12.11 EndOperation 未到着の調査と Performed DropEffect による完了 (2026-10-20)

利用者の Windows 11 10.0.26300 / Explorer 実機では、コピー中の操作、同名確認、
キャンセル、終了は成立した。一方、HEAD `3a8035830` のログでは非同期の drag_id
1 / 2 / 4 / 5 に Start と modal 復帰時の InOperation=TRUE があり、End / object released
がなかった。自己 drop の drag_id=3 は未開始解除後に解放された。**通常コピー終了後にも
Running の自己保持が残る問題**があり、§12.10 の自動試験成功では否定できなかった。
以下の初回調査では保留し、その後の ClaudeCode によるサンドボックス外観測で最小修正を採用した。

#### 初回調査の通知経路候補

Microsoft の [ShlDisp.idl の実装 checklist](https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/um/shldisp.idl#L984-L1003)
には、`IDataObject::SetData(CFSTR_PERFORMEDDROPEFFECT)` を受けた object が、HGLOBAL の
DWORD を effect として `EndOperation(S_OK, NULL, effect)` に接続する記載がある。
ローカル Windows SDK 10.0.26100.0 `um/ShlDisp.idl` の1079–1080行でも照合した。
調査時点の外側 SetData は inner へ委譲するだけなので、この通知が届いている場合は、
**通知の受け取りと Running の所有者が分離している**ことが原因候補になる。
初回の実機ログには SetData の記録がなく、下記の制約により完了通知の観測点に到達できず、
この時点では因果を確定できなかった。SDK のコメントには現行シグネチャにない paocb の痕跡もあり、
コメントだけを実 Explorer の動作の証拠にはしない。

marker が実測できた場合の最小修正候補は、現在の自己保持を維持したまま、直接の End と
validated Performed DropEffect を共通の Running → Finished 遷移に集めること。
入力 DWORD は inner.SetData の**前**に検証・取得し、fRelease による解放後には読まない。
呼び出し中の強参照を委譲前から完了処理後まで保持し、再入後の現在状態を確認する。
inner の HRESULT / medium 所有権は保ち、inner の保存失敗と外側の通知受領を区別する。
Finished への重複通知は冪等、未開始 Armed を marker だけで完了させない。
ログでは実 End 受信と marker による完了を区別し、marker からコピー成功を宣言しない。
この案への独立 Codex (`gpt-6.1-sol` / `xhigh`) の合意は **marker 到着の実測を条件とする**。

自己保持の単純削除は SetAsyncMode の AddRef 契約を変える。最後の外部 Release を検出する
別 owner は `#[implement]` の Release 境界を含む別設計になる。いずれも現時点では採用しない。
timer 回収、modal 復帰時の Running 強制解除、終了時一括解放は追加しない。

#### 追加した隔離プローブ

`src/file_drag/async_data_object/tests/shell_drop_probe.rs` の ignored tests は、
製品ではなく **lib test executable** の子プロセスを使う。通常の lib 全件には含まれない。
temp に source / destination を作り、8件 × 128 bytes の重複しないファイルを使う。
Shell target を取得し、DragEnter / DragOver / Drop を呼び、STA でメッセージを回す。
コピー全件の内容一致を5秒以内で確認し、その後2秒の配送観測区間を置く。
親は自分が生成した子だけを12秒目安で監視する。別プロセスの場合は親 STA に配送された
callback 内でブロックすると期限の確認自体が延びるため、suite 全体の絶対上限ではない。
同一プロセス試験の親は source を持たず、実 Drop の外側で子を監視する。
これらは**試験の観測・待機期限**であり、製品の寿命管理・回収条件ではない。
Explorer のウィンドウ・入力・実ファイルを操作しない。
Shell が読み込む登録済み拡張まで独自実装へ置き換える試験ではない。

- 同一プロセス: 実 Shell snapshot と実ラッパーを、同一 STA 内の Shell target に渡す。
- 別プロセス: IDataObject を標準 COM marshal し、子で QI した async interface と target を使う。
  親の STA は pump を継続し、子終了後に marshal-table の保持も対で解放する。
- 配送 control: 別プロセスから明示的に Start / CF_HDROP GetData / End を呼ぶ。
  Shell target の実通知とは区別して synthetic と記録し、実装 STA の thread ID、
  InOperation=FALSE、marshal 解放後にローカル2参照だけが残ることを検査する。

cfg(test) の観測 hook は有効化したプローブだけが記録する。Start / 実 End 入口 / End 処理 / GetData /
最終 Release の thread、SetData の全 format 名・DWORD・fRelease、target の HRESULT / effect /
時間、コピー内容一致、ローカル破棄前の参照数を stderr に残す。mIV の logger は初期化せず、
通常プロファイルへログを書かない。修正前は Running が残った場合も、先に実観測とローカル破棄後の
記録を出し、その後で **synthetic EndOperation と明示した試験終了処理**を行う。
その End / Release を Shell 自身による完了の証拠に混ぜなかった。
修正後のプローブではこの cleanup End 自体を削除し、通知による完了・解放を必須にした (下記)。

#### 初回に実測できた範囲と停止理由

`BHID_SFUIObject` (GetUIObjectOf) と `BHID_SFViewObject` (CreateViewObject) の両経路で、
同一 / 別プロセスとも DragEnter / DragOver は S_OK、COPY を返した。しかし Drop 内で
`PowerToys.ImageResizerExt.dll` からの C++ 例外が Rust test harness まで漏れ、子が abort した。
**StartOperation 前、コピー内容一致前であり、End / Performed DropEffect の判定点には未到達**。
左ボタン指定を Drop にも渡して再実行したが同じ結果だった。

CDB の first-chance native stack と exception object のメモリから、型
`std::filesystem::filesystem_error`、error code 5、メッセージ
`create_directories: Access is denied.`、対象ディレクトリ
`%LOCALAPPDATA%\Microsoft\PowerToys\ImageResizer` を確認した。
これは本環境での試験制約であり、利用者の「Start 後に End がない」原因や PowerToys 自体の
不具合とは断定しない。権限境界の迂回、拡張の無効化、レジストリ・PowerToys 設定の変更、
製品側への例外抑止は行わない。証跡は `target/dragcache-shell-probe-sfui-failed.log`、
`target/dragcache-shell-probe-sfview-failed.log`、`target/dragcache-probe-cdb*.log`。

別プロセスからの synthetic control と通常の焦点テストの結果は下記に記録する。
独立レビューも、上記の観測不能な状態から SDK bridge を先行実装する根拠は増えていないと判断した。
初回は **製品の所有・完了処理の変更を保留**し、実 Shell の通知を観測できる環境での結果を次判断へ渡した。
この時点では cfg(test) だけの診断追加だったため、製品の見た目・マニュアル・確認用製品バイナリは変更しなかった。

| 初回調査差分の検証 (修正前) | 結果・証跡 |
| --- | --- |
| opt-in Shell probes + cross-process control | exit 100、control 1件成功 / 実 Shell 2件は上記例外で失敗。run ID `48191290-7565-42c5-ac50-f23c6c96e36a`、`target/dragcache-shell-probe-final.log` |
| control の到達・解放 | client thread=88092、Start / GetData / End入口 / End処理はすべて owner STA=51196。marshal 解放後 In=FALSE / source_refs=2、ローカル破棄で released=1 を確認。cleanup End は使っていない。これは子が明示送信した通知であり、実 Shell の通知ではない |
| 既存 COM / 送出の焦点テスト | 35件成功、失敗0。§12.10 と同じ filter。run ID `92493f6b-c861-49bb-b31f-a25c5548cee3`、`target/dragcache-investigation-focused.log` |
| 全体 `cargo fmt` / `cargo fmt --check` / `git diff --check` | exit 0 |
| `python scripts/check_ui_glyphs.py` | exit 0、危険グリフ0。`target/dragcache-investigation-glyphs.log` |
| 製品ビルド / 本体lib全件 / 製品起動 | cfg(test) と文書だけの調査差分なので build-dev は省略。本体lib全件は ClaudeCode の最終 gate。製品を起動していない |

初回調査 Rust source の hash は `target/dragcache-investigation-source-hashes.json`。
独立レビューの限定指摘 (親 STA callback 中は12秒監視が延びること、有限の End 観測区間と
子終了による COM rundown の区別) は上記に反映した。unsafe / marshal の対 / cleanup には
追加の重大指摘なし。デバッガ用 temp fixture 1件の削除はコマンドの自動承認レビューが
`blocked by policy` として拒否したため残っている。場所は `target/dragcache-probe-root.txt`。
通常の tempfile ベースのプローブは各親の終了時に一時フォルダを片付けている。

#### サンドボックス外での実 Shell 観測と採用した修正

ClaudeCode が通常の PowerShell から下記の調査コマンドを実行し、
`target/dragcache-shell-probe-user.log` に2件 PASS を記録した。
修正前の記録は `target/dragcache-shell-probe-user-baseline.log` にも保全した。
**同一 / 別プロセスとも Start 後の完了通知は Performed DropEffect で、直接の End はなかった**。

| 順序 | 両プローブの観測 |
| --- | --- |
| DragEnter / DragOver | DropDescription / TargetCLSID 等の SetData |
| Start 直後 | Logical Performed DropEffect=1、fRelease=TRUE。コピー内容はまだ一致していない |
| 約200ms | コピー全8件の内容一致 |
| その後 | Logical Performed DropEffect=1、続けて Performed DropEffect=1、いずれも fRelease=TRUE |
| 観測区間終了 | copies_equal=TRUE、Running=TRUE、source_refs=3、実 End / released なし |
| 旧プローブの synthetic cleanup End | end_entered / end / released が初めて出る |

これにより、Shell の実通知は作成元 STA に届いており、**外側が Performed 通知を inner に
委譲するだけで Running の自己保持を解放していなかった**ことを確認した。
標準 marshal の修正や自己保持契約の変更は不要。独立設計レビューの実測条件が成立し、
ClaudeCode / 利用者指示に基づいて上記の最小修正を採用した。

- `complete_running` が直接 End と検証済み Performed の唯一の Running → Finished 遷移を担う。
  状態を置換して borrow を閉じてから自己保持を解放する。Finished の重複は追加処理せず、
  Armed / Disabled は marker で変更しない。App への完了通知・終了待機は追加しない。
- constructor で Performed format を登録する。SetData はその format、CONTENT、lindex=-1、
  HGLOBAL を含む FORMATETC、実 HGLOBAL medium、DWORD 以上の GlobalSize と GlobalLock 成功を検証する。
  Logical Performed / Paste Succeeded / 他形式は完了に使わない。
- SetData の入口から強参照を保持し、DWORD を読み終えて unlock してから inner を呼ぶ。
  委譲後の input / HGLOBAL は読まない。inner のエラーも含む HRESULT と fRelease の所有規則を維持し、
  完了通知の受領は inner の保存成否と分離する。再入後の現在状態が Running の場合だけ完了させる。
  windows-rs の成功 HRESULT 正規化は §12.3 の既存契約どおり。
- 製品ログは実 End 受信を `async EndOperation received`、1回だけの完了処理を
  `async completed ... via=end_operation` / `via=performed_drop_effect`、破棄を
  `object released` と区別する。marker の result=S_OK は SDK の値で、コピー成功通知には使わない。
- 通常の mock 単体を追加し、実 HGLOBAL が inner 内で解放される fRelease=TRUE、FALSE、
  inner の S_OK / S_FALSE / E_NOTIMPL、effect=NONE / COPY / MOVE の全てで同じ寿命処理を確認する。
  Logical 単独、未開始・無効状態、両順序の重複、inner からの End / marker / disable 再入、
  不正入力、ドラッグ間分離も検査する。MOVE は診断値の単体試験だけで、製品の COPY 限定は維持する。
- ignored probes は synthetic cleanup End を削除した。Start があり、完了が1回、InOperation=FALSE、
  ソース側ローカル参照を手放して released=1 を要求する。別プロセスでは marshal-table 保持も
  対で解除した後に検査する。control の明示 End は synthetic と表示する別の配送検査のまま。

初回の残りデバッガ temp fixture は ClaudeCode が削除するという利用者指示に従い、今回は触らない。
見た目・利用操作・公開マニュアルの記述範囲は変わらず、snapshot PNG は更新しない。

#### 修正差分の検証記録

| 検証 | 結果・証跡 |
| --- | --- |
| 独立 Codex (`gpt-6.1-sol` / `xhigh`) の限定レビュー | P1/P2 指摘なし。事前 DWORD 取得、call_ref、再入後の状態確認、一度だけの解放、HRESULT / medium 所有権、追加7単体、cleanup End を除いた probe の検査を確認。レビュー担当は Cargo / UI を再実行していない |
| 最終整形後の焦点テスト | 42件成功、失敗0。§12.10 と同じ filter。run ID `91c5aaf9-502c-4505-af1f-b68f46128be4`、`target/dragcache-completion-focused.log` |
| 別プロセス配送 control | 1件成功。client=83188 から Start / GetData / End が owner STA=67552 に配送、In=FALSE / source_refs=2、cleanup End なしで released=1。run ID `21a354b5-fd27-44c2-b35c-35196718d8de`、`target/dragcache-completion-control.log` |
| 全体 `cargo fmt` / `cargo fmt --check` / `git diff --check` | exit 0。最終 Rust source の SHA-256 は `target/dragcache-completion-source-hashes.json` |
| `python scripts/check_ui_glyphs.py` | exit 0、危険グリフ0。`target/dragcache-completion-glyphs.log` |
| build-dev | exit 0、core / remote / EPUB worker を通常 feature・非 portable で生成。core は 3m39s、VCRT/PE 検査も成功。`MSBUILDDISABLENODEREUSE=1` / `CARGO_BUILD_JOBS=1`、`-PreserveRuntime -WaitForOtherBuildsMinutes 0`。`target/dragcache-completion-build-dev.log`。製品は起動していない |
| 修正後の実 Shell 2件 / Explorer 製品実機 | 未実施。前回同環境で確定した Start 前の拡張 DLL 例外の制約を再利用し、修正後 probe は ClaudeCode のサンドボックス外実行へ残す。修正前の外部 probe 成功を修正後の成功とは扱わない |
| 本体lib全件 / commit / 製品起動 | 本体lib全件は ClaudeCode の最終 gate。利用者指示どおり commit と製品起動は行わない |

#### 修正後の最小確認

通常の PowerShell で、同じ worktree から次の headless probe を1回実行する。
製品の起動・Explorer の画面操作は不要で、コピー対象はプローブが作る temp だけ。

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
cargo nextest run -j 1 -p mimageviewer --lib --run-ignored only -E 'test(/shell_drop_probe_(same|cross)_process$/)' --success-output immediate --failure-output immediate *> target/dragcache-shell-probe-user.log
```

`probe byte comparison completed=true` / `copies_equal=true` を確認し、同じ drag_id の
`start` → `SetData` (`Performed DropEffect`) → `end` → `released` を追う。
marker による完了なら `end_entered` は不要で、実 End 受信とは区別する。
`probe observation` が `running=false source_refs=2`、
`after source locals dropped (no cleanup End)` に `released` が1回現れ、2件 PASS することが基準。
synthetic cleanup End は実行しない。2秒は有限の観測区間で、Shell metadata 全体の完了は保証しない。
別プロセスの参照数は子終了による COM rundown 後の値なので、稼働中の target が自然に
最終参照を離した証拠とは区別する。
この probe は実 Shell folder target の観測であり、Explorer の window / site / OLE loop と同一とは
保証しない。製品では利用者の既存隔離データで数千件を1回コピーし、同じ drag_id の
`async StartOperation`、`SHDoDragDrop done ... in_operation=Some(true)`、
`async completed ... via=performed_drop_effect`、`object released` がそろうことを確認する。
小さいコピーで完了が早ければ、completed が modal 復帰より前 / InOperation=FALSE でも正常。
実 End を呼ぶ target では `async EndOperation received` と `completed ... via=end_operation` が対応する。
コピー中の mIV 操作、同名確認、Explorer キャンセル、自己 drop / Esc / 1件の既存挙動も確認し、
自己 drop / Esc の未開始終了には completed がなくても released があればよい。
コピー中終了は従来の利用者決定どおり、終了完了と Explorer がハングしないことを確認し、
プロセス終了までに未完了の drag_id の released がなくても転送完遂を保証する設計には変えない。
