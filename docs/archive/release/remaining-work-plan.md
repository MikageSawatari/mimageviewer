# 残り作業 (Phase 6 / 7 / 8) — 統合実装ブリーフ

**対象**: Phase 6 (Ctrl+E エクスポート) + Phase 7 (統合テスト) + Phase 8 (ドキュメント整備)
**進め方**: Codex GUI で実装 → ClaudeCode で手作業レビュー (= 既存運用と同じ)
**前提**: Phase 0〜5 + 隠蔽加工本体 + 消しゴム subtract shape + パネル高さ修正 + 選択ツールアウトラインまで完了済み (`44b2e5b7` まで)

**進捗メモ (2026-05-27 / 2026-05-28 更新)**: Phase 6 の本体
(`src/export_dialog.rs`、Ctrl+E 起動、worker 合成・保存、進捗モーダル、
最低限の仕様ドキュメント更新)、Phase 7 の統合テスト拡充、Phase 8 の詳細マニュアル
整備は実装済み。

このブリーフは前 2 件 (`docs/archive/editing/erase-cache-refactor-plan.md` /
`docs/archive/ui-input/panel-height-fix-plan.md`) と同じ運用方針で使う。各 Phase は **独立に着手・コミット
可能** だが、Phase 6 → 7 → 8 の順を強く推奨 (= 6 の API が固まらないと 7 の統合テスト
が書けない、6+7 の挙動が確定しないと 8 のマニュアル記述がブレる)。

依存元の元設計は [docs/conceal-feature-plan.md](../../conceal-feature-plan.md) §10〜§15。
本ブリーフはそれを **実装直前のチェックリスト形式に圧縮**して、Codex が context を
切り替えずに着手できる粒度に並べ直したもの。詳細仕様で迷ったら元設計を参照。

---

## 0. 着手前に必ず読む

- [CLAUDE.md](../../../CLAUDE.md) §「作業開始時に必読」「コード修正時のドキュメント同時更新」
  「動画メタ情報の扱いと外部ダウンローダの言及禁止ポリシー」「モザイク・成人向け画像
  処理の表記ポリシー」「Markdown / テキストファイルのエンコーディング」「UI 文字列の
  Unicode グリフ選定ルール」「IME 対応」「Formatting」「永続データ・スキーマ変更時の判断」
- [docs/conceal-feature-plan.md](../../conceal-feature-plan.md) §10 (Ctrl+E)、§14 (テスト計画)、
  §15 (ドキュメント更新)、§16 (確定事項)
- [docs/ui-responsiveness.md](../../ui-responsiveness.md) §1〜§4 (UI スレッド同期 I/O 禁止)
- [docs/async-architecture.md](../../async-architecture.md) §3 (worker パターン)
- [docs/architecture-overview.md](../../architecture-overview.md) (どこに何があるか)

各 Phase の作業開始前にこれらに目を通すこと。設計を変えたら同じ箇所を**更新する**。

---

# Phase 6: Ctrl+E エクスポートダイアログ

## 6.1 ゴール

「現在のフルスクリーン表示画像 (= 全ての編集結果が反映された display pixels) を、
**バリエーション一括出力対応**で元と同じ場所に元の形式で保存する」汎用機能を完成
させる。**隠蔽加工専用ではない**: 消しゴム済み画像・補正のみ・何もしていない画像
でも動作する。

## 6.2 既存資産 (再利用するもの)

実装ベースはほぼ揃っているので、Phase 6 は **UI + worker + 配線**のみ。

| 資産 | パス | 用途 |
|---|---|---|
| `save_image_with_metadata` | [src/save_with_metadata.rs:171](../src/save_with_metadata.rs) | 1 ファイル書き出し、`OpenOptions::create_new(true)` |
| `save_image_with_metadata_unique` | [src/save_with_metadata.rs:259](../src/save_with_metadata.rs) | `_NNNN` 連番探索付き、衝突回避 |
| `SrcFormat::from_path` / `SrcFormat::supports_metadata_writeback` | 同上 | Jpeg/Png/Webp 判定 |
| `SaveOptions` | 同上 | jpeg_quality / webp_quality / include_metadata / caller_applied_orientation |
| `SaveError` | 同上 | `Display` impl 済み、UI でそのまま表示可 |
| `ExportFallbackFormat::{Jpeg95, Png}` | [src/conceal.rs:302](../src/conceal.rs) | HEIC/AVIF/JXL/RAW/TIFF からのフォールバック先 |
| `Settings::export_embed_metadata` | [src/settings.rs:788](../src/settings.rs) | チェックボックス初期値 |
| `Settings::export_last_directory` | [src/settings.rs:792](../src/settings.rs) | 「元フォルダ以外を選んだ場合」の前回値 |
| `Settings::export_fallback_format` | [src/settings.rs:796](../src/settings.rs) | HEIC 等のフォールバック先 |
| `Settings::export_batch_selection: [bool; 5]` | [src/settings.rs:800](../src/settings.rs) | バリエーションチェック前回値 (現在/p1/p2/p3/p4) |
| `default_export_batch_selection()` | [src/settings.rs:1641](../src/settings.rs) | 既定値 (要確認) |

