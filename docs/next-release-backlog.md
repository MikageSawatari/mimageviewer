# 次リリース検討バックログ

このファイルは、まだ着手していない作業候補だけを置く恒久バックログ。
完了した項目はコミット履歴・リリースノート・個別設計メモに任せ、このファイルからは削除する。

運用ルール:

- 着手前に `docs/README.md` から該当領域の設計ドキュメントを読む。
- 着手中のものだけ `対応中` と明記してよい。完了したらこのファイルから削除する。
- 判断保留・見送りの理由は、次に再判断する人が困らない最小限だけ残す。
- 依存ライブラリ更新は `CLAUDE.md` のリリース手順チェックリスト Phase 2 と整合させる。

---

## 1. 優先候補

### 1.1 F12 別ウィンドウ中のソート変更で動画セッションが一度閉じる

- 背景: F12 の別ウィンドウで動画を表示中にメイン一覧側でソート順を変更すると、
  元の動画アイテムがソート後の一覧に残っていても動画ウィンドウが一度閉じる。
- 現状: ソート変更時の通常フォルダ再読み込みが `start_loading_items` 経由で走り、
  共通処理の `close_fullscreen()` により detached viewer session / native video presenter が
  破棄される。後段の detached viewer 同期は、session が閉じた後なので復帰できない。
- 方針:
  - ソート / フィルタ後の同一フォルダ再構築では、変更前の表示対象を idx ではなく
    `metadata_cache_key` / `ViewerSyncStamp` 相当の安定キーで保持する。
  - 再構築後の `items` から同じキーを探し、見つかれば detached session を閉じずに
    `fullscreen_idx` / `selected` / 同期 stamp を新 idx へ付け替える。
  - 見つからない場合、またはフォルダ移動・ZIP/PDF 仮想フォルダ遷移など同一対象維持が
    不明な場合は、既存どおり session close を許可する。
  - 静止画だけでなく detached 動画の native presenter / `fs_cache` / pending source swap への
    影響を確認し、通常 fullscreen とフォルダ移動を巻き込まないテストを先に用意する。
- 規模 / リスク: Medium / 中。修正方針は明確だが、`start_loading_items` / `close_fullscreen`
  周辺は fullscreen lifecycle、動画 presenter、index keyed cache、履歴復元にまたがるため
  リリース直前の小修正にはしない。
- 優先度: P3。UX 改善として次回以降に対応。v2.1.0 のリリースブロッカーにはしない。

### 1.2 画像専用の複数別ウィンドウ表示を検討

- 背景: mImageViewer 専用スレ 3 (総合スレ 864 への返信)。「画像表示ウィンドウの
  複数展開、別アーカイブの画像を同時表示ができない」という要望。
- 現状:
  - 既存の F12 別ウィンドウは、メイン一覧と同期する単一 viewer session として設計している。
  - `docs/detached-viewer-implementation-plan.md` でも、複数の独立ビューアウィンドウは
    初期設計の対象外としている。
  - 動画は native presenter / decoder / 音声 / overlay / focus 周りが単一 session 前提のため、
    複数化すると影響範囲が大きい。
- 方針:
  - 初期検討は静止画のみ。動画、動画フレーム再生、native presenter は対象外にする。
  - 目的は「別アーカイブや別フォルダの画像を見比べる」用途なので、既存の F12 同期ビューアとは
    別に、固定対象を持つ追加ビューアウィンドウを開ける形を検討する。
  - 対象候補は通常画像、ZIP 内画像、PDF ページ、変換済みアーカイブのキャッシュ画像。
    実装時は virtual item の stable key と元コンテナ情報を保持し、一覧のソート / フィルタ /
    フォルダ移動で idx が変わってもウィンドウが別画像へ化けないようにする。
  - 初期版は閲覧、ズーム、フィット、ページ送りなど必要最小限に絞る。補正編集、右パネル、
    ゲームパッド操作、スライドショー、動画、複数 window 間の同期は後回し。
  - 既存の F12 別ウィンドウの「メイン一覧カーソルへ追従する」挙動は維持し、追加ウィンドウは
    それとは別の明示オープン機能として扱う。
  - 各ウィンドウごとに current item key、container key、page index、zoom / pan / fit、
    texture / cache 参照を持つ必要がある。`fullscreen_idx` など singleton 状態へ直接ぶら下げない。
- 確認:
  - A.zip のページと B.zip のページを別ウィンドウで同時表示し、それぞれ独立して閉じられる。
  - メイン一覧の移動、ソート、フォルダ移動、F12 別ウィンドウの開閉で追加ウィンドウが誤って閉じたり、
    別画像へ切り替わったりしない。
  - 元コンテナが消えた / 変換キャッシュが失効した場合のエラー表示と close cleanup を確認する。
- 優先度: P3。需要はあるが設計変更が大きめなので、キー / マウスカスタマイズの後で検討する。

### 1.3 F11 仮想フルスクリーン中に Ctrl+↓ すると最大化ウィンドウに化ける

- 背景: 別ウィンドウ (detached viewer) を F11 で仮想フルスクリーン (装飾なし・モニタ全面の
  borderless) にした状態で Ctrl+↓ (フォルダナビ) すると、仮想フルスクリーンが解除され
  「最大化した通常ウィンドウ」になってしまう。**v2.2.0 でも再現する既存バグ**で、複数別
  ウィンドウ作業による退行ではない (2026-06-28 ユーザー報告)。
- 現状 (要調査):
  - F11 は `detached_viewer_borderless_fullscreen=true` + 復帰用 `detached_viewer_restore_placement`
    を立て、`build_detached_viewer_viewport_builder` が borderless 時は
    `detached_viewer_borderless_target_rect()` (decorations=false + モニタ全面)、非 borderless 時は
    通常 placement + `with_maximized(placement.maximized)` を使う。
  - `close_fullscreen_for_folder_nav_reopen` は `detached_viewer_borderless_fullscreen` /
    `detached_viewer_restore_placement` を preserve するが、folder-nav reopen 経路
    (`detached_viewer_folder_nav_reuse_window_once` → open_fullscreen が prepare/placement 再適用を
    skip) で borderless geometry が再適用されず、装飾あり + maximized の状態へ落ちている疑い。
- 方針:
  - folder-nav reopen 時に borderless 状態の detached window では、builder が borderless geometry
    (decorations=false + target rect) を再適用するようにする。通常 placement / `with_maximized` へ
    フォールバックさせない。
  - F11 トグル (`detached_viewer_borderless_fullscreen` の set/解除と restore placement) と、
    folder-nav / 別ファイル open 経路の builder 選択 (apply_placement / borderless 分岐) を突き合わせ、
    どのフレームで borderless が失われるか perf/debug ログで特定してから直す。
- 確認:
  - F11 仮想フルスクリーン中に Ctrl+↑↓ / 次ファイル移動しても borderless のままで、最大化や
    装飾ありに化けない。F11 解除時は元の通常ウィンドウ配置へ戻る。
- 優先度: P3。既存バグだが UX 影響は限定的。detached viewport ライフサイクル整理
  (`docs/detached-viewer-lifecycle-redesign-proposal.md`) と一緒に着手すると効率的。

### 1.4 UI スケール設定 (表示倍率) — メニュー「設定 > スケーリング」

- 背景: 高 DPI モニタで UI 文字が小さいという不満への対策。egui 既定のキーボードズーム
  (Ctrl+±) が実は効く (`Options::zoom_with_keyboard`) ことにユーザーが気付いたのが発端。
  これを隠し機能のままにせず、正式な設定として 50%〜200% を選べるようにする。
