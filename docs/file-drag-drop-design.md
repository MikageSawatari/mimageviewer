# ファイル D&D (ドラッグでコピー送出) 実装設計

グリッドのサムネイルを掴んで、エクスプローラや他アプリへ **ドラッグ＆ドロップでファイルを
コピー** できるようにする機能の設計ドキュメント。

このドキュメントは **実装前のレビュー用**。コードはまだ書いていない (調査のみ完了)。

**改訂履歴**:

- 2026-05-22: Codex レビュー第 1 回を反映。主な修正点 — COM 初期化の寿命管理
  (§5.1 / §6.3)、混在選択 (実ファイル + 仮想アイテム) の仕様化 (§5.4)、`SearchContainer`
  のスコープ明記 (§2 / §5.2)、pointer reset を「要実機検証」に格下げ (§6.1)、工数見積もりの
  引き上げ (§9)。Q1 / Q2 は解決済 (§8)。
- 2026-05-22: Codex レビュー第 2 回を反映。主な修正点 — ポインタ固着対策に
  `ctx.stop_dragging()` を必須手順として追加 (§5.5 / §6.1。egui の interaction 層は
  `PointerState` と別管理のため、raw リセットだけでは `dragged_id` が残る)、
  `SHParseDisplayName` 失敗を黙ってスキップせず件数をトーストで明示する仕様に変更
  (§5.1 / §5.5b。戻り値を `DragOutcome` 構造体化)、`native_drag_just_finished` を
  `SHDoDragDrop` 到達時のみ立てるよう修正 (§5.5b)。

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
- 複数選択ドラッグ: チェックボックスで複数選択した実ファイル群をまとめてドラッグ

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
- ドラッグ **受け取り** (他アプリから mIV へのドロップ): 本機能とは別。対象外。
- 移動 (MOVE) セマンティクス: §6.4 参照。

## 3. 既存コードの足場 (調査結果)

| 既にあるもの | 場所 | 用途 |
| --- | --- | --- |
| メインウィンドウ HWND | `App::main_hwnd: Option<isize>` ([app.rs:2872](../src/app.rs)) | 初フレームで `frame.window_handle()` から取得済 ([app.rs:16853](../src/app.rs)) |
| `windows` crate 0.61 + 必要 feature | [Cargo.toml:37-70](../Cargo.toml) | `Win32_System_Ole` / `_Com` / `_Com_StructuredStorage` / `_UI_Shell` / `_UI_Shell_Common` / `_System_Memory` すべて有効 |
| ドラッグ対象パス判定 | `GridItem::file_operation_path()` ([grid_item.rs:177](../src/grid_item.rs)) | 実ファイルパス抽出。ただしフォルダを除外しているので拡張が必要 (§5.1) |
| 複数選択モデル | `App::checked: HashSet<usize>` | チェックボックス選択。`collect_checked_paths()` で実ファイルパス収集 |
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
  → SHParseDisplayName()           → *mut ITEMIDLIST (PIDL)
  → SHCreateShellItemArrayFromIDLists(&pidls) → IShellItemArray
  → array.BindToHandler(None, &BHID_DataObject) → IDataObject   ← 完成済み
  → SHDoDragDrop(hwnd, &dataobject, None, DROPEFFECT_COPY)      ← ドラッグ開始
```

この `IDataObject` は `CF_HDROP` に加え、シェル形式 (`CFSTR_SHELLIDLIST` 等) も保持するため
**エクスプローラへのドロップも他アプリへのドロップも正しく動く**。`SHDoDragDrop` は
既定のドラッグ画像・ドロップカーソルも自動で提供する。

**自前 COM インターフェース実装はゼロ。** 追加クレート・追加 Cargo feature も不要。

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
/// ポインタリセットの要否と失敗トーストの要否を判断する。
#[derive(Debug, Clone, Copy)]
pub struct DragOutcome {
    /// `SHDoDragDrop` まで到達したか。**true のときだけ** §5.5(a) のポインタ
    /// リセットが要る (到達しなければ winit が通常どおり WM_LBUTTONUP を受ける)。
    pub started: bool,
    /// `SHParseDisplayName` に失敗したパス数 (0 が正常)。>0 ならトーストで明示する。
    pub failed_paths: usize,
}
```

