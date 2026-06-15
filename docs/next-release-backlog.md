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

### 1.1 RTL 見開き時の左クリックページ送り方向

- Source: 5ch software thread 780 (v1.6.0 feedback).
- Request:
  - 右→左の見開き表示では、画像の左側クリックで次ページへ進むようにしてほしい。
  - 投稿者は「好みの問題かもしれない」「カスタマイズ機能があれば」と書いているが、既存のキー操作との整合性から通常修正でよい可能性が高い。
- Current likely behavior:
  - キー操作は `spread_mode.is_rtl()` を見て左右方向を反転している。
  - マウス左クリックは `handle_fs_wheel_and_click` で `pos.x > full_rect.center().x` をそのまま `+1/-1` にしており、RTL 反転が入っていない。
- Proposed fix:
  - 静止画フルスクリーンのページ単位クリック送りで、通常の左右キーと同じく RTL 時は左右半分の意味を反転する。
  - 右→左見開き時は左半分クリック = 次、右半分クリック = 前。
  - 単ページ表示でも `spread_mode.is_rtl()` が有効ならキー操作と同じ方向に揃えるかを確認し、少なくとも見開き時は反転する。
  - 連結読み、ズーム/パン中、下部シークバー、左右パネル、ポップアップ表示中、動画クリック再生/停止には影響させない。
- Tests / docs:
  - `handle_fs_wheel_and_click` の UI 入力は直接テストしにくいので、左右クリック位置から nav delta を決める小さな純関数へ切り出すと回帰テストしやすい。
  - `docs/keymap-spec.md` の「マウス左クリック」に、画像の左右クリック送りも RTL ではキー操作と同じく反転する旨を追記する。
- Priority: P2. 小さな挙動修正だが、漫画の右綴じ閲覧では体感しやすい。

---

## 2. アーカイブ / 仮想フォルダ

現時点ではなし。

---

## 3. フォルダツリーペイン

### 3.1 folder pane scan worker の thread 構成判断

- 背景: `scan_real_subfolders` はノードごとに短命 thread を spawn する。
- 現状: `folder_pane/scan_subfolders` perf event で ms / entry 数 / dir 数 / cancel / error を記録済み。
  cancel 付きで thread leak は見えていない。
- 方針:
  - 低速共有や大量ノード展開で遅い scan / concurrent scan が見えた場合だけ、dispatcher / pool 方式へ寄せる。
- 優先度: P3。

## 4. 補正 / AI

### 4.1 local-adjust layers の入場時同期 DB 読み

- 背景: フルスクリーン入場初回フレームで `LocalAdjustDb::get_layers` を同期実行する。
- 現状: フォルダ open 一括読みを避けるための意図的 tradeoff。
- 方針:
  - 数十 MB 級ページで hitch が報告 / 計測された場合に worker 化する。
  - read-only 経路の not-loaded は現状どおり None 返しを維持する。
- 優先度: P3 monitor。

---

## 5. リリース前確認 / 依存更新

### 5.1 ネイティブ依存

| 対象 | 現状 / 次の確認 | 注意点 |
| --- | --- | --- |
| PDFium | vendor 更新後の PDF 表示手動確認が必要 | PDF 開封、ページ列挙、サムネ、フルスクリーン、パスワード PDF |
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

### 5.3 Microsoft Defender false positive follow-up

- v1.3.0 ZIP package の Defender 誤検知について、Microsoft analysis の結果を確認する。
- 検出が残る場合は再パッケージングと再提出を検討する。
- 確認結果は必要に応じてリリースノートやユーザー返信へ反映する。

---

## 6. 着手時に読み直す関連ドキュメント

| 領域 | ドキュメント |
| --- | --- |
| UI 同期 I/O / worker 化 | `docs/ui-responsiveness.md`, `docs/async-architecture.md` |
| ZIP / PDF / 変換アーカイブ | `docs/virtual-folders.md`, `docs/shell-file-operations-context-menu-plan.md` |
| フォルダ移動 / Ctrl+↑↓ | `docs/fullscreen-navigation-consistency.md`, `docs/keymap-spec.md` |
| 表示 / AI / 補正 | `docs/display-pipeline.md`, `docs/preset-and-adjustment.md` |
| リリース / 依存更新 | `CLAUDE.md` のリリース手順、各 native 依存管理節 |
