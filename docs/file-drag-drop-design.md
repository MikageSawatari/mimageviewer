# ファイル D&D (ドラッグでコピー送出) 実装設計

グリッドのサムネイルを掴んで、エクスプローラや他アプリへ **ドラッグ＆ドロップでファイルを
コピー** できるようにする機能の設計ドキュメント。

**状態**: 実装済み (2026-05-22)。`src/file_drag.rs` 新規 + `grid_item.rs` /
`app.rs` / `ui_main.rs` 改修 + ユニットテスト 8 件。`cargo check` / `cargo test`
通過。残るのは §8.2 の実機検証 (Q3 / Q5〜Q8)。

**改訂履歴**:

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
  `poll_paste_pending` の doc comment を元に戻した。§11 を更新。
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
- `docs/feature-expansion-ideas.md` の「見送る機能」に **「ドラッグ送出 — 工数大、別議論」**
  として記録されていた項目。本ドキュメントはその「別議論」にあたる。
- 調査の結果、想定より低コストで実装できる見込みが立ったため、改めて設計に起こす。

## 2. スコープ

### 対象 (やる)

- **実ファイル**: 画像 / 動画 / ZIP 本体 / PDF 本体 / 変換前アーカイブ (7z/LZH 等)
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
| クリップボード経由コピー | `copy_files_to_clipboard()` ([context_menu.rs:825](../src/ui_dialogs/context_menu.rs)) | **PowerShell 経由なので D&D には流用不可** (後述) |

### 3.1 既存クリップボード実装が流用できない理由

`copy_files_to_clipboard()` は PowerShell スクリプトを起動して
`[System.Windows.Forms.Clipboard]::SetFileDropList()` を呼ぶ方式。これはクリップボードへの
**非同期書き込み**には使えるが、OLE ドラッグ＆ドロップは **同一プロセス内の `IDataObject` を
`DoDragDrop` に渡す** 必要があり、別プロセス (PowerShell) では実現できない。D&D は
ゼロから COM 経路で組む。

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

**`IDataObject` を自前実装せず、シェルが用意済みのものを使う。** これで上記の面倒な部分が
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

**自前 COM インターフェース実装はゼロ。** 追加クレートは不要。追加 Cargo feature も
**primary path (§4.2) では不要** (フォールバック §4.4 を採る場合のみありうる。§9 参照)。

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
}
// 構築は private コンストラクタ経由: not_started() / failed_before_start(failed,
// error) / after_modal(failed, error)。started を取り違えないようにするため。
// SHDoDragDrop の結果 DROPEFFECT (コピー成立 / キャンセル) は内部ログにのみ出す
// — 呼び出し側に渡しても消費されなかったため DragOutcome には保持しない。

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
7. `SHDoDragDrop(Some(HWND(hwnd as *mut _)), &data, Option::<IDropSource>::None,
   DROPEFFECT_COPY)` を呼ぶ。**ここでブロック** (ドロップ/キャンセルまで戻らない)。