**注意**: 上記 Settings フィールドはすでに `serde(default)` 付きで永続化対応済み。
Phase 6 で**追加 Settings は不要**な見込み。もし追加する場合は CLAUDE.md「永続データ・
スキーマ変更時の判断」§未リリース vs リリース済み に従う (= これらは v0.9.x で
未リリース機能なので破壊的変更 OK)。

## 6.3 新規モジュール

### `src/export_dialog.rs` (新設)

ダイアログ UI + worker 投入 + 結果集約の専用モジュール。`ui_dialogs/` の他ファイル
と違って `Ctrl+E` で開く特殊ダイアログなので、ui_dialogs 配下ではなく **`src/` 直下**
に置く ([docs/conceal-feature-plan.md §10](../../conceal-feature-plan.md) の構成案通り)。

公開 API (案):

```rust
pub(crate) struct ExportPending {
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<ExportEvent>,
    pub total: usize,        // 投入時のチェック済みエントリ数
    pub done: usize,         // UI スレッドが rx から受信して加算
    pub started_at: Instant,
    pub last_message: String,  // "_1.jpg 生成中…" 等のラベル
    pub last_error: Option<String>,
}

pub(crate) enum ExportEvent {
    Started { entry_idx: usize, label: String },
    Completed { entry_idx: usize, dst_path: PathBuf },
    Failed { entry_idx: usize, error: String },
    Cancelled,
    AllDone,
}

pub(crate) struct ExportRequest {
    pub src_path: Option<PathBuf>,        // ZIP/PDF なら None
    pub src_bytes: Option<Arc<Vec<u8>>>,  // ZIP/PDF はバイト列を渡す
    pub src_format: SrcFormat,
    pub output_dir: PathBuf,
    pub basename: String,                  // 例: "foo_edited"
    pub entries: Vec<ExportEntry>,         // チェックされたエントリのみ
    pub include_metadata: bool,
    pub options_per_entry: SaveOptions,    // 共通の SaveOptions (品質等)
}

pub(crate) struct ExportEntry {
    pub label: String,                     // "現在の設定" / "プリセット 1" 等
    pub suffix_num: u8,                    // 0=現在, 1-4=プリセット
    pub conceal_preset: Option<ConcealPreset>, // worker 側で合成する隠蔽パラメータ
}
```

Codex の判断ポイント:

- 実装は **worker thread で合成 → encode → 書き込み** の方針。UI スレッドは
  `base_pixels: Arc<ColorImage>` と composite mask、各エントリの `ConcealPreset`
  だけを snapshot して worker に渡す。
- これにより 5 エントリ × 4K RGBA を UI スレッドで先に合成して保持する必要がなく、
  Mosaic / Blur の CPU コストも保存ボタン直後の UI 停止にならない。

## 6.4 ダイアログ UI

設計は [docs/conceal-feature-plan.md §10.1](../../conceal-feature-plan.md) 通り。要件:

- **ファイル名 (ベース)**: 既定は `元ファイル名_edited`。TextEdit。
  - **IME 対応**: `dialog_enter_pressed` / `dialog_escape_pressed` ヘルパー必須
    (CLAUDE.md「IME 対応」)。
- **保存先**: 既定は元と同じフォルダ。「変更…」ボタンで `rfd::FileDialog::pick_folder`。
  最後に選んだディレクトリは `Settings::export_last_directory` に永続化。
- **形式**: 既定は元形式 (`SrcFormat::from_path(src_path)`)。Dropdown で JPEG / PNG /
  WebP のいずれかへ変更可。元形式が非対応 (HEIC 等) のときは fallback (Jpeg95 / Png)
  を **強調表示** + 注意文。
- **バリエーションチェックリスト** (`[bool; 5]`):
  - `☑ 現在の設定 → ..._0.<ext>`
  - `☑/☐ プリセット 1〜4 → ..._{1..4}.<ext>`
  - チェック状態は `Settings::export_batch_selection` に永続化 (ダイアログ閉じる時)
  - **少なくとも 1 つ ON でないと「保存」ボタンが disable**
- **AI プロンプト / EXIF を埋め込む** チェック → `Settings::export_embed_metadata`
- **保存 / キャンセル ボタン** (右下、`Enter` で保存、`Esc` でキャンセル、IME 中は無効)
- 配色: ダイアログなのでテーマに従う (= 隠蔽パネル等と違って固定 dark にしない)
- **特定の投稿サイト名・基準名は書かない** (CLAUDE.md「モザイク・成人向け画像処理の
  表記ポリシー」)

### 進捗モーダル

5 エントリ生成中はモーダル進捗ダイアログ。設計は [§10.1 進捗ダイアログ](../../conceal-feature-plan.md)
通り:

- `total / done` を表示、`ProgressBar` で視覚化
- 現在処理中のファイル名 (`last_message`)
- `[ キャンセル ]` ボタン (`cancel.store(true, Relaxed)`)
  - worker は **現エントリ完了後に中断** (生成済みファイルはそのまま残す、削除しない)
