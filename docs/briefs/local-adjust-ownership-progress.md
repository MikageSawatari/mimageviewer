# R-07 + R-14 の進捗と設計判断

[local-adjust-ownership-brief.md](local-adjust-ownership-brief.md) の作業記録。
**ブリーフが「何を直すか」、これが「どこまで直したか / どう直すと決めたか」**。

作業場所は worktree `C:/home/mimageviewer-localadjust`、ブランチ `local-adjust-ownership`
(master HEAD `cfa5e309` 起点)。master 側では別セッションが細かいバグ修正を並行している。

## 済み — 段 (c) 契約変更

| コミット | 内容 |
| --- | --- |
| `116ca8cd` | 毎フレームの文書複製を廃止し、`LocalAdjustCanvasEdit` で「何をしたか」を申告する契約へ |
| `a9f69d14` | Codex レビュー指摘 3 件 (P2 × 2 / P3 × 1) の修正 |

`cargo test -p mimageviewer --lib` は 6759 passed / 0 failed。新規テスト 15 件。
**各修正は変異させた実装で落ちることを確認済み** (ブリーフ §9 の要求)。

### 判断の記録

- **`PreparedOnly` は保存しない。** 材質化 (昇格 / リサイズ) を「変更だから保存」に
  すると、空振りの操作 1 回ごとに DB 保存と Undo が増える。メモリにだけ残し、次の
  実際の編集で一緒に保存される。
- **材質化は 3 種類に分かれ、取り消してよいのは「作成」だけ。** 空スロットの作成は
  編集を当て込んだ先行投資なので、編集が効かなければ取り消す。昇格とリサイズは内容を
  保つ正規化なので残す (毎回やり直すのは無駄)。
- **ブラシの override 経路だけは寸法違いの既存マスクを置き換える** (他はリサイズ)。
  ここを一律「作成」扱いにして `None` へ戻すと**利用者のマスクを消す**ので、元が
  `None` だったときだけ取り消し可能として扱う。この分岐は既存仕様であり、今回は
  揃えていない。

## 済み — 段 (d-1) 文書の所有権を `Arc` へ

| コミット | 内容 |
| --- | --- |
| `a6367fb1` | 文書を `local_adjust_core::LocalAdjustmentLayers` (= `Arc<Vec<LocalAdjustmentLayer>>`) で配り、書き手だけが `Arc::make_mut` で複製を払う形へ |
| `c093f0d7` | Codex レビュー指摘 (P2 / P3) の修正 |

`cargo test -p mimageviewer --lib` は 6763 passed / 0 failed。新規テスト 4 件
(所有権 3 + サイドカー形式 1)。**4 件とも変異させた実装で落ちることを確認済み**
(強制複製 / `Arc::get_mut` / before を編集後に取る / 直列化名の変更)。

### 消えた複製

着手前の見積もりは「サイドカー用の 2 枚目と `items_mut` の 3 枚目」だったが、
実際に見つかったのはもっと多い。**一番大きいのは見落としていた**:

- `apply_local_adjust_panel_actions` が **1 フレームに 2 枚**深い複製を作っていた
  (`.cloned()` と `before_layers = layers.clone()`)。しかも要求の有無を見る**前**なので、
  補正パネルを開いているだけで毎フレーム 2 枚。
- `LocalMaterializeLayers::Memory` — 描画 worker へ渡すたびに 1 枚。
- `set_local_adjust_layers_for_idx` のサイドカーミラー用に 1 枚。
- `set_local_adjust_layers_for_idx_with_undo` の `after` 再取得で 1 枚。
- `*_before_layers` 3 フィールド (図形ドラッグ / キャンバスドラッグ / ブラシ) が
  各操作の開始時に 1 枚。

### 判断の記録

- **型エイリアスを `local_adjust_core` に置いた。** `undo_stack` / `sidecar` /
  `edit_bundle` / `books` が全部この型を持つので、所有権の方針を型が定義されている場所に
  1 度だけ書く。
- **Undo の `before` は編集より前に取る**という要件がここで初めて**壊れうる**ものになった。
  複製していた頃は「後で取っても複製だから別物」だったが、共有所有では live と同じ割り当てを
  指すので、後で取ると `Arc::make_mut` で分岐した後の姿を掴み、before == after で
  Undo が捨てられる。seam にコメントを置き、テストで固定した。
