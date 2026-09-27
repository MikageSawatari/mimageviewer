# RAW の LibRaw 対応 — 引き継ぎメモ (2026-09-27)

調査セッションから、設計を始める新しいセッションへの引き継ぎ。設計書が出来たら、この文書の
内容は設計書へ吸収して削除してよい。

- worktree: `C:\home\mimageviewer-raw` / branch `raw-libraw` (master `edbac5f37` から分岐)
- vendor 7 種 (pdfium / ort / ffmpeg / models / susie-worker / twemoji / vst3-host) は実体コピー済み。
  `target/debug/deps` に FFmpeg DLL 配置済み (lib test 用)。`build.rs` の必須ファイルは全部揃っている
- まだ一度もビルドしていない

## 1. 目的

カメラ RAW を Microsoft Store の「Raw 画像拡張機能」なしで表示できるようにする。
LibRaw を同梱し、**RAW 拡張子は WIC を一切通らない形でリリースする**。
HEIC / AVIF / JXL / TIFF は従来どおり WIC のまま。

## 2. 利用者の決定事項 (確定)

1. S4 (配布・ライセンス・文書) まで揃えてからリリースする。master への統合は完成後に一度
2. フルスクリーンは **常に「埋め込みプレビュー → フル現像に差し替え」**
3. RAW フル現像の先読みは **先 2 枚・前 1 枚**
4. RAW フル現像の並列数は **既定 3、設定で 1〜10**
5. **編集機能 (補正 / AI アップスケール / 消しゴム / モザイク等) はフル現像完了まで待つ**。プレビューにはかけない

## 3. 現状のコード (master edbac5f37 で確認)

- RAW 拡張子一覧: `src/wic_decoder.rs:68-69` (dng, cr2, cr3, nef, nrw, arw, srf, sr2, raf, orf, rw2, pef, ptx, rwl, iiq)
- WIC デコードは最初のフレームをフル解像度で取得 (`src/wic_decoder.rs:200`)。
  GetThumbnail / GetPreview の経路は無い。途中経過・途中キャンセルの口も無い
- `wic_decoder::` の直接呼び出しが約 11 か所ある。RAW 判定の一本化が必要:
  `src/app.rs:72031`, `src/books.rs:1283/1301`, `src/canonical_image_loader.rs:238/242`,
  `src/similar_image.rs:264-265`, `src/thumb_loader.rs:570/601/947/3387`,
  `src/ui_dialogs/context_menu.rs:2381/2429`, `src/bin/bench_dupe.rs:1988`
  (`video_thumb.rs:162` は COM 初期化のみで無関係)
- 向き情報: RAW は rexif で読めず WIC で読む (`src/thumb_loader.rs:599-601`)
- 拡張子判定は他にも `src/folder_tree.rs`、`src/zip_loader.rs`、`crates/remote-web/src/image_support.rs` にある
- フルスクリーン読み込み: 別スレッドで decode (`src/app.rs` の `start_fs_load_with_purpose` 内 `std::thread::spawn`)。
  プロセス全体の実行枠は `src/fs_page_load_scheduler.rs:12-13` (合計 6、表示中用予約 2)
- 先読み範囲の既定: 後 4・前 12 (`src/settings.rs:6666-6671`)。先読み状況のドット表示あり
- 利用者向け記述: `htdocs/mimageviewer/manual/formats.html:113-131` (DNG は標準、他は Store 拡張が必要と案内)、
  `docs/spec.md:1330`

**未観測 (実機で誰も測っていない)**: WIC で RAW を開いたときの所要時間、WIC の最初のフレームが
フル現像なのかプレビュー相当なのか。数値を設計書に書くときは「未計測」と明記する。

## 4. エンジン調査の結論

| 候補 | ライセンス | 判断 |
| --- | --- | --- |
| **LibRaw** 0.22.2 (C++) | LGPL-2.1 / CDDL-1.0 の選択制。dcraw の GPL 部分は除外済み | **採用予定** |
| rawler / dnglab (Rust) | LGPL-2.1 | Rust の静的リンクと LGPL の相性が悪い。API 未安定 |
| rawloader / quickraw (Rust) | LGPL-2.1 | CR3 非対応 / 保守が薄い |
| dcraw | 独自 | 更新停止、CR3 非対応 |

- Microsoft の Raw 画像拡張自体が LibRaw ベース (Store 説明・報道より)。拡張の LibRaw は古く、
  新機種が開けないという利用者報告が Microsoft Q&A にある
- CDDL を選べば静的リンク可 (LibRaw 部分のソース公開義務のみ)。LGPL を選ぶなら DLL 分離。
  どちらにするかは設計で決める。FFmpeg と同様に mikage.to へ対応ソースを置く運用になる