- 完了したら自動で閉じる
- エラーが出たら `last_error` を表示してモーダルを残す (= 「OK」ボタンで閉じる)

## 6.5 Worker thread

[docs/async-architecture.md §3](../../async-architecture.md) のパターンに従って worker
thread + cancel token + mpsc。

```rust
fn spawn_export_worker(request: ExportRequest) -> ExportPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let cancel_clone = Arc::clone(&cancel);
    std::thread::Builder::new()
        .name("export-worker".into())
        .spawn(move || run_export(request, cancel_clone, tx))
        .expect("spawn export worker");
    ExportPending {
        cancel,
        rx,
        total: request.entries.len(),
        done: 0,
        started_at: Instant::now(),
        last_message: String::new(),
        last_error: None,
    }
}

fn run_export(req: ExportRequest, cancel: Arc<AtomicBool>, tx: mpsc::Sender<ExportEvent>) {
    for (i, entry) in req.entries.iter().enumerate() {
        if cancel.load(Relaxed) {
            let _ = tx.send(ExportEvent::Cancelled);
            return;
        }
        let label = format!("{}_{}.{}", req.basename, entry.suffix_num, req.src_format.extension());
        let _ = tx.send(ExportEvent::Started { entry_idx: i, label: label.clone() });
        match save_image_with_metadata_unique(
            &entry.pixels,
            req.src_path.as_deref(),
            req.src_bytes.as_deref().map(|v| v.as_slice()),
            &req.output_dir,
            &format!("{}_{}", req.basename, entry.suffix_num),
            req.src_format.clone(),
            &req.options_per_entry,
            0,
            9999,
        ) {
            Ok(dst) => { let _ = tx.send(ExportEvent::Completed { entry_idx: i, dst_path: dst }); }
            Err(e) => { let _ = tx.send(ExportEvent::Failed { entry_idx: i, error: e.to_string() }); }
        }
    }
    let _ = tx.send(ExportEvent::AllDone);
}
```

UI スレッド側 (`App::update` から呼ぶ poll):

```rust
fn poll_export(&mut self, ctx: &egui::Context) {
    let Some(pending) = self.export_pending.as_mut() else { return; };
    let mut closed = false;
    while let Ok(event) = pending.rx.try_recv() {
        match event {
            ExportEvent::Started { label, .. } => pending.last_message = label,
            ExportEvent::Completed { .. } => pending.done += 1,
            ExportEvent::Failed { error, entry_idx } => {
                pending.last_error = Some(format!("エントリ {} エラー: {}", entry_idx, error));
                pending.done += 1;  // 進捗は前進させる (= 残りエントリは続行)
            }
            ExportEvent::Cancelled | ExportEvent::AllDone => closed = true,
        }
    }
    if closed && pending.last_error.is_none() {
        self.export_pending = None;
        self.show_feedback_toast("[エクスポート完了]".into());
    } else {
        ctx.request_repaint();
    }
}
```

## 6.6 ファイル名衝突回避

- 単一エントリ: `save_image_with_metadata_unique` が `_0000.jpg` から空き番号探索済み。
  ベース名衝突回避はモジュール内で完結。
- **バリエーション一括時** ([§10.1 衝突回避](../../conceal-feature-plan.md)): 5 エントリで
  `_0.jpg` ... `_4.jpg` のうち 1 つでも既存ファイルと衝突する場合、**セッション番号
  `_NNNN_`** を挟む:
  - 初回: `foo_edited_0.jpg`, `foo_edited_1.jpg`, ...
  - 衝突: `foo_edited_0001_0.jpg`, `foo_edited_0001_1.jpg`, ...
  - 4 桁、衝突しない最小値

実装方針: **worker 起動前** に UI スレッドで衝突チェック → セッション番号を確定 →
ExportRequest にセッション番号付きベース名を渡す。

```rust
fn resolve_session_basename(base: &str, ext: &str, output_dir: &Path, num_entries: u8) -> String {
    // base="foo_edited", ext="jpg" として、_0..(num_entries-1) のいずれかが存在
    // するかチェック。衝突なし → base をそのまま返す。
    // 衝突あり → _0001_, _0002_, ... を試して全エントリ衝突無しの最初を返す。
}
```

worker 内では `save_image_with_metadata_unique` の `seq_start=0, seq_max=9999` を
そのまま使えるので、追加ロジック不要。

## 6.7 Ctrl+E ホットキー

[src/ui_fullscreen.rs](../src/ui_fullscreen.rs) のキーイベント処理に追加。

- 動画 / 動画フレームキャプチャ中 / ZIP separator / 補正モード / 消しゴムモード /
  隠蔽モードでは **何を出すか要判断**:
  - 編集モード中 (erase / conceal / adjustment) → モード抜けてから Ctrl+E、または
    モード中でも Ctrl+E でダイアログを開く (= 編集途中の状態を export する)
  - 動画 → **無効** (export 非対応、toast で通知)
  - PDF ページ / ZIP 内画像 → **対応** (src_bytes 経由でメタデータ取得)