- 正本: **`docs/ui-scale-plan.md`** (Codex レビュー済み、実装可能粒度)。以下は要約。
- 確定仕様:
  - **範囲/刻み**: 50%〜200% を 10% 刻み (16 段)。
  - **UI**: メニュー「設定 > スケーリング」サブメニューから選択 (現在値にチェック)。
  - **単一の真実源 = メイン egui `ctx.zoom_factor()`**。`settings.ui_scale_factor` に永続化し、
    起動時に `set_zoom_factor` で適用。キーボードズーム (Ctrl+±) は初回は**無効化**
    (メイン + presenter overlay の両 Context で `zoom_with_keyboard=false`)。
  - **VST プラグイン GUI 画面のみ非スケール** (別 HWND、bridge 所有で拡縮不可・利用者少で許容)。
    それ以外 — メイン窓 UI、静止画フルスクリーン、**動画 overlay の通常 panel (タグ設定 /
    チャプター・ブックマーク一覧 / HUD)** — は全て反映する。
- 実装の要点 (詳細は正本):
  - 動画 overlay は native presenter の**別 egui Context**が描く。presenter の単一 ppp 源
    `self.pixels_per_point` に `(dpi/96) × ui_scale` を注入すれば overlay 全体が自己整合してスケール。
    伝搬は `NativeVideoOutput` に `cur_ui_scale` を持ち presenter 再生成時に再適用 (detached も同経路)。
  - OS DPI を直読みする箇所 (`native_video.rs` の VST overlap ~1407 / tile size ~4533、
    `video/mod.rs:2859` の WM_DPICHANGED、`mod.rs:7858` 初期 DPI) を `× ui_scale` に統一。
  - **50% (100% 未満) 対応**: presenter の `max(1.0)` 床 4 箇所 (`compute_hud_regions` mod.rs:4971 /
    cursor polling 2834 / IME 7734 / ring guide overlay_draw.rs:1962) を生 ppp に直す。
    **ppp ≥ 1.0 では no-op** なので既存 (100%以上) 挙動への回帰リスクほぼゼロ。
  - サムネ解像度はスケール連動のまま (2048px cap で許容)。未リリース機能につき settings migration 不要。
- 規模 / リスク: Medium / 低〜中。コード変更自体は小さく回帰リスクは低いが、動画 HUD が
  歴史的に finicky なため **50% での動画 HUD 実機クリック確認**が要る (正本 §5.1 にチェックリスト:
  軽い確認 + リング/タイル/小アイコン/長文ダイアログ/IME/VST overlap を各 1 回)。VST overlap のみ
  実プラグイン確認。多モニタ DPI 全網羅は不要 (DPI 変更パス流用)。
- 影響なし確認済み: VST3 音声処理・キャプチャ・スナップショットテスト・detached 窓配置判定。
- 優先度: P2 candidate。設計・Codex レビュー済みで着手しやすい。動画まわりに触るため、
  操作カスタマイズ系 (4.x) と時期が重なる場合は presenter への同時変更に注意。

### 1.6 グリッドのソートで実フォルダ / アーカイブ類 / 画像 / 動画・音声の表示順を設定

- 状態: **実装済み**。既定は従来互換の 1 行目「実フォルダ + アーカイブ類」、2 行目
  「画像 + 動画・音声」。全文検索結果は従来どおり一律ソート。

- 背景: mImageViewer 専用スレ 37。当初要望は「ソートのオプションでファイルとフォルダを
  分けて表示したい。フォルダを前 / 後に配置も選べると尚よい」。専用スレ 39 で、要望の主旨は
  「フォルダを通常ファイルと混ぜて名前順にする / フォルダだけ先頭へ寄せる / フォルダだけ末尾へ
  寄せる」を選びたいというものだと分かった。
  一方、mIV では ZIP / PDF / 変換アーカイブを「本」コンテナとして扱う使い方や、
  「画像のみのフォルダを本として扱う」設定もあるため、アーカイブ類を常に画像・動画側へ
  混ぜる固定仕様にすると不便になる利用者も出そう。
- 現状 (調査済み 2026-07-08):
  - スキャン時点で `folders` (Folder / ZipFile / PdfFile / ConvertibleArchive) と `all_media`
    (Image / Video / Audio) の 2 配列に分離済み (`src/app/folder_scan.rs`
    `scan_directory_with_convertible_archives`)。
  - ただし `folders` ブロック内は 4 種を**種別区別せず** `sort_order` だけで一列に並べる
    (`crate::grid_item::sort_folder_block`, `src/grid_item.rs:447`)。結果、実フォルダと ZIP / PDF /
    変換アーカイブが名前順で交互に混在する (例: `apple/` `banana.zip` `cat/` `dog.pdf`)。
  - `items` の組み立ては「`folders` 先頭 → `all_media`」の固定 2 段構成 (`src/app.rs:12574` 付近)。
    前後の入れ替えはできない。
  - ソート順は `folders` / `all_media` とも同一の `sort_order`
    (`self.book_sort_order_for_path(&path)`, 通常 `settings.sort_order`) を使う。グループ別の
    ソート順は持っていない (`src/app.rs:12530-12542`)。
- 方針:
  - 表示順を 4 段の表形式で設定できるようにする。対象カテゴリは
    `実フォルダ` / `アーカイブ類 (ZIP / PDF / 変換アーカイブ / 直接閲覧 RAR)`
    / `画像` / `動画・音声`。
  - UI イメージ:

    | 表示順 | フォルダ | アーカイブ類 (ZIP/PDF/RAR 等) | 画像 | 動画・音声 |
    | --- | --- | --- | --- | --- |
    | 1 | □ | □ | □ | □ |
    | 2 | □ | □ | □ | □ |
    | 3 | □ | □ | □ | □ |
    | 4 | □ | □ | □ | □ |

  - 各カテゴリはいずれか 1 行にだけ所属する。空行は許可して詰めて扱うか、保存時に正規化する。
  - 同じ行に複数カテゴリを入れた場合は、同一グループとして**混ぜて `sort_order` でソート**する。
    例: `アーカイブ類 + 画像 + 動画・音声` を同じ行にすると、`A.zip / C.rar / E.lzh / page01.jpg`
    のように通常ファイル側として混在できる。`フォルダ` を別行にすればフォルダ前置 / 後置が可能。
  - 既定値は既存挙動に近い `1: フォルダ + アーカイブ類`、`2: 画像 + 動画・音声` とする。
    ただしリリース前に、初回表示の分かりやすさを優先して
    `1: フォルダ`、`2: アーカイブ類`、`3: 画像`、`4: 動画・音声` を既定にするか再判断する。
  - **各グループ別のソート順**までは持たせない。各行の中は従来どおり同じ `sort_order` を使う。
  - `items` を組み立てる経路が複数あるため、グループ順の適用を共通ヘルパーに集約して全経路で
    揃える: メインの `load_folder_with_scan` (`src/app.rs`) / ZIP 内列挙 (`finalize_zip_enumerate`) /
    ファイル名スタックの materialize (`src/filename_stack.rs`) / レーティング一覧 / サブフォルダ
    展開ビュー。全文検索の結果一覧 (`src/global_search_ui.rs:782` `build_flat_items`) は元々
    「フォルダ / ファイル区別なく一律ソート」なので、この表示順設定を適用するか従来どおりにするかを
    別途決める。