- **Undo スタックも `Arc` にした。** ここを `Vec` のまま残すと `capture_local_adjustment_undo`
  で `(*arc).clone()` が要り、消したはずの複製が戻る。スコープ拡大ではなく、
  変更が意味を持つための必要条件。
- **必要になったときだけ複製する形へ 2 箇所直した**。`materialize_local_adjust` は
  寸法が合っているなら複製しない (以前は無条件に複製してから寸法を見ていた)。
  `local_adjust_layers_until` は prefix が有効なときだけ `to_vec` する。
- **サイドカーの形式は変わらない** (`serde` の `rc` feature が `Arc<T>` を `T` として
  直列化する)。出荷済みのディスク形式なので、配列であることをテストで固定した。
  フォルダ移動時の復元がここを読む。

### Codex レビューで直したこと (`c093f0d7`)

- **[P2] 確定のたびに必ず複製していた。** 両 persist 経路が「live の `Arc` を複製 →
  無条件に `Arc::make_mut`」の形だった。マップが常に 1 つ握っているので、**refcount は
  必ず 1 より大きく、`make_mut` は必ず実複製になる**。落とすものが無い通常の
  ストロークでも 96MB。この段の目的の大半をここで失っていた。
  「落とすものがあるときだけ複製する」を単一の owner
  (`compact_local_adjust_manual_override_in`) に集約し、判定と実際の圧縮が一致することを
  テストで固定した (両方向に食い違うと害があるため)。
- **[P3] ポインタ同一性のテストが無かった。** 上の退行を検出できる形
  (複製しない確定 / 複製する確定) と、未検査だった `PageEditBundle::prepare` の
  直列化経路 (貼り付け → `local_adjust.db` JSON) を追加。

Codex は **意味的な別名化・スナップショット順序の欠陥は無し**、サイドカーと
`local_adjust.db` の形式は読み書き両方向で不変、と確認している。

## 未修正で見つかったもの — 補正パネルの編集器が毎フレーム 1 レイヤーを複製する

**[Codex P1、段 (b) で扱う]** `draw_selected_local_adjust_layer_editor`
([src/ui_adjustment_panel.rs](../../src/ui_adjustment_panel.rs) `3566` 付近) の
`let mut edited = layer.clone();` が、補正パネルを開いている間**毎フレーム**走る
(`draw_local_adjust_panel` の入口ガードは `local_adjust_mode` + フルスクリーンだけ)。
`LocalAdjustmentLayer` の複製は `RasterVectorMask::alpha` を再帰的に複製するので、
24MP なら 1 枚 96MB。`manual_override` の add / subtract も持てば最大 3 枚。

**これは段 (c) で直したのと同じ形の契約**である。「文書を複製 → UI に貸す →
変わっていたら publish」で、複製が正しさ (何が変わったかの判定) を支えている。

調査済み: **編集器はマスクの画素を書かない。** 書くのは
`mask_inverted` / `opacity` / `mask_before_effect` / `mask_after_effect` /
`mask_expand_px` / `mask_feather_px` の scalar、`mask` の**パラメータ**
(グラデーション幾何・許容幅・target RGB)、`shapes` (ベクタ、太さスライダを動かしたときだけ)、
`effect` の 4 種類だけ。原寸の `alpha` 配列は読むだけ。つまり**複製の大部分は純粋な無駄**。

直し方の候補を検討した結果:

- `Arc<LocalAdjustmentLayer>` にしても効かない。egui のウィジェットは毎フレーム
  `&mut f32` を要求するので、`Arc::make_mut` が毎フレーム走る。
- 画素配列だけ抜いて貸し、戻すときに差し戻す形は、抜き忘れが静かに壊れる。
- **正解は「編集中のレイヤー」を App の state owner に持たせること** — 選択が変わった
  ときに 1 度作り、変更を publish する。これは**段 (b) が作ろうとしている
  「保留中の編集の単一 state owner」と同じもの**なので、段 (b) に畳む。

## 次の次 — 段 (d-2) 保存の worker 化 (R-26 を開け直さない)

