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

現時点ではなし。

---

## 2. アーカイブ / 仮想フォルダ

### 2.1 変換キャッシュ downgrade の UI 不可視

- 背景: 将来または旧版との行き来で未知 `format` 行が `archive_cache::format_from_db` から
  `None` になり、cache manager UI に出ない。
- 影響: auto-prune は効くため disk leak ではない。未リリース/互換境界の P3。
- 方針:
  - 未知 format を汎用行として表示する、または意図をコメントで固定する。
  - cache manager 側で削除できる形にするなら、format 文字列の表示名を失わない。

### 2.2 ZipDir サムネ queue routing の再検討

- 背景: ZipDir サムネ要求は軽量キュー扱いだが、代表解決で ZIP 列挙が走る場合がある。
- 現状: `zipdir_resolve` 系の perf ログがある前提で、実測を見て振り分け本体を判断する。
- 方針:
  - 低速アーカイブで UI / サムネ worker が詰まるログが出た場合だけ、`LoadRequest` の解決戦略も見て heavy I/O 側へ寄せる。

---

## 3. フォルダツリーペイン

### 3.1 scan worker の perf 計装と thread 構成

- 背景: `scan_real_subfolders` はノードごとに短命 thread を spawn する。
- 現状: cancel 付きで thread leak は見えていないが、低速共有での悪化を追いやすくしたい。
- 方針:
  - `scan_real_subfolders` に `perf::event` を追加する。
  - 実測で問題が出る場合、dispatcher / pool 方式へ寄せる。
- 優先度: P3。

### 3.2 folder pane open scan の優先順位整理

- 背景: folder pane open worker 完了時、通常の nav 優先順位判定より前に
  `load_folder_with_scan` へ流れる。
- リスク: 保留中クリックと同フレームの検索開始 / 別ナビが競合すると、古いクリック先が一瞬適用される余地がある。
- 方針:
  - folder pane open を通常 nav と同じ優先順位で裁定する。
  - 高優先操作や検索開始時に `cancel_folder_pane_open` する。
- 優先度: P3。

---

## 4. 起動 / 単一インスタンス / Explorer 連携

### 4.1 startup / activation path resolve の worker 化判断

- 背景: 稼働中インスタンスへ渡されたパスを開く経路で、`resolve_openable_path` が
  `is_file` / `is_dir` / 親探索を行う。
- 現状: perf 計装済み。遅い / 切断ネットワークパスで stall が見える場合に worker 化する。
- 方針:
  - perf log で `startup/open_path_resolve` を確認する。
  - 問題が見えた場合だけ、パス解決を activation worker に逃がす。

---

## 5. 補正 / AI

### 5.1 PDF render / upload latency の保持キャッシュ検討

- 背景: retained final AI は AI 完了済みピクセルを保持するが、PDF ページの初回 rasterize や GPU upload は別レイヤ。
- 方針:
  - 実機ログで PDF rasterize / final composite / upload の内訳を確認する。
  - 体感遅延が残る場合、PDF render cache / page raster cache の保持やキャンセル方針を別タスク化する。

### 5.2 legacy `adjustment_cache` の upscaled 誤判定

- 背景: `ai_upscale_enabled` が true だと、legacy `adjustment_cache` 済み AI 結果を一律 upscaled 扱いし得る。
- リスク: upscale が範囲外 / 失敗で denoise だけが cache を作った場合、smart sharpen が誤って skip される。
- 方針:
  - legacy AI cache entry に `used_upscale` を保存する。
  - または final composite と同じく cache 出力寸法と source 寸法を比較する。
- 優先度: P3 latent。

### 5.3 capture 再補正経路の sharpen

- 背景: `capture.rs` の re-adjust 分岐が `effective_smart_sharpen` を経由せず raw `smart_sharpen` を適用する。
- 現状: 本番呼び出し元なしのテスト専用 latent。
- 方針:
  - AI upscaled source へ接続するなら `output_is_ai_upscaled` 相当を渡す。
  - テスト専用として維持するなら、その旨をコメントで固定する。
- 優先度: P3。

### 5.4 local-adjust layers の入場時同期 DB 読み

- 背景: フルスクリーン入場初回フレームで `LocalAdjustDb::get_layers` を同期実行する。
- 現状: フォルダ open 一括読みを避けるための意図的 tradeoff。
- 方針:
  - 数十 MB 級ページで hitch が報告 / 計測された場合に worker 化する。
  - read-only 経路の not-loaded は現状どおり None 返しを維持する。
- 優先度: P3 monitor。

---

## 6. リリース前確認 / 依存更新

### 6.1 ネイティブ依存

| 対象 | 現状 / 次の確認 | 注意点 |
| --- | --- | --- |
| PDFium | vendor 更新後の PDF 表示手動確認が必要 | PDF 開封、ページ列挙、サムネ、フルスクリーン、パスワード PDF |
| FFmpeg LGPL shared | 動画再生の手動確認と LGPL ソース tarball 配置更新 | DLL 名が変わる更新では `setup-ffmpeg.sh` / loader / `build.rs` を揃える |
| ONNX Runtime | `ort-sys` 要求 DLL と setup script の VERSION を確認 | C API バージョン一致、`+crt-static` + `load-dynamic` 維持 |
| VST3 SDK / bridge | C++ ソース変更がなければ再ビルド不要 | 更新時は商用プラグインで実機確認 |

### 6.2 Rust クレート

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

### 6.3 Microsoft Defender false positive follow-up

- v1.3.0 ZIP package の Defender 誤検知について、Microsoft analysis の結果を確認する。
- 検出が残る場合は再パッケージングと再提出を検討する。
- 確認結果は必要に応じてリリースノートやユーザー返信へ反映する。

---

## 7. 着手時に読み直す関連ドキュメント

| 領域 | ドキュメント |
| --- | --- |
| UI 同期 I/O / worker 化 | `docs/ui-responsiveness.md`, `docs/async-architecture.md` |
| ZIP / PDF / 変換アーカイブ | `docs/virtual-folders.md`, `docs/shell-file-operations-context-menu-plan.md` |
| フォルダ移動 / Ctrl+↑↓ | `docs/fullscreen-navigation-consistency.md`, `docs/keymap-spec.md` |
| 表示 / AI / 補正 | `docs/display-pipeline.md`, `docs/preset-and-adjustment.md` |
| リリース / 依存更新 | `CLAUDE.md` のリリース手順、各 native 依存管理節 |