- 起動条件: フルスクリーンモードかつ画像系アイテム (Image / ZipImage / PdfPage)
- 起動時に preview 中ジョブがあれば cancel (= 結果が下のレイヤに来るのを待たない)

## 6.8 ZIP / PDF 対応

ZIP 内画像は `src_bytes` (= ZIP から抽出した生バイト列、Arc 化) を渡す。`src_path`
は None。PDF ページは `src_bytes=None` + `src_path=None` でメタデータ無し export
(PDF ページにはオリジナルファイル概念がないため、`include_metadata=true` でも
空のメタになる)。

UI 側でファイル名既定値を出すときも:
- 画像: `<元ファイル名>_edited` (拡張子は決定後付ける)
- ZIP 内: `<ZIP名>_<エントリ名>_edited`
- PDF ページ: `<PDF名>_page<NN>_edited`

## 6.9 既知の落とし穴 / レビュー観点

- **Worker thread でのキャンセル細粒度**: `for entry in entries` のループ冒頭で
  cancel チェック。1 エントリの save_image_with_metadata 中はキャンセル不可
  (encode が同期処理)。途中でキャンセルされたら **完了したファイルは残す** が
  以降のエントリは生成しない。生成中エントリは完了させる。
- **メモリプレッシャー**: 5 エントリ × 4K RGBA ≈ 400MB の Arc<ColorImage> を保持。
  worker 完了後すぐに drop されるよう、UI スレッド側で `ExportRequest` を `Some`
  → `take()` パターンで持つ。
- **エラー時の挙動**: 1 エントリ失敗しても他は続行 ([§10.1 同期実行](../../conceal-feature-plan.md)
  の方針通り)。失敗したエントリは done++ してスキップ、`last_error` を更新 (ただし
  最新エラーのみ表示でOK)。完了モーダルで「N 件成功 / M 件失敗」を集計表示。
- **`caller_applied_orientation`**: フルスクリーン表示中の pixels は canonical
  orientation 適用済みなので `true` (= EXIF Orientation を 1 に書き換え)。
  ZIP 内 JPEG も表示 decode 時に bytes から Orientation を適用するため true。
  実装時は `App` の `fs_cache` 経由で取得したピクセルが canonical かを確認すること。
- **TextEdit + Enter/Escape**: IME 変換中の Enter が「保存」を発火しないよう、必ず
  `dialog_enter_pressed` / `dialog_escape_pressed` を使う (CLAUDE.md)。
- **モーダル diag のドラッグ移動**: `default_pos()` で初期位置を指定し `anchor()` は
  使わない (CLAUDE.md「ダイアログ」)。
- **配色**: ダイアログはテーマに従う。`*ui.visuals_mut() = Visuals::dark();` のような
  固定はしない (パネルと違う扱い)。
- **特定サイト名禁止**: ダイアログ文言・toast・エラー文に投稿サイト基準名等が混ざら
  ないこと。`grep` で機械的に確認 (CLAUDE.md「モザイク・成人向け画像処理の表記
  ポリシー」§適合確認)。
- **UI 文字列の Unicode**: `python scripts/check_ui_glyphs.py` で tofu 検出。
- **マニフェスト**: dropdown / checkbox / button の文字列は Yu Gothic で表示可能な
  漢字 + ひらがな + カタカナのみ。✕/🎚/絵文字は使わない (CLAUDE.md「UI 文字列の
  Unicode グリフ選定ルール」、× は U+00D7 を使う)。

## 6.10 Phase 6 完了条件

- `Ctrl+E` でダイアログが開く (画像系アイテムのフルスクリーン時のみ)
- バリエーション 1〜5 個チェック → 「保存」で worker 起動 → 進捗モーダル表示
- 既存ファイル衝突時にセッション番号挟む (`_0001_`)
- キャンセルボタンで worker 中断 (現エントリ完了後に停止)
- メタデータ保持 ON/OFF が `Settings::export_embed_metadata` と連動
- 元形式が非対応 (HEIC 等) のとき fallback (Jpeg95/Png) で書ける
- ZIP 内画像 / PDF ページでも動作 (PDF はメタデータなし)
- 動画は対象外 (toast「動画は対象外」等で告知)
- `cargo fmt --check` / `cargo check` / `cargo test --lib` / `cargo build --release`
  すべて通る

---

# Phase 7: 統合テスト

## 7.1 ゴール

Phase 0〜6 の主要シナリオを `tests/` 配下の統合テスト (`#[test]`) でカバーする。
**UI を含まない、ロジック中心**のテストに留める (= eframe 起動なしで `MaskDb` /
`ConcealDb` / `save_image_with_metadata` / `export_dialog` の worker 部分を直接
叩く)。

## 7.2 既存資産

`tests/sidecar_import.rs` を**テンプレ**として使う:

- `TestEnv` 構造体: `TempDir` + `MaskDb` を 1 セットで持つ
- `MaskDb::open_at(&path)` で in-memory ではなく一時ファイル DB を作る
- `assert_eq!(env.mask_db.get(...).unwrap(), expected)` 形式

