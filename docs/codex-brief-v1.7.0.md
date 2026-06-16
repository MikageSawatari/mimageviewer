# Codex 実装ブリーフ — mImageViewer v1.7.0

> このファイルは Codex GUI に渡すため **UTF-8 BOM 付き**で保存している (CP932 誤読による
> 文字化け回避、CLAUDE.md「Markdown のエンコーディング」方針)。

## 0. 進め方 (Codex 実装 → Claude Code レビュー)

- **実装は Codex が行い、完成後に Claude Code が全体レビューする。**
- レビューを通しやすくするため §3 の規約を守り、**機能ごとに論理的にまとまったコミット**にする。
- **設計の正本は §2 の 2 つのプランドキュメント**。本ブリーフはそれを実装に落とすための文脈・
  再利用マップ・受け入れ条件であり、設計を再記述しない。**確定事項は蒸し返さない** (下記 §4/§5 の
  「確定」は決定済み。迷ったらプラン doc と本ブリーフに従う)。

## 1. スコープ

v1.7.0 = **独立した 2 機能**:
- **機能A: 製本機能** (ページ収集 → 順序つき複製)
- **機能B: AI モデル名ファセット** (スマートフィルタ拡張)

両者は独立。実装順は自由 (小さい B から慣らす / 目玉の A から、どちらでも)。**別コミット
(または別ブランチ/PR) に分ける**こと。

**非スコープ (やらない)**: カラーマネジメント、参照型プレイリスト、スクリプトエンジン、
RAR/7z 直読み、Ctrl+G グローバルでのモデル検索 (将来)、製本の来歴/追加順ビュー、サイドカー。

## 2. 必読ドキュメント (実装前に読む / 正本)

- **docs/compile-book-plan.md** — 機能A の正本 (確定設計・複製方法・リネーム/フラッシュ・置き場所)
- **docs/ai-model-facet-plan.md** — 機能B の正本
- **docs/architecture-overview.md** — 全体像・モジュールマップ・永続化ストア一覧
- **docs/ui-responsiveness.md** — §4 チェックリスト + §2 worker テンプレ (新規 I/O は必ず通す)
- **docs/virtual-folders.md** — ZIP/PDF ページの複製経路で参照 (通常画像との分岐)
- **docs/keymap-spec.md** — 新規キー操作の追加規約
- **docs/details-view-and-filter-plan.md** — 機能B = `FacetFilter` 拡張の土台 (遅延列 worker 基盤)
- **docs/display-pipeline.md** — サムネ/描画/補正の適用順に触るなら
- **CLAUDE.md** — プロジェクト規約全般

## 3. 厳守する規約 (CLAUDE.md 抜粋)

- **UI スレッド同期 I/O 禁止**。ファイル読み・decode・`ctx.load_texture`・`read_dir`・SQLite 全件は
  worker + mpsc + cancel に逃がす (ui-responsiveness.md §4 を通す)。**特に製本の複製・リネーム
  フラッシュ・ホバープレビュー decode は worker 化必須**。
- **新規キー操作**は `KeyAction` + helper 経由。`ini_name()` / `context()` / `trigger()` /
  `default_chords()` / `ALL_ACTIONS` / `docs/keymap.ini.default` を揃える。
- **TextEdit を含むダイアログ**の Enter/Escape は `dialog_enter_pressed` / `dialog_escape_pressed`
  を使う (IME 変換の Enter/Escape 破壊を防ぐ)。新ビューポートを足すなら入口で `update_ime_state`。
- **コミット前に `cargo fmt` (引数なし・全体)**。pre-commit フックが番人。
- **UI 文字列は安全なグリフのみ**。`python scripts/check_ui_glyphs.py` を通す。`✕`/`🎚` 等は使わず
  `×` (U+00D7) 等に。
- **テスト**: `cargo test --bin mimageviewer-core` (app::tests を含む)。リリース前相当のフルは
  `cargo test` (パイプ無し・実 exit code)。
- **配色/レイアウトを変えたら** egui_kittest スナップショット (`UPDATE_SNAPSHOTS=1 cargo test
  --test ui_snapshot`) を更新し PNG を目視確認。
- **永続データ**: 両機能とも**未リリース = マイグレーション不要**。`settings.db` 新規フィールドは
  既存 migration 経路で後方互換に追加。製本フォルダは番号付き画像のみの素フォルダ (スキーマなし)。
- **ドキュメント同時更新** (§7)。

---

## 4. 機能A: 製本機能

**正本: docs/compile-book-plan.md。** 以下は確定事項の要点 (詳細・根拠は doc)。

### 確定設計 (蒸し返さない)
- **本 = コピー型スナップショット**。複数の本/フォルダから**ページ**を集めて任意順に並べた束。
- **本の実体 = ただの実フォルダ**。中身は**番号付き画像ファイルのみ**。**マーカー/サイドカーを
  一切置かない**。
