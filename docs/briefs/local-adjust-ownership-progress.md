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

## 次 — 段 (d-1) 文書の所有権を `Arc` へ

`local_adjust_page_layers: HashMap<usize, Vec<LocalAdjustmentLayer>>` を
`HashMap<usize, Arc<Vec<LocalAdjustmentLayer>>>` にし、
`SidecarEntry::local_adjust_layers` も `Option<Arc<Vec<..>>>` にする。
これで R-14 の残り 2 複製 (サイドカー用の 2 枚目と `items_mut` の 3 枚目) が消える。

**調査済みの前提** (着手時に再調査しなくてよい):

- 参照は 141 箇所あるが、`Arc<Vec<T>>` は `Vec<T>` へ Deref するので読み取りはそのまま
  通る。**実際に直す必要がある変異箇所は 3 つだけ** (`src/app.rs` の `remove` /
  `insert`、`src/ui_adjustment_panel.rs` のテスト `insert`)。残りは `.cloned()` の
  戻り型が変わる箇所で、コンパイラが全部指す。
- `serde` は `features = ["rc"]` 済み ([Cargo.toml](../../Cargo.toml))。`Arc<Vec<T>>` を
  そのままサイドカー JSON へ載せられる。
- 書き換えは seam の `Arc::make_mut`。writer / worker が前の snapshot を持っている間
  だけ 1 回複製する。**`SidecarFile::items` (`df245720`) と同じ形**なので、設計の前例が
  既にある。

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