- 永続化 / 移行:
  - 4 カテゴリの行割り当てを `Settings` に新フィールドとして追加する。
    例: `Vec<Vec<GridItemDisplayKind>>` または固定長 `[DisplayGroup; 4]`。保存時はカテゴリ重複 /
    未所属を正規化し、壊れた設定は既定値へフォールバックする。
  - 既定値を既存互換にする場合は「重要な変更点」告知は不要。既定値を分離寄りに変える場合は
    `version_highlights.rs` へ追記する。
- 規模 / リスク: Medium / 低〜中。中核の並べ替えロジックは小さいが、`items` 構築経路が分散して
  いるため各経路への反映漏れに注意する。着手前に `docs/virtual-folders.md` (グリッド構成) と
  `CLAUDE.md` の「サムネイルロード / Grid contents」節を読む。
- 優先度: P3。今後のバージョンで検討 (2026-07-08 ユーザー要望)。すぐの実装は予定しない。

### 1.7 detached 中の発火面解決の残り (ゲームパッド / マウス割当ボタン) — BA 報告

- 背景: v2.3.0 出荷前監査 (2026-07-09/10、docs/review-v2.3.0/claude-audit-selection.md)。
  監査当初の指摘のうち、以下は 2026-07-10 に**解決/反証済み**:
  - コンテナ★ (viewer 内 Shift+F1-F6) / FsPin (P): **誤検出と確定**。native キー処理は
    `with_active_detached_viewer_context` 内 (bundle マウント) で走り、`current_folder` /
    `zip_nav` は bundle swap 対象のため、窓自身のフォルダに正しく付く。契約テスト
    `container_rating_in_bundled_media_window_targets_window_folder` で固定済み。
  - グリッド Delete キー無効 / スタック集約の保留: **修正済み** (v2.3.0)。
  - マウス右ドラッグリング/ジェスチャ: **問題なしを検証済み** (開始時に面を焼き付け、
    ガイド描画と適用は面不一致で棄却。tests.rs の cross-surface テストで固定)。
- 残り (設計判断が要る、detached リワーク後続で対応):
  1. **グローバル入力のコンテキスト解決** (P2×2): `current_ring_shortcut_context`
     (src/app/gamepad_input.rs) が fullscreen_idx 優先のため、unbundled detached 窓の
     表示中にゲームパッド X リング (ItemRating 等) やマウス割当ボタン (中クリック=回転等の
     カスタム割当時) がグリッド操作のつもりでも窓のアイテムに効く。付随して detached 中は
     パッドでグリッド操作が一切できない + パッドリングガイドがグリッド上に FS 用ラベルで
     出る。対応 = パッド/マウス割当にも発火面 (前面ウィンドウ or 最後に触った面へ追従) の
     設計を導入する (ユーザー方針 2026-07-10: 前面 or 最後に触った場所追従が自然)。
  2. **? キーのショートカット一覧** (P3): detached 中にグリッドから開くと FS 用一覧が
     出る。修正には発火元の面情報を consume サイトから通す必要がある。
  3. **トーストの面粒度** (P3): 発火面のビューアを閉じた後に完了したバッチのトーストが
     不可視のまま消える / active detached + main fullscreen 同時表示時に Viewer 面が
     両ビューアに出る (2 値 enum の粒度の限界)。許容中。
  4. **複数ウィンドウ×スタックの理想形** (P3 改善): スタックをクリックするとメイン一覧が
     フラット読書ビューに切り替わる (フル機能と同じ挙動)。理想は「メインは集約のまま、
     窓側だけフラット文脈を持つ」だが、窓が自前のフラット items 文脈 (bundle 化された
     stack 状態) を持つ §1.2 系の設計が必要。現行でも Shift+↓↑ ジャンプは動作する。
- 優先度: P2〜P3。detached リワークの後続ステージ (発火面設計の一般化) で対応。

### 1.8 rename transaction — path-keyed 永続データの移行 (タグ/★/回転/編集レイヤー等)

**→ 2026-07-10 に段階 1+2 とも v2.3.0 で実装済み** (ユーザー判断「広めに実機検証を行う
今回のうちに段階 2 まで」)。実装 = `src/rename_key_migration.rs` (worker、
zip_key_migration 方式の UPDATE OR IGNORE + DELETE、substr 等値 prefix 照合) +
`App::spawn_rename_key_migration` / `poll_rename_migration_pending` / in-memory
presence set・resume キー書換。対象ストア・許容制限の正本はモジュール doc。
以下は当時の調査記録として保持。

- 背景: v2.3.0 角度④レビュー (docs/review-v2.3.0/sol-angle-reviews.md (C))。rename は
  v2.2.0 出荷時から path-keyed DB を一切移行しておらず、リネームでタグ/★/回転/
  編集レイヤー/マスク/ブックマーク/再生位置などが旧 path キーに置き去りになる。
- **ストア全数調査済み (2026-07-10、Explore agent)**。移行必須 (authoritative):
  - page-key 8 ストア (rating / adjustment / mask / conceal / local_adjust / comic /
    export_crop / tags) — **`App::apply_book_page_edit_moves` (app.rs:20099) /
    `move_book_page_edit_key` (20195) がほぼそのまま使える前例** (per-store
    `move_entry_key` helper + 2 相 temp-key + in-memory cache 更新まで実装済み)
  - 前例に**入っていない**ギャップ: rotation.db / video_pins.db / video_bookmarks.db
    (=音楽ブックマーク) / settings.video_resume_positions (in-memory map キー改名) /
    **動画 `.xmp` sidecar のファイル改名** (xmp_writer::sidecar_path_for、現状
    リネームで孤児化) / mimageviewer.dat の rel-key (sidecar backup 有効時のみ) /
    pdf_passwords (キーが SHA-256 ハッシュ → remove+set、平文がセッション内にある時のみ) /
    コンテナ行 (reading_history / book_resume / spread / folder_thumb_pins 自行)
  - スキップ可 (rebuildable): サムネ catalog / tile・chapter thumbs / audio_normalize /
    auto_aspect / 検索索引 (search_watcher が rename を Remove+Upsert 分解して自己修復) /
    archive_cache
- キー正規化は 2 系統ある点に注意: `adjustment_db::normalize_path` (drive 保持、主流) と
  `path_key::normalize` (drive 除去: spread / book_resume / pdf_passwords / archive_cache)。
- **段階案**:
  - 段階 1 = 単一ファイル rename (画像/動画/音声 + コンテナ自体の exact キー)。前例 +
    ギャップ helper 追加 + sidecar 改名。目安 1 日 (Codex 検収込み)、リスク中低。
  - 段階 2 = フォルダ rename の配下 prefix 書換 (`old/%`) + コンテナ rename の
    アーカイブ内 composite キー (`old::%`) + drive 除去系 + sidecar_sync/tag_sidecar_sync。
    SQL 雛形 = src/zip_key_migration.rs (`UPDATE OR IGNORE` + `DELETE`)。目安 +1〜1.5 日、
    リスク中 (キー規則 2 系統の混在に注意)。
- 優先度: P2 (ユーザー要望 2026-07-10「可能ならば設定値を引き継ぎたい」)。

### 1.9 parked 窓のリソース制御 (サムネ pipeline 停止 / VRAM 合算予算) — 角度⑥送り

