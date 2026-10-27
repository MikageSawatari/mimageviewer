# 実装依頼: テスト待ち時間の削減 (全単体テストの回数削減 + 遅いテストの高速化)

- 担当: Codex GPT-6.1 Sol / high (実装・テスト)
- 作業場所: worktree `G:\home\mimageviewer-testspeed`、ブランチ `test-speed` (master d3f2e913c から分岐)
- 依頼者: ClaudeCode Opus (設計・検収)。完了後に別の Sol / xhigh が独立レビューする
- 対象外: 製品コードの挙動変更。今回はテスト・テスト用スクリプト・開発文書だけを扱う

作業前に `AGENTS.md`、`docs/development-build-and-test.md`、`scripts/test-full.ps1` を読むこと。
CLAUDE.md は別セッションで並行して整理中なので、この作業では編集しない。

## 背景 (Codex 作業ログの集計、2026-10-01〜15)

- 本体の全単体テスト (`cargo test -p mimageviewer --lib` を filter なしで実行、約1万件) が
  **733 回**走り、1 回平均 **約18分**、合計 **224 時間**。内訳は実装側 499 回 (151 時間)、
  レビュー側 234 回 (72 時間)。1 セッションで 54 回走らせた例がある。
- `docs/development-build-and-test.md` の「検証の担当と結果の再利用」節には、反復中は絞ったテスト、
  成功結果はレビュー担当も再利用、と既に書いてある。**文書は既にあるが守られていない**ので、
  今回は文書を足すのではなく、守りやすくする道具を作る。
- 60 秒超の警告が多く出たテスト (警告回数、上位):
  - 259 `video::tests::real_decoder_eof_full_loop_preserves_single_tail_burst_per_epoch`
  - 142 `ui_main::section207_tests::section207_snapshot_label_keeps_input_minimum_order_and_panel_bounds`
  - 112 `ui_main::section207_tests::section207_reported_controls_keep_left_input_right_order`
  - 110 `ui_main::section207_tests::section207_production_layout_all_controls_and_positions_keep_input_usable`
  - 87 `tests::all_cc0_sample_dimensions_match`
  - 66 `ui_fullscreen::tests::still_seek_f4_screen_last_pixel_closes_strip_with_real_geometry`
  - 36 `ui_fullscreen::tests::raw_blocked_preview_settles_navigation_and_folder_lock_with_or_without_rendition`
  - 34 `ui_fullscreen::tests::still_seek_overlay_requests_survive_continuous_landing`
  - 33 `raw::tests::all_cc0_samples_info_develop_and_preview_orientation`
  - ほか `ui_fullscreen::tests::*` (still_seek / original_preview_hold / strip 系) が 15〜30 回

## 作業 A: 遅いテストの実測 (最初に行う)

1. テストごとの所要時間を実測する。stable toolchain のまま、libtest の実行時引数で取れるか試す:
   `RUSTC_BOOTSTRAP=1` を環境変数に置き、テスト実行体へ `-Z unstable-options --format json --report-time`
   を渡す (再ビルド不要の想定)。取れなければ別の方法を考える。
   **cargo-nextest など新しいツールのインストールはしない** (必要なら止めて理由を報告する)。
2. 集計を `docs/test-speed-report.md` に残す: 全体の所要時間、テストごとの上位 30 件 (秒)、
   上位 N 件で全体時間の何割を占めるか、測定時の並列負荷 (他の worktree のビルドが同時に動いていたか。
   `Get-Process cargo,rustc` 程度でよい)。
   別セッションが同じ PC でビルドしているため値は揺れる。絶対値より順位と割合を重視する。
3. 上位のテストについて、遅い理由を分類する: 実時間の sleep / timeout 待ち、実デコーダ・実ファイル I/O、
   egui の多フレーム描画、大きな fixture、グローバル lock による直列化、など。
   `src/app/tests.rs` などの `thread::sleep` が多い箇所も見る。

## 作業 B: 遅いテストの高速化

- **検証している内容を弱めない**。期待値の緩和・assert の削除・テストの削除は禁止。
  速くする手段の優先順位:
  1. 実時間待ちを、テスト内で進められる時計・条件待ち・イベント駆動に置き換える
     (既存のテスト用 clock や hook があれば流用する)
  2. 同じ性質を検証できる、より小さい入力・少ない反復にする (縮めても検証の意味が保たれる根拠を
     コミットメッセージに書く)
  3. 上記ができず、どうしても重いものだけ `#[ignore = "slow: <理由>"]` にし、
     `scripts/test-full.ps1` で `--ignored` 付きの実行を追加して**全体ゲートでは必ず走る**ようにする。
     ignore にしたテストの一覧と理由を `docs/development-build-and-test.md` に残す