8. `SHDoDragDrop` の戻り値で分岐 (いずれも `started: true`):
   - `Ok(effect)` → `effect` (コピー成立 / キャンセル) をログに出し、
     `DragOutcome::after_modal(failed, None)`。
   - `Err(e)` → `DragOutcome::after_modal(failed, Some(FileDragError::DoDragDrop(e.code())))`。
     **`started` は true** — モーダルループに入ったのでポインタリセットは必要。

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
///   - ZipSeparator        — 擬似アイテム
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
ConvertibleArchive が `Some`、ZipImage/PdfPage/ZipSeparator/**SearchContainer** が
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

既存の複数選択は `if response.clicked()` 内で Ctrl+クリック (トグル) / Shift+クリック
(範囲) を処理している ([ui_main.rs:1483-1529](../src/ui_main.rs))。egui では
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

### 6.2 モーダルブロック中は再描画が止まる

`SHDoDragDrop` が戻るまで `update()` が返らない = ドラッグ中はウィンドウが再描画されない。
**ただしこれはエクスプローラ含め全アプリ共通の標準挙動** (ドラッグ元ウィンドウは固まって
見えるのが正常)。直前フレームの絵が残るだけなので実害なし。

`docs/ui-responsiveness.md` §4 の「UI スレッド同期 I/O 禁止」とは性質が異なる
(バックグラウンド I/O ではなく、ユーザー起点のモーダル操作)。違反ではないが、
`ui-responsiveness.md` に「native D&D のモーダルブロックは意図的な例外」と一文補足する。

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

### 6.7 eframe の present タイミング

§5.5(b) は「`update()` 末尾でブロックすると、このフレームの絵はまだ present されておらず
画面には前フレームが残る」想定。eframe 0.33 が `update()` の戻り後にテッセレーション →
present する順序であれば正しい。ドラッグ中に表示されるのは「ドラッグ開始 1 つ前の
グリッド」になる — 実用上問題ないが、正確な present 順序は要確認。→ §8 Q6。

## 7. テスト計画

OS モーダル操作のため `egui_kittest` スナップショットテストは**不可**。実機手動テスト必須。

### ユニットテスト (自動)

- `GridItem::drag_source_path()`: Folder/Image/Video/ZipFile/PdfFile/ConvertibleArchive が
  `Some`、ZipImage/PdfPage/ZipSeparator/SearchContainer が `None`。
- `decide_drag_payload()` (§5.4.3): 単体実パス / 単体仮想 / 複数実パス /
  混在選択 (実 + 仮想) / 全仮想 / 未チェック item の各分岐で、`DragDecision` の種別・
  ドラッグ対象パス・`post_drag_toast` 文言が仕様通りか。payload が **index 昇順**で
  並ぶことも検証する (§5.4.2)。

### 手動テスト (実機)

OS モーダル操作なので自動化不可。以下を実機で確認する。

**基本動作**

- グリッドの画像をエクスプローラの別フォルダへドラッグ → コピーされる。
- グリッドのフォルダをエクスプローラへドラッグ → フォルダごとコピーされる。
- ZIP 本体 / PDF 本体 / 7z をドラッグ → コピーされる。
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
- 操作は **コピーのみ** (送出と同じ方針)。同名既存は上書き (`Copy-Item -Force`、
  既存の Ctrl+V ペーストと同挙動)。

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
  2. 残り 0 件なら `CopyOutcome::notice` に「ドロップ先と重なる N 件を除外した結果、
     コピー対象が 0 件になりました」をセットして送る (Codex P2-2 対応: 旧実装はここで
     `CopyOutcome::default()` を送って poll が無音化していたため、ユーザーは
     「コピーしています…」のあと拒否理由を見られなかった)。
  3. 残りがあれば `copy_paths_into_folder` を呼んで内部 worker から `CopyOutcome` を受け取り、
     UI へ転送。`recv()` の `Err` は `CopyOutcome::all_failed(attempted, reason)` で
     全件失敗扱いに格上げする (Codex P2-1 対応: Disconnected を成功扱いに潰さない)。
- `file_drag::dir_copy_would_recurse(src, dest)`: 両パスを `canonicalize` 正規化し、
  コピー先 `dest/basename(src)` が `src` 自身または配下かを小文字化 + コンポーネント
  単位の前方一致で判定する。純粋判定部 `copy_target_inside_src` はユニットテスト済み。
- コピーは `ui_dialogs::context_menu::copy_paths_into_folder` —
  `Copy-Item -LiteralPath -Recurse -Force` を `try/catch` で囲って失敗カウントと先頭 5 件の
  エラーメッセージを stdout の `::FAILED::N` / `::ERR::msg` マーカーで返す。spawn 失敗 /
  tmp 書き込み失敗 / `powershell` 実行失敗 / 非ゼロ終了 / マーカー欠落はすべて
  `failed=attempted` で報告する (Codex P2-1 対応: 旧実装は `SilentlyContinue` で全部
  飲んでいた)。
- UI 側の受け取り経路:
  - 完了は `App::drop_copy_pending: Vec<Receiver<CopyOutcome>>` に積まれ、
    `poll_paste_pending` が `pending_reload` を立てつつ:
    - `failed > 0` のとき「成功 K / 失敗 N (例: ...)」トーストを出す
    - `notice = Some(_)` のとき (全件除外等) その文面をトーストで出す
    - それ以外 (`failed == 0 && notice == None`) は静かに reload のみ
  - 既存の `paste_pending: Vec<Receiver<()>>` はクリップボード paste 経路向けで現状維持
    (出力に構造化情報が要らない単純経路)。

### 11.3 送出との非干渉

`SHDoDragDrop` (送出) と winit の `RegisterDragDrop` (受け取り) は独立。送出実装は
受け取りを壊さない (送出追加前から受け取りの下地はあったが、それを読むコードが
無かっただけ)。
