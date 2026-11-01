# 実装 brief: P4e 配布（音楽ビデオ）

担当: Codex GPT-6.1 Sol / high（実装・テスト）。設計・検収: ClaudeCode Opus 5.5。独立レビューは別担当。
作業場所: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）。報告は日本語。

## 1. 正本

`docs/music-video-export-plan.md` の「### P4e の配布経路」節の表と直後の段落、§7 の **P4e** 行、D10「ライフサイクルと runtime」、
CLAUDE.md の「FFmpeg LGPL DLL 管理」「Distribution」「リリース手順チェックリスト」の配布ビルドの記述。
前提がコードと食い違ったら、実装で吸収せず止めて報告する。

## 2. 方針

- **EPUB worker（`mimageviewer-epub-pdf.exe`）と同じ扱いでエディタ（`mimageviewer-musicvideo.exe`）を足す。**
  `git grep -n "mimageviewer-epub-pdf\|epub-pdf-worker"` で launcher・build-release・build-dist・build-portable・build-dev・
  test-full・PE gate・署名・清掃・文書の全箇所を列挙し、同じ位置にエディタを加える。列挙した一覧を実装記録に書く。
  エディタの release ビルドは `cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo`（portable は
  `--target-dir target-portable`）。
- 単体 exe 版・インストーラ版: launcher がエディタを内包し、`runtime/<version>/` の core の隣へ展開する（hash 照合・atomic 置換は
  既存の資産と同じ）。インストーラは launcher だけを配置するので、`.iss` の変更が要らないことを確かめる。
- ポータブル版: core の隣に loose で同梱する（`target-portable` でビルド、署名、zip に入れる）。
- 署名: 埋め込み前にエディタへ署名する（内側の PE → launcher の順。CLAUDE.md の署名順）。
- PE gate（`check-vcrt-pe-dependencies.ps1`）: 判定処理は変えず、入力対象にエディタを加える（P4d で subsystem 検査は追加済み）。
- **エディタを build スクリプトの自動 `Stop-Process` 対象にしない**（未保存の編集を失う）。出力先の exe を掴んだエディタが稼働中なら、
  利用者に保存・終了を求めてビルドを止める（メッセージは ASCII）。**P4d で `build-dev.ps1` がエディタを停止対象に入れているので、
  これも直す**（`-PreserveRuntime` の有無に関係なく、エディタは止めずにビルドを中止する）。core / remote / EPUB worker の既存の
  扱いは変えない。
- test-full の前提: launcher の build.rs が要求する release exe にエディタが加わるので、CLAUDE.md・
  `docs/development-build-and-test.md` の「test-full.ps1 の前提」とビルド順の説明（core → remote → EPUB worker → launcher）を
  エディタを含む順に更新する。
- runtime 清掃: エディタの lease（P4c）と清掃の競合は D10 の割り切りのまま。既存の清掃テストがエディタを含む配置で通ることを確かめる。

## 3. 作業範囲

1. launcher（`crates/launcher/build.rs`・`src/main.rs`）: 前提検査・hash・rerun・`ASSETS`・展開と内容一致のテスト。
2. `build-release.ps1`: エディタの build → （署名時は署名）→ launcher 内包。
3. `build-dist.ps1`: 通常 / portable の clean 対象にエディタのパッケージを加える。最終の PE gate の入力に含める。
4. `build-portable.ps1`: 同じ build・copy・署名、zip への同梱。
5. `build-dev.ps1`: エディタを停止しない（上記）。安全性テスト（`scripts/test-build-dev-safety.ps1` など）を合わせて更新する。
6. 文書: CLAUDE.md（ビルド順・内包物の一覧・test-full の前提）、`docs/development-build-and-test.md`、
   `docs/release-operations.md` の該当箇所。製品ページ・マニュアルは P4f。
7. 設計書に「P4e 実装記録」を追記し、§7 の P4e 行を更新する。exe サイズ（エディタ単体、launcher の増分）とビルド時間を記録する。

## 4. 検証

- launcher のテスト（エディタの展開と内容一致、既存資産の不変）。
- `.\scripts\build-release.ps1`（署名なし）が成功し、`target\release\mimageviewer.exe` がエディタを内包すること。
- `.\scripts\build-portable.ps1`（署名なし）が成功し、zip とフォルダにエディタが入ること。
- `.\scripts\build-dist.ps1 -NoSign` を最後に 1 回通す（全体テスト → clean → 全ビルド → ISCC → portable → PE gate）。時間がかかるので
  タイムアウトに注意する。ISCC が無い等で通せない場合は、どこまで通ったかと理由を記録する。
- PE gate がエディタを入力に含めて成功すること。
- build スクリプトの安全性テスト: 稼働中のエディタを止めずにビルドを中止すること（テスト用の偽プロセスで確かめる。実際のエディタは起動しない）。
- 各新テスト・検査が、意図的に壊した実装で落ちることを確かめて記録する。
- スクリプトは `*>&1` を付けずに呼ぶ（CLAUDE.md の stderr の罠）。

## 5. 注意

- 本体アプリもエディタ GUI も起動しない（launcher も実行しない。展開はテストで確かめる）。`%APPDATA%\mimageviewer` に触れない。
- コミットしない。`cargo fmt` はかける。新しい `.ps1` の記述は ASCII のみ。
- 署名（SimplySign）は使わない（`-NoSign` / 既定 OFF の経路だけ）。
- 自分の確認を独立レビューと記録に書かない。
- 緑の区切りで止めて報告しなくてよい。止めるのは、設計判断が要るときと前提の矛盾を見つけたときだけ。

## 6. 報告

変更ファイル、列挙した箇所の一覧、各検証の結果（build-dist の通し結果を含む）、サイズとビルド時間、正本との相違（あれば）を日本語で。
