# レーン 4: エクスポートの復旧と一括化

v3.5.0 の並行レーンの 1 本。**このブリーフを最初に読み、そのうえで
[docs/README.md](../README.md) から
[display-pipeline.md](../display-pipeline.md) と
[preset-and-adjustment.md](../preset-and-adjustment.md) を開くこと。**

## 作業ツリーとブランチ

- 作業ツリー: `C:\home\mimageviewer-export` (このツリー、新規作成)
- ブランチ: `export-batch` (master `ea233160` から分岐)
- `vendor/` は master から実体コピー済み (junction ではない)。**`target/` は空なので
  初回ビルドはフルビルドになる。**
- **master へ merge しない。** master では別セッションがリリース作業中。
  区切りごとにこのブランチへコミットし、完了したら報告するところまでが担当。
- 他の worktree (`-extlaunch` / `-r2e` / `-pano` / `-video-strip`) と
  `C:\home\mimageviewer` のファイルは**読むのも書くのも行わない**。
- `git worktree remove` を使わない (junction 再帰削除の事故があるため)。

## 担当する項目 (この順で)

正本は [docs/next-release-backlog.md](../next-release-backlog.md) の各節。**着手前に必ず
その節を読む** (ここは要約であって正本ではない)。

### 1. §1.144 Ctrl+E の隠蔽プリセット出力が v1.1.0 から常に選べない (P1・退行)

**機能が丸ごと死んでいる。原因は確定済み。**

- `export_page_pixels_for_idx` が `conceal_mask: None` を固定で渡している
  ([ui_fullscreen.rs:34233](../../src/ui_fullscreen.rs))。その結果 `has_conceal_mask` が常に
  false になり、チェックボックスの有効条件 `has_conceal_mask && slot.is_some()`
  ([ui_fullscreen.rs:34410](../../src/ui_fullscreen.rs)) が成立しない。
  **マスクを描いていても、どの画像でも必ず無効。** プリセットの保存自体は正常。
- 混入は `be05cfef` (2026-06-03)、出荷は **v1.1.0 から**。
- **なぜ切られたかを先に読む。** 直前のコメントに「フルスクリーン Ctrl+E は
  conceal_mask=None なので、ここで焼いた注釈が worker の conceal preset 合成に潰されない」と
  ある。表示パイプラインは `raw → erase → local_adjust → conceal` の順で、
  `ensure_final_composite_pixels` が返す時点で隠蔽が焼き込み済み。マスクも渡すと**二重適用**する。
- **したがって「マスクを渡す」だけでは直らない。** 隠蔽適用前の段 (`raw → erase → local_adjust`)
  を base として渡し、そこへプリセットごとに隠蔽をかける。
- **同じ構造が製本側にある。** `BakedEditSnapshot` はマスクとプリセットを別々に持ち、
  [books.rs:1085](../../src/books.rs) が「global AI upscale / denoise だけを除外し、それ以外は
  Ctrl+E と同じ順で適用する」と書いている。**ここを参照する。**
- 回帰確認: マスクを描いた画像でプリセット 1〜4 が有効になること、`_0`〜`_4` が同時に出ること、
  注釈のあるページで二重適用にならないこと。**「マスクが無い画像では無効」という本来の条件は残す。**

### 2. §1.148 複数選択したまま Ctrl+E で一括エクスポート

- 用途: モザイクを付けた画像を縮小しつつ特定フォルダへまとめて出す (メール添付用の
  「送信用」フォルダ運用)。ファイル名は `<filename>` / `<dirname>` のような展開マクロ。
- 仕様 (2026-08-31 決定): グリッドで複数選択 → `Ctrl+E` → 一括用ダイアログ。
  出力先フォルダ / 画像形式 / 出力サイズ / ファイル名テンプレートを選ぶ。
  **隠蔽加工のプリセット選択は持たない** (各画像は自分の最終合成をそのまま出す)。
  これにより §1.144 の「隠蔽前 base が要る」問題を踏まずに済む。
- ⚠️ **製本の合成ワーカーを共有する。新しいパイプラインを書かない。** 必要なものは既にある:

  | | 製本 | 一括エクスポート |
  | --- | --- | --- |
  | UI スレッドでの edit snapshot | `book_baked_edit_snapshot` | そのまま |
  | グリッド選択から N 件を組み立て | `add_grid_selection_to_named_book` ([ui_fullscreen.rs:33447](../../src/ui_fullscreen.rs))。スタック展開込み | そのまま |
  | ワーカーでの N 件ループ | `append_pages_at` → `start_book_op` | そのまま |
  | デコード → 合成 → エンコード | `write_source` の `Composited` 分岐 ([books.rs:932](../../src/books.rs)) / `write_composited_color_image` ([books.rs:1263](../../src/books.rs)) | そのまま |
  | 出力先 | 本フォルダ固定 | フォルダ選択 |
  | ファイル名 | ゼロ埋め連番 | テンプレート |
  | 縮小 | なし | `ExportScale::scaled_size` を書き出し直前に 1 段 |

