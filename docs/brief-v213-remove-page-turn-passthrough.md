# ブリーフ: ページ送りの通過表示を v2.13.0 から取り除く

## 前提 (必ず守ること)

- **アプリを起動しない**。ビルドとテストまでで止める。
- **git 操作をしない**。master の作業ツリーに未コミットのまま残す。統合はこちらで行う。
- 着手前に [docs/display-pipeline.md](display-pipeline.md) §2.5 を読むこと。**この節は残す**
  (次版でやり直すときの正本)。

## なぜ取り除くか

§1.58「ページ送りの引っかかり」の対策 1 (通過表示 = 通り過ぎるページをカタログサムネイルで
描く) を、2026-08-11 の実機確認で 5 回直して 5 回とも失敗した。最後の状態は v2.12.0 より
悪い (カラー化した本で、ページごとにカラー / 白黒が入れ替わり、ページ間隔が中央値 463ms /
最大 2763ms)。

原因は §2.5 に記録した。**通過表示は最初から「ページごとに変わる条件」で描画元を選んでいた**
(そのページのサムネイルがあるか、レンディションが作れるか)。サムネイルと完成画像の差が
解像度だけのうちは見えず、カラー化で差が色になった瞬間に露出した。

§1.58 は**改善であって不具合修正ではない**ので、直せないまま出荷しない。**削除**して、次版で
テスト基盤 (アプリ内蔵のテストスクリプト実行) を先に作ってからやり直す。

**「常に無効を返す」形で残さないこと。** 無効化した経路が残ると、次にやり直す人が壊れた設計を
土台にしてしまう。設計の知見は §2.5 に文章で残してあるので、コードは消してよい。

## 取り除くもの

`1ea4d824` / `5a57a67b` / `00d23a33` / `99310881` / `e1b6db27` が入れた、**通過表示に関わる
実装すべて**。git revert ではなく、**手で外す** (これらのコミットには残したい別物も含まれる)。

- `FsPageTurnDecision` / `FsPageTurnPaintSource` / `page_turn_decision_for_inputs` /
  `page_turn_decision_reason` / `FsPageTurnFrameDecision` / `FsPageTurnDisplayUnitReadiness`
- `fs_page_turn_materialization_for_frame` / `fs_page_turn_display_unit_readiness` /
  `fs_page_turn_ordinary_context_blocker` / `fs_page_turn_chord_is_unambiguous` /
  `fs_page_turn_candidate_chords` / `FS_PAGE_TURN_COALESCE_ACTIONS` / `FS_FIXED_PAGE_TURN_CHORDS`
  など、この判定のためだけに足された定数・ヘルパー
- **通過レンディション一式**: `passthrough_rendition_cache`、`ensure_passthrough_rendition`、
  `passthrough_colorize_decision`、`VramSubsystem::PassthroughRenditionCache` と
  `vram_accounting` への追加、`vram_budget.rs` の対応分
- 消費側 3 か所を**元に戻す**:
  - `prepare_fullscreen_state` — 常に `resolve_fs_processed_texture` を呼ぶ
  - `poll_final_effects` — 保留せず常に結果を回収する
  - `fs_upload_backlog` の消化 — 保留せず従来のペースで流す
- 上記のためだけに足された perf event (`fs.page_turn_decision` / `fs.page_turn_ready` /
  `fs.page_turn_winit_input` / `fs.page_turn_egui_input`) と、それらの emit 関数
- 上記に紐づく回帰テスト

**結果として、フルスクリーンのページ表示は §1.58 以前と同じ経路だけを通ること。**
「通過表示に入るかどうか」という分岐が存在しない状態にする。

## 残すもの (消さないこと)

- **[docs/display-pipeline.md](display-pipeline.md) §2.5 と §2.5.2.1** — 次版の正本。
  ただし「現在の実装」を指す記述があれば、**「v2.13.0 では削除。次版でやり直す」と分かる形へ
  書き換える** (要件 R1〜R4、加工ごとの扱い、2 軸、安定した信号、不変条件はそのまま残す)。
- **`scripts/analyze_perf.py` の `page-turn --check`** と `scripts/test_analyze_perf.py`。
  入力となる perf event が消えるので **`--check` は「該当イベントが無い」で素通り (exit 0) に
  なる**はず。そこを確認し、必要なら「イベントが無い場合」の扱いをコメントで明記する。
  **判定ロジック自体は消さない**。
- **`scripts/page-turn-smoke.ps1`** (未完成のまま残す)。
- **`src/colorize.rs` の計測テスト** (`thumbnail_effect_cost_measurement`、`#[ignore]`)。
- **`keymap.rs` の `key_held_chord_via_os`** — 汎用ヘルパーとして残す。他から使われて
  いなければ `#[allow(dead_code)]` ではなく、**使われていない事実を確認したうえで残すか消すかを
  報告すること** (判断はこちらでする)。
- §3.3 (`items_generation_cache.rs`)、§1.61 (`page_dims.rs`) は**一切触らない**。

## 完了条件

- `git grep -n "passthrough\|PageTurnPaintSource\|page_turn_decision" src/` が、残すと決めた
  ものだけになる。
- `cargo fmt --check` が通る。
- `cargo check -p mimageviewer --bin mimageviewer-core` が **warning なし**で通る
  (未使用の関数・定数・enum variant が残っていないこと)。
- `cargo test -p mimageviewer --lib` が全件通る。
- `cargo test --test ui_snapshot` が通る。
- `python scripts/test_analyze_perf.py` が通る。

## 報告してほしいこと

- 消した項目の一覧。
- 消費側 3 か所を元に戻した内容 (差分の要点)。
- `key_held_chord_via_os` を残したか消したか、その理由。
- `--check` が perf event 不在でどう振る舞うか。
- 消したテストと、残したテストの数。