- 出典: v2.3.0 角度⑥レビュー (Sol/Terra 一致、docs/review-v2.3.0/sol-angle-reviews.md)。
  いずれも bounded な効率問題でデータ喪失は無し。park/再活性ライフサイクルの構造に
  踏み込むため、リリース直前パッチではなく detached リワーク後続で設計対応する。
  1. **P2: park してもサムネ pipeline が止まらない**: `pause_background_work_keep_current_frame`
     は fullscreen/AI 系のみ cancel し、bundle の `cancel_token` / `reload_queue` /
     `heavy_io_queue` に触れない。park 時点で積まれていたデコードが走り切り、結果
     `ColorImage` が誰も poll しない rx に溜まる (窓の再活性化 / close まで保持)。
     対応案 = park 時に queue を drain (worker pool は殺さない)。ただし Requested 状態の
     サムネが再活性化時に再要求される仕組みの確認が必要 (state が Requested のまま
     queue から消えると復帰後にロードされない恐れ)。
  2. **P2: サムネ VRAM 上限が文脈単位**: `update_keep_range_and_requests` の予算は
     mounted 文脈にしか効かず、parked bundle N 個がそれぞれ上限近くまで保持し得る
     (動画サムネは eviction 対象外なのでフォルダサイズ分)。対応 = 全 bundle 合算の
     予算会計 + cross-bundle eviction (リワークの資源予算ステージ)。
  3. P3: `display_px_shared` が App-global のため、detached 文脈のデコード解像度が
     メイングリッドの表示密度に引きずられる (適用ミスは無し、品質/CPU の無駄のみ)。
- 関連: v2.3.0 で対応済みの境界 = bundle Drop の解放 (角度①でクリーン確認)、
  文脈別 channel/cancel/世代 (P2-9)。

### 1.10 終了時の削除 worker 未調整 — 角度⑤送り (P2、実害小)

- 出典: v2.3.0 角度⑤ Sol P2。数百件削除の実行中に終了すると、実行中の
  IFileOperation チャンクは完走するが後続チャンクは開始されず、部分削除の最終報告と
  完了後クリーンアップ (resume purge 等) が走らない。一覧は次回起動の走査で実態に
  収束するため破壊は無し。対応案 = 終了時に cancel を立てて現行チャンクの完了を
  短時間待つ + 次回起動時の注意トースト。v2.2.0 出荷時からの既存挙動。

### 1.11 視聴中ファイル削除の初回失敗 (sharing violation リトライ)

- 出典: v2.3.0 実機検証 §4 #24 (2026-07-10)。別窓で再生中のファイルを削除すると、
  (A)(B) 実装どおり窓は閉じるが、プレイヤーのファイルハンドル解放が非同期のため
  初回の削除が共有違反で「削除に失敗しました」になることがある (2 回目で成功)。
  ユーザー許容済み (「この動作でも大丈夫」)。
- 対応案: 削除 worker 側で ERROR_SHARING_VIOLATION 時に短時間リトライ
  (例: 200ms×5 回バックオフ)。削除対象が再生解放直後のケースだけ効き、
  他の共有違反 (他プロセス占有) はリトライ後に従来どおり失敗表示。

### 1.12 detached 静止画窓から音声/動画へのフォルダ内ナビ不可 (メディア昇格導線)

- 出典: v2.3.0 実機検証 §5 #33 (2026-07-10)。別窓 (静止画) で ↑↓ ナビ中、音声/動画の
  アイテムへは移動できない (静止画窓はメディア再生セッションを持てない設計境界)。
  音楽ビュー→画像の方向は移動できるため非対称。ユーザー許容済み (「一旦これでもよさそう」)。
- 対応案: detached リワークの後続ステージで「静止画窓がメディアに到達したらメディア
  セッションへ昇格 (または既存メディア窓へ委譲)」の導線を設計。凍結ルール下では
  症状パッチを入れない。

### 1.13 duration 不明/不正 MPEG-PS のシークバー不能

- 背景: `sample.mpg` で実機確認済み。duration が不明または不正な MPEG-PS は
  シークバーで移動できない。VLC は同じファイルをバイトシーク + PTS 再スキャン方式で
  シークできる。
- 対応案: decode EOF で実尺を学習して `VideoInfo` / HUD の duration へ反映するか、
  duration を信頼できない場合にバイト位置シークへフォールバックする。
- 裁定: v2.2.0 以前から同挙動で、ユーザー裁定により次版送り。優先度 P3。
  final-report 追補 5 の P3 参照。

### 1.14 本として開く場合のページ順序を一覧ソートから分離

- 背景: mImageViewer 専用スレ 47。一覧側を更新日時順・サイズ順などにしていると、
  ZIP/PDF/対応アーカイブや画像フォルダを「本」としてページ送りする時も同じ順序になり、
  読書用途では不自然になるという指摘。読書時はサムネイル一覧の整理用ソートではなく、
  ファイル名順でページが進む方が自然そう。
- 現状:
  - フルスクリーン / detached viewer のページ移動は、基本的に現在の `items` 順を参照する。
  - そのため一覧の `sort_order` が日付順・サイズ順・★順などの場合、読書時のページ順も
    それに引きずられる。
  - 一覧整理用のソートと、ZIP/PDF/画像フォルダを本として読む時のページ順が同じ概念に
    なっている。
- 方針:
  - 一覧側の表示順は変えず、**本として開いた viewer context のページ列だけ**を別順序にできる
    仕組みを検討する。
  - 対象候補は、ZIP/PDF/対応アーカイブ、画像のみフォルダを本扱いするケース、明示的に
    「ページを開く」で開いた画像フォルダ。検索結果、タグ一覧、レーティング一覧などの
    集約ビューは結果順自体に意味があるため、初期対応では対象外にする。
  - 既定は「本として読めるケースでは名前順」。ただし、現在の一覧順をそのまま読み順に
    使いたいユーザーもいるため、設定候補は `一覧順を使う` / `名前順にする` /
    `本とみなせる場合は自動で名前順` などを比較する。
  - 名前順は v2.1.0 以降の Windows に近い自然順比較を使い、既存の「番号順（区切り無視）」とは
    混同しない。
  - 右綴じ / 左綴じ、見開き、見開き 1 ページずらし、連結読み、読書位置保存、Ctrl+↑↓ の
    フォルダ移動と矛盾しないよう、viewer context 側の page list / stable key を明示する。
- 確認:
  - 一覧を更新日時順にした状態で ZIP/PDF/画像フォルダをページとして開いても、ページ送りは
    ファイル名順に進む。
  - 一覧へ戻った時のサムネイル順はユーザーが選んだ一覧ソートのまま変わらない。
  - 読書位置復元、見開き組み合わせ、Ctrl+←/→ の 1 ページずらし、連結読みのスクロール位置が
    viewer 側の順序に対して一貫する。
- 優先度: P3。読書 UX 改善。実装時は `docs/virtual-folders.md` と
  `docs/fullscreen-navigation-consistency.md` を読む。

### 1.15 フルスクリーン左右パネルの 3 状態表示モード

- 背景: mImageViewer 専用スレ 47。フルスクリーンの左右パネルは、画面端ホバーで出るため
  誤って表示される / 表示後に消えにくい / 編集パネルが出ること自体が怖く見える、という
  使いにくさがある。一方で完全 OFF にすると、意図せず切り替えた時に戻し方が分かりにくい。