処理手順:

1. `paths` が空なら `DragOutcome { started: false, failed_paths: 0 }` を返す。
2. **COM 初期化は行わない** (採用方針、§6.3 参照)。`start_file_drag` は UI スレッド
   (= winit のイベントループスレッド) からのみ呼ばれ、winit 0.30.13 がウィンドウ作成時に
   `OleInitialize` → `RegisterDragDrop` 済みなので、このスレッドは既に STA。
   `start_file_drag` 内で重ねて `OleInitialize` を呼ぶと、Microsoft Learn の
   [OleInitialize ドキュメント](https://learn.microsoft.com/windows/win32/api/ole2/nf-ole2-oleinitialize)
   が要求する「成功した呼び出し (`S_FALSE` 含む) ごとに `OleUninitialize` でバランスを
   取る」義務が生じる。これを取り違えると winit の COM 寿命を壊すため、**そもそも呼ばない**
   のが安全。万一どうしても自前初期化が要ると判明した場合は、RAII ガード
   (`Drop` で必ず対応する `OleUninitialize` を呼ぶ型) に必ず包む。
3. 各 `PathBuf` を UTF-16 (`\0` 終端) 化し、`SHParseDisplayName` で `*mut ITEMIDLIST` を取得。
   **失敗を黙ってスキップしない** (§5.4.2 の「黙って一部だけ」回避方針と揃える。
   実ファイルが parse 失敗するのは「選択後ドラッグ前にファイルが消えた」等の異常系):
   失敗したパス数を数えて `DragOutcome.failed_paths` に載せる。成功した PIDL だけで
   ドラッグを続行し、呼び出し側 (§5.5b) が失敗件数をトーストで明示する。
   全パスが失敗したら以降の手順をスキップして
   `DragOutcome { started: false, failed_paths: paths.len() }` を返す。
4. `SHCreateShellItemArrayFromIDLists(&pidls)` で `IShellItemArray` を作る。
5. 取得した PIDL を `CoTaskMemFree` で順に解放 (配列側がコピー済みなので安全)。
   早期 return する経路でも漏れなく解放するよう、ガード型か明示的な解放順序で組む。
6. `array.BindToHandler(None, &BHID_DataObject)` で `IDataObject` を得る。
7. `SHDoDragDrop(Some(HWND(hwnd as *mut _)), &data, Option::<IDropSource>::None,
   DROPEFFECT_COPY)` を呼ぶ。**ここでブロック** (ドロップ/キャンセルまで戻らない)。
8. `SHDoDragDrop` の戻り値 (`DROPEFFECT`) はログに出すだけ。
   `DragOutcome { started: true, failed_paths: <手順 3 の失敗数> }` を返す。

`#[cfg(windows)]` でガードし、非 Windows では
`DragOutcome { started: false, failed_paths: 0 }` を返す空実装 (他のプラットフォーム
分岐に揃える)。

### 5.2 `GridItem` にドラッグ対象パス抽出を追加 (約 15 行)

`file_operation_path()` はフォルダを除外している (コメント: 「フォルダは OS / エクスプローラ
側で扱う領分」)。D&D ではフォルダも送出したいので別アクセサを追加する:

```rust
/// D&D で送出できる実ファイル / 実フォルダのパス。
/// file_operation_path() との違いは Folder を含むこと。
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

### 5.3 `App` への状態追加 (約 5 行)

```rust
/// この値が Some の間、フレーム末尾で start_file_drag を実行する。
/// egui closure 内から SHDoDragDrop を直接呼べない (self 借用衝突・再入) ための受け渡し。
pub(crate) pending_native_drag: Option<Vec<PathBuf>>,
/// 直前フレームで native ドラッグを実行した。次フレーム冒頭で egui の
/// ポインタ状態をリセットするためのフラグ (§6.1)。
pub(crate) native_drag_just_finished: bool,
```

`Default` impl で両方初期化。

### 5.4 `ui_main.rs` — ドラッグ検出 (約 70 行)

`handle_cell_interaction()` ([ui_main.rs:1481](../src/ui_main.rs)) の
`egui::Sense::click()` を `egui::Sense::click_and_drag()` に変更。
`clicked()` / `double_clicked()` / `secondary_clicked()` は `click_and_drag()` でも従来通り
発火するので、選択・フルスクリーン化・右クリックメニューは壊れない。

#### 5.4.1 modifier の扱い (選択操作との非競合)

既存の複数選択は `if response.clicked()` 内で Ctrl+クリック (トグル) / Shift+クリック
(範囲) を処理している ([ui_main.rs:1483-1529](../src/ui_main.rs))。egui では
**`clicked()` と `drag_started()` は相互排他** — ポインタが閾値以上動けば `drag_started()`、
動かなければ `clicked()` のどちらか一方しか発火しない。したがって:

- **ドラッグ送出は modifier の有無に関わらず `drag_started()` で開始する。**
  mIV は COPY 限定 (§6.4) なので、Ctrl (コピー強制) / Shift (移動強制) を握っていても
  挙動は変わらない。OS 側がカーソル装飾を変える程度。
- Ctrl+クリック / Shift+クリックによる選択操作は `clicked()` 側のまま不変。drag とは
  co-fire しないので競合しない。
- 既知のトレードオフ: Ctrl+クリックのつもりでもポインタが egui のドラッグ閾値
  (約数 px) を超えて動くと `drag_started()` 側に倒れ、選択トグルでなくドラッグになる。
  これは `click_and_drag()` 採用の不可避な副作用。閾値は十分小さく実害は薄いが、
  手動テスト (§7) で確認する。

#### 5.4.2 ドラッグ対象の決定 — 混在選択の仕様

⚠ **Codex 指摘の最重要修正点**: `checked` には実ファイルと仮想アイテム
(`ZipImage` / `PdfPage`、どちらも `is_checkable()` が true) が混在しうる。素朴に
`filter_map` で実ファイルだけ拾うと、ユーザーには **「選択したうち一部しかコピー
されなかった」** ように見え危険。明示仕様にする。

`drag_started()` 時、セル `idx` について:

**(A) `idx` が複数選択の一部 (`self.checked.contains(&idx)`) の場合**

`checked` 全件を 2 群に分割する:

- `draggable`: `drag_source_path()` が `Some` のもの (実ファイル/実フォルダ)
- `virtual_excluded`: 仮想アイテム (`ZipImage` / `PdfPage`)

判定:

- `draggable` が空 → ドラッグ開始しない。トースト「ドラッグできる実ファイルが
  選択されていません」。
- `virtual_excluded` が非空 (= 混在選択) → **`draggable` をドラッグしつつ、除外を
  明示するトースト**を出す: 「仮想フォルダ内の N 件は除外しました (実ファイル M 件を
  コピー)」。"黙って一部だけ" を避けるのが目的。Codex が挙げた 2 案
  (「ドラッグ不可+トースト」/「除外を明示」) のうち **後者を採用** — 実ファイルは
  確実にコピーでき、かつ除外をユーザーが認識できる。
- 混在なし → `draggable` をそのままドラッグ。

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
決定したパス群を `self.pending_native_drag` に積むだけ。実行は §5.5(b)。

```rust
if response.drag_started() {
    // §5.4.2 のロジックでパス群とトースト要否を決定
    let decision = self.decide_drag_payload(idx);   // 新規ヘルパ
    if let Some(paths) = decision.paths {            // 空でないことは内部で保証
        self.pending_native_drag = Some(paths);
    }
    if let Some(msg) = decision.toast {
        self.show_feedback_toast(msg);
    }
}
```

`decide_drag_payload` を独立メソッドにして `#[cfg(test)]` でユニットテスト可能にする
(混在選択 / 全仮想 / 単体実ファイル / 単体仮想 の各分岐)。

### 5.5 `app.rs` — フレーム末尾で実行 + ポインタリセット (約 30 行)

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
if let Some(paths) = self.pending_native_drag.take() {
    match self.main_hwnd {
        Some(hwnd) => {
            let outcome = crate::file_drag::start_file_drag(hwnd, &paths); // モーダルブロック
            // SHDoDragDrop に到達したときだけポインタリセットが要る。
            // ドラッグ未開始 (paths 全滅等) なら winit が通常どおり
            // WM_LBUTTONUP を受け取るので、リセットすると逆に副作用が無駄に出る。
            if outcome.started {
                self.native_drag_just_finished = true;
            } else {
                crate::logger::log("file_drag: drag did not start");
            }
            // SHParseDisplayName 失敗を黙って捨てない (§5.1 手順 3)。
            if outcome.failed_paths > 0 {
                self.show_feedback_toast(format!(
                    "{} 件のファイルはドラッグできませんでした",
                    outcome.failed_paths,
                ));
            }
        }
        None => {
            // 初フレーム前など main_hwnd 未取得。ドラッグ自体が走らないので
            // ポインタリセットも不要。ログだけ残す。
            crate::logger::log("file_drag: main_hwnd unavailable, drag skipped");
        }
    }
    ctx.request_repaint();
}
```

`SHDoDragDrop` 呼び出し時点でマウスボタンはまだ押下中 (このフレームで `drag_started` が
出たばかり) なので、`DoDragDrop` の前提 (ボタン押下中に呼ぶ) を満たす。

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

放置すると、次にマウスを動かしたとき別セル上で `drag_started()` が誤発火し
**幽霊ドラッグ** が起きる。

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
  `drag_started()` を無視する。幽霊ドラッグ「再発火」だけは確実に防げる。
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

初版スコープは「単体フォルダ + 複数ファイル」とし、複数フォルダ同時は将来課題。
→ §8 Q4。

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
- `decide_drag_payload()` (§5.4.3): 単体実ファイル / 単体仮想 / 複数実ファイル /
  混在選択 (実 + 仮想) / 全仮想 の各分岐で、ドラッグ対象パスとトースト要否が仕様通りか。

### 手動テスト (実機)

OS モーダル操作なので自動化不可。以下を実機で確認する。

**基本動作**

- グリッドの画像をエクスプローラの別フォルダへドラッグ → コピーされる。
- グリッドのフォルダをエクスプローラへドラッグ → フォルダごとコピーされる。
- ZIP 本体 / PDF 本体 / 7z をドラッグ → コピーされる。
- デスクトップ・他アプリ (ブラウザ、別画像ソフト等) へのドロップ。

**選択モデルとの組み合わせ**

- 複数チェック選択 (全部実ファイル) → どれか 1 つを掴んでドラッグ → 選択全部がコピー。
- **混在選択**: `checked` に実ファイルと ZipImage/PdfPage を混ぜる → ドラッグ →
  実ファイルだけコピーされ、除外トーストが出る (§5.4.2(A))。
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
- **混在選択の仕様** (Codex 指摘 4): 解決。実ファイル + 仮想アイテム混在時は実ファイルを
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
| 新規 `src/file_drag.rs` (COM 糊コード) | 約 100〜150 行 |
| `grid_item.rs` `drag_source_path()` + テスト | 約 30 行 |
| `app.rs` 状態追加 + フレーム末尾実行 (失敗ハンドリング込み) + ポインタリセット | 約 55 行 |
| `ui_main.rs` ドラッグ検出 + `decide_drag_payload()` + テスト | 約 70 行 |
| **合計** | **約 255〜305 行、新規モジュール 1 本** |

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