**調査済みの前提**:

- **雛形がある**: [src/rating_write_worker.rs](../../src/rating_write_worker.rs) が
  `spawn` / `submit` / `try_recv_result` と、**Drop で「shutdown かつキュー空」まで
  drain してから join する**構造を持つ。ブリーフが懸念していた「非同期化すると終了時に
  書き損なう」はこの既存パターンで塞がる。
- **DB の複数接続は既に作法**: `LocalAdjustDb::open_readonly` を smart folder /
  subfolder expansion / metadata import が既にワーカー側で使っている。保存ワーカーが
  自分の書き込み接続を持つのは延長線上。`busy_timeout` は明示的に設定する
  (既定値の記憶は当てにしない)。
- **保存経路の入口は狭い**: `set_local_adjust_layers_for_idx` 系の呼び出しは 14 箇所、
  ほとんどが `_with_undo` 経由 (`ui_adjustment_panel.rs` / `undo_ops.rs`)。

### R-26 の境界をどう保つか

現状の同期版は、**presence は成否に関わらず立て、サイドカーのミラーだけを成功時に書く**
(`schedule_current_smart_folder_metadata_refresh` と `record_content_identity_for_idx` も
失敗時は early return で飛ぶ)。守るべきはここ。

非同期版:

1. UI: key を採り、preview cache を無効化し、presence を立て、**メモリを即更新**し、
   `(key, sidecar 座標, Arc snapshot, generation)` を worker へ積む。
2. worker: キーごとに**最新 generation だけ**を処理する (古いものは捨てる。
   後続 snapshot が上書きするので結果は不要。各要求が原寸マスクを抱えるので、
   合体しないとメモリが積み上がる)。書けたら `EditStoreOutcome` を返す。
3. UI の poll: 世代が最新のものだけを見る。`Committed` のときだけ、**そのとき書いた
   snapshot そのもの**をサイドカーへ写す。失敗なら従来どおりトーストのみでミラーを
   書かない。

この形は現状より**強い**。「書いた内容」と「写した内容」が同一であることが型で保証され、
`Unavailable` も「起動時に開けなかった」ではなく「**書き手が開けなかった**」の意味になり、
判定する主体と書く主体が一致する。

**idx の陳腐化に注意**: 応答が返る頃には一覧の idx がずれ得る。サイドカー座標は
enqueue 時に固定する (`with_sidecar_coords_mut` が既にこの用途)。idx を使う後続処理
(`record_content_identity_for_idx`) は、完了時に `page_path_key(idx)` が同じ key を
返すことを確認してから走らせる。

**計装**: この経路には現在 perf イベントが 1 つも無い。worker 化と同時に UI 側の
enqueue と worker 側の所要時間へ `perf::event` を入れる (CLAUDE.md「追加した同期処理の
区間には perf::event を必ず差し込む」)。ブリーフ §10 の「perf log で確認」はこれが前提。

## 最後 — 段 (b) キー移動 / 回転の確定タイミング

**調査済み**: 編集状態を畳む箇所は 14 箇所あり、`src/app.rs` (4) /
`src/ui_adjustment_panel.rs` (9) / `src/ui_fullscreen.rs` (1) に散っている。
**その多くは `= None` で破棄している** — ブラシはそれでよいが、キー移動は完了した編集
なので破棄できない、というブリーフの警告どおりの形になっている。

方向: 破棄と確定を各所で判断させず、**単一の state owner に畳む**
(CLAUDE.md「相互排他的な状態を複数の bool / Option で表さない」)。
既存の `local_adjust_mask_brush_stroke` / `local_adjust_shape_drag` +
`persist_*` が同型の先例。監査テストは「畳む箇所すべてがその owner を通ること」を
ソース走査で検査する形が合う (`layer_parse_audit` が同じ手法)。

## 環境メモ

新しい worktree では **FFmpeg DLL を `target/debug/{,deps/}` へ置かないとテスト exe が
`STATUS_DLL_NOT_FOUND` で起動しない**。`cp vendor/ffmpeg/bin/*.dll target/debug/deps/` 等。
`cargo test ... | tail` は exit code を隠すので、出力そのものを読んで判断すること。