その他の参照:
- `tests/zip_integration.rs` — ZIP からの読み出しテスト
- `tests/test_archive_convert.rs` — RAR/7z/LZH 変換テスト
- `src/mask_db.rs::tests` — ShapeOp 永続化、レガシー JSON 互換 (Phase 0 で実装済み)
- `src/conceal_db.rs::tests` — ラウンドトリップ (Phase 4 で実装済み)

## 7.3 追加すべきテスト

`tests/` 直下に新規ファイルとして追加。1 ファイル 1 テーマ、ファイル名は既存
パターン (`_integration.rs`) を踏襲。

### `tests/mask_db_migration.rs` (新規)

| テスト名 | 内容 |
|---|---|
| `legacy_line_object_json_reads_as_add_shape` | 旧 `LineObject` 素 JSON を `Vec<Shape>` として読んだとき、`ShapeOp::Add` で復元される |
| `legacy_op_missing_reads_as_add` | 新形式の `Shape::Rect` JSON で `op` フィールドが無いものを読んだとき `Add` |
| `subtract_roundtrip` | `Shape::Rect { op: Subtract, ... }` の JSON → 読み戻し → 同じ |
| `mixed_legacy_new_array` | 旧 LineObject 素 JSON と新形式 `Shape` JSON が**混在**した配列を読めるか |
| `mask_db_set_get_with_subtract` | bitmap 全 false + subtract Shape のみ保存 → 再読み込みで shapes が復元される (= 削除されない) |
| `mask_db_delete_when_empty` | bitmap 全 false + shapes 空 → `set` が `delete` を呼んで DB から消える |
| `rasterize_shapes_apply_op_order` | Add → Subtract → Add の順序合成が `rasterize_shapes_into` で正しく動く (= 既存テストの統合テスト版) |

### `tests/sidecar_shape_migration.rs` (新規)

| テスト名 | 内容 |
|---|---|
| `sidecar_legacy_lineobject_roundtrip` | サイドカーに旧形式 `Vec<LineObject>` を書いて読むと `Vec<Shape>` (Add のみ) になる |
| `sidecar_new_shape_roundtrip` | 新形式 `Vec<Shape>` を書いて読むと同じ |
| `sidecar_subtract_persists` | Subtract Shape が含まれる `Vec<Shape>` を書いて、サイドカーから再構築しても保持される |

### `tests/conceal_db_integration.rs` (新規)

| テスト名 | 内容 |
|---|---|
| `conceal_db_roundtrip_basic` | mask + shapes を保存 → 読み戻し → 同じ |
| `conceal_db_mask_slot_save_load` | スロット保存 (`__slot_1`) → 別ページに load → スロット内容が反映 |
| `conceal_db_delete_on_empty` | mask 全 false + shapes 空 → 自動削除 |
| `conceal_db_pdf_zoom_resize` | 異なる解像度で保存・読み出ししたとき mask が最近傍法でリスケールされる |

### `tests/export_integration.rs` (新規)

UI 抜きで worker + save_with_metadata API を統合テストする。

| テスト名 | 内容 |
|---|---|
| `export_single_jpeg_with_metadata` | JPEG 入力 → 編集後ピクセル + `include_metadata=true` で save → 出力 JPEG から EXIF / XMP を読んで input と一致 |
| `export_batch_no_collision` | base名 + entries `[0,1,2]` で同フォルダ初回出力 → 連番付きで 3 ファイル生成 |
| `export_batch_with_collision_uses_session_number` | 既存 `foo_edited_0.jpg` がある状態で同 basename を export → セッション番号 `_0001_` が挟まる |
| `export_batch_partial_failure` | 中間エントリで意図的に失敗させて (= read-only ファイル衝突等)、他エントリは続行されること |
| `export_cancel_mid_batch` | worker 起動後すぐ cancel → 生成済みファイルは残り、以降は生成されない |
| `export_fallback_format_for_heic` | `src_format=Other("heic")` で `ExportFallbackFormat::Jpeg95` を指定 → JPEG として書ける |
| `export_zip_source_no_path` | `src_path=None, src_bytes=Some(zip_bytes)` でも書ける |
| `export_animated_webp_fails` | アニメ WebP 入力で `AnimatedWebpNotSupported` が返る |
| `export_orientation_canonical` | `caller_applied_orientation=true` で書いた JPEG の EXIF Orientation が 1 |
| `export_orientation_preserved` | `caller_applied_orientation=false` で書いた JPEG の EXIF Orientation が元の値のまま |

### `tests/erase_inpaint_integration.rs` (新規、optional)

MI-GAN 自体は GPU 依存なので skip。`#[ignore]` 付きで実機専用テストとして用意し、
ローカル開発時に手動で `cargo test --test erase_inpaint_integration -- --ignored`
で走らせる枠だけ用意するのが現実的。スコープは Codex が判断 (= 時間が許せば追加、
無理ならスキップ)。

## 7.4 テスト書き方の指針

- **eframe / egui を起動しない**: 統合テストは UI を含まない。`App::new` を呼ばず
  に各モジュール (`MaskDb`, `ConcealDb`, `save_with_metadata`, `export_dialog::run_export`)
  を直接叩く。
