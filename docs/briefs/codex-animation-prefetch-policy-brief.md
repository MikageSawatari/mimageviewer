# アニメーション展開の方針統一 — 先読みは 1 フレーム / アーカイブ内も再生 / 昇格時に進捗表示

対象: 利用者報告 (専用スレ >>241、2026-08-17) と、その調査で判明した実装の非対称。
backlog へ新規 entry として追記する (番号は採番して報告)。

**利用者の方針判断 (2026-08-17、確定)**:

- 先読みは**全形式で 1 フレーム目のみ**にする
- **アーカイブ内でもアニメーションする**ようにそろえる
- **閲覧対象 (現ページ) が全フレームをメモリに展開するのは許容する**
- 背景で展開している間は**進捗表示を出す** (位置は既存表示と同じ右上)

## 0. 何が起きているか

利用者報告: アニメ WebP を含むアーカイブで「1〜2 枚だけ表示されてその後は出てこない」
「メモリ消費もかなり大きくなるタイミングがある」「シークバーを弄っても元の場所に戻される」。

調査で判明した実装の非対称 ([canonical_image_loader.rs](../../src/canonical_image_loader.rs)):

```rust
if source.extension == "gif"  && !source.is_archive_entry { ... }   // アーカイブ内は静止画
if source.extension == "png"  && !source.is_archive_entry { ... }   // 同じ
if source.extension == "webp" { ... }                              // ← ガード無し
```

**アーカイブ内では GIF / APNG はアニメ展開されないのに、WebP だけ全フレーム展開される。**
そしてこの経路は先読みでも通り、`prefetch_forward` の既定は **12**。
つまりアニメ WebP を含む ZIP を開くと**最大 13 本のアニメが同時に全フレーム展開され得る**。
報告が WebP 限定である理由と、メモリ・停止の症状がこれで説明できる。

### 0.1 ガードは性能上の判断ではない (履歴で確認済み)

リファクタ前 (`7b8b31a1^` の `src/app.rs`) のコメント:

```
// GIF:  アニメーション試行 (通常パスのみ, ZIP は未対応)
// PNG:  APNG アニメーション試行 (通常パスのみ, ZIP は未対応)
// WebP: アニメーション試行。通常ファイルと ZIP 内 bytes の両方に対応する。
```

**単に未実装だった。** WebP だけ後から ZIP bytes 対応が入り、GIF / APNG が取り残された。
安全上の制約ではないので、そろえるのが正しい。

## 1. やること (4 つ)

### ① 先読みは全形式で 1 フレーム目のみ

- `CanonicalDecodeOptions` へ typed な方針を足す
  (例 `AnimationPolicy { FullFrames, FirstFrameOnly }`)。**bool を増やさない。**
- **構築箇所は 2 つだけ** ([app.rs](../../src/app.rs) の fullscreen load worker と
  [remote_ipc/container.rs](../../crates/../src/remote_ipc/container.rs))。配管は小さい。
- 現ページか先読みかは spawn 側で分かる
  (`update_prefetch_window(current_idx)` が `prefetch_targets` を作る)。
- **新しいデコードコードは要らない。** `FirstFrameOnly` はアニメ 3 分岐を skip するだけで、
  そのまま下の `image::open` / `image::load_from_memory` に落ちる。
  それが既に 1 フレーム目を返す (現在のアーカイブ内 GIF の挙動そのもの)。

### ② アーカイブ内でもアニメーションする

- `gif` / `png` から `&& !source.is_archive_entry` を**外す**。
- byte 版デコーダ `decode_gif_frames_from_bytes` / `decode_apng_frames_from_bytes` は
  **既に存在し、既に使われている**。新規実装は不要。
- `FsCacheEntry::Animated` は `frames: Vec<(TextureHandle, f64)>` の**純メモリ表現**で
  パス依存が無い ([fs_animation.rs](../../src/fs_animation.rs))。構造的な障害は無い。

### ③ 先読み済み静止画を、現ページ化した時点でアニメへ昇格 (**ここが本体**)

①の結果、先読みされたアニメページは `Static` (1 フレーム目) でキャッシュされる。
そのページへ移動したとき、アニメとして再デコードする必要がある。

- キャッシュエントリが「**これはアニメの 1 フレーム目である**」ことを持つ。
  ファイルを再判定して推測しない (判定は decode 時に分かっている事実)。
- 現ページ化で昇格を開始する。**1 フレーム目を表示したまま**背景で全フレームを取り、
  揃ったら差し替える。移動が待たされない形にする。
- 昇格中に別ページへ移動したら**キャンセルする**。世代 / idx で stale を捨てる。
- 昇格が失敗した場合は 1 フレーム目のまま (静止画として見える)。`Failed` にしない。

### ④ 昇格中の進捗表示

**既存の前例に倣う。新しい機構を作らない。**
[ui_erase.rs](../../src/ui_erase.rs) の `draw_erase_inpaint_progress` が、
消しゴムの MI-GAN 補完 (同じく数秒かかる処理) で既にこれをやっている:

```rust
if drawing_surface != target_surface || self.erase_inpaint_pending.is_empty() { return; }  // 状態駆動
let elapsed = started_at.elapsed();
if elapsed < Duration::from_millis(150) { return; }                                        // ちらつき抑制
```