- **本の識別 = 場所ベース**: 製本ルート直下の各フォルダを本とみなす。ルート外へ移動したら通常
  フォルダ扱いでよい。
- **ページ順 = `NNNN_元名.ext` の 4 桁ゼロ埋め連番が正本** (ファイル名順 = ページ順)。
  **1 冊上限 9999 ページ、10000 ページ目の追加はエラーで拒否**。
- **複製方法 (判定順)**: ①元ページにアクティブな編集 (補正/レイヤー/隠蔽/注釈/AI) があれば
  焼き込みエンコード。無ければ ②標準画像ファイル = byte コピー、③アーカイブ内画像 = 格納 bytes
  抽出 (再エンコードなし)、④PDF ページ/動画フレーム = decode → エンコード。
  **エンコードが要るのは PDF/動画/焼き込みの 3 ケースのみ**。
- **編集中はメモリ上の順序、リネームは遅延フラッシュ**。フラッシュトリガ = **並べ替え専用モード
  退出時**。フラッシュは **2 パス (全 temp 名 → 全 final 名) で中断耐性**、worker、
  対象フォルダの**監視を一時停止**して実行。
- **本フォルダは検索インデックス対象外**かつお気に入り登録対象外。本ページは追加時の焼き込み済み画像を
  正本にしつつ、mIV 内部 DB のタグ/★/補正/消しゴム/補正レイヤー/隠蔽/注釈/切り取りは後段で許可する。
  グローバル補正やお気に入り標準は継承せず、非破壊回転はページ順・見開きとの混乱を避けるため抑制する。
- **並べ替えは専用モード**: 本棚表示時の「並べ替え」ボタンで、小サムネ敷き詰め + ホバーで拡大
  プレビューの専用ページに入り、その中だけで drag 並べ替え。**通常グリッドのインライン drag は
  使わない** (shell ドラッグアウトとの曖昧化・ウィンドウ外誤コピーを避けるため)。
- **置き場所 = 全ビルド共通 `Pictures\mimageviewer\books` 既定・ユーザー設定可** (portable も
  Pictures)。**`capture.rs` の `default_output_dir()` は変更しない** (キャプチャ/エクスポート既定は
  現状維持)。`data_dir.rs` も触らない (books は data_dir ではなくユーザーコンテンツ根)。
- **追加トリガ = ショートカット 1 つ**で「追加先の本」へ複製追加。文脈別:
  グリッド = カーソル位置 / 選択 (チェック) があれば選択画像すべて、画像表示 = その画像、
  動画 = 現在の再生フレーム。
- **既定は名前なしの本** 1 冊が追加先の本。製本画面でリネーム/本の追加削除/追加先の本の選択。
- **mIV 内表示は「本棚 > 本名」の仮想ネームスペース**、右クリックで実フォルダを開く。

### 再利用マップ (既存資産)
| 必要な処理 | 既存 |
| --- | --- |
| ページ画素の取得 (通常/ZIP内/PDF) | `export_page_pixels_for_idx` 系 (Ctrl+E export 経路) |
| 動画の現在フレーム | `src/capture.rs` (Ctrl+S フレームキャプチャ) |
| エンコード+保存 (JPEG/PNG/WebP・メタ保持) | `save_with_metadata::save_image_with_metadata`、`src/export_dialog.rs` (`ExportFormat`/`SaveOptions`) |
| ファイルコピー/リネーム | `src/shell_file_ops.rs`、`std::fs`、worker パターン |
| Pictures 解決の作法 | `src/capture.rs` の `default_output_dir()` / `pictures_dir()` を**参考に** books ルートを別途作る (default_output_dir は変えない) |
| フォルダ=グリッド表示・見開き・通し読み | 既存グリッド (本=実フォルダなので追加ほぼ不要) |
| ページ順=ファイル名自然順 | 既存の自然順ソート (`ui_helpers::natural_sort_key`)。新ソート種は不要で自然順でよい |
| 索引除外 | `src/indexer_manager.rs` / 自動インデックス対象判定で books ルート配下を除外 |
| キー操作 | `KeyAction` + keymap (§3) |

### 注意
- 並べ替え専用モードは**独立サーフェス**。`src/ui_main.rs` のセルは既に `Sense::click_and_drag` +
  shell ドラッグアウト (`src/file_drag.rs` / `SHDoDragDrop`) を持つが、**専用モードにはその経路を
  持ち込まない**。仮想スクロールでの挿入位置ヒットテスト + 挿入インジケータが主作業。
- 補助としてキーボード/ゲームパッドの「前へ/後ろへ移動」も用意するとドラッグ前に最低限動く。
- §B の細部 (衝突サフィックス規則・エンコード品質・並べ替え実装段階・追加先の本の切替 UX・
  削除時の実体扱い・重複追加可否) は妥当な既定で実装し、後で調整する前提でよい
  (compile-book-plan.md §10-B)。