- **`TempDir` で隔離**: `tempfile::TempDir::new()` を毎テストで作って `Drop` で
  自動削除。DB ファイル名衝突を防ぐ。
- **`#[ignore]` 付きで GPU テスト分離**: DirectML や AI 推論を伴うテストは
  `#[ignore]` で標準実行から除外し、`cargo test -- --ignored` でのみ走らせる。
- **環境変数で skip 制御**: CI で skip したいテスト (= ローカル GPU 必須) は
  `if std::env::var("MIV_CI").is_ok() { return; }` のような guard を入れる。
- **アサーションは値 + メッセージ**: `assert_eq!(actual, expected, "context: {}", ctx)`
  形式でデバッグしやすく。

## 7.5 Phase 7 完了条件

- 上記 4 ファイル (mask_db_migration / sidecar_shape_migration / conceal_db_integration /
  export_integration) が追加されている
- `cargo test --test mask_db_migration` 等で個別に走る
- 全 `cargo test --lib` + 統合テストが通る
- `#[ignore]` のテストを除いた状態で CI フローが green
- 失敗シナリオ (collision / partial failure / cancel) の挙動が**ドキュメント済みの仕様と
  一致**することを確認

---

# Phase 8: ドキュメント整備

## 8.1 ゴール

Phase 0〜7 の成果を、**ユーザー向けマニュアル**と**開発者向け設計ドキュメント**
の両系統で反映する。漏れの多い領域なので、**まず全マニュアルページのサイドバー
整合性チェック**から始める (= 新ページ追加時にどこか必ず更新漏れが出る)。

## 8.2 ユーザー向けマニュアル (`htdocs/mimageviewer/manual/`)

### 8.2.1 新規ページ

| 新ページ | 内容 |
|---|---|
| `conceal.html` | 隠蔽加工機能のマニュアル。Ctrl+M でモード入退、4 タイプ (Mosaic / WhiteFill / BlackFill / Blur)、タイルサイズ 2 モード、不透明度、境界処理、マスクスロット 2 個、プリセット 4 個、Undo/Redo、ベクタオブジェクト後編集 |
| `export.html` | Ctrl+E エクスポートのマニュアル。ダイアログ操作、バリエーション一括、ファイル名規則、メタデータ保持、フォールバック形式、ZIP/PDF からの export |

両ページとも:
- **特定の投稿サイト名・基準名・基準への適合判定を一切記載しない** (CLAUDE.md
  「モザイク・成人向け画像処理の表記ポリシー」)
- **バージョン番号タグを書かない** (例: "v0.9.x+" は NG)
- **実装内部用語を書かない** (例: ShapeOp / subtract shape / worker thread は NG。
  「上から削るオブジェクト」「並列処理」のような一般語に置換)
- **特定の外部ダウンローダ名等は書かない** (CLAUDE.md「動画メタ情報の扱いと外部
  ダウンローダの言及禁止ポリシー」、本ページとは無関係だが横断ルール)

### 8.2.2 既存ページの更新

| 既存ページ | 必要な更新 |
|---|---|
| `index.html` | サイドバーに `conceal.html` / `export.html` のリンク追加 |
| `erase.html` | サイドバーに同 2 リンク追加。本文の「保存」セクションに「`Ctrl+E` で別途エクスポートできる」旨を追記 |
| `adjustment.html` | サイドバー追加 |
| `analysis.html` | サイドバー追加 |
| `faq.html` | サイドバー追加。「FAQ: 編集結果を別ファイルに保存するには?」を新設して `export.html` へ誘導 |
| `formats.html` | サイドバー追加。エクスポート対応形式 (Jpeg/Png/Webp) と非対応形式 (HEIC 等のフォールバック) を整理 |
| `fullscreen.html` | サイドバー追加 |
| `getting-started.html` | サイドバー追加 |
| `grid.html` | サイドバー追加 |
| `search.html` | サイドバー追加 |
| `settings.html` | サイドバー追加。エクスポート関連設定 (メタデータ保持初期値 / フォールバック形式 / 前回チェック状態の永続化) を説明 |
| `shortcuts.html` | サイドバー追加。`Ctrl+E` と `Ctrl+M` のショートカット表を更新 |
| `tags.html` | サイドバー追加 |
| `troubleshooting.html` | サイドバー追加 |
| `video.html` | サイドバー追加 |

**サイドバー整合性チェック**: 全 17 ページ (= 既存 15 + 新規 2) で同じリンク一覧が
出るように、CLAUDE.md リリース手順 §1-6 のチェックを行う:

```bash
cd htdocs/mimageviewer/manual && for f in *.html; do
  echo "=== $f ==="
  sed -n '/sidebar-section/,/<\/nav>/p' "$f" \
    | grep -E 'href="[a-z-]+\.html"' | wc -l
done
```

すべて同じ数 (= 17 になるはず) でなければ不整合。

### 8.2.3 製品ページ (`htdocs/mimageviewer/index.html`)

- 機能一覧に「隠蔽加工 (Ctrl+M)」「バリエーション一括エクスポート (Ctrl+E)」を追記
- スクリーンショットセクションに必要なら隠蔽パネル / エクスポートダイアログの
  画像を入れる (= ユーザー判断、スクリーンショットは別途撮影が必要)