- ⚠️ **`append_pages_at` を直接呼ばない。** 本固有の事情 (ページ番号の採番、`MAX_BOOK_PAGES`、
  無編集時の byte-copy fast path、`restore_declines` の記録、`edit_copies` / `semantic_copies` の
  集計) が付いてくる。**1 件ぶんの「デコード → 合成 → エンコード → 書き出し」を関数として
  切り出し、製本とエクスポートの両方がそれを使う。** 切り出す単位は `write_source` の
  `Composited` 分岐がほぼその形をしている。
- 決めること:
  - テンプレートの置換子 (`<filename>` / `<dirname>` / 連番)。同名衝突時の扱い。
  - 対象外 (動画 / 音声 / フォルダ) を選択に含んでいたときの扱い。黙って飛ばすか件数に出すか。
  - 進捗表示とキャンセル。既存の `ExportPending` (`total` / `done` / `successes` / `errors`) が
    そのまま使える形か確認する。
- **入口は `Ctrl+E` とグリッド選択にする** (右クリックメニューに依存させない = レーン A と
  衝突しない)。

### 3. §1.149 製本フォルダごとに上限ピクセルサイズを設定して自動縮小 (副産物・任意)

- §1.148 で縮小段を共通化すれば、製本側は同じ段へ上限を渡すだけになる。
- **逆に §1.148 だけで用が足りる可能性もある** (集めた後に本フォルダから一括エクスポートすればよい)。
  報告者も「エクスポート機能の拡張の方が柔軟に使える気もする」と書いている。**§1.148 の後に判断する。**
- やる場合の論点: 既にある本のページに遡って効かせるか (効かせるなら再エンコードが走るので明示操作)。

## 共有登録簿 — A が着地するまで触らない

レーン A (`external-tool-launch` worktree、右クリックメニューと外部ツール起動) が、
`src/ui_dialogs/context_menu.rs` (+新設 `context_menu_model.rs`) /
`src/ui_dialogs/preferences.rs` + `preferences/pages.rs` /
`src/settings.rs` + `src/settings_db.rs` / `src/keymap.rs` + `docs/keymap.ini.default` を
全面的に書き換えている。**先に触ると解決不能なコンフリクトになる。**

このレーンで該当し得るのは、エクスポート設定の永続化 (出力先 / 形式 / テンプレートの記憶) を
`Settings` へ置く場合。必要なら**そのレーンの最後に専用コミット 1 本**へまとめること。
新しいダイアログは `src/ui_dialogs/` に 1 ファイル 1 メソッドで足す (CLAUDE.md のパターン) ので、
A とはファイルが別になる。

## 進め方

- 修正前に、観測された失敗・守るべき不変条件・違反を作った経路を特定する。症状を消す
  guard / delay / retry / silent fallback を根本原因の代わりに置かない。
- §1.144 は**「なぜ None にしたか」を読んでから直す**。同じ理由を踏み直すと二重適用の退行になる。
- UI スレッドから同期 I/O / デコード / GPU アップロードを増やさない
  ([ui-responsiveness.md](../ui-responsiveness.md) §4 のチェックリストを通す)。
- 実装を Codex へ出すなら**出す前にコミットする**。**1 worktree につき Codex は 1 本まで。**
- コミット前に `cargo fmt` (引数なし・ワークスペース全体)。
- テストは `cargo test -p mimageviewer --lib <filter>` で最小に。

## 実機確認の頼み方

`.\scripts\build-dev.ps1` を回し、
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe` を利用者へ渡す。
**エージェント自身は起動しない** (実利用中の `%APPDATA%\mimageviewer` を触るため)。
**§1.148 は利用者のファイルを書き出す機能なので、出力先を空フォルダにしてもらう指示を必ず添える。**

**実機確認は利用者 1 人しかいない直列資源で、いま 4 レーンが並行している。**
細かく何度も頼まず、区切りでまとめて 1 回にする。

## 他のレーン (参考)

| レーン | ツリー | 中身 |
| --- | --- | --- |
| A | `-extlaunch` | 外部ツール起動 §1.117 (進行中) |
| 1 | `-r2e` | §1.142 → §1.143 → §1.150/§1.151 |
| 2 | `-pano` | §1.161/§1.159/§1.154 → §1.158 / §1.145 / R-19 |
| 3 | `-video-strip` | 動画シークストリップ §1.155 |
| 4 | **`-export` (ここ)** | §1.144 → §1.148 → (§1.149) |