- 方針:
  - 上 HUD の `i` ボタンを 3 状態トグルにする。
    1. 通常ホバー: 現行に近く、左右端ホバーでパネル表示。
    2. ピン留め: 右パネルを常時表示。青い `i` など、通常と違う色で状態を示す。
    3. クリック表示: ホバーだけではパネルを開かず、画面端の細い呼び出しバーをクリックした時だけ開く。
  - クリック表示モードでは、左右端 2% 程度の細いエリアにカーソルを置いた時だけ縦長バーを表示する。
    左側バーには `▶`、右側バーには `◀` のような矢印を出し、クリックで対応する左 / 右パネルを開く。
  - 完全 OFF にはしない。クリック表示モードでも呼び出しバーが残るため、機能が消えたように見えず、
    ほとんどの場面では邪魔にならない状態を目指す。
  - クリック表示状態のアイコンは、`i` の右下に小さいマウスカーソルを重ねる案を第一候補にする。
    視認性が悪い場合は `i` + カーソル横置き、または tooltip で補足する。
  - 一度パネルが表示された後、マウスを大きく動かさないと消えない現状挙動もあわせて見直す。
  - 画像フルスクリーン、detached viewer、動画 / 音声 overlay で左右パネルの実装経路が異なるため、
    初期対応は静止画フルスクリーンから始め、動画 overlay は必要に応じて別スライスにする。
- 確認:
  - 通常ホバー / ピン留め / クリック表示を上 HUD の `i` ボタンで循環でき、現在状態が視覚的に分かる。
  - クリック表示モードでは、画面端を通過しただけではパネルが開かず、細いバークリックでだけ開く。
  - 左右パネルを意図せず開きにくくなりつつ、ロック状態から復帰できない状態にならない。
  - 見開き / 連結読み / 表示トリム / 右パネルタグ編集 / 左パネル補正編集の操作と干渉しない。
- 優先度: P3。操作感改善。実装時は `docs/display-pipeline.md`、
  `docs/detached-viewer-implementation-plan.md`、動画へ広げる場合は `docs/video-architecture.md` を読む。

## 2. フォルダツリーペイン

### 2.1 folder pane scan worker の thread 構成判断

- 背景: `scan_real_subfolders` はノードごとに短命 thread を spawn する。
- 現状: `folder_pane/scan_subfolders` perf event で ms / entry 数 / dir 数 / cancel / error を記録済み。
  cancel 付きで thread leak は見えていない。
- 方針:
  - 低速共有や大量ノード展開で遅い scan / concurrent scan が見えた場合だけ、dispatcher / pool 方式へ寄せる。
- 優先度: P3。

## 3. 補正 / AI

### 3.1 local-adjust layers の入場時同期 DB 読み

- 背景: フルスクリーン入場初回フレームで `LocalAdjustDb::get_layers` を同期実行する。
- 現状: フォルダ open 一括読みを避けるための意図的 tradeoff。
- 方針:
  - 数十 MB 級ページで hitch が報告 / 計測された場合に worker 化する。
  - read-only 経路の not-loaded は現状どおり None 返しを維持する。
- 優先度: P3 monitor。

### 3.2 補正パラメータ変更後に AI アップスケールキャッシュが優先される疑い (再現待ち)

- 背景: 5ch レス 792 の追跡項目。「画像補正パラメータを変更しても AI アップスケールキャッシュが
  優先され、ページを行き来すると変更が効いていないように見える」という報告。
- 現状 (2026-06-18): 通常環境と v1.7.0 ポータブル版の追加テストで再現せず。現在の設計では、
  色調補正や AI 設定の変更は final AI / final composite cache のキー差分または明示クリアで反映される。
  一方、最終段スマートシャープなど post-filter 系は final AI cache を再利用して final composite だけを
  作り直す。さらに AI アップスケール出力にはスマートシャープを適用しない固定仕様なので、
  操作内容によっては「変わらない」ように見える場合がある。
- 方針: 具体的な再現手順が出るまではコード修正しない。再報告時は、変更したパラメータが色調補正 /
  AI ON/OFF / デノイズ / post-filter / スマートシャープのどれかを最初に切り分ける。
- 優先度: P3 monitor / 再現待ち。

### 3.3 表示トリム / 余白カット

- 背景: 5ch レス 792-⑥。要望文は「crop 後フィット」だったが、既存の crop は投稿 / 書き出し用の
  「切り取り」で、漫画ビューア用途の「読みながらサクッと余白を詰める」機能とは目的が違う。
- 実装済み (2026-06-17):
  - 左ホバーの補正パネルを「画像補正 / 表示トリム」のタブ式にし、選択タブを設定保存する。
    上部バー右側の表示トリムボタン / 画像補正アイコンは削除した。
  - 表示トリムタブで、トリムなし / 自動余白カット / 本全体の設定を適用、を
    ラジオで切り替えられる。このページ個別設定は現在ページだけのチェックで適用し、
    前後ページへ移動するとチェックは外れる。
  - 本全体 / このページの手動設定では、単ページ / 見開き連動 (上・下・中央側・外側) /
    見開き左右別を 0〜20% のスライダーで調整できる。
  - 自動余白カットは表示中ページごとに検出する。自動検出ボタンは現在ページ / 見開きの
    単色余白を本全体 / このページの手動スライダーへ反映する。
  - `draw_fs_image` / `draw_fs_spread` の content bbox 経路に統合し、ページ全体 / 横幅 / 縦幅 /
    100% 原寸でも表示トリム後の矩形を fit 基準にする。
  - bbox 外は描画せず背景色に落とす。中央側のトリムは見開きの見える端が gap に合うよう再配置する。
  - スライダー操作時は、対象の手動設定モードを適用する。
  - 見開き連動 / 左右別の切替時は値を移行し、左右別→連動では平均値にする。
  - 基本適用モード / 本全体設定は本キー、ページ個別設定値は page_path_key で
    `view_trim.db` に保存する。ページ個別チェック状態は保存せず、自動余白カットは
    モードだけ保存し、検出 bbox は保存しない。
  - 出力用 crop / 保存 / Ctrl+E / 補正 / AI キャッシュには影響しない。
- 残:
  - 実機で使用感確認後、枠ドラッグ操作を追加するか判断。
- 優先度: P3。

### 3.4 トーン漫画の縮小モアレ対策 (手動 post_filter 縮小 → 将来 LOD)

- 正本: **`docs/downscale-moire-lod-plan.md`** (調査結果 + 対策方針)。以下は要約。
- 背景: トーン (スクリーントーン) を貼った漫画を縮小表示するとモアレが出る
  (ユーザー報告 2026-07-07)。トーンの高周波が縮小で折り返す aliasing。
- 現状 (調査済み):
  - 縮小は `src/fast_resize.rs` (`fast_image_resize`, Bilinear / Lanczos3 の 2 択) に集約。
  - **主因はフルスクリーン**: `fs_cache` は原寸 (最大 8192px) を保持し、`draw_fs_image` が
    原寸テクスチャを GPU の **naive bilinear (mipmap なし)** で大縮小するため、
    縮小率が 0.5 を切るとトーンが折り返す。サムネは Lanczos3 の負ローブで副次的に出る。
  - `TextureOptions.mipmap_mode` は **egui-wgpu では効かない** (epaint が「egui_glow のみ」と
    明記、renderer は `mip_level_count:1` 固定・`create_sampler` が mipmap_mode 無視)。
    実ソースで確認済み → native mipmap は不可。