## 8.3 開発者向け設計ドキュメント (`docs/`)

| ファイル | 更新内容 |
|---|---|
| `../../spec.md` | 「Ctrl+E エクスポート」節を新設。`Settings::export_*` の永続化フィールドを記載 |
| `../../keymap-spec.md` (なければ新設、なくても可) | `Ctrl+M` / `Ctrl+E` を全体ホットキー表に追加。隠蔽モード内ホットキー (T で類型切替、1-4 でプリセット) も |
| `../../architecture-overview.md` | `src/export_dialog.rs` を「UI 層」に追加。`save_with_metadata.rs` (既存) を「Persistence」に明記 |
| `../../preset-and-adjustment.md` | §4 (cache 階層) は変更なし、§5 (消しゴム) と §10 (隠蔽加工) の export 関連節を再確認 |
| `../../conceal-feature-plan.md` | Phase 6/7/8 完了をマーク (= 「Phase 6: …(完了)」のように)。詳細は変更しない (= 当初設計の記録として保持) |
| `../../async-architecture.md` | `export-worker` thread のエントリを §3 (worker 一覧) に追加 |

## 8.4 ../../README.md と CHANGELOG

リリース時 (= Phase 8 完了 = v0.9.x → 次バージョン) に更新するもの:

- `../../README.md` の「更新履歴」セクションに新バージョンエントリ追加
  - **書き方は CLAUDE.md「リリース手順チェックリスト Phase 0」に従う**:
    - 内部実装語 (`ShapeOp` / `Subtract` / `worker thread` / `mpsc`) 不可
    - バージョン番号タグ不可
    - 過去のユーザー報告日付不可
    - 「⚠️」は初回起動時の注意 (索引再構築等) に限定
  - ユーザーに ../../README.md 更新内容を**承認してもらってから**バージョン番号変更へ進む
- バージョン番号変更は CLAUDE.md「リリース手順 Phase 1」のチェックリストに従う
  (Cargo.toml / installer/mimageviewer.iss / installer/readme.txt / index.html /
  manual/index.html を一括同期、5 箇所)

## 8.5 UI スナップショット

[`tests/snapshots/`](../tests/snapshots/) には現状パネル系の PNG が無いので、
**新規追加は不要**だが、egui_kittest でのカバレッジ拡大は将来の安全網。Phase 8
スコープ外として既存方針維持。

## 8.6 Phase 8 完了条件

- `htdocs/mimageviewer/manual/conceal.html` と `export.html` が新設されている
- 全 17 マニュアルページのサイドバーが同じリンク一覧を持つ
  (= 上記 CLAUDE.md 整合性チェック sed で確認)
- `docs/spec.md` / `../../architecture-overview.md` / `../../async-architecture.md` が更新
  されている
- 製品ページ (`htdocs/mimageviewer/index.html`) に新機能が反映されている
- 全マニュアル / docs / README に **特定の投稿サイト名・基準名・外部ダウンローダ名**
  が含まれない (CLAUDE.md 横断ポリシー)
- 全マニュアル / docs / README に **バージョン番号タグ** (v0.9+ 等) が含まれない
  (= 現行版に存在する機能は現在形で書く、index.html のダウンロードバッジは除く)
- `python scripts/check_ui_glyphs.py` が exit 0 (= 危険な Unicode 文字が UI 文字列に
  入っていない)
- `python scripts/write_utf8_bom.py` で外部ツール向け Markdown に BOM が付いている
  (= Codex GUI に渡す ブリーフ系ドキュメントが必要なら)

---

# 全 Phase 共通: 着手前チェックリスト

着手前に毎回確認:

- [ ] `git status` で working tree が clean (= 前セッションの未コミット差分なし)
- [ ] `bash scripts/bootstrap-vendor.sh` 不要 (= 既存 vendor/ で開発するなら)
- [ ] 必要なドキュメント (本ファイル + 元設計の該当節 + CLAUDE.md 関連節) を読了
- [ ] 着手する Phase を 1 つに絞る (= Phase 6 が終わるまで 7/8 に手を出さない)

# 全 Phase 共通: コミット前チェックリスト

```bash
cargo fmt                                       # 全体 fmt (CLAUDE.md「Formatting」)
cargo check                                     # type check
cargo test --lib                                # 単体テスト
cargo test --test <new_test_file>               # 追加した統合テスト個別
cargo test --test ui_snapshot                   # UI snapshot (差分があれば UPDATE_SNAPSHOTS=1 で更新 + 目視)
cargo build --release                           # ランチャー含めて release ビルド通る
python scripts/check_ui_glyphs.py               # UI 文字列の Unicode 検査 (Phase 6 で UI 追加した場合)
git diff --check                                # whitespace チェック
git grep -i -E '<禁止サイト名>' -- '*.md' '*.html' '*.txt' '*.rs'   # 投稿サイト名混入チェック
```

pre-commit hook (`cargo fmt --check`) が `.git/hooks/pre-commit` に居る前提。
無ければ CLAUDE.md「Formatting」節の手順で再作成。