守ること:

- **状態駆動にする。** 既存の `show_feedback_toast` は
  `(text, Instant::now(), duration_secs)` の**時間で消えるトースト**なので使わない。
  これを使うと「展開中なのに消える」か「終わったのに残る」のどちらかになる。
  表示条件は**昇格が in-flight であること**から導く。
- **150ms 未満は出さない。** 短いアニメで一瞬光るのを防ぐ。
  ⚠️ これは憲法 5 (時間窓で競合を吸収しない) に**反しない**。競合を隠すのではなく
  「提示するかどうか」の UI 判断である。この区別をコメントに残すこと。
- **位置は右上**。既存 toast (`min.y + 60.0`) と inpaint 進捗 (`min.y + 110.0`) の下に
  3 段目として置き、重ならないようにする。左下は使わない (利用者判断: 同種の
  「時間のかかる処理の進捗」を 1 箇所に集める)。
- 文言は内部語を避ける。「展開」より進捗が見える形を優先する。逐次デコードで
  フレーム数が分かるなら `アニメーションを読み込み中… 12 / 48` のように出す
  (`erase_inpaint_progress_label` と同じ発想)。分からなければ枚数なしで可。

## 2. やらないこと

- **ストリーミング / 有界リングバッファにしない。** 現ページの全展開は許容する方針
  (利用者判断)。WebP / GIF はフレーム間依存があり、ループで先頭へ戻るたび再デコードが
  必要になるので、別案件として扱う。
- **アーカイブ内アニメ再生を落とす方向で解決しない** (① の 1 行版)。機能は維持する。
- サムネイル生成経路に手を出さない (別経路)。
- 進捗表示のために `show_feedback_toast` を流用しない (§1 ④)。
- 描画元をページごとの条件で切り替えない。§3 参照。

## 3. 過去の失敗から (必ず読むこと)

v2.13.0 で「ページ送りの通過表示」を **5 回直して 5 回失敗し、削除した**
([brief-v213-remove-page-turn-passthrough.md](brief-v213-remove-page-turn-passthrough.md))。
原因は「**ページごとに変わる条件で描画元を選んでいた**」ことで、サムネイルと完成画像の差が
解像度だけのうちは見えず、カラー化で差が色になった瞬間に露出した。

今回の ④ は**描画元を切り替えず、状態を文字で伝えるだけ**なので同じ罠ではない。
ただし ③ は**表示テクスチャを 1 フレーム目 → 全フレームへ差し替える**ので、
差し替えの原子性に注意する。1 フレーム目と 1 フレーム目のアニメは同じ絵なので、
差し替えで見た目が飛ばないことを確認すること。

## 4. 触ってよいファイル

- `src/canonical_image_loader.rs`
- `src/app.rs` (load worker / prefetch spawn / 昇格)
- `src/fs_animation.rs` (エントリに「1 フレーム目である」印を足す場合)
- `src/ui_erase.rs` または `src/ui_fullscreen.rs` (④ の進捗表示。前例の隣に置く)
- `src/remote_ipc/container.rs` (① の呼び出し 1 箇所)
- `src/app/tests.rs` / 各 test module
- `docs/next-release-backlog.md` / `docs/display-pipeline.md` / `docs/detached-rework-plan.md`

## 5. テスト

1. `FirstFrameOnly` で GIF / APNG / WebP のいずれも `Static` を返し、
   **1 フレーム目の絵が一致する** (既存の `decode_*_frames` の先頭フレームと比較)。
2. `FullFrames` で GIF / APNG / WebP のいずれも `Animated` を返す。
3. **アーカイブ内でも** 2 と同じ (これが ② の回帰テスト)。
4. 先読み対象は `FirstFrameOnly`、現ページは `FullFrames` で spawn される。
5. `Static` (1 フレーム目) が現ページ化したら昇格が開始される。
6. 昇格中に別ページへ移動したらキャンセルされ、stale な結果が適用されない。
7. 昇格失敗は 1 フレーム目のまま (`Failed` にしない)。
8. ④ の表示述語: in-flight でないとき false / in-flight でも 150ms 未満は false /
   in-flight かつ 150ms 以上で true / 昇格完了で false。
9. 既存のアニメ関連テストが無修正で通る (`canonical_image_loader.rs` の
   `decode_*_frames` 一致テスト群を含む)。

**9 が赤くなったら報告して止まること。**

## 6. 凍結ルール

[detached-rework-plan.md](detached-rework-plan.md) §2 (憲法) の対象
(フルスクリーン表示経路に触れる)。着手前に §2 を読むこと。完了時に §11 へ追記。

## 7. 実機確認 (利用者が後で行う。手順を報告に書くこと)

- アニメ WebP を多数含む ZIP を開く → **メモリが跳ねない / 1〜2 枚で止まらない**
- アーカイブ内の **GIF / APNG がアニメーションする** (今まで静止画だった)
- 先読み済みページへ移動 → 1 フレーム目が即出て、右上に進捗が出て、アニメに切り替わる
- 短いアニメでは進捗が光らない (150ms ゲート)
- 通常フォルダの GIF / APNG / WebP が従来どおり
- 移動を連打しても昇格が溜まらない / 表示が壊れない