- Rust バインディング crate の保守状況は未確認。turbojpeg と同様に `cc` で自前ビルド + 薄い FFI を想定
- LibRaw は「スレッドごとに別インスタンス」なら並列可。進捗コールバックがあり、戻り値で中断できる。
  `unpack_thumb` で埋め込みプレビュー取得、半解像度モードあり (いずれも設計時に公式ドキュメントで再確認する)

出典: https://github.com/LibRaw/LibRaw / https://www.libraw.org/download /
https://github.com/dnglab/dnglab / https://lib.rs/crates/rawloader / https://lib.rs/crates/quickraw /
https://learn.microsoft.com/en-us/answers/questions/3873803/please-update-the-raw-image-extension-app-to-suppo

## 5. 設計の叩き台 (未確定、設計書で詰める)

- **実行枠の分離**: RAW フル現像は既存の fs 読み込み枠 (6 本) と別の専用枠 (設定値、既定 3)。
  同じ枠だと数秒の現像が通常画像・PDF の枠をふさぐ。表示中ページの現像は先読みを追い越す
- **先読み範囲の分離**: 埋め込みプレビューは軽いので既存範囲 (後 4・前 12) に乗せる。
  フル現像だけ先 2・前 1
- **差し替え**: プレビューとフル現像は寸法が違うことがある。拡大率と表示位置を元画像比で持ち、
  差し替えで表示が飛ばないこと (`docs/display-pipeline.md`)
- **進捗とキャンセル**: プレビュー表示中に現像の進み具合を出す。ページ送りで途中中断
- **サムネイル**: 埋め込みプレビューから生成 (フル現像しない)
- **メモリ**: 大型 RAW は現像中に 1 枚数百 MB (一般的な目安、未計測)。並列数は固定値で決め、
  実行時の空きメモリで変えない (プロジェクト方針)
- **フル現像結果**: メモリ上のキャッシュのみの想定 (ディスクへは保存しない)
- **DNG も LibRaw へ移す** (「RAW は全部 LibRaw」で説明を一本化)
- **新設定は未リリースなので移行処理は不要**

設計で決めること: LibRaw のライセンス選択 (CDDL 静的 / LGPL DLL)、現像パラメータ (WB・色空間・
ハイライト・明るさ自動調整・8bit 化)、OpenMP の有無と並列 3 との兼ね合い、ZIP 内 RAW の扱い、
Remote が返す画像 (プレビューかフルか)、書き出し・類似画像・本の各経路でどちらを使うか、
プレビューを持たない RAW (あれば) の扱い、テスト用 RAW サンプルの入手と利用条件。

## 6. 段階案

| 段 | 内容 |
| --- | --- |
| S1 | LibRaw の vendor 配置・ビルド・FFI。プレビュー取り出しとフル現像の関数とテスト |
| S2 | RAW 判定の一本化 (WIC 非経由)、サムネはプレビューから、向き、ZIP 内、書き出し・類似画像・本・Remote |
| S3 | フルスクリーンのプレビュー → フル現像差し替え、RAW 現像の専用枠と先読み、設定、進捗とキャンセル |
| S4 | 配布 (launcher 埋め込み / ポータブル / 署名対象 / `build.rs` / bootstrap)、ライセンス通知・対応ソース、マニュアル・製品ページ・spec、リリースチェックに LibRaw 更新確認を追加 |

## 7. 進め方

1. CLAUDE.md の必読 (`docs/README.md`, `docs/architecture-overview.md`) に加え、
   `docs/display-pipeline.md`, `docs/async-architecture.md`, `docs/ui-responsiveness.md`,
   `docs/virtual-folders.md`, `docs/preset-and-adjustment.md`, `docs/web-remote-plan.md` を読む
2. 設計書 `docs/raw-libraw-plan.md` を書く (上の叩き台をコードで裏取りしながら)
3. 非同期・所有境界の設計なので、**実装前に GPT-6 Sol / xhigh の独立レビュー**を受ける
   (AGENTS.md の Model Roles And Coordination)
4. 実装は Codex Sol (xhigh) に段ごとに委任。委任前に必ずコミット。この worktree で codex を 2 本同時に走らせない
5. master は定期的にこのブランチへ取り込む (app.rs は競合しやすい)。統合は S4 完了後

注意:
- codex は PATH 先頭の旧版ではなく npm 版をフルパスで呼ぶ (`/c/Users/mikag/AppData/Roaming/npm/codex`)。
  `codex exec resume` は model / effort が既定に戻るので毎回 `-m gpt-6-sol -c 'model_reasoning_effort="xhigh"'` を付ける
- worktree の撤収は `scripts/safe-worktree-remove.ps1` 経由のみ。vendor を junction で共有しない
- エージェントは製品バイナリを起動しない。実行時の挙動は利用者の観測として書く