---

## 5. 機能B: AI モデル名ファセット

**正本: docs/ai-model-facet-plan.md。**

### 確定設計
- **既存の `FacetFilter` に「AI モデル」軸を追加** (種類/拡張子/★/タグ/日付/サイズ/状態 に並ぶ
  もう 1 つのファセット)。副次的に「生成ツール」軸も追加可 (モデルだけ先行も可)。
- **非永続** (セッション/遅延キャッシュ)。catalog 等への永続列は作らない。
- **スコープ = 現在グリッドのみ** (Ctrl+G グローバル索引は将来)。
- **第二弾 (遅延) ファセット**として、`details-view-and-filter-plan.md` の遅延列 worker 基盤に
  乗せる (可視範囲から順に decode、Ready まで「読み込み中」、UI スレッドで decode しない)。
- モデル名抽出は `src/png_metadata.rs` の `AiMetadata` から純関数 `model_name(meta)` を新設:
  A1111 = params の `Model`、SwarmUI = `model`、Fooocus = `base_model`、InvokeAI = `model`、
  EasyDiffusion = `use_stable_diffusion_model` (basename 化)、**ComfyUI = best-effort** (checkpoint
  ノード名を集める / 「(複数)」)、**NovelAI = Source の hash のみ** (人間可読名なしなら「(NovelAI)」)。
  抽出不能/非 AI は「(モデル情報なし)」グループ。
- 正規化は **basename + 拡張子除去**で開始 (後で調整可、非永続なので無コスト)。

### 再利用マップ
| 必要な処理 | 既存 |
| --- | --- |
| ファセット状態 | `src/settings.rs` `FacetFilter`、`src/app/metadata_ops.rs` `facet_kind_for_item`/`facet_ext_for_item` |
| ファセット件数・チップ UI | `src/ui_main.rs` のタグファセット件数 (`facet_tag_counts`) パターン |
| メタデータ抽出 | `src/png_metadata.rs` (`AiMetadata`、A1111 params に `Model` 抽出済み) |
| 遅延読み | `details-view-and-filter-plan.md` の遅延列 worker |
| 実サンプル (テスト) | `H:\home\mimageviewer_old\testimage\metadata` (`MIV_AI_METADATA_SAMPLE_DIR` で smoke 有効化) |

---

## 6. テスト / 受け入れ条件

実行: `cargo test --bin mimageviewer-core` (app::tests 含む)。AI メタサンプルは
`MIV_AI_METADATA_SAMPLE_DIR=H:\home\mimageviewer_old\testimage\metadata`。

機能A:
- メモリ順序 → `0001..` 連番フラッシュが衝突なく完了 (2 パス temp 名)、中断しても前回コミット状態が
  無傷。ファイル名自然順 = 意図したページ順。
- 複製 4 系統 (通常画像 byte コピー / ZIP 内 bytes 抽出 / PDF ページ encode / 動画フレーム encode)。
- **元ファイル削除後も本が表示・通し読みできる** (コピー型の核心)。
- **9999 ページの本に 10000 ページ目を追加 → エラー拒否**。
- 本フォルダが検索インデックスに取り込まれない / 本の中で Ctrl+F・ツールバー絞り込みは in-memory で効く。
- 並べ替えバッチ中に UI が固まらない / 監視ストーム抑制。

機能B:
- `model_name(meta)` の形式別 (A1111/SwarmUI/Fooocus/InvokeAI で friendly 名、ComfyUI best-effort、
  NovelAI/非 AI で「情報なし」)。合成フィクスチャ + 実サンプル smoke。
- ファセット件数 = フィルタ後件数の整合。遅延読み込み中に UI が固まらない。

## 7. ドキュメント同時更新

- **docs/compile-book-plan.md / ai-model-facet-plan.md** のステータスを実装済みに更新。
- **docs/spec.md** の機能一覧。
- **htdocs/mimageviewer/manual/ + index.html** (ユーザー向け。内部用語/実装語を出さない方針。
  マニュアルは全 14 ページのサイドバー整合を保つ)。
- 必要に応じ **architecture-overview.md** (新ユーザーコンテンツ根) / **search-architecture.md**
  (モデル抽出経路・索引除外) / **virtual-folders.md** (ZIP/PDF ページ複製) / **keymap.ini.default**。

## 8. ブランチ / コミット

- **v1.7.0 用の feature ブランチ**で作業 (master/main へ直接書かない)。
- 機能A / 機能B を**別コミット (または別ブランチ/PR)** に分け、レビュー単位を明確に。
- コミットメッセージは「何を/なぜ」。fmt 済みでコミット。