- 方針 (段階投資):
  - ⓪ **(先行・当面の対応) 手動 post_filter 縮小フィルタ**。ユーザーが選ぶ post_filter として
    1/2 / 1/4 縮小を追加し、フィット表示のモアレを自衛できるようにする。CRT 系が既に
    post_filter でサイズを変える道を通しており下流 (`draw_fs_image` の `size_vec2()` レイアウト)
    が吸収するので、下流改修はほぼ不要 = 小規模。切替は既存 T (`FsPostFilterNext` が
    `PostFilter::ALL` 巡回) に自動で乗る。制約: フルスクリーン専用 (サムネ非適用) / 他 post_filter
    と排他 / 静的倍率。正本 §4.4 に触るファイル一覧 (`adjustment.rs` / `post_filter.rs` /
    `ui_adjustment_panel.rs` / `gamepad_input.rs` のドリルグループ + テスト)。
  - ① **CPU 2 段 (原寸 + 表示解像度版)** から。フィット表示は倍率固定なので worker で
    Lanczos 縮小した 1 枚を貼り、ズーム拡大時だけ原寸へ持ち替える。原寸→8192 縮小は
    既に `clamp_dynamic_for_gpu` が worker でやっているので **UI ブロックなし**。
    フィット / 連結 / 見開きのモアレの大半がこれで消える。
  - ② 足りなければ **CPU N 段の手動 LOD (手動 mipmap)** に拡張。
  - ③ ズーム往復の滑らかさまで要れば **GPU pyramid + native texture 登録** (コスト大)。
  - 縮小は post_filter の**前**・表示解像度基準で掛けるとモアレに強い (疑似カラー等の
    規則パターン系は原寸適用だと自らモアレる)。編集は原寸のまま、LOD は表示専用派生。
  - `draw_fs_image` は `handle.size_vec2()` を論理サイズに使う (10+ 経路) ので、
    「レイアウトは元サイズ・描画 handle だけ差し替え・UV 0..1」の分離が必須。
    ルーペは原寸固定、pixel grid は論理サイズ必須で LOD 除外。
  - 連結読み / 見開きはページ単位の個別テクスチャなので案がそのまま乗る (縮小率が高いぶん恩恵大)。
- 規模 / リスク: CPU 2 段=中 / CPU N 段=中〜大 / GPU pyramid=大。押し上げ要因は
  `size_vec2()` 経路の分離・`final_composite_cache` 回帰テスト群・編集全経路の再生成配線・
  **detached-rework 凍結ルール** (表示テクスチャ経路を共有するため、着手前に
  `docs/detached-rework-plan.md` §2 で境界を確定する)。
- 優先度: ⓪ 手動 post_filter 縮小フィルタ (回避策) = 小規模の先行実装候補。他セッションの
  並行作業が落ち着いてから着手予定 (2026-07-08 合意)。①〜③ の LOD による根本的解決 = P3、
  将来再検討 (画質要望の蓄積時 or detached-rework 完了後)。

---

## 4. 入力カスタマイズ / マウス / ゲームパッド

### 4.1 Shift / Alt + ホイールのカスタマイズ再設計

- 背景: v1.7.0 のリングショートカット / マウスボタン実装中に、Shift / Alt + ホイールのペアバインドを
  追加候補にしたが、実機確認で動画まわりの退行リスクが高いと判断した。
- 方針:
  - v1.7.0 では公開 UI / 入力経路から外し、通常ホイール、Ctrl+ホイール、中ボタンドラッグの既存挙動を維持する。
  - 将来再開する場合は、グリッド / 画像フルスクリーン / 動画フルスクリーンを別々に設計する。
  - native video overlay の consumed wheel、modifier 転送、動画タイルの Ctrl+ホイール、編集パネル / スクロールパネルとの
    優先順位を先に決める。
- 実装メモ: `ring_shortcuts.shift_wheel_pair` / `alt_wheel_pair` は互換読み込み用フィールドとして残すが、
  現行 UI / 入力経路からは参照しない。
- 規模 / リスク: Medium / 中。動画系の手動確認を含めて別タスクで扱う。

### 4.2 コマンド設定画面 + キーカスタマイズ GUI

- 背景: `keymap.ini` / `KeyAction` / `Chord` / exact modifier match の土台はあるが、
  現状は手書き ini 前提で GUI がない。競合検出は起動時 warning として実装済みだが、
  設定画面上で競合先を確認・編集・解除する UI はまだ無い。次バージョンでは、まず
  キーボード割り当ての設定画面を用意し、マウス/ゲームパッドの完全カスタマイズとは
  段階を分けつつ、NeeView の「コマンドに入力を割り当てる」構成に寄せたい。
- 方針:
  - 初期版はキーボード中心。マウス右ドラッグは「未使用 / リングショートカット / ジェスチャ」
    の選択へ整理し、リングショートカット設定も将来的にコマンド画面へ統合する。
    ゲームパッドもコマンドへ割り当てられる範囲を段階的に広げる。
    - `KeyAction` と既存 parser / default chord 定義を正本にし、旧 `keymap.ini` は初回起動時に
      `Settings.keymap` へ一度だけ取り込んで `keymap.ini.imported*.bak` へ退避する。以後の GUI は
      settings.db 側を書き換える。既存ユーザーの手書き設定ファイルはバックアップとして残す。
  - 修飾キーは現行どおり `ModifierHold` の中から選ぶ仕組みを維持する。一般キー hold と
    modifier hold を混ぜない。
  - 競合検出は `(context, trigger kind, chord)` を基本にし、同時に有効になり得る context の
    重なりだけ警告する。Press と Hold は完全同一視せず、同じ物理キーで誤爆しそうな場合は
    警告として扱う。
  - 競合時は保存禁止より、NeeView 風に「この割り当てを使うなら既存のどれを外すか」を
    選べる解消 UI を優先する。競合先の各コマンド設定を簡単に開けるようにし、競合先を
    別キーへ変更したり、割り当て解除したりできる形にする。
  - 既定値へ戻す、割り当て削除、最大3 chord、未対応/固定操作の説明、手書き ini の読み込みエラー表示を
    初期UIに含める。
- 優先度: P2 candidate。入力カスタマイズの需要に対して、既存土台を活かしやすい初期スライス。

### 4.3 リング / マウスジェスチャ / マウスボタンの追加候補

- 背景: mImageViewer 専用スレ 11。v2.2.0 の操作カスタマイズを試した上での追加要望。
- 要望:
  - リングショートカット / マウスジェスチャの割り当て候補に「閉じる」と「最小化」が欲しい。
  - 画像フルスクリーンの「見開き 1 ページずらし」をマウスジェスチャにも割り当てたい。
  - マウス戻る / 進むボタン設定と同じ場所で、ホイールクリック (中ボタンクリック) も設定したい。
    用途例として、中ボタンに Z ズームを割り当てたい。