- 実デコーダ・実ファイルを使うこと自体がテストの目的であるもの (例: `real_decoder_*`、cc0 サンプル照合) は、
  手段 1・2 が無理なら 3 にしてよい。
- 製品コードの変更が必要になったら (テスト用 hook の追加を含め) 止めて報告する。勝手に入れない。
- 目標: filter なしの `cargo test -p mimageviewer --lib` の所要時間を、作業前の実測から半分以下。
  届かなければ、残りの時間が何で占められているかを報告する。

## 作業 C: 全単体テストの回数を減らす道具

### C1. 結果台帳付きの全体テスト実行 `scripts/test-lib-full.ps1`

- filter なしの `cargo test -p mimageviewer --lib` (と feature 指定) を実行するラッパー。
- 成功 (exit 0、実行件数が閾値以上、0件一致ではない) した結果を台帳へ記録する。
  記録内容: ソース状態の識別子、command、feature/profile、実行件数、失敗件数、所要時間、日時、ログの場所。
- 次に同じソース状態・同じ command で呼ばれたら、**テストを走らせずに記録した結果を表示して exit 0**。
  `-Force` で再実行できる。
- ソース状態の識別子は、HEAD だけでは不十分 (未コミットの差分があるため)。
  追跡ファイルの差分と、テスト結果に影響する未追跡ファイルを含めた内容から作る
  (`git stash create` や一時 index での `git write-tree` など、作業ツリーと index を変えない方法で)。
  `target/` や生成物は含めない。方法と、どの変更を検知しないかを文書に明記する。
- 台帳の置き場所は、**同じ worktree のレビュー担当 (read-only sandbox) が読める**場所にする。
  別 worktree で同じソース状態なら共有できると望ましい (git の共通ディレクトリ配下など)。判断は任せる。
- 失敗結果は「再実行不要」の根拠にしない (記録はしてよい)。
- PowerShell 5.1 で動くこと。`.ps1` は **ASCII のみ**で書く (CLAUDE.md の方針)。

### C2. 変更箇所に絞ったテスト `scripts/test-focused.ps1`

- 作業ツリーの変更 (基準 commit を引数で指定可、既定は HEAD) から、`src/foo/bar.rs` → `foo::bar::` のように
  テスト filter を作って `cargo test -p mimageviewer --lib <filter>` を実行する。
  `tests/<name>.rs` の変更は `--test <name>`、他 crate の変更は `-p <crate>` にする。
- `src/app/tests.rs` のような横断的なテストファイル、`lib.rs` / `app.rs` など影響が広いファイルを
  どう扱うかは決めて文書化する (例: その場合は全体へ回すよう表示する)。
- 完全な選択は求めない。目的は「反復中に全体を走らせる理由を減らす」こと。何を選んだかを毎回表示する。
- 0 件一致はエラー扱い。

### C3. 文書

- `docs/development-build-and-test.md` に C1・C2 の使い方を追記し、既存の「検証の担当と結果の再利用」と
  つなげる。追記は短く、既存の文と重複させない。
- `AGENTS.md` の該当箇所に、短い規則を 1 か所だけ足す:
  - 反復中は `test-focused.ps1` か filter 付き実行を使う
  - filter なしの本体全体テストは `test-lib-full.ps1` 経由で実行する (直接 `cargo test -p mimageviewer --lib` を filter なしで打たない)
  - レビュー担当は台帳に同じソース状態の成功があれば再実行しない
- CLAUDE.md は編集しない (並行して整理中)。CLAUDE.md 側に必要な変更があれば報告に書く。

## 進め方と報告

- A → B → C の順。A の結果で B の対象を決める。C は B と独立なので、A の全体テストの待ち時間に設計してよい。
- 区切りの判断で止めて報告しない。止めてよいのは、製品コードの変更が必要なとき、新しいツールの
  インストールが必要なとき、前提の矛盾を見つけたときだけ。
- コミットはこのブランチに、意味のまとまりごとに行う (`git commit -- <paths>` の pathspec 形式)。
  コミット前に `cargo fmt`。master への merge はしない。
- 完了時に次を報告する:
  - 作業前後の filter なし全体テストの所要時間 (同じ条件での比較)
  - 速くしたテストと手段、ignore にしたテストと理由
  - C1・C2 の動作確認結果 (台帳の hit / miss / 差分ありの再実行、0 件一致のエラー)
  - `.\scripts\test-full.ps1` の結果 (最後に 1 回だけ)
  - 未解決の点
- まれな失敗 (台帳ファイルの書き込み失敗など) は既定の割り切り方針に従う: ログを出して、
  台帳なしで通常どおりテストを実行すればよい。回復の仕組みは作らない。