# 全 Phase 共通: コミットメッセージ

CLAUDE.md「Git Workflow」に従う:

- 1 行目: 内容を短く要約 (例:「Ctrl+E エクスポートダイアログ + worker を追加 (Phase 6)」)
- 本文: なぜ / 何を / どう変えたかを 1-3 段落。レビュアー (= ClaudeCode) が
  context 切替なしで読める粒度
- 末尾: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`

---

# 工数感 (元設計より)

| Phase | 元設計 | 残作業見積もり |
|---|---|---|
| 6 (Ctrl+E + バリエーション一括 + worker) | 4-5 日 | 3-4 日 (`save_with_metadata` が既にあるため -1 日) |
| 7 (統合テスト 4 ファイル) | 2-3 日 | 2-3 日 |
| 8 (マニュアル 2 ページ新設 + 15 ページ更新 + docs 更新) | 1-2 日 | 1-2 日 |
| **合計** | **7-10 日** | **6-9 日** |

実装順は **6 → 7 → 8** を強く推奨。前 Phase が固まらないと後 Phase の対象が動く。

---

# レビュー観点 (ClaudeCode 側のチェックリスト)

Codex が Phase 6/7/8 を完成させた各コミットでレビューする際、以下を確認する:

## Phase 6 レビュー
- [ ] `export_dialog.rs` の worker パターンが [docs/async-architecture.md §3](../../async-architecture.md)
      準拠 (cancel + mpsc + UI スレッドポーリング)
- [ ] `save_image_with_metadata_unique` を直接使い、`create_new(true)` で衝突回避
      (= 自前で seek + atomic rename を書いていない)
- [ ] セッション番号 (`_NNNN_`) の挟み込みロジックが UI スレッドで先に解決される
      (= worker thread 内で別エントリと競合しない)
- [ ] IME 対応: `dialog_enter_pressed` / `dialog_escape_pressed` 使用、`Settings` の
      確認
- [ ] 配色: ダイアログはテーマ追従、固定 dark にしていない
- [ ] エラー時の挙動: 1 件失敗 → 他続行、最終モーダルで集計表示
- [ ] HEIC 等のフォールバック: `ExportFallbackFormat` で JPEG/PNG にダウングレード
- [ ] ZIP / PDF: `src_bytes` 経路で動作
- [ ] 動画: 対象外 (toast 通知)
- [ ] 投稿サイト名 / 外部ツール名混入なし
- [ ] テストに UI 抜き worker テストが含まれる

## Phase 7 レビュー
- [ ] 4 新ファイルが `tests/` に追加されている
- [ ] 各テストが `TempDir` で隔離されている
- [ ] `#[ignore]` GPU テストが別経路で実行可能 (= 標準 `cargo test` ではスキップ)
- [ ] 失敗シナリオ (collision / partial failure / cancel) のアサーションが具体的
- [ ] レガシー JSON 互換テストで Add デフォルトを明示
- [ ] Subtract Shape の persistence テストが含まれる
- [ ] `cargo test --lib` + `cargo test --test <new>` 個別実行で全 pass

## Phase 8 レビュー
- [ ] 全 17 マニュアルページのサイドバーが同じリンク一覧
- [ ] 投稿サイト名 / 基準名 / 外部ツール名混入なし (`git grep` で確認)
- [ ] バージョン番号タグなし (= 「v0.9.x+」「v1.0 非対応」等)
- [ ] 実装語なし (`ShapeOp` / `subtract` / `worker thread` / `mpsc` 等)
- [ ] 製品ページに新機能反映
- [ ] `docs/` 側の設計ドキュメント (spec / architecture-overview / async-architecture)
      が更新済み
- [ ] `python scripts/check_ui_glyphs.py` exit 0

---

# Codex への補足 (ClaudeCode より)

このブリーフは前 2 件 (`../editing/erase-cache-refactor-plan.md` /
`../ui-input/panel-height-fix-plan.md`) と異なり、**1 ファイルで Phase 6/7/8 すべてをカバー**
している。Phase ごとに別コミットを作る運用なので、各 Phase の節だけを抜き読みして
コーディング → コミット → 次の Phase に進むスタイルで OK。

進行中に**設計判断が必要**になったら、ユーザー / ClaudeCode に確認すること
(= 「export_dialog.rs を ui_dialogs/ 配下に置く?」「fallback 形式の dropdown は
ダイアログ初回起動時に出す? それとも非対応形式選択時のみ?」等の **本ブリーフで
触れていない細部**)。曖昧なまま実装して後でレビュー指摘で巻き戻しになるより、
事前確認の方がコスト低い。

Phase 6 完了時点で UI を実機確認したら、ユーザーから追加 FB が来る可能性大
(= 過去の消しゴム / 隠蔽 / パネル高さの FB ラウンドと同じ流れ)。FB を見越して
**初回はミニマル機能で出す**のが手戻り少。「バリエーション 5 個 + プリセット」を
最初から完璧に作るより、「現在の設定のみ 1 個生成」をまず動かして FB を取り、
段階的に増やす方が安全。