- 方針:
  - 「閉じる」は文脈ごとに意味を分ける。画像フルスクリーンでは `FsClose`、
    動画フルスクリーンでは `VideoCloseFullscreen` 相当として実装済み。グリッドでのアプリ終了 /
    ウィンドウ close は誤操作リスクが高いので、入れる場合は別アクション名・確認要否・最小化との違いを先に決める。
  - 「最小化」は `RingActionId::MinimizeWindow` として実装済み。グリッドはメインウィンドウ、
    画像フルスクリーンは現在のフルスクリーン / detached viewer、動画フルスクリーンは native presenter を
    直接最小化せず、実際のトップレベル host ウィンドウを最小化する。動画再生は最小化後も継続する。
  - 見開き 1 ページずらしは既存 `KeyAction::FsSpreadShiftLeft` /
    `FsSpreadShiftRight` 相当の左右 2 アクションに加え、PageUp/Down 系と同じ「前 / 次」の意味を
    RTL でも反転しない `KeyAction::FsSpreadShiftPrev` / `FsSpreadShiftNext` と
    `RingActionId::ImageSpreadShiftPrev` / `ImageSpreadShiftNext` を足す。
    これによりコマンド一覧、リングショートカット、マウスジェスチャの候補へ同時に出せる。
  - 中ボタンクリックは `MouseButtonProfile` を `back / forward / middle` に拡張し、
    UI では「マウスボタン」タブに「ホイールクリック」を追加済み。Grid / ImageFS / VideoFS で
    文脈別に設定でき、既定は未割り当て。
  - マウスボタン候補は Grid ではリングショートカットと同じ場所移動系も出すが、ImageFS / VideoFS では
    `C:\`〜`Z:\`、お気に入り、読書履歴、★一覧などの場所移動系を候補外にする。短クリックで
    フルスクリーン文脈を閉じたり別ビューへ移動したりする操作は誤発火時の影響が大きいため。
  - 既存の中ボタンドラッグズーム (`handle_middle_drag_zoom`) は維持する。中ボタン押下後、
    移動量がドラッグしきい値を超えた場合は従来どおりドラッグズームを優先し、移動が小さいまま
    release された時だけ「ホイールクリック」の割り当てを発火させる。
  - Z ズーム (`FsZoomMode`) は `KeyHold` 操作なので、単発 `RingActionId::ImageZoomMode` では
    押下中の照準表示をスキップし、現在のカーソル位置でズーム状態へ入る一発トグルとして扱う。
    ズーム状態中は中ボタン上下ドラッグで `fs_zoom_factor` を変更できる。
- 実装済み:
  - 画像 / 動画フルスクリーンの「閉じる」、ウィンドウ最小化、見開き 1 ページずらしの左右版・前後版、
    ホイールクリック、Z ズーム一発トグル、画像 / 動画フルスクリーンのマウスボタン候補整理。
- 残: なし。
- 確認:
  - 中ボタンへ Z ズームを割り当て、短クリックで全画面ズームモードへ入り、ズーム状態中の中ボタン上下ドラッグで倍率を変えられる。
  - 中ボタン押し込み + 上下ドラッグの従来ズーム、パネル上で開始した中ボタン無視の挙動が退行しない。
  - 右ドラッグのリング / マウスジェスチャで閉じる、見開き 1 ページずらしが候補に出る。
  - detached viewer / native video / 通常フルスクリーンで、閉じる後の focus と復帰が破綻しない。
  - グリッド / 画像フルスクリーン / detached viewer / native video で「ウィンドウ最小化」を実行でき、動画は最小化後も再生が続く。
- 優先度: P2 candidate。v2.2.0 の操作カスタマイズ拡張として相性がよいが、
  中ボタンは既存ドラッグ操作との競合があるため小さく確認しながら進める。

### 4.4 サムネイル画質設定の ZIP 内画像サンプル対応

- 背景: mImageViewer 専用スレ 12。「設定 → サムネイル画質設定」が ZIP 内画像選択時に
  「画像を1枚選択してからもう一度お試しください。」となり設定できないという報告。
- 現状:
  - `open_thumb_quality_dialog` は `last_selected_image_path: Option<PathBuf>` を使い、
    worker で `image::open(path)` して A/B サンプルを作る。
  - `update_last_selected_image` は `GridItem::Image` の実ファイルパスだけを保存し、
    `GridItem::ZipImage` / `PdfPage` は対象外。ZIP 内画像は実パスが無いため現状の構造では
    サンプルにできない。
- 方針:
  - `last_selected_image_path` ではなく、選択中 item からサンプル取得ジョブを作る構造へ変える。
    初期対応は `GridItem::Image` と `GridItem::ZipImage`。ZIP は `zip_loader::read_entry_bytes`
    で entry bytes を読み、`image::load_from_memory` でデコードする。
  - PDF ページは PDFium worker / render サイズの扱いが別になるため、同時対応するか別タスクにするか判断する。
    まずはメッセージを「ZIP/PDF内ページは未対応」ではなく実対応に寄せたい。
  - デコードは現在と同じく worker で行い、UI スレッドで ZIP 読み / 画像デコードをしない。
  - サンプル表示名は実パスだけでなく `book.zip > page.jpg` のような仮想表示名を持てるようにする。
- 確認:
  - 通常画像と ZIP 内画像でサムネイル画質 A/B ダイアログが開く。
  - 壊れた ZIP entry / 非対応画像 / ZIP が削除済みの場合は失敗メッセージを出し、UI は固まらない。
  - 画質適用後のキャッシュ再生成が通常画像 / ZIP 内画像で同じ設定を使う。
- 優先度: P2 candidate。報告としてはバグ寄りだが、PDF 対応まで含めると範囲が広がるため
  ZIP 内画像を先に小さく直す。

### 4.6 操作カスタマイズの共有・差分・世代取り込み

- 状態: **実装済み**。共有ファイル、差分表示、世代 / ファイル取り込み、ライブ適用、自動退避、マニュアル更新まで完了。

- 正本: **`docs/operation-customize-share-plan.md`**（設計・ファイル形式・UI・実装順・テストまで記載）。以下は要約。
- 背景: 操作カスタマイズ（キー割り当て / 右ドラッグ・リング・マウス・ゲームパッド /
  メニュー構成）は作り込む設定なので、「他人に配れるプリセットとして共有したい」「標準や
  現在との差分を見たい」「変な割り当てをしたので操作カスタマイズだけ 2 日前に戻したい」
  という需要がある。
- 現状（調査済み）:
  - 操作カスタマイズは `settings.db` 内の 3 フィールド（`Settings.keymap` /
    `ring_shortcuts` / `menu_layout`、いずれも serde 対応）に保存され、`settings.db` 世代
    バックアップ `bak1..bak10` + 「設定の復元」に含まれる（除外されていない）。
  - ただし世代ローテはプロセス起動ごとに 1 回、復元は settings.db 全体の差し替え（他設定も
    巻き込む・要再起動）、操作カスタマイズ専用の差分 / エクスポート / インポートは無い。
- 確定方針（ユーザー合意）:
  1. 既存の設定バックアップ（settings.db 世代 + 設定の復元）は現状維持。新しい永続化・
     スキーマ変更はしない。世代を読むだけ + ファイル入出力 + 純関数の差分で作る。
  2. 共有・差分・世代取り込みはすべて「設定の復元」ダイアログに集約し、縦長回避のため
     **タブ化**（`設定の復元` / `操作カスタマイズ`）。
  3. 共有は `.mivkeys.json`（3 点セット + `format_version` + `app_version` + `label`）を
     エクスポート / インポート。**インポートは置換のみ**（マージなし）、未知アクションは
     warn-and-skip、適用前に「取り込み元 vs 現在」の差分プレビュー。
  4. 差分は実効チョード（override or `default_chords`）単位で 追加/削除/変更 を表表示。
     比較対象は 標準 / 現在 / 前世代（前世代は起動ごとローテのため空が多い）。
  5. 世代からの取り込みも可能（2 日前の操作カスタマイズを現在へ、**再起動なしのライブ適用**
     = `apply_operation_customize_state` 経路）。取り込み前に現在設定を
     `before-import-*.mivkeys.json` へ自動退避して undo 経路を確保。
- 実装戦略: 新規純ロジック `src/operation_customize_share.rs`（Bundle / JSON / diff）+
  `settings_restore.rs` に世代抽出（既存 `validate_in_dir` の read-only 展開を流用）+
  `ui_dialogs/settings_restore.rs` のタブ化。keymap 側は `KeyAction::all` / `default_chords` /
  `effective_chords` / warnings を再利用（新規ロジックはほぼ不要）。ファイルダイアログは既存 `rfd`。
- 規模 / リスク: Medium / 低。スキーマ非変更・読み取り中心で、書き込みは取り込み時のみ
  （既存の save 経路 + 自動退避で保護）。detached 凍結ルールとは無関係。
- 段階実装: ① 純ロジック + test → ② 世代/現在から Bundle 抽出 + エクスポート →
  ③ 差分ビュー → ④ 取り込み（ファイル + 世代、プレビュー + ライブ適用 + 自動退避）→
  ⑤ ダイアログのタブ統合 → ⑥ マニュアル / 製品ページ更新。
- 優先度: P2 candidate。操作カスタマイズ系（4.2 / 4.3）と同時期に着手すると相性がよい。

### 4.5 サムネイル選択情報の下部 1 行バー / ツールチップ改善

- 背景: mImageViewer 専用スレ 17。サムネイル情報ツールチップが下の列のサムネイルに被って
  見えなくなるため、一定時間で消す、または ZipPla のようにウィンドウ下部に表示したいという要望。
  ツールチップを非表示にすると情報量が足りないため、情報表示自体は残したい。
- ZipPla 調査メモ:
  - ZipPla / ZipPlaFork は、上部のサムネイル領域と下部のファイルリスト / 詳細ビューを
    同時表示できる。スクリーンショット上の「下に別途出ているウィンドウ」はこの詳細ビューに近い。
  - さらに最下部には `statusStrip` があり、そこへ `selectedFileNameToolStripTextBox` を挿入している。
    選択中ファイル 1 件ではパス、画像解像度、ページ数、動画情報、ファイルサイズなどを下部に表示し、
    複数選択時は選択数を表示する。README 履歴でも、ステータスバーに対応ファイル数 /
    ファイル名またはフルパス / ファイルサイズ / ページ数を表示する変更が確認できる。
- 方針:
  - サムネイル選択情報の表示方式を設定化する。候補は `ツールチップ` (現状) /
    `下部情報バー` / `両方` / `非表示`。
  - 下部情報バーはメインウィンドウ下部の固定領域として表示し、サムネイルに重ならない形にする。
    初期案は「詳細表示の 1 行版」のように、選択中 item の主要情報を横一列で表示する。
  - 表示内容は既存の「サムネイルの選択情報ツールチップ」カスタマイズ項目
    (ファイル名、解像度、種類、サイズ、更新日時、作成日時、動画情報、場所など) を流用する。
    詳細表示の列設定と共有できるかは実装時に確認する。
  - 複数選択時は `N 個選択` を主表示にし、取得済みなら合計サイズなどを補助表示する。
    未取得メタデータのために UI スレッドで同期 I/O を増やさない。
  - ツールチップを継続する場合は、一定時間で自動的に消す / 画面下端では上側へ逃がす /
    下部情報バー有効時はツールチップを出さない、のいずれかを実装時に選ぶ。
  - サムネイル表示と詳細表示の両方で、選択カーソル変更に追従する。
  - ZipPla の上下分割に近い「サムネイル一覧 + 詳細リストの同時表示」も要望意図に含まれる可能性がある。
    ただし、mIV では既にサムネイル表示 / 詳細表示を切り替える設計なので、初期対応は軽量な
    下部情報バーに留め、同時表示ビューは別要望として切り分ける。
- 確認:
  - 最下段付近のサムネイルを選択しても、情報表示で下の行が隠れない。
  - 下部情報バーはサムネイル領域を大きく圧迫せず、詳細表示 1 行相当の情報量を読める。
  - ツールチップ非表示設定でも、下部情報バーから必要情報を確認できる。
  - 大量フォルダ / ZIP / PDF / 動画混在で、情報更新が UI のスクロールや選択移動をブロックしない。
- 優先度: P2 candidate。要望内容は明確で、既存のツールチップ情報生成を流用できる見込み。

---

## 5. リリース前確認 / 依存更新

### 5.1 ネイティブ依存

| 対象 | 現状 / 次の確認 | 注意点 |
| --- | --- | --- |
| PDFium | **新版 `chromium/7906` あり (2026-06-23 確認)**。v2.1.0 は v2.0.0 と同じ `151.0.7891.0` 維持で出荷 (PDF 再テスト回避のため見送り)。次回リリースで `setup-pdfium.sh` 更新 → PDF 表示手動確認 | PDF 開封、ページ列挙、サムネ、フルスクリーン、パスワード PDF |
| FFmpeg LGPL shared | 動画再生の手動確認と LGPL ソース tarball 配置更新 | DLL 名が変わる更新では `setup-ffmpeg.sh` / loader / `build.rs` を揃える |
| ONNX Runtime | `ort-sys` 要求 DLL と setup script の VERSION を確認 | C API バージョン一致、`+crt-static` + `load-dynamic` 維持 |
| VST3 SDK / bridge | C++ ソース変更がなければ再ビルド不要 | 更新時は商用プラグインで実機確認 |

### 5.2 Rust クレート

- 通常の `cargo update` は互換範囲でまとめて実施する。
- メジャー / rc 脱出は個別判断:
  - `ort`
  - `pdfium-render`
  - `ffmpeg-the-third`
  - `image`
  - `zip`
  - `sevenz-rust2`
  - `delharc`
  - `unrar`
  - `turbojpeg`
- 更新後に確認するもの:
  - `cargo test`
  - 検索 bench 回帰
  - perf smoke
  - `dumpbin /dependents` で不要な VC runtime DLL が復活していないこと

---

## 6. 着手時に読み直す関連ドキュメント

| 領域 | ドキュメント |
| --- | --- |
| UI 同期 I/O / worker 化 | `docs/ui-responsiveness.md`, `docs/async-architecture.md` |
| サブフォルダ展開 / フラット仮想ビュー | `docs/subfolder-expansion-view-plan.md`, `docs/ui-responsiveness.md`, `docs/async-architecture.md`, `docs/details-view-and-filter-plan.md`, `docs/virtual-folders.md` |
| ZIP / PDF / 変換アーカイブ | `docs/virtual-folders.md`, `docs/shell-file-operations-context-menu-plan.md` |
| フォルダ移動 / Ctrl+↑↓ | `docs/fullscreen-navigation-consistency.md`, `docs/keymap-spec.md` |
| 入力カスタマイズ / マウス / ゲームパッド | `docs/keymap-spec.md`, `docs/key-customization-impl-plan.md`, `docs/ring-shortcut-plan.md`, `docs/operation-customize-share-plan.md` |
| フルスクリーン / F12 別ウィンドウ / 連結読み | `docs/display-pipeline.md`, `docs/detached-viewer-implementation-plan.md`, `docs/fullscreen-navigation-consistency.md` |
| 表示 / AI / 補正 | `docs/display-pipeline.md`, `docs/preset-and-adjustment.md` |
| 詳細表示 / スマートフィルタ | `docs/details-view-and-filter-plan.md`, `CLAUDE.md` の UI / スクロール節 |
| タグ / フルスクリーン右パネル / 動画 overlay | `docs/tag-catalog-redesign-plan.md`, `docs/display-pipeline.md`, `docs/video-architecture.md`, `docs/detached-viewer-implementation-plan.md` |
| リリース / 依存更新 | `CLAUDE.md` のリリース手順、各 native 依存管理節 |
